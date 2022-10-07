#![allow(unused)]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::os::unix::io::RawFd;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::RwLock;

mod sockio;

static mut DEFAULT_CLIENT: (bool, MaybeUninit<RefCell<WlClient>>) = (false, MaybeUninit::uninit());

// SAFETY: should only be called from main thread
//
// it's initializing a static mut
pub unsafe fn default_client() -> std::io::Result<&'static RefCell<WlClient>> {
    if !unsafe { DEFAULT_CLIENT.0 } {
        unsafe {
            DEFAULT_CLIENT.1.write(RefCell::new(WlClient::new()?));
            DEFAULT_CLIENT.0 = true;
        }
    }
    Ok(unsafe { DEFAULT_CLIENT.1.assume_init_ref() })
}

pub struct ObjectRegistry(RefCell<Vec<Option<Box<dyn WlHandler>>>>);

impl ObjectRegistry {
    pub fn get_mut<T: WlHandler>(&mut self, r: WlRef<T>) -> Option<&mut T> {
        let id: usize = r.id.try_into().unwrap();
        let dyn_h = &mut **self.0.get_mut().get_mut(id)?.as_mut()?;
        // basically copying dyn Any impl
        if dyn_h.is::<T>() {
            // SAFETY: we just checked this is the correct type
            Some(unsafe { &mut *(dyn_h as *mut dyn WlHandler as *mut T) })
        } else {
            None
        }
    }

    pub fn new_id<T: WlHandler>(&self, init: T) -> WlRef<T> {
        let mut hs = self.0.borrow_mut();
        for (i, v) in hs.iter_mut().enumerate().filter(|(_, v)| v.is_none()) {
            v.replace(Box::new(init));
            return WlRef {
                id: i.try_into().unwrap(),
                marker: PhantomData,
            };
        }
        let i = hs.len();
        hs.push(Some(Box::new(init)));
        return WlRef {
            id: i.try_into().unwrap(),
            marker: PhantomData,
        };
    }

    fn destroy(&self, id: u32) -> bool {
        let mut hs = self.0.borrow_mut();
        let id: usize = id.try_into().unwrap();
        match hs.get_mut(id) {
            Some(v @ Some(_)) => {
                v.take();
                true
            }
            _ => false,
        }
    }

    fn make_ref<T: 'static>(&self, id: u32) -> Option<WlRef<T>> {
        let id_u: usize = id.try_into().unwrap();
        let hs = self.0.borrow();
        let mut dyn_h = hs.get(id_u)?;
        if dyn_h.is_none() {
            eprintln!("handlers[{id}] not initialized");
        }
        let dyn_h = dyn_h.as_ref()?;
        let concrete = (&**dyn_h).type_id();
        let ttid = std::any::TypeId::of::<T>();
        if concrete == ttid {
            let marker = PhantomData;
            Some(WlRef { id, marker })
        } else {
            None
        }
    }
}

pub struct WlClient {
    sock: UnixStream,
    buf: Vec<u8>,
    fdbuf: Vec<RawFd>,

    handlers: ObjectRegistry,
}

impl WlClient {
    // To find the Unix socket to connect to, most implementations just do what libwayland does:
    //
    // 1. If WAYLAND_SOCKET is set, interpret it as a file descriptor number on which the connection is already established, assuming that the parent process configured the connection for us.
    // 2. If WAYLAND_DISPLAY is set, concat with XDG_RUNTIME_DIR to form the path to the Unix socket.
    // 3. Assume the socket name is wayland-0 and concat with XDG_RUNTIME_DIR to form the path to the Unix socket.
    // 4. Give up.
    fn find_socket() -> std::io::Result<UnixStream> {
        if let Ok(sock) = std::env::var("WAYLAND_SOCKET") {
            return UnixStream::connect(sock);
        }
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap();
        if let Ok(disp) = std::env::var("WAYLAND_DISPLAY") {
            return UnixStream::connect(&format!("{runtime_dir}/{disp}"));
        }
        return UnixStream::connect(&format!("{runtime_dir}/wayland-0"));
    }

    fn print_error(
        _: &ObjectRegistry,
        object_id: WlRef<()>,
        code: u32,
        message: WlStr,
    ) -> Result<(), WlHandleError> {
        eprintln!("server: {object_id:?} {code}: {message}");
        Ok(())
    }

    pub fn on_error<CB>(&mut self, cb: CB)
    where
        CB: Fn(&ObjectRegistry, WlRef<()>, u32, WlStr) -> Result<(), WlHandleError> + 'static,
    {
        let disp = self.display();
        self.get_mut(disp).unwrap().on_event = Some(Box::new(move |reg, e| match e {
            wl_display::Event::Error {
                object_id,
                code,
                message,
            } => cb(reg, object_id, code, message),
            wl_display::Event::DeleteId { id } => {
                reg.destroy(id);
                Ok(())
            }
        }));
    }

    pub fn new() -> std::io::Result<WlClient> {
        let display = WlDisplay {
            on_event: Some(Box::new(|reg: &ObjectRegistry, e| match e {
                wl_display::Event::Error {
                    object_id,
                    code,
                    message,
                } => WlClient::print_error(reg, object_id, code, message),
                wl_display::Event::DeleteId { id } => {
                    reg.destroy(id);
                    Ok(())
                }
            })),
        };
        let mut c = WlClient {
            sock: WlClient::find_socket()?,
            buf: Vec::with_capacity(1024),
            fdbuf: Vec::with_capacity(255),
            handlers: ObjectRegistry(RefCell::new(vec![None, Some(Box::new(display))])),
        };
        Ok(c)
    }

    pub fn display(&self) -> WlRef<WlDisplay> {
        self.handlers.make_ref::<WlDisplay>(1).unwrap()
    }

    pub fn get_mut<T: WlHandler>(&mut self, r: WlRef<T>) -> Option<&mut T> {
        // we control the display
        if r.id != 1 {
            self.handlers.get_mut(r)
        } else {
            None
        }
    }

    pub fn new_id<T: WlHandler>(&mut self, init: T) -> WlRef<T> {
        self.handlers.new_id(init)
    }

    pub fn send<T: WlHandler>(
        &mut self,
        r: WlRef<T>,
        f: impl Fn(&mut Vec<u8>, &mut Vec<RawFd>) -> Result<(), WlPrimitiveSerError>,
    ) -> Result<(), WlPrimitiveSerError> {
        self.buf[..4].copy_from_slice(&r.id.to_ne_bytes());
        f(&mut self.buf, &mut self.fdbuf)?;
        let n = sockio::sendmsg(&mut self.sock, &self.buf, &self.fdbuf);
        if n < self.buf.len() {
            panic!("buffer underwrite");
        }
        Ok(())
    }

    pub fn poll(&mut self) -> Result<(), ()> {
        const WORD_SZ: usize = std::mem::size_of::<u32>();
        const HDR_SZ: usize = WORD_SZ * 2;
        sockio::recvmsg(&mut self.sock, &mut self.buf, &mut self.fdbuf)?;

        let mut i = 0;
        while self.buf[i..].len() > HDR_SZ {
            let buf = &mut self.buf[i..];
            let id = u32::from_ne_bytes(buf[..WORD_SZ].try_into().unwrap());
            let (op, sz): (u16, usize) = {
                let opsz = u32::from_ne_bytes(buf[WORD_SZ..HDR_SZ].try_into().unwrap());
                (
                    (opsz >> 16).try_into().unwrap(),
                    (opsz & 0xffff).try_into().unwrap(),
                )
            };
            if buf.len() < HDR_SZ + sz {
                break;
            }
            i += HDR_SZ + sz;

            let objid: usize = usize::try_from(id).unwrap();
            let hborrow = self.handlers.0.borrow();
            let obj = hborrow.get(objid).and_then(Option::as_ref);
            let obj = match obj {
                Some(obj) => obj,
                None => {
                    eprintln!("message for non-existent object (@{id})");
                    continue;
                }
            };

            let buf = &mut buf[HDR_SZ..(HDR_SZ + sz)];
            obj.handle(&self.handlers, op, buf, &mut self.fdbuf)
                .map_err(|e| eprintln!("handle err: {e:?}"))?;
        }

        // move unhandled bytes to front of buf, truncate
        self.buf.copy_within(i.., 0);
        self.buf.truncate(self.buf.len() - i);

        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct WlRef<T: ?Sized> {
    id: u32,
    marker: PhantomData<T>,
}

impl std::fmt::Debug for WlRef<()> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "object@{}", self.id)
    }
}

pub type WlStr = String;
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct WlFixed(i32);
pub type WlArray = Vec<u8>;
#[derive(Debug)]
pub struct WlFd(RawFd);

impl WlFixed {
    fn from_ne_bytes(bs: [u8; 4]) -> WlFixed {
        WlFixed(i32::from_ne_bytes(bs))
    }

    fn to_ne_bytes(&self) -> [u8; 4] {
        self.0.to_ne_bytes()
    }
}

impl std::ops::Mul for WlFixed {
    type Output = WlFixed;

    #[inline]
    fn mul(self, rhs: Self) -> Self::Output {
        let lhs: i64 = self.0.into();
        let rhs: i64 = rhs.0.into();
        let out = lhs * rhs >> 8;
        // we want this to panic if the i64->i32 doesn't work
        WlFixed(out.try_into().unwrap())
    }
}

macro_rules! delegate_op {
    ($t:ident $( $op:ident :: $f:ident ( $($a:ident),* ) )+) => { $(
        impl std::ops::$op for $t {
            type Output = $t;
            #[inline]
            fn $f(self $(, $a : Self)*) -> Self::Output {
                $t( self.0.$f( $($a.0),* ) )
            }
        }
    )* };
}
delegate_op! { WlFixed Neg::neg() Add::add(rhs) Sub::sub(rhs) }

pub trait WlHandler: 'static {
    fn handle(
        &self,
        reg: &ObjectRegistry,
        op: u16,
        buf: &mut [u8],
        fds: &mut Vec<RawFd>,
    ) -> Result<(), WlHandleError>;
    fn type_id(&self) -> std::any::TypeId;
}

impl dyn WlHandler {
    fn is<T: WlHandler>(&self) -> bool {
        let concrete = self.type_id();
        let ttid = std::any::TypeId::of::<T>();
        concrete == ttid
    }
}

impl WlHandler for () {
    fn handle(
        &self,
        reg: &ObjectRegistry,
        op: u16,
        buf: &mut [u8],
        fds: &mut Vec<RawFd>,
    ) -> Result<(), WlHandleError> {
        panic!("this should only be a placeholder in the object registry, which should not exist across polls")
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<()>()
    }
}

#[derive(Debug)]
pub enum WlHandleError {
    UnrecognizedOpcode(u16),
    Unhandled,
}

impl From<std::convert::Infallible> for WlHandleError {
    fn from(err: std::convert::Infallible) -> WlHandleError {
        match err {}
    }
}

trait WlDsr: Sized {
    type Error;
    fn dsr(
        reg: &ObjectRegistry,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), Self::Error>;
}

trait WlSer {
    type Error;
    fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), Self::Error>;
}

macro_rules! impl_dsr {
    ($t:ty) => {
        impl WlDsr for $t {
            type Error = std::array::TryFromSliceError;

            fn dsr(
                reg: &ObjectRegistry,
                buf: &mut [u8],
                fdbuf: &mut Vec<RawFd>,
            ) -> Result<(Self, usize), Self::Error> {
                let sz = std::mem::size_of::<$t>();
                let val = <$t>::from_ne_bytes(buf.try_into()?);
                Ok((val, sz))
            }
        }
    };
}

impl_dsr! { u32 }
impl_dsr! { i32 }
impl_dsr! { WlFixed }

impl WlDsr for WlArray {
    type Error = <u32 as WlDsr>::Error;

    fn dsr(
        reg: &ObjectRegistry,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), Self::Error> {
        let (z, n) = u32::dsr(reg, buf, fdbuf)?;
        let z = usize::try_from(z).unwrap();
        Ok((buf[n..(n + z)].to_vec(), n + z))
    }
}

impl WlDsr for WlStr {
    type Error = <WlArray as WlDsr>::Error;

    fn dsr(
        reg: &ObjectRegistry,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), Self::Error> {
        let (arr, sz) = WlArray::dsr(reg, buf, fdbuf)?;
        Ok((String::from_utf8(arr).unwrap(), sz))
    }
}

impl WlDsr for WlFd {
    type Error = ();

    fn dsr(
        reg: &ObjectRegistry,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), Self::Error> {
        match fdbuf.pop() {
            Some(fd) => Ok((WlFd(fd), 0)),
            None => Err(()),
        }
    }
}

impl<T: 'static> WlDsr for Option<WlRef<T>> {
    type Error = <u32 as WlDsr>::Error;

    fn dsr(
        reg: &ObjectRegistry,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), Self::Error> {
        let (id, sz) = u32::dsr(reg, buf, fdbuf)?;
        Ok((reg.make_ref(id), sz))
    }
}

pub enum WlPrimitiveSerError {
    InsufficientSpace(usize),
}

impl From<std::convert::Infallible> for WlPrimitiveSerError {
    fn from(i: std::convert::Infallible) -> Self {
        match i {}
    }
}
macro_rules! impl_ser {
    ($t:ty) => {
        impl WlSer for $t {
            type Error = WlPrimitiveSerError;

            fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), Self::Error> {
                let bs = self.to_ne_bytes();
                let rest = buf.spare_capacity_mut();
                if rest.len() < bs.len() {
                    return Err(WlPrimitiveSerError::InsufficientSpace(bs.len()));
                }

                // SAFETY: doing this until 'maybe_uninit_write_slice' is stabilized
                unsafe {
                    let rest: &mut [u8] = std::mem::transmute(rest);
                    rest[..bs.len()].copy_from_slice(&bs);
                    buf.set_len(buf.len() + bs.len());
                }
                Ok(())
            }
        }
    };
}

impl_ser! { u32 }
impl_ser! { i32 }
impl_ser! { WlFixed }

impl WlSer for WlArray {
    type Error = <u32 as WlSer>::Error;

    fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), Self::Error> {
        let sz: u32 = self.len().try_into().unwrap();
        sz.ser(buf, fdbuf)?;
        buf.extend_from_slice(self);
        Ok(())
    }
}

impl WlSer for WlStr {
    type Error = <WlArray as WlSer>::Error;

    fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), Self::Error> {
        let sz: u32 = self.len().try_into().unwrap();
        sz.ser(buf, fdbuf)?;
        buf.extend_from_slice(self.as_bytes());
        Ok(())
    }
}

impl WlSer for WlFd {
    type Error = std::convert::Infallible;

    fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), Self::Error> {
        fdbuf.push(self.0);
        Ok(())
    }
}

impl<T: 'static> WlSer for WlRef<T> {
    type Error = <u32 as WlSer>::Error;

    fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), Self::Error> {
        self.id.ser(buf, fdbuf)
    }
}

impl<T: 'static> WlSer for Option<WlRef<T>> {
    type Error = <WlRef<T> as WlSer>::Error;

    fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), Self::Error> {
        match self {
            Some(r) => r.ser(buf, fdbuf),
            None => 0u32.ser(buf, fdbuf),
        }
    }
}

include!(concat!(env!("OUT_DIR"), "/binding.rs"));
