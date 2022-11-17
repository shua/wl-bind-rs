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
use sockio::BufStream;

pub struct WlClient {
    read: RefCell<BufStream>,
    write: Rc<RefCell<BufStream>>,

    pub display: WlRef<WlDisplay>,

    pub ids: IdSpace,
}

#[derive(Default)]
pub struct IdSpace {
    bound_tid: RefCell<Vec<std::any::TypeId>>,
    bound_name: RefCell<Vec<&'static str>>,
    bound: RefCell<Vec<RefCell<Option<Box<dyn Interface>>>>>,
    created: RefCell<Vec<Box<dyn Interface>>>,
    available: RefCell<Vec<usize>>,

    resources: RefCell<Vec<Box<dyn Any>>>,
}

pub struct IdResource<T: ?Sized> {
    id: usize,
    m: PhantomData<T>,
}

impl<T: ?Sized> Clone for IdResource<T> {
    fn clone(&self) -> Self {
        IdResource {
            id: self.id,
            m: self.m,
        }
    }
}
impl<T: ?Sized> Copy for IdResource<T> {}

pub struct IdResourceRef<'id, T> {
    ids: std::cell::Ref<'id, Vec<Box<dyn Any>>>,
    id: IdResource<T>,
}
pub struct IdResourceMut<'id, T> {
    ids: std::cell::RefMut<'id, Vec<Box<dyn Any>>>,
    id: IdResource<T>,
}

impl<T> IdResource<T> {
    pub fn borrow<'id>(&self, ids: &'id IdSpace) -> IdResourceRef<'id, T> {
        IdResourceRef {
            ids: ids.resources.borrow(),
            id: *self,
        }
    }

    pub fn borrow_mut<'id>(&self, ids: &'id IdSpace) -> IdResourceMut<'id, T> {
        IdResourceMut {
            ids: ids.resources.borrow_mut(),
            id: *self,
        }
    }
}

impl<'r, T: 'static> std::ops::Deref for IdResourceRef<'r, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.ids[self.id.id].downcast_ref().unwrap()
    }
}
impl<'r, T: 'static> std::ops::Deref for IdResourceMut<'r, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.ids[self.id.id].downcast_ref().unwrap()
    }
}
impl<'r, T: 'static> std::ops::DerefMut for IdResourceMut<'r, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ids[self.id.id].downcast_mut().unwrap()
    }
}

impl IdSpace {
    pub fn new() -> IdSpace {
        IdSpace::default()
    }

    pub fn with_mut_value(&self, id: u32, f: impl FnOnce(Option<&mut Box<dyn Interface>>)) {
        let idx = usize::try_from(id - 1).unwrap();
        let bound = self.bound.borrow();
        if bound.len() > idx {
            let rcell = &self.bound.borrow()[idx];
            let mut rcell = rcell.borrow_mut();
            let v = rcell.as_mut();
            f(v);
        } else if idx < bound.len() + self.created.borrow().len() {
            let mut new_ids = self.created.borrow_mut();
            let v = &mut new_ids[idx - bound.len()];
            f(Some(v));
        } else {
            f(None);
        }
    }

    pub fn with_value(&self, id: u32, f: impl FnOnce(Option<&Box<dyn Interface>>)) {
        let idx = usize::try_from(id - 1).unwrap();
        let bound = self.bound.borrow();
        if bound.len() > idx {
            let rcell = &self.bound.borrow()[idx];
            let rcell = rcell.borrow();
            let v = rcell.as_ref();
            f(v);
        } else if idx < bound.len() + self.created.borrow().len() {
            let v = &self.created.borrow()[idx - bound.len()];
            f(Some(v));
        } else {
            f(None);
        }
    }

    pub fn resource<T: 'static>(&self, id: IdResource<T>) -> IdResourceRef<'_, T> {
        IdResourceRef {
            ids: self.resources.borrow(),
            id,
        }
    }
    pub fn resource_mut<T: 'static>(&self, id: IdResource<T>) -> IdResourceMut<'_, T> {
        IdResourceMut {
            ids: self.resources.borrow_mut(),
            id,
        }
    }

    pub fn with_mut_resource<T: 'static, R>(
        &self,
        id: &IdResource<T>,
        f: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        let mut res = self.resources.borrow_mut();
        let vany = res.get_mut(id.id)?;
        let val: &mut T = vany.downcast_mut().expect("value types to match");
        Some(f(val))
    }

    pub fn name_of(&self, id: u32) -> Option<&'static str> {
        let idx = usize::try_from(id - 1).unwrap();
        let bnames = self.bound_name.borrow();
        bnames.get(idx).copied()
    }

    pub fn type_matches(&self, id: u32, tid: &std::any::TypeId) -> bool {
        let idx = usize::try_from(id - 1).unwrap();
        let btids = self.bound_tid.borrow();
        btids.get(idx).map(|objtid| objtid == tid).unwrap_or(false)
    }

    pub fn update_ids(&self) {
        let new_ids = std::mem::replace(&mut *self.created.borrow_mut(), vec![]);
        for h in new_ids.into_iter() {
            self.bound.borrow_mut().push(RefCell::new(Some(h)));
        }
    }

    pub fn new_resource<T: Any>(&self, val: T) -> IdResource<T> {
        self.resources.borrow_mut().push(Box::new(val));
        IdResource {
            id: self.resources.borrow().len() - 1,
            m: PhantomData,
        }
    }

    pub fn new_resource_cyclic<T: Any>(
        &self,
        mk: impl FnOnce(IdResource<T>) -> T,
    ) -> IdResource<T> {
        let weak = self.new_resource(());
        let id = weak.id;
        let weak: IdResource<T> = IdResource { id, m: PhantomData };
        let val = Box::new(mk(weak));
        std::mem::replace(&mut self.resources.borrow_mut()[id], val);
        IdResource { id, m: PhantomData }
    }

    pub fn new_id<T: Interface>(
        &self,
        mut init: T,
        write_stream: std::rc::Weak<RefCell<BufStream>>,
    ) -> WlRef<T> {
        if let Some(i) = self.available.borrow_mut().pop() {
            self.bound_tid.borrow_mut()[i] = init.type_id();
            self.bound_name.borrow_mut()[i] = init.name();
            self.bound.borrow()[i].borrow_mut().replace(Box::new(init));
            let r = WlRef {
                wr: write_stream,
                id: (i + 1).try_into().unwrap(),
                marker: PhantomData,
            };
            return r;
        }

        let mut hs = self.created.borrow_mut();
        let i = self.bound_tid.borrow().len();
        self.bound_tid.borrow_mut().push(init.type_id());
        self.bound_name.borrow_mut().push(init.name());
        hs.push(Box::new(init));
        WlRef {
            wr: write_stream,
            id: (i + 1).try_into().unwrap(),
            marker: PhantomData,
        }
    }

    fn destroy(&self, id: u32) -> bool {
        let mut hs = self.bound.borrow();
        let idx: usize = (id - 1).try_into().unwrap();
        match hs.get(idx) {
            Some(v) => {
                if v.borrow().is_some() {
                    v.borrow_mut().take();
                    self.available.borrow_mut().push(idx);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
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
        _: WlRef<WlDisplay>,
        object_id: WlRef<dyn Interface>,
        code: u32,
        message: WlStr,
    ) -> Result<(), WlHandleError> {
        eprintln!("server: {object_id:?} {code}: {}", message);
        panic!("fatal error");
    }

    pub fn new() -> std::io::Result<Rc<WlClient>> {
        let display = WlDisplay::from_fn(|display, ids, e| match e {
            wl_display::Event::Error {
                object_id,
                code,
                message,
            } => WlClient::print_error(display, object_id, code, message),
            wl_display::Event::DeleteId { id } => {
                ids.destroy(id);
                Ok(())
            }
        });

        let sock = WlClient::find_socket()?;
        let write_sock = sock.try_clone().unwrap();
        sock.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let write_stream = Rc::new(RefCell::new(BufStream::new(write_sock)));
        let weak_write = Rc::downgrade(&write_stream);

        let ids = IdSpace::default();
        let display = ids.new_id(display, weak_write);

        let cl: Rc<WlClient> = Rc::new_cyclic(|cl| WlClient {
            read: RefCell::new(BufStream::new(sock)),
            write: write_stream,
            display,
            ids,
        });

        cl.ids.update_ids();

        cl.sync();
        Ok(cl)
    }

    pub fn sync(&self) {
        let done = self.ids.new_resource(false);
        let cb = self.display.sync(
            &self.ids,
            WlCallback::from_fn(move |me, ids, e| match e {
                wl_callback::Event::Done { callback_data } => {
                    ids.with_mut_resource(&done, |b| *b = true);
                    Ok(())
                }
            }),
        );
        let wr = std::rc::Rc::downgrade(&self.write);
        let mut bsock = self.read.borrow_mut();
        while !self.ids.with_mut_resource(&done, |b| *b).unwrap_or(true) {
            WlClient::static_poll(&mut *bsock, wr.clone(), &self.ids, true);
        }
    }

    fn send<T, R>(bsock: &mut BufStream, r: &WlRef<T>, req: R) -> Result<(), WlSerError>
    where
        T: InterfaceDesc,
        WlRef<T>: std::fmt::Debug,
        R: std::fmt::Debug,
        R: WlSer,
    {
        if wayland_debug_enabled() {
            println!("[{}] {:?} -> {:?}", timestamp(), r, req);
        }

        bsock.send(r.id, req)
    }

    fn print_buf_u32(buf: &[u8]) {
        for c in buf.chunks(4) {
            print!("{:08x} ", u32::from_ne_bytes(c.try_into().unwrap()));
        }
        println!();
    }

    pub fn poll(&self, nonblocking: bool) -> Option<()> {
        let wr = std::rc::Rc::downgrade(&self.write);
        let mut bsock = self.read.borrow_mut();
        WlClient::static_poll(&mut *bsock, wr.clone(), &self.ids, true)
    }

    pub fn static_poll(
        bsock: &mut BufStream,
        wr: std::rc::Weak<RefCell<BufStream>>,
        ids: &IdSpace,
        blocking: bool,
    ) -> Option<()> {
        bsock
            .sock
            .set_nonblocking(!blocking)
            .expect("set nonblocking");

        if let Some(mut msg) = bsock.recv() {
            ids.update_ids();
            let id = msg.id;
            let (buf, fdbuf) = msg.bufs();

            ids.with_mut_value(id, |obj| {
                let obj = match obj {
                    Some(obj) => obj,
                    _ => {
                        eprintln!("message for non-existent object (@{id})");
                        return;
                    }
                };

                match obj.handle_event(wr, &ids, id, buf, fdbuf) {
                    Err(WlHandleError::Unhandled) => {}
                    Err(err) => panic!("handle_event {}: {:?}", id, err),
                    Ok(_) => {}
                }
            });
        }

        ids.update_ids();

        Some(())
    }

    fn static_make_ref<T: Interface + ?Sized>(
        ids: &IdSpace,
        write_stream: std::rc::Weak<RefCell<BufStream>>,
        id: u32,
    ) -> Option<WlRef<T>> {
        let ttid = std::any::TypeId::of::<T>();
        let dyntid = std::any::TypeId::of::<dyn Interface>();
        if ttid == dyntid || ids.type_matches(id, &ttid) {
            Some(WlRef {
                wr: write_stream.clone(),
                id,
                marker: PhantomData,
            })
        } else {
            None
        }
    }
}

pub struct WlRef<T: ?Sized> {
    wr: std::rc::Weak<RefCell<BufStream>>,
    id: u32,
    marker: PhantomData<T>,
}

impl<T: ?Sized> Clone for WlRef<T> {
    fn clone(&self) -> Self {
        WlRef {
            wr: self.wr.clone(),
            id: self.id,
            marker: self.marker,
        }
    }
}

impl std::fmt::Debug for WlRef<dyn Interface> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", "object", self.id)
    }
}
impl<T> std::fmt::Debug for WlRef<T>
where
    T: InterfaceDesc,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", T::NAME, self.id)
    }
}

impl<T: Interface> WlRef<T> {
    fn as_dyn(self) -> WlRef<dyn Interface> {
        WlRef {
            wr: self.wr,
            id: self.id,
            marker: PhantomData,
        }
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
    const NAME: &'static str;
    const VERSION: u32;
}

pub trait Interface: 'static {
    fn handle_event(
        &mut self,
        wr: std::rc::Weak<RefCell<BufStream>>,
        ids: &IdSpace,
        id: u32,
        buf: &mut [u8],
        fds: &mut Vec<RawFd>,
    ) -> Result<(), WlHandleError>;

    fn type_id(&self) -> std::any::TypeId;

    fn name(&self) -> &'static str {
        "object"
    }
    fn version(&self) -> u32 {
        0
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
        ids: &IdSpace,
        write_stream: &std::rc::Weak<RefCell<BufStream>>,
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

impl From<RawFd> for WlFd {
    fn from(value: RawFd) -> Self {
        WlFd(value)
    }
}

macro_rules! impl_dsr {
    ($t:ty) => {
        impl WlDsr for $t {
            fn dsr(
                ids: &IdSpace,
                write_stream: &std::rc::Weak<RefCell<BufStream>>,
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
        ids: &IdSpace,
        write_stream: &std::rc::Weak<RefCell<BufStream>>,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), WlDsrError> {
        let (z, n) = u32::dsr(ids, write_stream, buf, fdbuf)?;
        let z = usize::try_from(z).unwrap();
        Ok((buf[n..(n + z)].to_vec(), ((n + z) + 0x3) & !0x3))
    }
}

impl WlDsr for WlStr {
    fn dsr(
        ids: &IdSpace,
        write_stream: &std::rc::Weak<RefCell<BufStream>>,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), WlDsrError> {
        let (mut arr, sz) = WlArray::dsr(ids, write_stream, buf, fdbuf)?;
        assert_eq!(arr.pop(), Some(b'\0'));
        Ok((String::from_utf8(arr).unwrap(), sz))
    }
}

impl WlDsr for WlFd {
    fn dsr(
        ids: &IdSpace,
        write_stream: &std::rc::Weak<RefCell<BufStream>>,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), WlDsrError> {
        match fdbuf.pop() {
            Some(fd) => Ok((WlFd(fd), 0)),
            None => Err(WlDsrError::MissingFd),
        }
    }
}

impl<T: Interface + ?Sized> WlDsr for WlRef<T> {
    fn dsr(
        ids: &IdSpace,
        write_stream: &std::rc::Weak<RefCell<BufStream>>,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(WlRef<T>, usize), WlDsrError> {
        let (id, sz) = u32::dsr(ids, write_stream, buf, fdbuf)?;
        Ok((
            WlClient::static_make_ref(ids, write_stream.clone(), id).expect("null ref"),
            sz,
        ))
    }
}

impl<T: Interface + ?Sized> WlDsr for Option<WlRef<T>> {
    fn dsr(
        ids: &IdSpace,
        write_stream: &std::rc::Weak<RefCell<BufStream>>,
        buf: &mut [u8],
        fdbuf: &mut Vec<RawFd>,
    ) -> Result<(Self, usize), WlDsrError> {
        let (id, sz) = u32::dsr(ids, write_stream, buf, fdbuf)?;
        Ok((WlClient::static_make_ref(ids, write_stream.clone(), id), sz))
    }
}

#[derive(Debug)]
pub enum WlSerError {
    InsufficientSpace(usize),
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
        let bs = self.as_bytes();
        let sz: u32 = bs.len().try_into().unwrap();
        let sz = sz + 1;
        sz.ser(buf, fdbuf)?;
        buf.extend_from_slice(bs);
        buf.push(0);
        if buf.len() % 4 != 0 {
            for i in 0..(4 - buf.len() % 4) {
                buf.push(0);
            }
        }
        Ok(())
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
