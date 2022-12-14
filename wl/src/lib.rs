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

pub struct BufStream {
    sock: UnixStream,
    buf: Vec<u8>,
    fdbuf: Vec<RawFd>,
}

// event handler needs &mut self, &mut writebuf
//   self.data = new_data        <- uses &mut self
//   self.other_ref.request(...) <- uses &mut writebuf
// poll needs &mut readbuf, &mut senders, &mut writebuf
//   obj = &mut senders[event_obj_id]
//   obj.handle(event...)        <- dynamically uses &mut writebuf
// request dispatch needs &self, &mut writebuf

pub struct WlClient {
    read: RefCell<BufStream>,
    write: Rc<RefCell<BufStream>>,

    pub display: WlRef<WlDisplay>,

    ids: Rc<IdSpace>,
    globals: RefCell<Vec<(u32, WlStr, u32)>>,
}

pub struct IdSpace {
    bound_tid: RefCell<Vec<std::any::TypeId>>,
    bound: RefCell<Vec<RefCell<Option<Box<dyn Interface>>>>>,
    created: RefCell<Vec<Box<dyn Interface>>>,
    available: RefCell<Vec<usize>>,
    me: std::rc::Weak<IdSpace>,
}

impl IdSpace {
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

    pub fn new_id<T: Interface>(
        &self,
        mut init: T,
        write_stream: std::rc::Weak<RefCell<BufStream>>,
    ) -> WlRef<T> {
        if let Some(i) = self.available.borrow_mut().pop() {
            self.bound_tid.borrow_mut()[i] = init.type_id();
            self.bound.borrow()[i].borrow_mut().replace(Box::new(init));
            let r = WlRef {
                wr: write_stream,
                ids: self.me.clone(),
                id: (i + 1).try_into().unwrap(),
                marker: PhantomData,
            };
            return r;
        }

        let mut hs = self.created.borrow_mut();
        let i = self.bound_tid.borrow().len();
        self.bound_tid.borrow_mut().push(init.type_id());
        hs.push(Box::new(init));
        WlRef {
            wr: write_stream,
            ids: self.me.clone(),
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
        eprintln!(
            "server: {object_id:?} {code}: {}",
            message.to_str().unwrap()
        );
        panic!("fatal error");
    }

    pub fn new() -> std::io::Result<Rc<WlClient>> {
        let display = WlDisplay::new(|display, e| match e {
            wl_display::Event::Error {
                object_id,
                code,
                message,
            } => WlClient::print_error(display, object_id, code, message),
            wl_display::Event::DeleteId { id } => {
                if let Some(ids) = display.ids.upgrade() {
                    ids.destroy(id);
                }
                Ok(())
            }
        });

        let sock = WlClient::find_socket()?;
        let write_sock = sock.try_clone().unwrap();
        sock.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let write_stream = Rc::new(RefCell::new(BufStream {
            sock: write_sock,
            buf: Vec::with_capacity(1024),
            fdbuf: Vec::with_capacity(4),
        }));
        let weak_write = Rc::downgrade(&write_stream);
        let ids = Rc::new_cyclic(|me| IdSpace {
            bound_tid: RefCell::new(vec![Interface::type_id(&display)]),
            bound: RefCell::new(vec![RefCell::new(Some(Box::new(display)))]),
            created: RefCell::new(vec![]),
            available: RefCell::new(vec![]),
            me: me.clone(),
        });

        let cl: Rc<WlClient> = Rc::new_cyclic(|cl| WlClient {
            read: RefCell::new(BufStream {
                sock,
                buf: Vec::with_capacity(1024),
                fdbuf: Vec::with_capacity(4),
            }),
            write: write_stream,

            display: WlRef {
                wr: weak_write.clone(),
                ids: Rc::downgrade(&ids),
                id: 1,
                marker: PhantomData,
            },

            ids,
            globals: RefCell::new(vec![]),
        });

        cl.ids.update_ids();

        cl.sync();
        Ok(cl)
    }

    pub fn sync(&self) {
        let done = Rc::new(RefCell::new(false));
        let cb_done = done.clone();
        let cb = self.display.sync(WlCallback::new(move |me, e| match e {
            wl_callback::Event::Done { callback_data } => {
                *cb_done.borrow_mut() = true;
                Ok(())
            }
        }));
        while !*done.borrow() {
            self.poll(true);
        }
    }

    pub fn describe_globals(&self) -> Vec<(String, u32)> {
        let globals = self.globals.borrow();
        globals
            .iter()
            .filter(|(n, _, _)| *n != 0)
            .map(|(_, i, v)| (i.to_str().unwrap().to_string(), *v))
            .collect()
    }

    fn send<T>(bsock: &mut BufStream, r: &WlRef<T>, req: T::Request) -> Result<(), WlSerError>
    where
        T: InterfaceDesc,
        WlRef<T>: std::fmt::Debug,
        T::Request: std::fmt::Debug,
    {
        if wayland_debug_enabled() {
            println!("[{}] {:?} -> {:?}", timestamp(), r, req);
        }

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

    pub fn poll(&self, blocking: bool) -> Option<()> {
        let mut bsock = &mut *self.read.borrow_mut();
        bsock
            .sock
            .set_nonblocking(!blocking)
            .expect("set nonblocking");
        const WORD_SZ: usize = std::mem::size_of::<u32>();
        const HDR_SZ: usize = WORD_SZ * 2;
        let buf_len = bsock.buf.len();
        let fdbuf_len = bsock.fdbuf.len();

        let (bufn, fdbufn) =
            sockio::recvmsg(&mut bsock.sock, &mut bsock.buf, &mut bsock.fdbuf).expect("recvmsg");
        if bufn == 0 && fdbufn == 0 {
            return None;
        }

        let mut i = 0;
        while bsock.buf[i..].len() > HDR_SZ {
            self.ids.update_ids();
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

            self.ids.with_mut_value(id, |obj| {
                let obj = match obj {
                    Some(obj) => obj,
                    _ => {
                        eprintln!("message for non-existent object (@{id})");
                        return;
                    }
                };

                let buf = &mut buf[..sz];
                obj.handle_event(self, id, buf, &mut bsock.fdbuf)
                    .expect("handle");
            });
        }

        // move unhandled bytes to front of buf, truncate
        bsock.buf.copy_within(i.., 0);
        bsock.buf.truncate(bsock.buf.len() - i);
        self.ids.update_ids();

        Some(())
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

    fn make_ref<T: Interface>(&self, id: u32) -> Option<WlRef<T>> {
        WlClient::static_make_ref(&self.ids, Rc::downgrade(&self.write), id)
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
                ids: ids.me.clone(),
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
    ids: std::rc::Weak<IdSpace>,
    id: u32,
    marker: PhantomData<T>,
}

impl<T: ?Sized> Clone for WlRef<T> {
    fn clone(&self) -> Self {
        WlRef {
            wr: self.wr.clone(),
            ids: self.ids.clone(),
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
            wr: self.wr,
            ids: self.ids,
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

pub trait Handler {
    type Event: WlDsr;
    fn on_event(me: WlRef<Self>, event: Self::Event) -> Result<(), WlHandleError>;
}

pub trait InterfaceDesc {
    type Request: WlSer;
    type Event: WlDsr;
    const NAME: &'static std::ffi::CStr;
    const VERSION: u32;
}

pub trait Interface: 'static {
    fn handle_event(
        &mut self,
        cl: &WlClient,
        id: u32,
        buf: &mut [u8],
        fds: &mut Vec<RawFd>,
    ) -> Result<(), WlHandleError>;

    fn type_id(&self) -> std::any::TypeId;

    fn name(&self) -> &'static std::ffi::CStr {
        unsafe { std::ffi::CStr::from_bytes_with_nul_unchecked(b"object\0") }
    }
    fn version(&self) -> u32 {
        0
    }
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
        let (arr, sz) = WlArray::dsr(ids, write_stream, buf, fdbuf)?;
        Ok((std::ffi::CString::from_vec_with_nul(arr).unwrap(), sz))
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
    ) -> Result<(Self, usize), WlDsrError> {
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
