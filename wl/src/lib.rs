#![allow(unused)]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::os::unix::io::RawFd;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::{Once, RwLock};

mod sockio;

struct BufStream {
    sock: UnixStream,
    buf: Vec<u8>,
    fdbuf: Vec<RawFd>,
}

pub struct WlClient {
    read: RefCell<BufStream>,
    write: RefCell<BufStream>,

    display: WlRef<WlDisplay>,
    registry: WlRef<WlRegistry>,

    bound: RefCell<Vec<RefCell<Option<Box<dyn Interface>>>>>,
    new_ids: RefCell<Vec<Box<dyn Interface>>>,
    globals: RefCell<Vec<(u32, WlStr, u32)>>,
}

impl WlClient {
    // To find the Unix socket to connect to, most implementations just do what libwayland does:
    //
    // 1. If WAYLAND_SOCKET is set, interpret it as a file descriptor number on which the connection is already established, assuming that the parent process configured the connection for us.
    // 2. If WAYLAND_DISPLAY is set, concat with XDG_RUNTIME_DIR to form the path to the Unix socket.
    // 3. Assume the socket name is wayland-0 and concat with XDG_RUNTIME_DIR to form the path to the Unix socket.
    // 4. Give up.
    fn find_socket() -> std::io::Result<UnixStream> {
        let sock = if let Ok(sock) = std::env::var("WAYLAND_SOCKET") {
            sock
        } else {
            let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap();
            if let Ok(disp) = std::env::var("WAYLAND_DISPLAY") {
                let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap();
                format!("{runtime_dir}/{disp}")
            } else {
                format!("{runtime_dir}/wayland-0")
            }
        };
        println!("connecting to {sock}");
        UnixStream::connect(sock)
    }

    fn print_error(
        _: &WlDisplay,
        _: &WlClient,
        object_id: WlRef<dyn Interface>,
        code: u32,
        message: WlStr,
    ) -> Result<(), WlHandleError> {
        eprintln!(
            "server: {object_id:?} {code}: {}",
            message.to_str().unwrap()
        );
        panic!("fatal error");
    }

    pub fn new() -> std::io::Result<Rc<RefCell<WlClient>>> {
        let display = WlDisplay::new(|display, cl, e| match e {
            wl_display::Event::Error {
                object_id,
                code,
                message,
            } => WlClient::print_error(display, cl, object_id, code, message),
            wl_display::Event::DeleteId { id } => {
                cl.destroy(id);
                Ok(())
            }
        });
        let registry = WlRegistry::new(|reg: &_, cl, e| match e {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => Ok(cl.global(name, interface, version)),
            wl_registry::Event::GlobalRemove { name } => Ok(cl.global_remove(name)),
        });

        let sock = WlClient::find_socket()?;
        let write_sock = sock.try_clone().unwrap();
        sock.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let cl = Rc::new_cyclic(|cl| {
            RefCell::new(WlClient {
                read: RefCell::new(BufStream {
                    sock,
                    buf: Vec::with_capacity(1024),
                    fdbuf: Vec::with_capacity(4),
                }),
                write: RefCell::new(BufStream {
                    sock: write_sock,
                    buf: Vec::with_capacity(1024),
                    fdbuf: Vec::with_capacity(4),
                }),

                display: WlRef {
                    cl: cl.clone(),
                    id: 1,
                    marker: PhantomData,
                },
                registry: WlRef {
                    cl: cl.clone(),
                    id: 0,
                    marker: PhantomData,
                },

                bound: RefCell::new(vec![RefCell::new(Some(Box::new(display)))]),
                new_ids: RefCell::new(vec![]),
                globals: RefCell::new(vec![]),
            })
        });

        let registry = cl.borrow().display.get_registry(registry).unwrap();
        cl.borrow_mut().registry = registry;
        let cb = cl
            .borrow()
            .display
            .sync(WlCallback::new(|me, cl, e| match e {
                wl_callback::Event::Done { callback_data } => Ok(println!("done getting globals")),
            }));
        Ok(cl)
    }

    pub fn bind_global<T>(&self, global: T) -> Result<WlRef<T>, WlSerError>
    where
        T: Interface,
        T: InterfaceDesc,
    {
        let name = {
            let name = &<T as InterfaceDesc>::NAME;
            let version = <T as InterfaceDesc>::VERSION;
            let globals = self.globals.borrow();

            if let Some((n, _, _)) = globals
                .iter()
                .find(|(_, i, v)| i.as_c_str() == *name && *v == version)
            {
                *n
            } else {
                return Err(WlSerError::BindWrongType);
            }
        };

        let registry = self.registry.clone();
        let rdyn = registry.bind(name, global)?;
        Ok(WlRef {
            cl: self.display.cl.clone(),
            id: rdyn.id,
            marker: PhantomData,
        })
    }

    pub fn describe_globals(&self) -> Vec<(String, u32)> {
        let globals = self.globals.borrow();
        globals
            .iter()
            .filter(|(n, _, _)| *n != 0)
            .map(|(_, i, v)| (i.to_str().unwrap().to_string(), *v))
            .collect()
    }

    fn send<T>(&self, r: &WlRef<T>, req: T::Request) -> Result<(), WlSerError>
    where
        T: InterfaceDesc,
        WlRef<T>: std::fmt::Debug,
        T::Request: std::fmt::Debug,
    {
        if wayland_debug_enabled() {
            println!("[{}] {:?} -> {:?}", timestamp(), r, req);
        }

        let bsock = &mut *self.write.borrow_mut();
        bsock.buf.truncate(0);
        bsock.buf.extend(r.id.to_ne_bytes());
        req.ser(&mut bsock.buf, &mut bsock.fdbuf)?;
        let n = sockio::sendmsg(&mut bsock.sock, &bsock.buf, &bsock.fdbuf);
        bsock.buf.truncate(0);
        bsock.fdbuf.truncate(0);
        if n < bsock.buf.len() {
            // not even sure we can retry the message here?
            // TODO: check wayland docs for what we can do on socket errors
            panic!("buffer underwrite");
        }
        Ok(())
    }

    fn print_buf_u32(buf: &[u8]) {
        for c in buf.chunks(4) {
            print!("{:08x} ", u32::from_ne_bytes(c.try_into().unwrap()));
        }
        println!();
    }

    pub fn poll(&self, blocking: bool) -> Result<Option<()>, ()> {
        let mut bsock = &mut *self.read.borrow_mut();
        bsock.sock.set_nonblocking(!blocking).map_err(|_| ())?;
        const WORD_SZ: usize = std::mem::size_of::<u32>();
        const HDR_SZ: usize = WORD_SZ * 2;
        let buf_len = bsock.buf.len();
        let fdbuf_len = bsock.fdbuf.len();

        let (bufn, fdbufn) = sockio::recvmsg(&mut bsock.sock, &mut bsock.buf, &mut bsock.fdbuf)?;
        if bufn == 0 && fdbufn == 0 {
            return Ok(None);
        }

        let mut i = 0;
        while bsock.buf[i..].len() > HDR_SZ {
            self.update_ids();
            let buf = &mut bsock.buf[i..];
            let id = u32::from_ne_bytes(buf[..WORD_SZ].try_into().unwrap());
            let (sz, op): (usize, u16) = {
                let szop = u32::from_ne_bytes(buf[WORD_SZ..HDR_SZ].try_into().unwrap());
                (
                    (szop >> 16).try_into().unwrap(),
                    (szop & 0xffff).try_into().unwrap(),
                )
            };
            if buf.len() < sz {
                break;
            }
            i += sz;

            let objid: usize = usize::try_from(id).unwrap();
            let hborrow = self.bound.borrow();
            let obj = hborrow.get(objid - 1).map(RefCell::borrow);
            let obj = match obj {
                Some(obj) if obj.is_some() => obj,
                _ => {
                    eprintln!("message for non-existent object (@{id})");
                    continue;
                }
            };
            let obj = obj.as_ref().unwrap();

            let buf = &mut buf[..sz];
            obj.handle(self, id, op, buf, &mut bsock.fdbuf)
                .map_err(|e| eprintln!("handle err: {e:?}"))?;
        }

        // move unhandled bytes to front of buf, truncate
        bsock.buf.copy_within(i.., 0);
        bsock.buf.truncate(bsock.buf.len() - i);
        self.update_ids();

        Ok(Some(()))
    }

    fn print_ids(&self) {
        println!(
            "ids: {}: {:?} (+{})",
            self.bound.borrow().len(),
            self.bound
                .borrow()
                .iter()
                .map(|c| c.borrow().is_some())
                .collect::<Vec<_>>(),
            self.new_ids.borrow().len(),
        );
    }

    fn update_ids(&self) {
        let new_ids = std::mem::replace(&mut *self.new_ids.borrow_mut(), vec![]);
        for h in new_ids.into_iter() {
            self.bound.borrow_mut().push(RefCell::new(Some(h)));
        }
    }

    pub fn new_id<T: Interface>(&self, mut init: T) -> WlRef<T> {
        let mut hs = self.bound.borrow();
        if let Some((i, v)) = hs.iter().enumerate().find(|(_, v)| (*v).borrow().is_none()) {
            v.borrow_mut().replace(Box::new(init));
            let r = WlRef {
                cl: self.display.cl.clone(),
                id: (i + 1).try_into().unwrap(),
                marker: PhantomData,
            };
            return r;
        }

        let i = hs.len();
        let mut hs = self.new_ids.borrow_mut();
        let i = i + hs.len();
        hs.push(Box::new(init));
        WlRef {
            cl: self.display.cl.clone(),
            id: (i + 1).try_into().unwrap(),
            marker: PhantomData,
        }
    }

    pub fn global(&self, name: u32, interface: WlStr, version: u32) {
        let mut globals = self.globals.borrow_mut();
        if let Some(v) = globals
            .iter_mut()
            .find(|(_, i, v)| i.as_bytes() == interface.to_bytes() && *v == version)
        {
            v.0 = name;
        } else {
            globals.push((name, interface, version));
        }
    }

    pub fn global_remove(&self, name: u32) {
        let mut globals = self.globals.borrow_mut();
        if let Some((i, _)) = globals.iter().enumerate().find(|(_, (n, _, _))| *n == name) {
            globals.remove(i);
        }
    }

    fn destroy(&self, id: u32) -> bool {
        let mut hs = self.bound.borrow();
        let id: usize = id.try_into().unwrap();
        match hs.get(id - 1) {
            Some(v) => {
                let mut v = v.borrow_mut();
                if v.is_some() {
                    v.take();
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn make_ref<T: 'static + ?Sized>(&self, id: u32) -> Option<WlRef<T>> {
        let id_u: usize = id.try_into().unwrap();
        let hs = self.bound.borrow();
        let mut dyn_h = match hs.get(id_u).map(RefCell::borrow) {
            Some(r) => r,
            None => {
                eprintln!("handlers[{id}] not initialized");
                return None;
            }
        };
        let dyn_h = dyn_h.as_ref()?;
        let ttid = std::any::TypeId::of::<T>();
        let concrete = (&**dyn_h).type_id();
        if ttid == std::any::TypeId::of::<dyn Interface>() || concrete == ttid {
            let marker = PhantomData;
            Some(WlRef {
                cl: self.display.cl.clone(),
                id,
                marker,
            })
        } else {
            None
        }
    }
}

pub struct WlRef<T: ?Sized> {
    cl: std::rc::Weak<RefCell<WlClient>>,
    id: u32,
    marker: PhantomData<T>,
}

impl<T: ?Sized> Clone for WlRef<T> {
    fn clone(&self) -> Self {
        WlRef {
            cl: self.cl.clone(),
            id: self.id,
            marker: self.marker,
        }
    }
}

impl std::fmt::Debug for WlRef<dyn Interface> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "object@{}", self.id)
    }
}
impl<T> std::fmt::Debug for WlRef<T>
where
    T: InterfaceDesc,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", T::NAME.to_str().unwrap(), self.id)
    }
}

impl<T: Interface> WlRef<T> {
    fn as_dyn(self) -> WlRef<dyn Interface> {
        WlRef {
            cl: self.cl,
            id: self.id,
            marker: PhantomData,
        }
    }
}

pub type WlStr = std::ffi::CString;
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

    fn to_ne_bytes(self) -> [u8; 4] {
        self.0.to_ne_bytes()
    }

    fn try_from_f32(n: f32) -> Option<WlFixed> {
        let bits = n.to_bits();
        let mut exp = ((bits & 0x7f80_0000) >> 23) as i32;
        let mut man = (bits & 0x807f_ffff) as i32;
        if exp == 0xff {
            // infinity or NaN, neither of which we can represent
            return None;
        }

        exp -= 127;
        if exp == 0 {
            if man == 0 {
                return Some(WlFixed(0));
            }
            // "subnormal" is 0.manbits
        } else {
            // normal floats are 1.manbits
            man |= 0x0080_0000;
        }

        // have
        // n = 2^exp * (2^-23 * man)
        // want
        // n = 2^-8 * man
        exp -= 23;
        // n = 2^exp * man
        let diff = exp + 8;
        man <<= diff;
        // man' = man * 2^(exp + 8)
        // n = 2^-8 * man' = 2^-8 * man * 2^(exp + 8) = 2^exp * man
        Some(WlFixed(man))
    }

    fn to_f32(self) -> f32 {
        let mut bits = self.0;
        if bits == 0 {
            return 0.0;
        }
        let mut exp = -8i32;
        while bits & 0x7f80_0000 != 0 {
            bits >>= 1;
            exp += 1;
        }
        while bits & 0x00C0_0000 == 0 {
            bits <<= 1;
            exp -= 1;
        }
        bits &= !0x00C0_0000; // remove the leading 1
        exp += 23;
        exp += 127;
        // exp started as -8, couldn't have gone more than +/- 23, so it could never be < 0
        // we only need to check for > 0xff
        if exp > 0xff {
            if bits > 0 {
                return f32::INFINITY;
            } else {
                return f32::NEG_INFINITY;
            }
        }
        let bits = bits as u32;
        let exp = ((exp & 0xff) as u32) << 23;
        f32::from_bits(bits | exp)
    }
}

impl std::ops::Mul for WlFixed {
    type Output = WlFixed;

    #[inline]
    fn mul(self, rhs: Self) -> Self::Output {
        let lhs: i64 = self.0.into();
        let rhs: i64 = rhs.0.into();
        let out = (lhs * rhs) >> 8;
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

pub trait InterfaceDesc {
    type Request: WlSer;
    type Event: WlDsr;
    const NAME: &'static std::ffi::CStr;
    const VERSION: u32;
}

pub trait Interface: 'static {
    fn handle(
        &self,
        cl: &WlClient,
        id: u32,
        op: u16,
        buf: &mut [u8],
        fds: &mut Vec<RawFd>,
    ) -> Result<(), WlHandleError>;

    fn type_id(&self) -> std::any::TypeId;
}

impl dyn Interface {
    fn is<T: Interface>(&self) -> bool {
        let concrete = self.type_id();
        let ttid = std::any::TypeId::of::<T>();
        concrete == ttid
    }
}

#[derive(Debug)]
pub enum WlHandleError {
    Deser(WlDsrError),
    Unhandled,
}

impl From<std::convert::Infallible> for WlHandleError {
    fn from(err: std::convert::Infallible) -> WlHandleError {
        match err {}
    }
}

fn wayland_debug_enabled() -> bool {
    std::env::var("WAYLAND_DEBUG")
        .map(|v| v.as_str() == "1")
        .unwrap_or(false)
}

pub trait WlDsr: Sized {
    fn dsr(
        cl: &WlClient,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), WlDsrError>;
}

pub trait WlSer {
    fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), WlSerError>;
}

#[derive(Debug)]
pub enum WlDsrError {
    MissingFd,
    Primitive(std::array::TryFromSliceError),
    UnrecognizedOpcode(u16),
}

macro_rules! impl_dsr {
    ($t:ty) => {
        impl WlDsr for $t {
            fn dsr(
                cl: &WlClient,
                buf: &mut [u8],
                fdbuf: &mut Vec<RawFd>,
            ) -> Result<(Self, usize), WlDsrError> {
                let sz = std::mem::size_of::<$t>();
                let val = <$t>::from_ne_bytes(buf[..sz].try_into().map_err(WlDsrError::Primitive)?);
                Ok((val, sz))
            }
        }
    };
}

impl_dsr! { u32 }
impl_dsr! { i32 }
impl_dsr! { WlFixed }

impl WlDsr for WlArray {
    fn dsr(
        cl: &WlClient,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), WlDsrError> {
        let (z, n) = u32::dsr(cl, buf, fdbuf)?;
        let z = usize::try_from(z).unwrap();
        Ok((buf[n..(n + z)].to_vec(), ((n + z) + 0x3) & !0x3))
    }
}

impl WlDsr for WlStr {
    fn dsr(
        cl: &WlClient,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), WlDsrError> {
        let (arr, sz) = WlArray::dsr(cl, buf, fdbuf)?;
        Ok((std::ffi::CString::from_vec_with_nul(arr).unwrap(), sz))
    }
}

impl WlDsr for WlFd {
    fn dsr(
        cl: &WlClient,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), WlDsrError> {
        match fdbuf.pop() {
            Some(fd) => Ok((WlFd(fd), 0)),
            None => Err(WlDsrError::MissingFd),
        }
    }
}

impl<T: 'static + ?Sized> WlDsr for WlRef<T> {
    fn dsr(
        cl: &WlClient,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), WlDsrError> {
        let (id, sz) = u32::dsr(cl, buf, fdbuf)?;
        Ok((cl.make_ref(id).expect("null ref"), sz))
    }
}

impl<T: 'static + ?Sized> WlDsr for Option<WlRef<T>> {
    fn dsr(
        cl: &WlClient,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), WlDsrError> {
        let (id, sz) = u32::dsr(cl, buf, fdbuf)?;
        Ok((cl.make_ref(id), sz))
    }
}

#[derive(Debug)]
pub enum WlSerError {
    InsufficientSpace(usize),
    BindWrongType,
    ClientUnavailable,
}

impl From<std::convert::Infallible> for WlSerError {
    fn from(i: std::convert::Infallible) -> Self {
        match i {}
    }
}
macro_rules! impl_ser {
    ($t:ty) => {
        impl WlSer for $t {
            fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), WlSerError> {
                let bs = self.to_ne_bytes();
                let rest = buf.spare_capacity_mut();
                if rest.len() < bs.len() {
                    return Err(WlSerError::InsufficientSpace(bs.len()));
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

fn ser_bytes(bs: &[u8], buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), WlSerError> {
    let sz: u32 = bs.len().try_into().unwrap();
    sz.ser(buf, fdbuf)?;
    buf.extend_from_slice(bs);
    if buf.len() % 4 != 0 {
        for i in 0..(4 - buf.len() % 4) {
            buf.push(0);
        }
    }
    Ok(())
}

impl WlSer for WlArray {
    fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), WlSerError> {
        ser_bytes(self.as_slice(), buf, fdbuf)
    }
}

impl WlSer for WlStr {
    fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), WlSerError> {
        let s = self.as_bytes_with_nul();
        ser_bytes(s, buf, fdbuf)
    }
}

impl WlSer for WlFd {
    fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), WlSerError> {
        fdbuf.push(self.0);
        Ok(())
    }
}

impl<T: ?Sized> WlSer for WlRef<T> {
    fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), WlSerError> {
        self.id.ser(buf, fdbuf)
    }
}

impl<T: 'static> WlSer for Option<WlRef<T>> {
    fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), WlSerError> {
        match self {
            Some(r) => r.ser(buf, fdbuf),
            None => 0u32.ser(buf, fdbuf),
        }
    }
}

pub fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

include!(concat!(env!("OUT_DIR"), "/binding.rs"));
