#![allow(non_camel_case_types)]

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::convert::{TryFrom as _, TryInto as _};
use std::fmt::{self, Debug};
use std::io::Write as _;
use std::os::fd::{AsRawFd as _, FromRawFd as _, IntoRawFd as _, OwnedFd, RawFd};
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub struct fixed(u32);

#[derive(Clone, Copy)]
pub struct object<T, const NEW: bool = false>(u32, T);
pub type new_id<T> = object<T, true>;

impl<T> object<T, true> {
    pub fn to_object(self) -> object<T> {
        object(self.0, self.1)
    }
}

impl<T: Debug, const NEW: bool> Debug for object<T, NEW> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if NEW {
            f.write_str("new ")?;
        }
        write!(f, "{:?}@{}", self.1, self.0)
    }
}

#[derive(Debug, Clone)]
pub struct Dyn {
    name: String,
    version: u32,
}

static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
pub fn wl_debug_enabled() -> bool {
    std::env::var_os("WAYLAND_DEBUG").map(|s| !s.is_empty()).unwrap_or(false)
}
macro_rules! debug {
    ($fmt:literal $(, $arg:expr)* $(,)?) => {
        if wl_debug_enabled() {
            let now = START.get_or_init(std::time::Instant::now).elapsed();
            let now = now.as_secs_f32() + (now.subsec_millis() as f32 / 1000.0);
            print!("[{now:.3}]: ");
            println!($fmt $(,$arg)*);
        }
    }
}

impl<T: Object, const NEW: bool> object<T, NEW> {
    pub fn to_dyn(self, version: u32) -> object<Dyn, NEW> {
        object(self.0, T::new_dyn(version))
    }
}

pub trait Ser {
    fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str>;
}

pub trait Dsr: Sized {
    fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str>;
}

impl Ser for u32 {
    fn serialize(self, mut buf: &mut [u8], _fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
        let bs = self.to_ne_bytes();
        let len = bs.len();
        buf.write(&bs).map_err(|_| "write err")?;
        Ok(len)
    }
}

impl Ser for i32 {
    fn serialize(self, mut buf: &mut [u8], _fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
        let bs = self.to_ne_bytes();
        let len = bs.len();
        buf.write(&bs).map_err(|_| "write err")?;
        Ok(len)
    }
}

impl Ser for fixed {
    fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
        self.0.serialize(buf, fds)
    }
}

impl Ser for OwnedFd {
    fn serialize(self, _buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
        fds.push(self);
        Ok(0)
    }
}

impl<T: Ser, const NEW: bool> Ser for object<T, NEW> {
    fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
        let n = self.1.serialize(buf, fds)?;
        let m = self.0.serialize(&mut buf[n..], fds)?;
        Ok(n + m)
    }
}

impl<T: Ser, const NEW: bool> Ser for Option<object<T, NEW>> {
    fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
        if let Some(obj) = self {
            obj.0.serialize(buf, fds)
        } else {
            0u32.serialize(buf, fds)
        }
    }
}

impl<'s> Ser for &'s str {
    fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
        let slen: u32 = self.len().try_into().unwrap();
        let mut i = (slen + 1).serialize(buf, fds)?;
        (&mut buf[i..]).write(self.as_bytes()).map_err(|_| "write err")?;
        i += self.len();
        (&mut buf[i..]).write(&[0u8]).map_err(|_| "write err")?;
        i += 1;
        if i % 4 != 0 {
            let pad = 4 - (i % 4);
            (&mut buf[i..]).write(&[0, 0, 0, 0][..pad]).map_err(|_| "write err")?;
            i += pad;
        }
        Ok(i)
    }
}

impl<'s> Ser for &'s [u8] {
    fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
        let slen: u32 = self.len().try_into().unwrap();
        let mut i = slen.serialize(buf, fds)?;
        (&mut buf[i..]).write(self).map_err(|_| "write err")?;
        i += self.len();
        if i % 4 != 0 {
            let pad = 4 - (i % 4);
            (&mut buf[i..]).write(&[0, 0, 0, 0][..pad]).map_err(|_| "write err")?;
            i += pad;
        }
        Ok(i)
    }
}

impl Ser for Dyn {
    fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
        let n = self.name.serialize(buf, fds)?;
        let m = self.version.serialize(&mut buf[n..], fds)?;
        Ok(n + m)
    }
}

impl Ser for () {
    fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
        Ok(0)
    }
}

impl Dsr for u32 {
    fn deserialize(buf: &[u8], _fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
        const N: usize = std::mem::size_of::<u32>();
        let Some(buf) = buf.get(..N) else {
            return Err("dsr:u32: unexpected end of buffer");
        };
        let n = u32::from_ne_bytes(buf.try_into().unwrap());
        Ok((n, N))
    }
}

impl Dsr for i32 {
    fn deserialize(buf: &[u8], _fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
        const N: usize = std::mem::size_of::<i32>();
        let Some(buf) = buf.get(..N) else {
            return Err("dsr:i32: unexpected end of buffer");
        };
        let n = i32::from_ne_bytes(buf.try_into().unwrap());
        Ok((n, N))
    }
}

impl Dsr for fixed {
    fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
        let (n, len) = u32::deserialize(buf, fds)?;
        Ok((fixed(n), len))
    }
}

impl Dsr for OwnedFd {
    fn deserialize(_buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
        let Some(fd) = fds.pop() else {
            return Err("unexpected end of fds");
        };
        Ok((fd, 0))
    }
}

impl Dsr for String {
    fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
        let (n, mut i) = u32::deserialize(buf, fds)?;
        let n = usize::try_from(n).unwrap();
        if buf[i..].len() < n {
            return Err("dsr:str: unexpected end of buffer");
        }
        let s = std::str::from_utf8(&buf[i..][..(n - 1)]).map_err(|_| "invalid string")?;
        i += s.len();
        if i % 4 != 0 {
            let pad = 4 - i % 4;
            if buf[i..].len() < pad {
                return Err("dsr:str: unexpected end of buffer");
            }
            i += pad;
        }
        Ok((s.to_string(), i))
    }
}

impl Dsr for Vec<u8> {
    fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
        let (n, mut i) = u32::deserialize(buf, fds)?;
        let n = usize::try_from(n).unwrap();
        if buf[i..].len() < n {
            return Err("dsr:array: unexpected end of buffer");
        }
        let v = Vec::from(&buf[i..][..n]);
        i += v.len();
        if i % 4 != 0 {
            let pad = 4 - i % 4;
            if buf[i..].len() < pad {
                return Err("dsr:array: unexpected end of buffer");
            }
            i += pad;
        }
        Ok((v, i))
    }
}

impl<T: Dsr, const NEW: bool> Dsr for object<T, NEW> {
    fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
        let (v, m) = T::deserialize(buf, fds)?;
        let (n, len) = u32::deserialize(&buf[m..], fds)?;
        if n == 0 {
            Err("null object where none was expected")
        } else {
            Ok((object(n, v), len + m))
        }
    }
}

impl<T: Dsr, const NEW: bool> Dsr for Option<object<T, NEW>> {
    fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
        let (n, len) = u32::deserialize(buf, fds)?;
        if n == 0 {
            Ok((None, len))
        } else {
            let (v, m) = T::deserialize(buf, fds)?;
            Ok((Some(object(n, v)), len + m))
        }
    }
}

impl Dsr for Dyn {
    fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
        let (name, n) = String::deserialize(buf, fds)?;
        let (version, m) = u32::deserialize(&buf[n..], fds)?;
        Ok((Dyn { name, version }, n + m))
    }
}

impl Dsr for () {
    fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
        Ok(((), 0))
    }
}

pub trait Msg<T>: Ser + Dsr + std::fmt::Debug {
    const OP: u16;
    const SINCE: u32 = 1;
    fn new_id(&self) -> Option<(u32, ObjMeta)> {
        None
    }
}
pub trait Request<T>: Msg<T> {}
pub trait Event<T>: Msg<T> {}

pub trait Object: Any + std::fmt::Debug + Ser + Dsr {
    const DESTRUCTOR: Option<u16> = None;
    fn new_obj() -> Self;
    fn new_dyn(version: u32) -> Dyn;
}

pub struct ObjMeta {
    alive: bool,
    destructor: Option<u16>,
    type_id: TypeId,
}

pub struct Client {
    conn: OwnedFd,
    txbuf: Vec<u8>,
    txfds: Vec<OwnedFd>,
    rxbuf: Vec<u8>,
    rxbufi: usize,
    rxfds: Vec<OwnedFd>,
    maxid: u32,
    objs: HashMap<u32, ObjMeta>,
    deleted_ids: Arc<RefCell<Vec<u32>>>,
    listeners: Vec<Box<dyn for<'r> FnMut(RecvState<'r>) -> RecvState<'r>>>,
}

fn conn_send(conn: &OwnedFd, buf: &[u8], fds: &mut Vec<OwnedFd>) -> std::io::Result<()> {
    const RAWFD_SZ: usize = std::mem::size_of::<RawFd>();
    let fdlen = fds.len();

    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut libc::iovec { iov_base: buf.as_ptr().cast_mut().cast(), iov_len: buf.len() };
    msg.msg_iovlen = 1;

    let mut msg_control = [0u8; RAWFD_SZ * 10];
    if fdlen > 0 {
        let cmsg_space = unsafe { libc::CMSG_SPACE((fdlen * RAWFD_SZ) as u32) as usize };
        if msg_control.len() < cmsg_space as usize {
            return Err(std::io::Error::from_raw_os_error(libc::ENOMEM));
        }

        msg.msg_control = msg_control.as_mut_ptr().cast();
        msg.msg_controllen = cmsg_space;

        let cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        assert!(!cmsg.is_null(), "msg_control is not null, how is cmsg?");
        let data_ptr: *mut RawFd = unsafe { libc::CMSG_DATA(cmsg) }.cast();

        let fd = fds.pop().unwrap();
        let fd = fd.into_raw_fd();
        unsafe { *data_ptr = fd };

        // SAFETY: we know cmsg is a valid pointer, but it may be filled
        // with uninit memory. It is safe to write to in order to initialize it.
        *(unsafe { cmsg.as_mut() }).unwrap() = libc::cmsghdr {
            cmsg_len: unsafe { libc::CMSG_LEN((fdlen * RAWFD_SZ) as u32) as usize },
            cmsg_level: libc::SOL_SOCKET,
            cmsg_type: libc::SCM_RIGHTS,
        };
    }

    //print_hex_u32(data);

    // SAFETY: even with *mut _ inside msghdr struct, sendmsg should not change any of them
    let ret = {
        let val = unsafe { libc::sendmsg(conn.as_raw_fd(), &msg, 0) };
        if val >= 0 {
            Ok(val)
        } else {
            Err(std::io::Error::last_os_error())
        }
    };
    //println!("sendmsg({}, ..) = {ret:?}", conn.as_raw_fd());

    ret.map(|_| ())
}

fn conn_recv(
    conn: &OwnedFd,
    buf: &mut [u8],
    fds: &mut Vec<OwnedFd>,
) -> std::io::Result<(usize, usize)> {
    const RAWFD_SZ: usize = std::mem::size_of::<RawFd>();
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
    msg.msg_iovlen = 1;

    let mut msg_control = [0u8; RAWFD_SZ * 10];
    msg.msg_control = msg_control.as_mut_ptr().cast();
    msg.msg_controllen = msg_control.len();
    let ret = unsafe { libc::recvmsg(conn.as_raw_fd(), &mut msg, 0) };
    if ret < 0 {
        return Err(std::io::Error::last_os_error());
    }
    //debug!("recvmsg(..) = {ret:?}");

    let resdata_len = usize::try_from(ret).unwrap();

    let fds_len = fds.len();
    let mut cmsg_ptr = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    while let Some(cmsg) = unsafe { cmsg_ptr.as_ref() } {
        if cmsg.cmsg_level == libc::SOL_SOCKET && cmsg.cmsg_type == libc::SCM_RIGHTS {
            let buf: *mut RawFd = unsafe { libc::CMSG_DATA(cmsg_ptr) }.cast();
            if let (Some(&raw_fd), true) = (unsafe { buf.as_ref() }, cmsg.cmsg_len > 0) {
                fds.push(unsafe { OwnedFd::from_raw_fd(raw_fd) });
                break;
            }
        }
        cmsg_ptr = unsafe { libc::CMSG_NXTHDR(&msg, cmsg) };
    }
    Ok((resdata_len, fds.len() - fds_len))
}

impl Client {
    pub fn connect() -> Result<(Client, object<wl_display::wl_display>), &'static str> {
        use std::env::var as env;
        let runtime_dir = env("XDG_RUNTIME_DIR").unwrap();
        let sock = match (env("WAYLAND_SOCKET"), env("WAYLAND_DISPLAY")) {
            (Ok(sock), _) => sock,
            (_, Ok(disp)) => format!("{runtime_dir}/{disp}"),
            _ => format!("{runtime_dir}/wayland-0"),
        };
        let sock =
            std::os::unix::net::UnixStream::connect(sock).expect("connect to wayland socket");
        sock.set_nonblocking(false).expect("set nonblocking");

        let conn = sock.into();

        let mut c = Client {
            conn,
            txbuf: vec![0; 1024],
            txfds: Vec::with_capacity(1),
            rxbuf: vec![0; 1024],
            rxbufi: 1024,
            rxfds: Vec::with_capacity(1),
            maxid: 2,
            objs: HashMap::new(),
            deleted_ids: Arc::new(RefCell::new(vec![])),
            listeners: vec![],
        };
        let display = object(1, wl_display::wl_display);
        let display_meta = ObjMeta {
            alive: true,
            destructor: None,
            type_id: TypeId::of::<wl_display::wl_display>(),
        };
        c.objs.insert(display.0, display_meta);
        c.listen(display, |wl_display::event::error { object_id, code, message }| {
            eprintln!("error {object_id:?}:{code}: {message}");
            Ok(())
        });
        let delids = Arc::downgrade(&c.deleted_ids);
        c.listen(display, move |wl_display::event::delete_id { id }| {
            let Some(delids) = delids.upgrade() else {
                return Ok(());
            };
            delids.borrow_mut().push(id);
            Ok(())
        });
        Ok((c, display))
    }

    pub fn new_id<T: Object>(&mut self) -> new_id<T> {
        let id;
        if let Some(delid) = self.deleted_ids.borrow_mut().pop() {
            id = delid;
        } else {
            id = self.maxid;
            self.maxid += 1;
        }
        self.objs.insert(
            id,
            ObjMeta { alive: true, destructor: T::DESTRUCTOR, type_id: TypeId::of::<T>() },
        );
        object(id, T::new_obj())
    }

    pub fn construct<T, N, M, F>(&mut self, obj: object<T>, f: F) -> Result<object<N>, &'static str>
    where
        T: Object,
        N: Object,
        M: Request<T>,
        F: FnOnce(new_id<N>) -> M,
    {
        let nid = self.new_id();
        let id = object(nid.0, N::new_obj());
        let msg = f(nid);
        self.send(obj, msg)?;
        Ok(id)
    }

    pub fn send<T: std::fmt::Debug + Ser, M: Request<T>>(
        &mut self,
        obj: object<T>,
        msg: M,
    ) -> Result<(), &'static str> {
        debug!("{:?} {:?} ->", obj, msg);
        let mut n = obj.serialize(&mut self.txbuf, &mut self.txfds)?;
        let szi = n;
        n += 0u32.serialize(&mut self.txbuf[n..], &mut self.txfds)?;
        n += msg.serialize(&mut self.txbuf[n..], &mut self.txfds)?;
        let szop = u32::from(u16::try_from(n).unwrap());
        let szop = (szop << 16) | u32::from(M::OP);
        szop.serialize(&mut self.txbuf[szi..], &mut self.txfds)?;
        conn_send(&self.conn, &self.txbuf[..n], &mut self.txfds).map_err(|e| {
            eprintln!("error: {e}");
            "conn send error"
        })
    }

    pub fn recv<'r>(&'r mut self) -> Result<RecvState<'r>, &'static str> {
        if self.rxbufi >= self.rxbuf.len() {
            self.rxfds.reverse(); // FIFO behaviour on pop
            self.rxbuf
                .extend(std::iter::repeat(0u8).take(self.rxbuf.capacity() - self.rxbuf.len()));
            self.rxbufi = 0;

            let mut bufn = 0;
            loop {
                let (bn, fdn) = conn_recv(&self.conn, &mut self.rxbuf[bufn..], &mut self.rxfds)
                    .map_err(|e| {
                        eprintln!("conn_recv:{e}");
                        "conn recv error"
                    })?;
                bufn += bn;
                if bufn < self.rxbuf.len() {
                    break;
                }
                self.rxbuf.extend(std::iter::repeat(0u8).take(self.rxbuf.len()));
            }
            self.rxfds.reverse(); // FIFO behaviour on pop
            self.rxbuf.truncate(bufn);
        }

        if self.rxbuf.len() == 0 {
            return Ok(RecvState::None);
        }

        let rxbuf = &self.rxbuf[self.rxbufi..];
        let (id, mut n) = u32::deserialize(&rxbuf, &mut self.rxfds)?;
        let (szop, m) = u32::deserialize(&rxbuf[n..], &mut self.rxfds)?;
        n += m;
        let sz = usize::try_from((szop >> 16) & 0xffff).unwrap();
        let op = u16::try_from(szop & 0xffff).unwrap();
        self.rxbufi += sz;
        //debug!("recv {id} {op} {sz}");
        let objs = &mut self.objs;
        let new_id = Box::new(move |id, meta| {
            objs.insert(id, meta);
        });
        let mut recv =
            RecvState::Some(Recv { new_id, id, op, buf: &rxbuf[..sz][n..], fds: &mut self.rxfds });
        for l in &mut self.listeners {
            recv = l(recv);
            let RecvState::Some(_) = recv else {
                return Ok(recv);
            };
        }
        Ok(recv)
    }

    pub fn listen<T, M, CB>(&mut self, obj: object<T>, mut cb: CB)
    where
        T: std::fmt::Debug + 'static,
        M: Event<T>,
        CB: FnMut(M) -> Result<(), &'static str> + 'static,
    {
        self.listeners.push(Box::new(move |recv| {
            let RecvState::Some(recv) = recv else {
                return recv;
            };
            if recv.id != obj.0 || recv.op != M::OP {
                return RecvState::Some(recv);
            }
            let Recv { id, mut new_id, buf, fds, .. } = recv;
            let msg = match M::deserialize(buf, fds) {
                Ok((msg, _)) => msg,
                Err(err) => return RecvState::Done(Err(err)),
            };
            debug!("{:?} <- {:?}", obj, msg);
            if let Some((id, meta)) = msg.new_id() {
                new_id(id, meta);
            }
            RecvState::Done(cb(msg))
        }));
    }
}

pub struct Recv<'r> {
    new_id: Box<dyn FnMut(u32, ObjMeta) + 'r>,
    id: u32,
    op: u16,
    buf: &'r [u8],
    fds: &'r mut Vec<OwnedFd>,
}

impl std::fmt::Debug for Recv<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Recv")
            .field("id", &self.id)
            .field("op", &self.op)
            .field("buf", &self.buf)
            .field("fds", &self.fds)
            .finish()
    }
}
pub enum RecvState<'r> {
    None,
    Some(Recv<'r>),
    Done(Result<(), &'static str>),
}

impl RecvState<'_> {
    pub fn on<T, M, CB>(self, obj: object<T>, cb: CB) -> Self
    where
        T: std::fmt::Debug,
        M: Event<T>,
        CB: FnOnce(M),
    {
        let RecvState::Some(recv) = self else {
            return self;
        };
        if recv.id != obj.0 || recv.op != M::OP {
            return RecvState::Some(recv);
        }
        let Recv { mut new_id, buf, fds, .. } = recv;
        let msg = match M::deserialize(buf, fds) {
            Ok((msg, _)) => msg,
            Err(err) => return RecvState::Done(Err(err)),
        };
        debug!("{:?} <- {:?}", obj, msg);
        if let Some((id, meta)) = msg.new_id() {
            new_id(id, meta);
        }
        cb(msg);
        RecvState::Done(Ok(()))
    }
}
/// core global object
///
/// The core global object.  This is a special singleton object.  It
/// is used for internal Wayland protocol features.
///
pub mod wl_display {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_display";
    pub const VERSION: u32 = 1;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// asynchronous roundtrip
        ///
        /// The sync request asks the server to emit the 'done' event
        /// on the returned wl_callback object.  Since requests are
        /// handled in-order and events are delivered in-order, this can
        /// be used as a barrier to ensure all previous requests and the
        /// resulting events have been handled.
        ///
        /// The object returned by this request will be destroyed by the
        /// compositor after the callback is fired and as such the client must not
        /// attempt to use it after that point.
        ///
        /// The callback_data passed in the callback is the event serial.
        ///
        #[derive(Debug)]
        pub struct sync {
            // callback object for the sync request
            pub callback: new_id<wl_callback::Obj>,
        }
        impl Ser for sync {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.callback.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for sync {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (callback, m) = <new_id<wl_callback::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((sync { callback }, n))
            }
        }
        impl Msg<Obj> for sync {
            const OP: u16 = 0;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.callback.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_callback::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_callback::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for sync {}
        /// get global registry object
        ///
        /// This request creates a registry object that allows the client
        /// to list and bind the global objects available from the
        /// compositor.
        ///
        /// It should be noted that the server side resources consumed in
        /// response to a get_registry request can only be released when the
        /// client disconnects, not when the client side proxy is destroyed.
        /// Therefore, clients should invoke get_registry as infrequently as
        /// possible to avoid wasting memory.
        ///
        #[derive(Debug)]
        pub struct get_registry {
            // global registry object
            pub registry: new_id<wl_registry::Obj>,
        }
        impl Ser for get_registry {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.registry.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for get_registry {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (registry, m) = <new_id<wl_registry::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((get_registry { registry }, n))
            }
        }
        impl Msg<Obj> for get_registry {
            const OP: u16 = 1;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.registry.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_registry::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_registry::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for get_registry {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// fatal error event
        ///
        /// The error event is sent out when a fatal (non-recoverable)
        /// error has occurred.  The object_id argument is the object
        /// where the error occurred, most often in response to a request
        /// to that object.  The code identifies the error and is defined
        /// by the object interface.  As such, each interface defines its
        /// own set of error codes.  The message is a brief description
        /// of the error, for (debugging) convenience.
        ///
        #[derive(Debug)]
        pub struct error {
            // object where the error occurred
            pub object_id: object<()>,
            // error code
            pub code: u32,
            // error description
            pub message: String,
        }
        impl Ser for error {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.object_id.serialize(&mut buf[n..], fds)?;
                n += self.code.serialize(&mut buf[n..], fds)?;
                n += self.message.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for error {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (object_id, m) = <object<()>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (code, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (message, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((error { object_id, code, message }, n))
            }
        }
        impl Msg<Obj> for error {
            const OP: u16 = 0;
        }
        impl Event<Obj> for error {}
        /// acknowledge object ID deletion
        ///
        /// This event is used internally by the object ID management
        /// logic. When a client deletes an object that it had created,
        /// the server will send this event to acknowledge that it has
        /// seen the delete request. When the client receives this event,
        /// it will know that it can safely reuse the object ID.
        ///
        #[derive(Debug)]
        pub struct delete_id {
            // deleted object ID
            pub id: u32,
        }
        impl Ser for delete_id {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for delete_id {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((delete_id { id }, n))
            }
        }
        impl Msg<Obj> for delete_id {
            const OP: u16 = 1;
        }
        impl Event<Obj> for delete_id {}
    }
    /// global error values
    ///
    /// These errors are global and can be emitted in response to any
    /// server request.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// server couldn't find object
        invalid_object = 0,
        /// method doesn't exist on the specified interface or malformed request
        invalid_method = 1,
        /// server is out of memory
        no_memory = 2,
        /// implementation error in compositor
        implementation = 3,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::invalid_object => 0,
                error::invalid_method => 1,
                error::no_memory => 2,
                error::implementation => 3,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::invalid_object),
                1 => Ok(error::invalid_method),
                2 => Ok(error::no_memory),
                3 => Ok(error::implementation),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_display;
    pub type Obj = wl_display;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_display;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = None;
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// global registry object
///
/// The singleton global registry object.  The server has a number of
/// global objects that are available to all clients.  These objects
/// typically represent an actual object in the server (for example,
/// an input device) or they are singleton objects that provide
/// extension functionality.
///
/// When a client creates a registry object, the registry object
/// will emit a global event for each global currently in the
/// registry.  Globals come and go as a result of device or
/// monitor hotplugs, reconfiguration or other events, and the
/// registry will send out global and global_remove events to
/// keep the client up to date with the changes.  To mark the end
/// of the initial burst of events, the client can use the
/// wl_display.sync request immediately after calling
/// wl_display.get_registry.
///
/// A client can bind to a global object by using the bind
/// request.  This creates a client-side handle that lets the object
/// emit events to the client and lets the client invoke requests on
/// the object.
///
pub mod wl_registry {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_registry";
    pub const VERSION: u32 = 1;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// bind an object to the display
        ///
        /// Binds a new, client-created object to the server using the
        /// specified name as the identifier.
        ///
        #[derive(Debug)]
        pub struct bind {
            // unique numeric name of the object
            pub name: u32,
            // bounded object
            pub id: new_id<Dyn>,
        }
        impl Ser for bind {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.name.serialize(&mut buf[n..], fds)?;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for bind {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (name, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (id, m) = <new_id<Dyn>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((bind { name, id }, n))
            }
        }
        impl Msg<Obj> for bind {
            const OP: u16 = 0;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta { alive: true, destructor: None, type_id: TypeId::of::<()>() };
                Some((id, meta))
            }
        }
        impl Request<Obj> for bind {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// announce global object
        ///
        /// Notify the client of global objects.
        ///
        /// The event notifies the client that a global object with
        /// the given name is now available, and it implements the
        /// given version of the given interface.
        ///
        #[derive(Debug)]
        pub struct global {
            // numeric name of the global object
            pub name: u32,
            // interface implemented by the object
            pub interface: String,
            // interface version
            pub version: u32,
        }
        impl Ser for global {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.name.serialize(&mut buf[n..], fds)?;
                n += self.interface.serialize(&mut buf[n..], fds)?;
                n += self.version.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for global {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (name, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (interface, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                let (version, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((global { name, interface, version }, n))
            }
        }
        impl Msg<Obj> for global {
            const OP: u16 = 0;
        }
        impl Event<Obj> for global {}
        /// announce removal of global object
        ///
        /// Notify the client of removed global objects.
        ///
        /// This event notifies the client that the global identified
        /// by name is no longer available.  If the client bound to
        /// the global using the bind request, the client should now
        /// destroy that object.
        ///
        /// The object remains valid and requests to the object will be
        /// ignored until the client destroys it, to avoid races between
        /// the global going away and a client sending a request to it.
        ///
        #[derive(Debug)]
        pub struct global_remove {
            // numeric name of the global object
            pub name: u32,
        }
        impl Ser for global_remove {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.name.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for global_remove {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (name, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((global_remove { name }, n))
            }
        }
        impl Msg<Obj> for global_remove {
            const OP: u16 = 1;
        }
        impl Event<Obj> for global_remove {}
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_registry;
    pub type Obj = wl_registry;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_registry;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = None;
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// callback object
///
/// Clients can handle the 'done' event to get notified when
/// the related request is done.
///
/// Note, because wl_callback objects are created from multiple independent
/// factory interfaces, the wl_callback interface is frozen at version 1.
///
pub mod wl_callback {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_callback";
    pub const VERSION: u32 = 1;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// done event
        ///
        /// Notify the client when the related request is done.
        ///
        #[derive(Debug)]
        pub struct done {
            // request-specific data for the callback
            pub callback_data: u32,
        }
        impl Ser for done {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.callback_data.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for done {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (callback_data, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((done { callback_data }, n))
            }
        }
        impl Msg<Obj> for done {
            const OP: u16 = 0;
        }
        impl Event<Obj> for done {}
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_callback;
    pub type Obj = wl_callback;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_callback;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = None;
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// the compositor singleton
///
/// A compositor.  This object is a singleton global.  The
/// compositor is in charge of combining the contents of multiple
/// surfaces into one displayable output.
///
pub mod wl_compositor {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_compositor";
    pub const VERSION: u32 = 6;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// create new surface
        ///
        /// Ask the compositor to create a new surface.
        ///
        #[derive(Debug)]
        pub struct create_surface {
            // the new surface
            pub id: new_id<wl_surface::Obj>,
        }
        impl Ser for create_surface {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for create_surface {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((create_surface { id }, n))
            }
        }
        impl Msg<Obj> for create_surface {
            const OP: u16 = 0;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_surface::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_surface::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for create_surface {}
        /// create new region
        ///
        /// Ask the compositor to create a new region.
        ///
        #[derive(Debug)]
        pub struct create_region {
            // the new region
            pub id: new_id<wl_region::Obj>,
        }
        impl Ser for create_region {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for create_region {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<wl_region::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((create_region { id }, n))
            }
        }
        impl Msg<Obj> for create_region {
            const OP: u16 = 1;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_region::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_region::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for create_region {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_compositor;
    pub type Obj = wl_compositor;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_compositor;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = None;
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// a shared memory pool
///
/// The wl_shm_pool object encapsulates a piece of memory shared
/// between the compositor and client.  Through the wl_shm_pool
/// object, the client can allocate shared memory wl_buffer objects.
/// All objects created through the same pool share the same
/// underlying mapped memory. Reusing the mapped memory avoids the
/// setup/teardown overhead and is useful when interactively resizing
/// a surface or for many small buffers.
///
pub mod wl_shm_pool {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_shm_pool";
    pub const VERSION: u32 = 1;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// create a buffer from the pool
        ///
        /// Create a wl_buffer object from the pool.
        ///
        /// The buffer is created offset bytes into the pool and has
        /// width and height as specified.  The stride argument specifies
        /// the number of bytes from the beginning of one row to the beginning
        /// of the next.  The format is the pixel format of the buffer and
        /// must be one of those advertised through the wl_shm.format event.
        ///
        /// A buffer will keep a reference to the pool it was created from
        /// so it is valid to destroy the pool immediately after creating
        /// a buffer from it.
        ///
        #[derive(Debug)]
        pub struct create_buffer {
            // buffer to create
            pub id: new_id<wl_buffer::Obj>,
            // buffer byte offset within the pool
            pub offset: i32,
            // buffer width, in pixels
            pub width: i32,
            // buffer height, in pixels
            pub height: i32,
            // number of bytes from the beginning of one row to the beginning of the next row
            pub stride: i32,
            // buffer pixel format
            pub format: super::wl_shm::format,
        }
        impl Ser for create_buffer {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                n += self.offset.serialize(&mut buf[n..], fds)?;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                n += self.stride.serialize(&mut buf[n..], fds)?;
                n += self.format.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for create_buffer {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<wl_buffer::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (offset, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (stride, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (format, m) = <super::wl_shm::format>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((create_buffer { id, offset, width, height, stride, format }, n))
            }
        }
        impl Msg<Obj> for create_buffer {
            const OP: u16 = 0;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_buffer::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_buffer::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for create_buffer {}
        /// destroy the pool
        ///
        /// Destroy the shared memory pool.
        ///
        /// The mmapped memory will be released when all
        /// buffers that have been created from this pool
        /// are gone.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 1;
        }
        impl Request<Obj> for destroy {}
        /// change the size of the pool mapping
        ///
        /// This request will cause the server to remap the backing memory
        /// for the pool from the file descriptor passed when the pool was
        /// created, but using the new size.  This request can only be
        /// used to make the pool bigger.
        ///
        /// This request only changes the amount of bytes that are mmapped
        /// by the server and does not touch the file corresponding to the
        /// file descriptor passed at creation time. It is the client's
        /// responsibility to ensure that the file is at least as big as
        /// the new pool size.
        ///
        #[derive(Debug)]
        pub struct resize {
            // new size of the pool, in bytes
            pub size: i32,
        }
        impl Ser for resize {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.size.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for resize {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (size, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((resize { size }, n))
            }
        }
        impl Msg<Obj> for resize {
            const OP: u16 = 2;
        }
        impl Request<Obj> for resize {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_shm_pool;
    pub type Obj = wl_shm_pool;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_shm_pool;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(1);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// shared memory support
///
/// A singleton global object that provides support for shared
/// memory.
///
/// Clients can create wl_shm_pool objects using the create_pool
/// request.
///
/// On binding the wl_shm object one or more format events
/// are emitted to inform clients about the valid pixel formats
/// that can be used for buffers.
///
pub mod wl_shm {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_shm";
    pub const VERSION: u32 = 1;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// create a shm pool
        ///
        /// Create a new wl_shm_pool object.
        ///
        /// The pool can be used to create shared memory based buffer
        /// objects.  The server will mmap size bytes of the passed file
        /// descriptor, to use as backing memory for the pool.
        ///
        #[derive(Debug)]
        pub struct create_pool {
            // pool to create
            pub id: new_id<wl_shm_pool::Obj>,
            // file descriptor for the pool
            pub fd: OwnedFd,
            // pool size, in bytes
            pub size: i32,
        }
        impl Ser for create_pool {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                n += self.fd.serialize(&mut buf[n..], fds)?;
                n += self.size.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for create_pool {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<wl_shm_pool::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (fd, m) = <OwnedFd>::deserialize(&buf[n..], fds)?;
                n += m;
                let (size, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((create_pool { id, fd, size }, n))
            }
        }
        impl Msg<Obj> for create_pool {
            const OP: u16 = 0;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_shm_pool::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_shm_pool::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for create_pool {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// pixel format description
        ///
        /// Informs the client about a valid pixel format that
        /// can be used for buffers. Known formats include
        /// argb8888 and xrgb8888.
        ///
        #[derive(Debug)]
        pub struct format {
            // buffer pixel format
            pub format: super::format,
        }
        impl Ser for format {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.format.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for format {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (format, m) = <super::format>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((format { format }, n))
            }
        }
        impl Msg<Obj> for format {
            const OP: u16 = 0;
        }
        impl Event<Obj> for format {}
    }
    /// wl_shm error values
    ///
    /// These errors can be emitted in response to wl_shm requests.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// buffer format is not known
        invalid_format = 0,
        /// invalid size or stride during pool or buffer creation
        invalid_stride = 1,
        /// mmapping the file descriptor failed
        invalid_fd = 2,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::invalid_format => 0,
                error::invalid_stride => 1,
                error::invalid_fd => 2,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::invalid_format),
                1 => Ok(error::invalid_stride),
                2 => Ok(error::invalid_fd),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }
    /// pixel formats
    ///
    /// This describes the memory layout of an individual pixel.
    ///
    /// All renderers should support argb8888 and xrgb8888 but any other
    /// formats are optional and may not be supported by the particular
    /// renderer in use.
    ///
    /// The drm format codes match the macros defined in drm_fourcc.h, except
    /// argb8888 and xrgb8888. The formats actually supported by the compositor
    /// will be reported by the format event.
    ///
    /// For all wl_shm formats and unless specified in another protocol
    /// extension, pre-multiplied alpha is used for pixel values.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum format {
        /// 32-bit ARGB format, [31:0] A:R:G:B 8:8:8:8 little endian
        argb8888 = 0,
        /// 32-bit RGB format, [31:0] x:R:G:B 8:8:8:8 little endian
        xrgb8888 = 1,
        /// 8-bit color index format, [7:0] C
        c8 = 538982467,
        /// 8-bit RGB format, [7:0] R:G:B 3:3:2
        rgb332 = 943867730,
        /// 8-bit BGR format, [7:0] B:G:R 2:3:3
        bgr233 = 944916290,
        /// 16-bit xRGB format, [15:0] x:R:G:B 4:4:4:4 little endian
        xrgb4444 = 842093144,
        /// 16-bit xBGR format, [15:0] x:B:G:R 4:4:4:4 little endian
        xbgr4444 = 842089048,
        /// 16-bit RGBx format, [15:0] R:G:B:x 4:4:4:4 little endian
        rgbx4444 = 842094674,
        /// 16-bit BGRx format, [15:0] B:G:R:x 4:4:4:4 little endian
        bgrx4444 = 842094658,
        /// 16-bit ARGB format, [15:0] A:R:G:B 4:4:4:4 little endian
        argb4444 = 842093121,
        /// 16-bit ABGR format, [15:0] A:B:G:R 4:4:4:4 little endian
        abgr4444 = 842089025,
        /// 16-bit RBGA format, [15:0] R:G:B:A 4:4:4:4 little endian
        rgba4444 = 842088786,
        /// 16-bit BGRA format, [15:0] B:G:R:A 4:4:4:4 little endian
        bgra4444 = 842088770,
        /// 16-bit xRGB format, [15:0] x:R:G:B 1:5:5:5 little endian
        xrgb1555 = 892424792,
        /// 16-bit xBGR 1555 format, [15:0] x:B:G:R 1:5:5:5 little endian
        xbgr1555 = 892420696,
        /// 16-bit RGBx 5551 format, [15:0] R:G:B:x 5:5:5:1 little endian
        rgbx5551 = 892426322,
        /// 16-bit BGRx 5551 format, [15:0] B:G:R:x 5:5:5:1 little endian
        bgrx5551 = 892426306,
        /// 16-bit ARGB 1555 format, [15:0] A:R:G:B 1:5:5:5 little endian
        argb1555 = 892424769,
        /// 16-bit ABGR 1555 format, [15:0] A:B:G:R 1:5:5:5 little endian
        abgr1555 = 892420673,
        /// 16-bit RGBA 5551 format, [15:0] R:G:B:A 5:5:5:1 little endian
        rgba5551 = 892420434,
        /// 16-bit BGRA 5551 format, [15:0] B:G:R:A 5:5:5:1 little endian
        bgra5551 = 892420418,
        /// 16-bit RGB 565 format, [15:0] R:G:B 5:6:5 little endian
        rgb565 = 909199186,
        /// 16-bit BGR 565 format, [15:0] B:G:R 5:6:5 little endian
        bgr565 = 909199170,
        /// 24-bit RGB format, [23:0] R:G:B little endian
        rgb888 = 875710290,
        /// 24-bit BGR format, [23:0] B:G:R little endian
        bgr888 = 875710274,
        /// 32-bit xBGR format, [31:0] x:B:G:R 8:8:8:8 little endian
        xbgr8888 = 875709016,
        /// 32-bit RGBx format, [31:0] R:G:B:x 8:8:8:8 little endian
        rgbx8888 = 875714642,
        /// 32-bit BGRx format, [31:0] B:G:R:x 8:8:8:8 little endian
        bgrx8888 = 875714626,
        /// 32-bit ABGR format, [31:0] A:B:G:R 8:8:8:8 little endian
        abgr8888 = 875708993,
        /// 32-bit RGBA format, [31:0] R:G:B:A 8:8:8:8 little endian
        rgba8888 = 875708754,
        /// 32-bit BGRA format, [31:0] B:G:R:A 8:8:8:8 little endian
        bgra8888 = 875708738,
        /// 32-bit xRGB format, [31:0] x:R:G:B 2:10:10:10 little endian
        xrgb2101010 = 808669784,
        /// 32-bit xBGR format, [31:0] x:B:G:R 2:10:10:10 little endian
        xbgr2101010 = 808665688,
        /// 32-bit RGBx format, [31:0] R:G:B:x 10:10:10:2 little endian
        rgbx1010102 = 808671314,
        /// 32-bit BGRx format, [31:0] B:G:R:x 10:10:10:2 little endian
        bgrx1010102 = 808671298,
        /// 32-bit ARGB format, [31:0] A:R:G:B 2:10:10:10 little endian
        argb2101010 = 808669761,
        /// 32-bit ABGR format, [31:0] A:B:G:R 2:10:10:10 little endian
        abgr2101010 = 808665665,
        /// 32-bit RGBA format, [31:0] R:G:B:A 10:10:10:2 little endian
        rgba1010102 = 808665426,
        /// 32-bit BGRA format, [31:0] B:G:R:A 10:10:10:2 little endian
        bgra1010102 = 808665410,
        /// packed YCbCr format, [31:0] Cr0:Y1:Cb0:Y0 8:8:8:8 little endian
        yuyv = 1448695129,
        /// packed YCbCr format, [31:0] Cb0:Y1:Cr0:Y0 8:8:8:8 little endian
        yvyu = 1431918169,
        /// packed YCbCr format, [31:0] Y1:Cr0:Y0:Cb0 8:8:8:8 little endian
        uyvy = 1498831189,
        /// packed YCbCr format, [31:0] Y1:Cb0:Y0:Cr0 8:8:8:8 little endian
        vyuy = 1498765654,
        /// packed AYCbCr format, [31:0] A:Y:Cb:Cr 8:8:8:8 little endian
        ayuv = 1448433985,
        /// 2 plane YCbCr Cr:Cb format, 2x2 subsampled Cr:Cb plane
        nv12 = 842094158,
        /// 2 plane YCbCr Cb:Cr format, 2x2 subsampled Cb:Cr plane
        nv21 = 825382478,
        /// 2 plane YCbCr Cr:Cb format, 2x1 subsampled Cr:Cb plane
        nv16 = 909203022,
        /// 2 plane YCbCr Cb:Cr format, 2x1 subsampled Cb:Cr plane
        nv61 = 825644622,
        /// 3 plane YCbCr format, 4x4 subsampled Cb (1) and Cr (2) planes
        yuv410 = 961959257,
        /// 3 plane YCbCr format, 4x4 subsampled Cr (1) and Cb (2) planes
        yvu410 = 961893977,
        /// 3 plane YCbCr format, 4x1 subsampled Cb (1) and Cr (2) planes
        yuv411 = 825316697,
        /// 3 plane YCbCr format, 4x1 subsampled Cr (1) and Cb (2) planes
        yvu411 = 825316953,
        /// 3 plane YCbCr format, 2x2 subsampled Cb (1) and Cr (2) planes
        yuv420 = 842093913,
        /// 3 plane YCbCr format, 2x2 subsampled Cr (1) and Cb (2) planes
        yvu420 = 842094169,
        /// 3 plane YCbCr format, 2x1 subsampled Cb (1) and Cr (2) planes
        yuv422 = 909202777,
        /// 3 plane YCbCr format, 2x1 subsampled Cr (1) and Cb (2) planes
        yvu422 = 909203033,
        /// 3 plane YCbCr format, non-subsampled Cb (1) and Cr (2) planes
        yuv444 = 875713881,
        /// 3 plane YCbCr format, non-subsampled Cr (1) and Cb (2) planes
        yvu444 = 875714137,
        /// [7:0] R
        r8 = 538982482,
        /// [15:0] R little endian
        r16 = 540422482,
        /// [15:0] R:G 8:8 little endian
        rg88 = 943212370,
        /// [15:0] G:R 8:8 little endian
        gr88 = 943215175,
        /// [31:0] R:G 16:16 little endian
        rg1616 = 842221394,
        /// [31:0] G:R 16:16 little endian
        gr1616 = 842224199,
        /// [63:0] x:R:G:B 16:16:16:16 little endian
        xrgb16161616f = 1211388504,
        /// [63:0] x:B:G:R 16:16:16:16 little endian
        xbgr16161616f = 1211384408,
        /// [63:0] A:R:G:B 16:16:16:16 little endian
        argb16161616f = 1211388481,
        /// [63:0] A:B:G:R 16:16:16:16 little endian
        abgr16161616f = 1211384385,
        /// [31:0] X:Y:Cb:Cr 8:8:8:8 little endian
        xyuv8888 = 1448434008,
        /// [23:0] Cr:Cb:Y 8:8:8 little endian
        vuy888 = 875713878,
        /// Y followed by U then V, 10:10:10. Non-linear modifier only
        vuy101010 = 808670550,
        /// [63:0] Cr0:0:Y1:0:Cb0:0:Y0:0 10:6:10:6:10:6:10:6 little endian per 2 Y pixels
        y210 = 808530521,
        /// [63:0] Cr0:0:Y1:0:Cb0:0:Y0:0 12:4:12:4:12:4:12:4 little endian per 2 Y pixels
        y212 = 842084953,
        /// [63:0] Cr0:Y1:Cb0:Y0 16:16:16:16 little endian per 2 Y pixels
        y216 = 909193817,
        /// [31:0] A:Cr:Y:Cb 2:10:10:10 little endian
        y410 = 808531033,
        /// [63:0] A:0:Cr:0:Y:0:Cb:0 12:4:12:4:12:4:12:4 little endian
        y412 = 842085465,
        /// [63:0] A:Cr:Y:Cb 16:16:16:16 little endian
        y416 = 909194329,
        /// [31:0] X:Cr:Y:Cb 2:10:10:10 little endian
        xvyu2101010 = 808670808,
        /// [63:0] X:0:Cr:0:Y:0:Cb:0 12:4:12:4:12:4:12:4 little endian
        xvyu12_16161616 = 909334104,
        /// [63:0] X:Cr:Y:Cb 16:16:16:16 little endian
        xvyu16161616 = 942954072,
        /// [63:0]   A3:A2:Y3:0:Cr0:0:Y2:0:A1:A0:Y1:0:Cb0:0:Y0:0  1:1:8:2:8:2:8:2:1:1:8:2:8:2:8:2 little endian
        y0l0 = 810299481,
        /// [63:0]   X3:X2:Y3:0:Cr0:0:Y2:0:X1:X0:Y1:0:Cb0:0:Y0:0  1:1:8:2:8:2:8:2:1:1:8:2:8:2:8:2 little endian
        x0l0 = 810299480,
        /// [63:0]   A3:A2:Y3:Cr0:Y2:A1:A0:Y1:Cb0:Y0  1:1:10:10:10:1:1:10:10:10 little endian
        y0l2 = 843853913,
        /// [63:0]   X3:X2:Y3:Cr0:Y2:X1:X0:Y1:Cb0:Y0  1:1:10:10:10:1:1:10:10:10 little endian
        x0l2 = 843853912,
        yuv420_8bit = 942691673,
        yuv420_10bit = 808539481,
        xrgb8888_a8 = 943805016,
        xbgr8888_a8 = 943800920,
        rgbx8888_a8 = 943806546,
        bgrx8888_a8 = 943806530,
        rgb888_a8 = 943798354,
        bgr888_a8 = 943798338,
        rgb565_a8 = 943797586,
        bgr565_a8 = 943797570,
        /// non-subsampled Cr:Cb plane
        nv24 = 875714126,
        /// non-subsampled Cb:Cr plane
        nv42 = 842290766,
        /// 2x1 subsampled Cr:Cb plane, 10 bit per channel
        p210 = 808530512,
        /// 2x2 subsampled Cr:Cb plane 10 bits per channel
        p010 = 808530000,
        /// 2x2 subsampled Cr:Cb plane 12 bits per channel
        p012 = 842084432,
        /// 2x2 subsampled Cr:Cb plane 16 bits per channel
        p016 = 909193296,
        /// [63:0] A:x:B:x:G:x:R:x 10:6:10:6:10:6:10:6 little endian
        axbxgxrx106106106106 = 808534593,
        /// 2x2 subsampled Cr:Cb plane
        nv15 = 892425806,
        q410 = 808531025,
        q401 = 825242705,
        /// [63:0] x:R:G:B 16:16:16:16 little endian
        xrgb16161616 = 942953048,
        /// [63:0] x:B:G:R 16:16:16:16 little endian
        xbgr16161616 = 942948952,
        /// [63:0] A:R:G:B 16:16:16:16 little endian
        argb16161616 = 942953025,
        /// [63:0] A:B:G:R 16:16:16:16 little endian
        abgr16161616 = 942948929,
    }

    impl From<format> for u32 {
        fn from(v: format) -> u32 {
            match v {
                format::argb8888 => 0,
                format::xrgb8888 => 1,
                format::c8 => 538982467,
                format::rgb332 => 943867730,
                format::bgr233 => 944916290,
                format::xrgb4444 => 842093144,
                format::xbgr4444 => 842089048,
                format::rgbx4444 => 842094674,
                format::bgrx4444 => 842094658,
                format::argb4444 => 842093121,
                format::abgr4444 => 842089025,
                format::rgba4444 => 842088786,
                format::bgra4444 => 842088770,
                format::xrgb1555 => 892424792,
                format::xbgr1555 => 892420696,
                format::rgbx5551 => 892426322,
                format::bgrx5551 => 892426306,
                format::argb1555 => 892424769,
                format::abgr1555 => 892420673,
                format::rgba5551 => 892420434,
                format::bgra5551 => 892420418,
                format::rgb565 => 909199186,
                format::bgr565 => 909199170,
                format::rgb888 => 875710290,
                format::bgr888 => 875710274,
                format::xbgr8888 => 875709016,
                format::rgbx8888 => 875714642,
                format::bgrx8888 => 875714626,
                format::abgr8888 => 875708993,
                format::rgba8888 => 875708754,
                format::bgra8888 => 875708738,
                format::xrgb2101010 => 808669784,
                format::xbgr2101010 => 808665688,
                format::rgbx1010102 => 808671314,
                format::bgrx1010102 => 808671298,
                format::argb2101010 => 808669761,
                format::abgr2101010 => 808665665,
                format::rgba1010102 => 808665426,
                format::bgra1010102 => 808665410,
                format::yuyv => 1448695129,
                format::yvyu => 1431918169,
                format::uyvy => 1498831189,
                format::vyuy => 1498765654,
                format::ayuv => 1448433985,
                format::nv12 => 842094158,
                format::nv21 => 825382478,
                format::nv16 => 909203022,
                format::nv61 => 825644622,
                format::yuv410 => 961959257,
                format::yvu410 => 961893977,
                format::yuv411 => 825316697,
                format::yvu411 => 825316953,
                format::yuv420 => 842093913,
                format::yvu420 => 842094169,
                format::yuv422 => 909202777,
                format::yvu422 => 909203033,
                format::yuv444 => 875713881,
                format::yvu444 => 875714137,
                format::r8 => 538982482,
                format::r16 => 540422482,
                format::rg88 => 943212370,
                format::gr88 => 943215175,
                format::rg1616 => 842221394,
                format::gr1616 => 842224199,
                format::xrgb16161616f => 1211388504,
                format::xbgr16161616f => 1211384408,
                format::argb16161616f => 1211388481,
                format::abgr16161616f => 1211384385,
                format::xyuv8888 => 1448434008,
                format::vuy888 => 875713878,
                format::vuy101010 => 808670550,
                format::y210 => 808530521,
                format::y212 => 842084953,
                format::y216 => 909193817,
                format::y410 => 808531033,
                format::y412 => 842085465,
                format::y416 => 909194329,
                format::xvyu2101010 => 808670808,
                format::xvyu12_16161616 => 909334104,
                format::xvyu16161616 => 942954072,
                format::y0l0 => 810299481,
                format::x0l0 => 810299480,
                format::y0l2 => 843853913,
                format::x0l2 => 843853912,
                format::yuv420_8bit => 942691673,
                format::yuv420_10bit => 808539481,
                format::xrgb8888_a8 => 943805016,
                format::xbgr8888_a8 => 943800920,
                format::rgbx8888_a8 => 943806546,
                format::bgrx8888_a8 => 943806530,
                format::rgb888_a8 => 943798354,
                format::bgr888_a8 => 943798338,
                format::rgb565_a8 => 943797586,
                format::bgr565_a8 => 943797570,
                format::nv24 => 875714126,
                format::nv42 => 842290766,
                format::p210 => 808530512,
                format::p010 => 808530000,
                format::p012 => 842084432,
                format::p016 => 909193296,
                format::axbxgxrx106106106106 => 808534593,
                format::nv15 => 892425806,
                format::q410 => 808531025,
                format::q401 => 825242705,
                format::xrgb16161616 => 942953048,
                format::xbgr16161616 => 942948952,
                format::argb16161616 => 942953025,
                format::abgr16161616 => 942948929,
            }
        }
    }

    impl TryFrom<u32> for format {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(format::argb8888),
                1 => Ok(format::xrgb8888),
                538982467 => Ok(format::c8),
                943867730 => Ok(format::rgb332),
                944916290 => Ok(format::bgr233),
                842093144 => Ok(format::xrgb4444),
                842089048 => Ok(format::xbgr4444),
                842094674 => Ok(format::rgbx4444),
                842094658 => Ok(format::bgrx4444),
                842093121 => Ok(format::argb4444),
                842089025 => Ok(format::abgr4444),
                842088786 => Ok(format::rgba4444),
                842088770 => Ok(format::bgra4444),
                892424792 => Ok(format::xrgb1555),
                892420696 => Ok(format::xbgr1555),
                892426322 => Ok(format::rgbx5551),
                892426306 => Ok(format::bgrx5551),
                892424769 => Ok(format::argb1555),
                892420673 => Ok(format::abgr1555),
                892420434 => Ok(format::rgba5551),
                892420418 => Ok(format::bgra5551),
                909199186 => Ok(format::rgb565),
                909199170 => Ok(format::bgr565),
                875710290 => Ok(format::rgb888),
                875710274 => Ok(format::bgr888),
                875709016 => Ok(format::xbgr8888),
                875714642 => Ok(format::rgbx8888),
                875714626 => Ok(format::bgrx8888),
                875708993 => Ok(format::abgr8888),
                875708754 => Ok(format::rgba8888),
                875708738 => Ok(format::bgra8888),
                808669784 => Ok(format::xrgb2101010),
                808665688 => Ok(format::xbgr2101010),
                808671314 => Ok(format::rgbx1010102),
                808671298 => Ok(format::bgrx1010102),
                808669761 => Ok(format::argb2101010),
                808665665 => Ok(format::abgr2101010),
                808665426 => Ok(format::rgba1010102),
                808665410 => Ok(format::bgra1010102),
                1448695129 => Ok(format::yuyv),
                1431918169 => Ok(format::yvyu),
                1498831189 => Ok(format::uyvy),
                1498765654 => Ok(format::vyuy),
                1448433985 => Ok(format::ayuv),
                842094158 => Ok(format::nv12),
                825382478 => Ok(format::nv21),
                909203022 => Ok(format::nv16),
                825644622 => Ok(format::nv61),
                961959257 => Ok(format::yuv410),
                961893977 => Ok(format::yvu410),
                825316697 => Ok(format::yuv411),
                825316953 => Ok(format::yvu411),
                842093913 => Ok(format::yuv420),
                842094169 => Ok(format::yvu420),
                909202777 => Ok(format::yuv422),
                909203033 => Ok(format::yvu422),
                875713881 => Ok(format::yuv444),
                875714137 => Ok(format::yvu444),
                538982482 => Ok(format::r8),
                540422482 => Ok(format::r16),
                943212370 => Ok(format::rg88),
                943215175 => Ok(format::gr88),
                842221394 => Ok(format::rg1616),
                842224199 => Ok(format::gr1616),
                1211388504 => Ok(format::xrgb16161616f),
                1211384408 => Ok(format::xbgr16161616f),
                1211388481 => Ok(format::argb16161616f),
                1211384385 => Ok(format::abgr16161616f),
                1448434008 => Ok(format::xyuv8888),
                875713878 => Ok(format::vuy888),
                808670550 => Ok(format::vuy101010),
                808530521 => Ok(format::y210),
                842084953 => Ok(format::y212),
                909193817 => Ok(format::y216),
                808531033 => Ok(format::y410),
                842085465 => Ok(format::y412),
                909194329 => Ok(format::y416),
                808670808 => Ok(format::xvyu2101010),
                909334104 => Ok(format::xvyu12_16161616),
                942954072 => Ok(format::xvyu16161616),
                810299481 => Ok(format::y0l0),
                810299480 => Ok(format::x0l0),
                843853913 => Ok(format::y0l2),
                843853912 => Ok(format::x0l2),
                942691673 => Ok(format::yuv420_8bit),
                808539481 => Ok(format::yuv420_10bit),
                943805016 => Ok(format::xrgb8888_a8),
                943800920 => Ok(format::xbgr8888_a8),
                943806546 => Ok(format::rgbx8888_a8),
                943806530 => Ok(format::bgrx8888_a8),
                943798354 => Ok(format::rgb888_a8),
                943798338 => Ok(format::bgr888_a8),
                943797586 => Ok(format::rgb565_a8),
                943797570 => Ok(format::bgr565_a8),
                875714126 => Ok(format::nv24),
                842290766 => Ok(format::nv42),
                808530512 => Ok(format::p210),
                808530000 => Ok(format::p010),
                842084432 => Ok(format::p012),
                909193296 => Ok(format::p016),
                808534593 => Ok(format::axbxgxrx106106106106),
                892425806 => Ok(format::nv15),
                808531025 => Ok(format::q410),
                825242705 => Ok(format::q401),
                942953048 => Ok(format::xrgb16161616),
                942948952 => Ok(format::xbgr16161616),
                942953025 => Ok(format::argb16161616),
                942948929 => Ok(format::abgr16161616),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for format {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for format {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for format")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_shm;
    pub type Obj = wl_shm;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_shm;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = None;
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// content for a wl_surface
///
/// A buffer provides the content for a wl_surface. Buffers are
/// created through factory interfaces such as wl_shm, wp_linux_buffer_params
/// (from the linux-dmabuf protocol extension) or similar. It has a width and
/// a height and can be attached to a wl_surface, but the mechanism by which a
/// client provides and updates the contents is defined by the buffer factory
/// interface.
///
/// If the buffer uses a format that has an alpha channel, the alpha channel
/// is assumed to be premultiplied in the color channels unless otherwise
/// specified.
///
/// Note, because wl_buffer objects are created from multiple independent
/// factory interfaces, the wl_buffer interface is frozen at version 1.
///
pub mod wl_buffer {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_buffer";
    pub const VERSION: u32 = 1;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// destroy a buffer
        ///
        /// Destroy a buffer. If and how you need to release the backing
        /// storage is defined by the buffer factory interface.
        ///
        /// For possible side-effects to a surface, see wl_surface.attach.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 0;
        }
        impl Request<Obj> for destroy {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// compositor releases buffer
        ///
        /// Sent when this wl_buffer is no longer used by the compositor.
        /// The client is now free to reuse or destroy this buffer and its
        /// backing storage.
        ///
        /// If a client receives a release event before the frame callback
        /// requested in the same wl_surface.commit that attaches this
        /// wl_buffer to a surface, then the client is immediately free to
        /// reuse the buffer and its backing storage, and does not need a
        /// second buffer for the next surface content update. Typically
        /// this is possible, when the compositor maintains a copy of the
        /// wl_surface contents, e.g. as a GL texture. This is an important
        /// optimization for GL(ES) compositors with wl_shm clients.
        ///
        #[derive(Debug)]
        pub struct release {}
        impl Ser for release {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for release {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((release {}, n))
            }
        }
        impl Msg<Obj> for release {
            const OP: u16 = 0;
        }
        impl Event<Obj> for release {}
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_buffer;
    pub type Obj = wl_buffer;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_buffer;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(0);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// offer to transfer data
///
/// A wl_data_offer represents a piece of data offered for transfer
/// by another client (the source client).  It is used by the
/// copy-and-paste and drag-and-drop mechanisms.  The offer
/// describes the different mime types that the data can be
/// converted to and provides the mechanism for transferring the
/// data directly from the source client.
///
pub mod wl_data_offer {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_data_offer";
    pub const VERSION: u32 = 3;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// accept one of the offered mime types
        ///
        /// Indicate that the client can accept the given mime type, or
        /// NULL for not accepted.
        ///
        /// For objects of version 2 or older, this request is used by the
        /// client to give feedback whether the client can receive the given
        /// mime type, or NULL if none is accepted; the feedback does not
        /// determine whether the drag-and-drop operation succeeds or not.
        ///
        /// For objects of version 3 or newer, this request determines the
        /// final result of the drag-and-drop operation. If the end result
        /// is that no mime types were accepted, the drag-and-drop operation
        /// will be cancelled and the corresponding drag source will receive
        /// wl_data_source.cancelled. Clients may still use this event in
        /// conjunction with wl_data_source.action for feedback.
        ///
        #[derive(Debug)]
        pub struct accept {
            // serial number of the accept request
            pub serial: u32,
            // mime type accepted by the client
            pub mime_type: String,
        }
        impl Ser for accept {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.mime_type.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for accept {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (mime_type, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((accept { serial, mime_type }, n))
            }
        }
        impl Msg<Obj> for accept {
            const OP: u16 = 0;
        }
        impl Request<Obj> for accept {}
        /// request that the data is transferred
        ///
        /// To transfer the offered data, the client issues this request
        /// and indicates the mime type it wants to receive.  The transfer
        /// happens through the passed file descriptor (typically created
        /// with the pipe system call).  The source client writes the data
        /// in the mime type representation requested and then closes the
        /// file descriptor.
        ///
        /// The receiving client reads from the read end of the pipe until
        /// EOF and then closes its end, at which point the transfer is
        /// complete.
        ///
        /// This request may happen multiple times for different mime types,
        /// both before and after wl_data_device.drop. Drag-and-drop destination
        /// clients may preemptively fetch data or examine it more closely to
        /// determine acceptance.
        ///
        #[derive(Debug)]
        pub struct receive {
            // mime type desired by receiver
            pub mime_type: String,
            // file descriptor for data transfer
            pub fd: OwnedFd,
        }
        impl Ser for receive {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.mime_type.serialize(&mut buf[n..], fds)?;
                n += self.fd.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for receive {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (mime_type, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                let (fd, m) = <OwnedFd>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((receive { mime_type, fd }, n))
            }
        }
        impl Msg<Obj> for receive {
            const OP: u16 = 1;
        }
        impl Request<Obj> for receive {}
        /// destroy data offer
        ///
        /// Destroy the data offer.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 2;
        }
        impl Request<Obj> for destroy {}
        /// the offer will no longer be used
        ///
        /// Notifies the compositor that the drag destination successfully
        /// finished the drag-and-drop operation.
        ///
        /// Upon receiving this request, the compositor will emit
        /// wl_data_source.dnd_finished on the drag source client.
        ///
        /// It is a client error to perform other requests than
        /// wl_data_offer.destroy after this one. It is also an error to perform
        /// this request after a NULL mime type has been set in
        /// wl_data_offer.accept or no action was received through
        /// wl_data_offer.action.
        ///
        /// If wl_data_offer.finish request is received for a non drag and drop
        /// operation, the invalid_finish protocol error is raised.
        ///
        #[derive(Debug)]
        pub struct finish {}
        impl Ser for finish {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for finish {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((finish {}, n))
            }
        }
        impl Msg<Obj> for finish {
            const OP: u16 = 3;
        }
        impl Request<Obj> for finish {}
        /// set the available/preferred drag-and-drop actions
        ///
        /// Sets the actions that the destination side client supports for
        /// this operation. This request may trigger the emission of
        /// wl_data_source.action and wl_data_offer.action events if the compositor
        /// needs to change the selected action.
        ///
        /// This request can be called multiple times throughout the
        /// drag-and-drop operation, typically in response to wl_data_device.enter
        /// or wl_data_device.motion events.
        ///
        /// This request determines the final result of the drag-and-drop
        /// operation. If the end result is that no action is accepted,
        /// the drag source will receive wl_data_source.cancelled.
        ///
        /// The dnd_actions argument must contain only values expressed in the
        /// wl_data_device_manager.dnd_actions enum, and the preferred_action
        /// argument must only contain one of those values set, otherwise it
        /// will result in a protocol error.
        ///
        /// While managing an "ask" action, the destination drag-and-drop client
        /// may perform further wl_data_offer.receive requests, and is expected
        /// to perform one last wl_data_offer.set_actions request with a preferred
        /// action other than "ask" (and optionally wl_data_offer.accept) before
        /// requesting wl_data_offer.finish, in order to convey the action selected
        /// by the user. If the preferred action is not in the
        /// wl_data_offer.source_actions mask, an error will be raised.
        ///
        /// If the "ask" action is dismissed (e.g. user cancellation), the client
        /// is expected to perform wl_data_offer.destroy right away.
        ///
        /// This request can only be made on drag-and-drop offers, a protocol error
        /// will be raised otherwise.
        ///
        #[derive(Debug)]
        pub struct set_actions {
            // actions supported by the destination client
            pub dnd_actions: super::wl_data_device_manager::dnd_action,
            // action preferred by the destination client
            pub preferred_action: super::wl_data_device_manager::dnd_action,
        }
        impl Ser for set_actions {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.dnd_actions.serialize(&mut buf[n..], fds)?;
                n += self.preferred_action.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_actions {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (dnd_actions, m) =
                    <super::wl_data_device_manager::dnd_action>::deserialize(&buf[n..], fds)?;
                n += m;
                let (preferred_action, m) =
                    <super::wl_data_device_manager::dnd_action>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_actions { dnd_actions, preferred_action }, n))
            }
        }
        impl Msg<Obj> for set_actions {
            const OP: u16 = 4;
        }
        impl Request<Obj> for set_actions {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// advertise offered mime type
        ///
        /// Sent immediately after creating the wl_data_offer object.  One
        /// event per offered mime type.
        ///
        #[derive(Debug)]
        pub struct offer {
            // offered mime type
            pub mime_type: String,
        }
        impl Ser for offer {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.mime_type.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for offer {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (mime_type, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((offer { mime_type }, n))
            }
        }
        impl Msg<Obj> for offer {
            const OP: u16 = 0;
        }
        impl Event<Obj> for offer {}
        /// notify the source-side available actions
        ///
        /// This event indicates the actions offered by the data source. It
        /// will be sent immediately after creating the wl_data_offer object,
        /// or anytime the source side changes its offered actions through
        /// wl_data_source.set_actions.
        ///
        #[derive(Debug)]
        pub struct source_actions {
            // actions offered by the data source
            pub source_actions: super::wl_data_device_manager::dnd_action,
        }
        impl Ser for source_actions {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.source_actions.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for source_actions {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (source_actions, m) =
                    <super::wl_data_device_manager::dnd_action>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((source_actions { source_actions }, n))
            }
        }
        impl Msg<Obj> for source_actions {
            const OP: u16 = 1;
        }
        impl Event<Obj> for source_actions {}
        /// notify the selected action
        ///
        /// This event indicates the action selected by the compositor after
        /// matching the source/destination side actions. Only one action (or
        /// none) will be offered here.
        ///
        /// This event can be emitted multiple times during the drag-and-drop
        /// operation in response to destination side action changes through
        /// wl_data_offer.set_actions.
        ///
        /// This event will no longer be emitted after wl_data_device.drop
        /// happened on the drag-and-drop destination, the client must
        /// honor the last action received, or the last preferred one set
        /// through wl_data_offer.set_actions when handling an "ask" action.
        ///
        /// Compositors may also change the selected action on the fly, mainly
        /// in response to keyboard modifier changes during the drag-and-drop
        /// operation.
        ///
        /// The most recent action received is always the valid one. Prior to
        /// receiving wl_data_device.drop, the chosen action may change (e.g.
        /// due to keyboard modifiers being pressed). At the time of receiving
        /// wl_data_device.drop the drag-and-drop destination must honor the
        /// last action received.
        ///
        /// Action changes may still happen after wl_data_device.drop,
        /// especially on "ask" actions, where the drag-and-drop destination
        /// may choose another action afterwards. Action changes happening
        /// at this stage are always the result of inter-client negotiation, the
        /// compositor shall no longer be able to induce a different action.
        ///
        /// Upon "ask" actions, it is expected that the drag-and-drop destination
        /// may potentially choose a different action and/or mime type,
        /// based on wl_data_offer.source_actions and finally chosen by the
        /// user (e.g. popping up a menu with the available options). The
        /// final wl_data_offer.set_actions and wl_data_offer.accept requests
        /// must happen before the call to wl_data_offer.finish.
        ///
        #[derive(Debug)]
        pub struct action {
            // action selected by the compositor
            pub dnd_action: super::wl_data_device_manager::dnd_action,
        }
        impl Ser for action {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.dnd_action.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for action {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (dnd_action, m) =
                    <super::wl_data_device_manager::dnd_action>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((action { dnd_action }, n))
            }
        }
        impl Msg<Obj> for action {
            const OP: u16 = 2;
        }
        impl Event<Obj> for action {}
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// finish request was called untimely
        invalid_finish = 0,
        /// action mask contains invalid values
        invalid_action_mask = 1,
        /// action argument has an invalid value
        invalid_action = 2,
        /// offer doesn't accept this request
        invalid_offer = 3,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::invalid_finish => 0,
                error::invalid_action_mask => 1,
                error::invalid_action => 2,
                error::invalid_offer => 3,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::invalid_finish),
                1 => Ok(error::invalid_action_mask),
                2 => Ok(error::invalid_action),
                3 => Ok(error::invalid_offer),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_data_offer;
    pub type Obj = wl_data_offer;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_data_offer;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(2);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// offer to transfer data
///
/// The wl_data_source object is the source side of a wl_data_offer.
/// It is created by the source client in a data transfer and
/// provides a way to describe the offered data and a way to respond
/// to requests to transfer the data.
///
pub mod wl_data_source {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_data_source";
    pub const VERSION: u32 = 3;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// add an offered mime type
        ///
        /// This request adds a mime type to the set of mime types
        /// advertised to targets.  Can be called several times to offer
        /// multiple types.
        ///
        #[derive(Debug)]
        pub struct offer {
            // mime type offered by the data source
            pub mime_type: String,
        }
        impl Ser for offer {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.mime_type.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for offer {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (mime_type, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((offer { mime_type }, n))
            }
        }
        impl Msg<Obj> for offer {
            const OP: u16 = 0;
        }
        impl Request<Obj> for offer {}
        /// destroy the data source
        ///
        /// Destroy the data source.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 1;
        }
        impl Request<Obj> for destroy {}
        /// set the available drag-and-drop actions
        ///
        /// Sets the actions that the source side client supports for this
        /// operation. This request may trigger wl_data_source.action and
        /// wl_data_offer.action events if the compositor needs to change the
        /// selected action.
        ///
        /// The dnd_actions argument must contain only values expressed in the
        /// wl_data_device_manager.dnd_actions enum, otherwise it will result
        /// in a protocol error.
        ///
        /// This request must be made once only, and can only be made on sources
        /// used in drag-and-drop, so it must be performed before
        /// wl_data_device.start_drag. Attempting to use the source other than
        /// for drag-and-drop will raise a protocol error.
        ///
        #[derive(Debug)]
        pub struct set_actions {
            // actions supported by the data source
            pub dnd_actions: super::wl_data_device_manager::dnd_action,
        }
        impl Ser for set_actions {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.dnd_actions.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_actions {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (dnd_actions, m) =
                    <super::wl_data_device_manager::dnd_action>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_actions { dnd_actions }, n))
            }
        }
        impl Msg<Obj> for set_actions {
            const OP: u16 = 2;
        }
        impl Request<Obj> for set_actions {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// a target accepts an offered mime type
        ///
        /// Sent when a target accepts pointer_focus or motion events.  If
        /// a target does not accept any of the offered types, type is NULL.
        ///
        /// Used for feedback during drag-and-drop.
        ///
        #[derive(Debug)]
        pub struct target {
            // mime type accepted by the target
            pub mime_type: String,
        }
        impl Ser for target {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.mime_type.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for target {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (mime_type, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((target { mime_type }, n))
            }
        }
        impl Msg<Obj> for target {
            const OP: u16 = 0;
        }
        impl Event<Obj> for target {}
        /// send the data
        ///
        /// Request for data from the client.  Send the data as the
        /// specified mime type over the passed file descriptor, then
        /// close it.
        ///
        #[derive(Debug)]
        pub struct send {
            // mime type for the data
            pub mime_type: String,
            // file descriptor for the data
            pub fd: OwnedFd,
        }
        impl Ser for send {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.mime_type.serialize(&mut buf[n..], fds)?;
                n += self.fd.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for send {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (mime_type, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                let (fd, m) = <OwnedFd>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((send { mime_type, fd }, n))
            }
        }
        impl Msg<Obj> for send {
            const OP: u16 = 1;
        }
        impl Event<Obj> for send {}
        /// selection was cancelled
        ///
        /// This data source is no longer valid. There are several reasons why
        /// this could happen:
        ///
        /// - The data source has been replaced by another data source.
        /// - The drag-and-drop operation was performed, but the drop destination
        /// did not accept any of the mime types offered through
        /// wl_data_source.target.
        /// - The drag-and-drop operation was performed, but the drop destination
        /// did not select any of the actions present in the mask offered through
        /// wl_data_source.action.
        /// - The drag-and-drop operation was performed but didn't happen over a
        /// surface.
        /// - The compositor cancelled the drag-and-drop operation (e.g. compositor
        /// dependent timeouts to avoid stale drag-and-drop transfers).
        ///
        /// The client should clean up and destroy this data source.
        ///
        /// For objects of version 2 or older, wl_data_source.cancelled will
        /// only be emitted if the data source was replaced by another data
        /// source.
        ///
        #[derive(Debug)]
        pub struct cancelled {}
        impl Ser for cancelled {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for cancelled {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((cancelled {}, n))
            }
        }
        impl Msg<Obj> for cancelled {
            const OP: u16 = 2;
        }
        impl Event<Obj> for cancelled {}
        /// the drag-and-drop operation physically finished
        ///
        /// The user performed the drop action. This event does not indicate
        /// acceptance, wl_data_source.cancelled may still be emitted afterwards
        /// if the drop destination does not accept any mime type.
        ///
        /// However, this event might however not be received if the compositor
        /// cancelled the drag-and-drop operation before this event could happen.
        ///
        /// Note that the data_source may still be used in the future and should
        /// not be destroyed here.
        ///
        #[derive(Debug)]
        pub struct dnd_drop_performed {}
        impl Ser for dnd_drop_performed {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for dnd_drop_performed {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((dnd_drop_performed {}, n))
            }
        }
        impl Msg<Obj> for dnd_drop_performed {
            const OP: u16 = 3;
        }
        impl Event<Obj> for dnd_drop_performed {}
        /// the drag-and-drop operation concluded
        ///
        /// The drop destination finished interoperating with this data
        /// source, so the client is now free to destroy this data source and
        /// free all associated data.
        ///
        /// If the action used to perform the operation was "move", the
        /// source can now delete the transferred data.
        ///
        #[derive(Debug)]
        pub struct dnd_finished {}
        impl Ser for dnd_finished {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for dnd_finished {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((dnd_finished {}, n))
            }
        }
        impl Msg<Obj> for dnd_finished {
            const OP: u16 = 4;
        }
        impl Event<Obj> for dnd_finished {}
        /// notify the selected action
        ///
        /// This event indicates the action selected by the compositor after
        /// matching the source/destination side actions. Only one action (or
        /// none) will be offered here.
        ///
        /// This event can be emitted multiple times during the drag-and-drop
        /// operation, mainly in response to destination side changes through
        /// wl_data_offer.set_actions, and as the data device enters/leaves
        /// surfaces.
        ///
        /// It is only possible to receive this event after
        /// wl_data_source.dnd_drop_performed if the drag-and-drop operation
        /// ended in an "ask" action, in which case the final wl_data_source.action
        /// event will happen immediately before wl_data_source.dnd_finished.
        ///
        /// Compositors may also change the selected action on the fly, mainly
        /// in response to keyboard modifier changes during the drag-and-drop
        /// operation.
        ///
        /// The most recent action received is always the valid one. The chosen
        /// action may change alongside negotiation (e.g. an "ask" action can turn
        /// into a "move" operation), so the effects of the final action must
        /// always be applied in wl_data_offer.dnd_finished.
        ///
        /// Clients can trigger cursor surface changes from this point, so
        /// they reflect the current action.
        ///
        #[derive(Debug)]
        pub struct action {
            // action selected by the compositor
            pub dnd_action: super::wl_data_device_manager::dnd_action,
        }
        impl Ser for action {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.dnd_action.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for action {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (dnd_action, m) =
                    <super::wl_data_device_manager::dnd_action>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((action { dnd_action }, n))
            }
        }
        impl Msg<Obj> for action {
            const OP: u16 = 5;
        }
        impl Event<Obj> for action {}
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// action mask contains invalid values
        invalid_action_mask = 0,
        /// source doesn't accept this request
        invalid_source = 1,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::invalid_action_mask => 0,
                error::invalid_source => 1,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::invalid_action_mask),
                1 => Ok(error::invalid_source),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_data_source;
    pub type Obj = wl_data_source;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_data_source;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(1);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// data transfer device
///
/// There is one wl_data_device per seat which can be obtained
/// from the global wl_data_device_manager singleton.
///
/// A wl_data_device provides access to inter-client data transfer
/// mechanisms such as copy-and-paste and drag-and-drop.
///
pub mod wl_data_device {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_data_device";
    pub const VERSION: u32 = 3;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// start drag-and-drop operation
        ///
        /// This request asks the compositor to start a drag-and-drop
        /// operation on behalf of the client.
        ///
        /// The source argument is the data source that provides the data
        /// for the eventual data transfer. If source is NULL, enter, leave
        /// and motion events are sent only to the client that initiated the
        /// drag and the client is expected to handle the data passing
        /// internally. If source is destroyed, the drag-and-drop session will be
        /// cancelled.
        ///
        /// The origin surface is the surface where the drag originates and
        /// the client must have an active implicit grab that matches the
        /// serial.
        ///
        /// The icon surface is an optional (can be NULL) surface that
        /// provides an icon to be moved around with the cursor.  Initially,
        /// the top-left corner of the icon surface is placed at the cursor
        /// hotspot, but subsequent wl_surface.attach request can move the
        /// relative position. Attach requests must be confirmed with
        /// wl_surface.commit as usual. The icon surface is given the role of
        /// a drag-and-drop icon. If the icon surface already has another role,
        /// it raises a protocol error.
        ///
        /// The input region is ignored for wl_surfaces with the role of a
        /// drag-and-drop icon.
        ///
        #[derive(Debug)]
        pub struct start_drag {
            // data source for the eventual transfer
            pub source: Option<object<wl_data_source::Obj>>,
            // surface where the drag originates
            pub origin: object<wl_surface::Obj>,
            // drag-and-drop icon surface
            pub icon: Option<object<wl_surface::Obj>>,
            // serial number of the implicit grab on the origin
            pub serial: u32,
        }
        impl Ser for start_drag {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.source.serialize(&mut buf[n..], fds)?;
                n += self.origin.serialize(&mut buf[n..], fds)?;
                n += self.icon.serialize(&mut buf[n..], fds)?;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for start_drag {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (source, m) =
                    <Option<object<wl_data_source::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (origin, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (icon, m) = <Option<object<wl_surface::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((start_drag { source, origin, icon, serial }, n))
            }
        }
        impl Msg<Obj> for start_drag {
            const OP: u16 = 0;
        }
        impl Request<Obj> for start_drag {}
        /// copy data to the selection
        ///
        /// This request asks the compositor to set the selection
        /// to the data from the source on behalf of the client.
        ///
        /// To unset the selection, set the source to NULL.
        ///
        #[derive(Debug)]
        pub struct set_selection {
            // data source for the selection
            pub source: Option<object<wl_data_source::Obj>>,
            // serial number of the event that triggered this request
            pub serial: u32,
        }
        impl Ser for set_selection {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.source.serialize(&mut buf[n..], fds)?;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_selection {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (source, m) =
                    <Option<object<wl_data_source::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_selection { source, serial }, n))
            }
        }
        impl Msg<Obj> for set_selection {
            const OP: u16 = 1;
        }
        impl Request<Obj> for set_selection {}
        /// destroy data device
        ///
        /// This request destroys the data device.
        ///
        #[derive(Debug)]
        pub struct release {}
        impl Ser for release {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for release {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((release {}, n))
            }
        }
        impl Msg<Obj> for release {
            const OP: u16 = 2;
        }
        impl Request<Obj> for release {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// introduce a new wl_data_offer
        ///
        /// The data_offer event introduces a new wl_data_offer object,
        /// which will subsequently be used in either the
        /// data_device.enter event (for drag-and-drop) or the
        /// data_device.selection event (for selections).  Immediately
        /// following the data_device.data_offer event, the new data_offer
        /// object will send out data_offer.offer events to describe the
        /// mime types it offers.
        ///
        #[derive(Debug)]
        pub struct data_offer {
            // the new data_offer object
            pub id: new_id<wl_data_offer::Obj>,
        }
        impl Ser for data_offer {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for data_offer {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<wl_data_offer::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((data_offer { id }, n))
            }
        }
        impl Msg<Obj> for data_offer {
            const OP: u16 = 0;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_data_offer::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_data_offer::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Event<Obj> for data_offer {}
        /// initiate drag-and-drop session
        ///
        /// This event is sent when an active drag-and-drop pointer enters
        /// a surface owned by the client.  The position of the pointer at
        /// enter time is provided by the x and y arguments, in surface-local
        /// coordinates.
        ///
        #[derive(Debug)]
        pub struct enter {
            // serial number of the enter event
            pub serial: u32,
            // client surface entered
            pub surface: object<wl_surface::Obj>,
            // surface-local x coordinate
            pub x: fixed,
            // surface-local y coordinate
            pub y: fixed,
            // source data_offer object
            pub id: Option<object<wl_data_offer::Obj>>,
        }
        impl Ser for enter {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.surface.serialize(&mut buf[n..], fds)?;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for enter {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (x, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                let (id, m) = <Option<object<wl_data_offer::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((enter { serial, surface, x, y, id }, n))
            }
        }
        impl Msg<Obj> for enter {
            const OP: u16 = 1;
        }
        impl Event<Obj> for enter {}
        /// end drag-and-drop session
        ///
        /// This event is sent when the drag-and-drop pointer leaves the
        /// surface and the session ends.  The client must destroy the
        /// wl_data_offer introduced at enter time at this point.
        ///
        #[derive(Debug)]
        pub struct leave {}
        impl Ser for leave {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for leave {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((leave {}, n))
            }
        }
        impl Msg<Obj> for leave {
            const OP: u16 = 2;
        }
        impl Event<Obj> for leave {}
        /// drag-and-drop session motion
        ///
        /// This event is sent when the drag-and-drop pointer moves within
        /// the currently focused surface. The new position of the pointer
        /// is provided by the x and y arguments, in surface-local
        /// coordinates.
        ///
        #[derive(Debug)]
        pub struct motion {
            // timestamp with millisecond granularity
            pub time: u32,
            // surface-local x coordinate
            pub x: fixed,
            // surface-local y coordinate
            pub y: fixed,
        }
        impl Ser for motion {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.time.serialize(&mut buf[n..], fds)?;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for motion {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (time, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (x, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((motion { time, x, y }, n))
            }
        }
        impl Msg<Obj> for motion {
            const OP: u16 = 3;
        }
        impl Event<Obj> for motion {}
        /// end drag-and-drop session successfully
        ///
        /// The event is sent when a drag-and-drop operation is ended
        /// because the implicit grab is removed.
        ///
        /// The drag-and-drop destination is expected to honor the last action
        /// received through wl_data_offer.action, if the resulting action is
        /// "copy" or "move", the destination can still perform
        /// wl_data_offer.receive requests, and is expected to end all
        /// transfers with a wl_data_offer.finish request.
        ///
        /// If the resulting action is "ask", the action will not be considered
        /// final. The drag-and-drop destination is expected to perform one last
        /// wl_data_offer.set_actions request, or wl_data_offer.destroy in order
        /// to cancel the operation.
        ///
        #[derive(Debug)]
        pub struct drop {}
        impl Ser for drop {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for drop {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((drop {}, n))
            }
        }
        impl Msg<Obj> for drop {
            const OP: u16 = 4;
        }
        impl Event<Obj> for drop {}
        /// advertise new selection
        ///
        /// The selection event is sent out to notify the client of a new
        /// wl_data_offer for the selection for this device.  The
        /// data_device.data_offer and the data_offer.offer events are
        /// sent out immediately before this event to introduce the data
        /// offer object.  The selection event is sent to a client
        /// immediately before receiving keyboard focus and when a new
        /// selection is set while the client has keyboard focus.  The
        /// data_offer is valid until a new data_offer or NULL is received
        /// or until the client loses keyboard focus.  Switching surface with
        /// keyboard focus within the same client doesn't mean a new selection
        /// will be sent.  The client must destroy the previous selection
        /// data_offer, if any, upon receiving this event.
        ///
        #[derive(Debug)]
        pub struct selection {
            // selection data_offer object
            pub id: Option<object<wl_data_offer::Obj>>,
        }
        impl Ser for selection {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for selection {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <Option<object<wl_data_offer::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((selection { id }, n))
            }
        }
        impl Msg<Obj> for selection {
            const OP: u16 = 5;
        }
        impl Event<Obj> for selection {}
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// given wl_surface has another role
        role = 0,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::role => 0,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::role),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_data_device;
    pub type Obj = wl_data_device;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_data_device;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(2);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// data transfer interface
///
/// The wl_data_device_manager is a singleton global object that
/// provides access to inter-client data transfer mechanisms such as
/// copy-and-paste and drag-and-drop.  These mechanisms are tied to
/// a wl_seat and this interface lets a client get a wl_data_device
/// corresponding to a wl_seat.
///
/// Depending on the version bound, the objects created from the bound
/// wl_data_device_manager object will have different requirements for
/// functioning properly. See wl_data_source.set_actions,
/// wl_data_offer.accept and wl_data_offer.finish for details.
///
pub mod wl_data_device_manager {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_data_device_manager";
    pub const VERSION: u32 = 3;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// create a new data source
        ///
        /// Create a new data source.
        ///
        #[derive(Debug)]
        pub struct create_data_source {
            // data source to create
            pub id: new_id<wl_data_source::Obj>,
        }
        impl Ser for create_data_source {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for create_data_source {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<wl_data_source::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((create_data_source { id }, n))
            }
        }
        impl Msg<Obj> for create_data_source {
            const OP: u16 = 0;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_data_source::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_data_source::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for create_data_source {}
        /// create a new data device
        ///
        /// Create a new data device for a given seat.
        ///
        #[derive(Debug)]
        pub struct get_data_device {
            // data device to create
            pub id: new_id<wl_data_device::Obj>,
            // seat associated with the data device
            pub seat: object<wl_seat::Obj>,
        }
        impl Ser for get_data_device {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                n += self.seat.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for get_data_device {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<wl_data_device::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (seat, m) = <object<wl_seat::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((get_data_device { id, seat }, n))
            }
        }
        impl Msg<Obj> for get_data_device {
            const OP: u16 = 1;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_data_device::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_data_device::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for get_data_device {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
    }
    /// drag and drop actions
    ///
    /// This is a bitmask of the available/preferred actions in a
    /// drag-and-drop operation.
    ///
    /// In the compositor, the selected action is a result of matching the
    /// actions offered by the source and destination sides.  "action" events
    /// with a "none" action will be sent to both source and destination if
    /// there is no match. All further checks will effectively happen on
    /// (source actions ∩ destination actions).
    ///
    /// In addition, compositors may also pick different actions in
    /// reaction to key modifiers being pressed. One common design that
    /// is used in major toolkits (and the behavior recommended for
    /// compositors) is:
    ///
    /// - If no modifiers are pressed, the first match (in bit order)
    /// will be used.
    /// - Pressing Shift selects "move", if enabled in the mask.
    /// - Pressing Control selects "copy", if enabled in the mask.
    ///
    /// Behavior beyond that is considered implementation-dependent.
    /// Compositors may for example bind other modifiers (like Alt/Meta)
    /// or drags initiated with other buttons than BTN_LEFT to specific
    /// actions (e.g. "ask").
    ///

    #[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct dnd_action {
        flags: u32,
    }
    impl Debug for dnd_action {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            let mut first = true;
            for (name, val) in Self::EACH {
                if val & *self == val {
                    if !first {
                        f.write_str("|")?;
                    }
                    first = false;
                    f.write_str(name)?;
                }
            }
            if first {
                f.write_str("EMPTY")?;
            }
            Ok(())
        }
    }
    impl std::fmt::LowerHex for dnd_action {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            <u32 as std::fmt::LowerHex>::fmt(&self.flags, f)
        }
    }
    impl Ser for dnd_action {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            self.flags.serialize(buf, fds)
        }
    }
    impl Dsr for dnd_action {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (flags, n) = u32::deserialize(buf, fds)?;
            Ok((dnd_action { flags }, n))
        }
    }
    impl std::ops::BitOr for dnd_action {
        type Output = Self;
        fn bitor(self, rhs: Self) -> Self {
            dnd_action { flags: self.flags | rhs.flags }
        }
    }
    impl std::ops::BitAnd for dnd_action {
        type Output = Self;
        fn bitand(self, rhs: Self) -> Self {
            dnd_action { flags: self.flags & rhs.flags }
        }
    }
    impl dnd_action {
        #![allow(non_upper_case_globals)]

        fn contains(&self, rhs: dnd_action) -> bool {
            (self.flags & rhs.flags) == rhs.flags
        }

        /// no action
        pub const none: dnd_action = dnd_action { flags: 0 };
        /// copy action
        pub const copy: dnd_action = dnd_action { flags: 1 };
        /// move action
        pub const move_: dnd_action = dnd_action { flags: 2 };
        /// ask action
        pub const ask: dnd_action = dnd_action { flags: 4 };
        pub const EMPTY: dnd_action = dnd_action { flags: 0 };
        pub const ALL: dnd_action = dnd_action {
            flags: Self::EMPTY.flags
                | Self::none.flags
                | Self::copy.flags
                | Self::move_.flags
                | Self::ask.flags,
        };
        pub const EACH: [(&'static str, dnd_action); 4] = [
            ("none", Self::none),
            ("copy", Self::copy),
            ("move_", Self::move_),
            ("ask", Self::ask),
        ];
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_data_device_manager;
    pub type Obj = wl_data_device_manager;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_data_device_manager;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = None;
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// create desktop-style surfaces
///
/// This interface is implemented by servers that provide
/// desktop-style user interfaces.
///
/// It allows clients to associate a wl_shell_surface with
/// a basic surface.
///
/// Note! This protocol is deprecated and not intended for production use.
/// For desktop-style user interfaces, use xdg_shell. Compositors and clients
/// should not implement this interface.
///
pub mod wl_shell {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_shell";
    pub const VERSION: u32 = 1;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// create a shell surface from a surface
        ///
        /// Create a shell surface for an existing surface. This gives
        /// the wl_surface the role of a shell surface. If the wl_surface
        /// already has another role, it raises a protocol error.
        ///
        /// Only one shell surface can be associated with a given surface.
        ///
        #[derive(Debug)]
        pub struct get_shell_surface {
            // shell surface to create
            pub id: new_id<wl_shell_surface::Obj>,
            // surface to be given the shell surface role
            pub surface: object<wl_surface::Obj>,
        }
        impl Ser for get_shell_surface {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                n += self.surface.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for get_shell_surface {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<wl_shell_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((get_shell_surface { id, surface }, n))
            }
        }
        impl Msg<Obj> for get_shell_surface {
            const OP: u16 = 0;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_shell_surface::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_shell_surface::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for get_shell_surface {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// given wl_surface has another role
        role = 0,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::role => 0,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::role),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_shell;
    pub type Obj = wl_shell;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_shell;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = None;
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// desktop-style metadata interface
///
/// An interface that may be implemented by a wl_surface, for
/// implementations that provide a desktop-style user interface.
///
/// It provides requests to treat surfaces like toplevel, fullscreen
/// or popup windows, move, resize or maximize them, associate
/// metadata like title and class, etc.
///
/// On the server side the object is automatically destroyed when
/// the related wl_surface is destroyed. On the client side,
/// wl_shell_surface_destroy() must be called before destroying
/// the wl_surface object.
///
pub mod wl_shell_surface {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_shell_surface";
    pub const VERSION: u32 = 1;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// respond to a ping event
        ///
        /// A client must respond to a ping event with a pong request or
        /// the client may be deemed unresponsive.
        ///
        #[derive(Debug)]
        pub struct pong {
            // serial number of the ping event
            pub serial: u32,
        }
        impl Ser for pong {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for pong {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((pong { serial }, n))
            }
        }
        impl Msg<Obj> for pong {
            const OP: u16 = 0;
        }
        impl Request<Obj> for pong {}
        /// start an interactive move
        ///
        /// Start a pointer-driven move of the surface.
        ///
        /// This request must be used in response to a button press event.
        /// The server may ignore move requests depending on the state of
        /// the surface (e.g. fullscreen or maximized).
        ///
        #[derive(Debug)]
        pub struct move_ {
            // seat whose pointer is used
            pub seat: object<wl_seat::Obj>,
            // serial number of the implicit grab on the pointer
            pub serial: u32,
        }
        impl Ser for move_ {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.seat.serialize(&mut buf[n..], fds)?;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for move_ {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (seat, m) = <object<wl_seat::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((move_ { seat, serial }, n))
            }
        }
        impl Msg<Obj> for move_ {
            const OP: u16 = 1;
        }
        impl Request<Obj> for move_ {}
        /// start an interactive resize
        ///
        /// Start a pointer-driven resizing of the surface.
        ///
        /// This request must be used in response to a button press event.
        /// The server may ignore resize requests depending on the state of
        /// the surface (e.g. fullscreen or maximized).
        ///
        #[derive(Debug)]
        pub struct resize {
            // seat whose pointer is used
            pub seat: object<wl_seat::Obj>,
            // serial number of the implicit grab on the pointer
            pub serial: u32,
            // which edge or corner is being dragged
            pub edges: super::resize,
        }
        impl Ser for resize {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.seat.serialize(&mut buf[n..], fds)?;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.edges.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for resize {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (seat, m) = <object<wl_seat::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (edges, m) = <super::resize>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((resize { seat, serial, edges }, n))
            }
        }
        impl Msg<Obj> for resize {
            const OP: u16 = 2;
        }
        impl Request<Obj> for resize {}
        /// make the surface a toplevel surface
        ///
        /// Map the surface as a toplevel surface.
        ///
        /// A toplevel surface is not fullscreen, maximized or transient.
        ///
        #[derive(Debug)]
        pub struct set_toplevel {}
        impl Ser for set_toplevel {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for set_toplevel {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((set_toplevel {}, n))
            }
        }
        impl Msg<Obj> for set_toplevel {
            const OP: u16 = 3;
        }
        impl Request<Obj> for set_toplevel {}
        /// make the surface a transient surface
        ///
        /// Map the surface relative to an existing surface.
        ///
        /// The x and y arguments specify the location of the upper left
        /// corner of the surface relative to the upper left corner of the
        /// parent surface, in surface-local coordinates.
        ///
        /// The flags argument controls details of the transient behaviour.
        ///
        #[derive(Debug)]
        pub struct set_transient {
            // parent surface
            pub parent: object<wl_surface::Obj>,
            // surface-local x coordinate
            pub x: i32,
            // surface-local y coordinate
            pub y: i32,
            // transient surface behavior
            pub flags: super::transient,
        }
        impl Ser for set_transient {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.parent.serialize(&mut buf[n..], fds)?;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                n += self.flags.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_transient {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (parent, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (flags, m) = <super::transient>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_transient { parent, x, y, flags }, n))
            }
        }
        impl Msg<Obj> for set_transient {
            const OP: u16 = 4;
        }
        impl Request<Obj> for set_transient {}
        /// make the surface a fullscreen surface
        ///
        /// Map the surface as a fullscreen surface.
        ///
        /// If an output parameter is given then the surface will be made
        /// fullscreen on that output. If the client does not specify the
        /// output then the compositor will apply its policy - usually
        /// choosing the output on which the surface has the biggest surface
        /// area.
        ///
        /// The client may specify a method to resolve a size conflict
        /// between the output size and the surface size - this is provided
        /// through the method parameter.
        ///
        /// The framerate parameter is used only when the method is set
        /// to "driver", to indicate the preferred framerate. A value of 0
        /// indicates that the client does not care about framerate.  The
        /// framerate is specified in mHz, that is framerate of 60000 is 60Hz.
        ///
        /// A method of "scale" or "driver" implies a scaling operation of
        /// the surface, either via a direct scaling operation or a change of
        /// the output mode. This will override any kind of output scaling, so
        /// that mapping a surface with a buffer size equal to the mode can
        /// fill the screen independent of buffer_scale.
        ///
        /// A method of "fill" means we don't scale up the buffer, however
        /// any output scale is applied. This means that you may run into
        /// an edge case where the application maps a buffer with the same
        /// size of the output mode but buffer_scale 1 (thus making a
        /// surface larger than the output). In this case it is allowed to
        /// downscale the results to fit the screen.
        ///
        /// The compositor must reply to this request with a configure event
        /// with the dimensions for the output on which the surface will
        /// be made fullscreen.
        ///
        #[derive(Debug)]
        pub struct set_fullscreen {
            // method for resolving size conflict
            pub method: super::fullscreen_method,
            // framerate in mHz
            pub framerate: u32,
            // output on which the surface is to be fullscreen
            pub output: Option<object<wl_output::Obj>>,
        }
        impl Ser for set_fullscreen {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.method.serialize(&mut buf[n..], fds)?;
                n += self.framerate.serialize(&mut buf[n..], fds)?;
                n += self.output.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_fullscreen {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (method, m) = <super::fullscreen_method>::deserialize(&buf[n..], fds)?;
                n += m;
                let (framerate, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (output, m) = <Option<object<wl_output::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_fullscreen { method, framerate, output }, n))
            }
        }
        impl Msg<Obj> for set_fullscreen {
            const OP: u16 = 5;
        }
        impl Request<Obj> for set_fullscreen {}
        /// make the surface a popup surface
        ///
        /// Map the surface as a popup.
        ///
        /// A popup surface is a transient surface with an added pointer
        /// grab.
        ///
        /// An existing implicit grab will be changed to owner-events mode,
        /// and the popup grab will continue after the implicit grab ends
        /// (i.e. releasing the mouse button does not cause the popup to
        /// be unmapped).
        ///
        /// The popup grab continues until the window is destroyed or a
        /// mouse button is pressed in any other client's window. A click
        /// in any of the client's surfaces is reported as normal, however,
        /// clicks in other clients' surfaces will be discarded and trigger
        /// the callback.
        ///
        /// The x and y arguments specify the location of the upper left
        /// corner of the surface relative to the upper left corner of the
        /// parent surface, in surface-local coordinates.
        ///
        #[derive(Debug)]
        pub struct set_popup {
            // seat whose pointer is used
            pub seat: object<wl_seat::Obj>,
            // serial number of the implicit grab on the pointer
            pub serial: u32,
            // parent surface
            pub parent: object<wl_surface::Obj>,
            // surface-local x coordinate
            pub x: i32,
            // surface-local y coordinate
            pub y: i32,
            // transient surface behavior
            pub flags: super::transient,
        }
        impl Ser for set_popup {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.seat.serialize(&mut buf[n..], fds)?;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.parent.serialize(&mut buf[n..], fds)?;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                n += self.flags.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_popup {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (seat, m) = <object<wl_seat::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (parent, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (flags, m) = <super::transient>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_popup { seat, serial, parent, x, y, flags }, n))
            }
        }
        impl Msg<Obj> for set_popup {
            const OP: u16 = 6;
        }
        impl Request<Obj> for set_popup {}
        /// make the surface a maximized surface
        ///
        /// Map the surface as a maximized surface.
        ///
        /// If an output parameter is given then the surface will be
        /// maximized on that output. If the client does not specify the
        /// output then the compositor will apply its policy - usually
        /// choosing the output on which the surface has the biggest surface
        /// area.
        ///
        /// The compositor will reply with a configure event telling
        /// the expected new surface size. The operation is completed
        /// on the next buffer attach to this surface.
        ///
        /// A maximized surface typically fills the entire output it is
        /// bound to, except for desktop elements such as panels. This is
        /// the main difference between a maximized shell surface and a
        /// fullscreen shell surface.
        ///
        /// The details depend on the compositor implementation.
        ///
        #[derive(Debug)]
        pub struct set_maximized {
            // output on which the surface is to be maximized
            pub output: Option<object<wl_output::Obj>>,
        }
        impl Ser for set_maximized {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.output.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_maximized {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (output, m) = <Option<object<wl_output::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_maximized { output }, n))
            }
        }
        impl Msg<Obj> for set_maximized {
            const OP: u16 = 7;
        }
        impl Request<Obj> for set_maximized {}
        /// set surface title
        ///
        /// Set a short title for the surface.
        ///
        /// This string may be used to identify the surface in a task bar,
        /// window list, or other user interface elements provided by the
        /// compositor.
        ///
        /// The string must be encoded in UTF-8.
        ///
        #[derive(Debug)]
        pub struct set_title {
            // surface title
            pub title: String,
        }
        impl Ser for set_title {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.title.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_title {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (title, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_title { title }, n))
            }
        }
        impl Msg<Obj> for set_title {
            const OP: u16 = 8;
        }
        impl Request<Obj> for set_title {}
        /// set surface class
        ///
        /// Set a class for the surface.
        ///
        /// The surface class identifies the general class of applications
        /// to which the surface belongs. A common convention is to use the
        /// file name (or the full path if it is a non-standard location) of
        /// the application's .desktop file as the class.
        ///
        #[derive(Debug)]
        pub struct set_class {
            // surface class
            pub class_: String,
        }
        impl Ser for set_class {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.class_.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_class {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (class_, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_class { class_ }, n))
            }
        }
        impl Msg<Obj> for set_class {
            const OP: u16 = 9;
        }
        impl Request<Obj> for set_class {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// ping client
        ///
        /// Ping a client to check if it is receiving events and sending
        /// requests. A client is expected to reply with a pong request.
        ///
        #[derive(Debug)]
        pub struct ping {
            // serial number of the ping
            pub serial: u32,
        }
        impl Ser for ping {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for ping {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((ping { serial }, n))
            }
        }
        impl Msg<Obj> for ping {
            const OP: u16 = 0;
        }
        impl Event<Obj> for ping {}
        /// suggest resize
        ///
        /// The configure event asks the client to resize its surface.
        ///
        /// The size is a hint, in the sense that the client is free to
        /// ignore it if it doesn't resize, pick a smaller size (to
        /// satisfy aspect ratio or resize in steps of NxM pixels).
        ///
        /// The edges parameter provides a hint about how the surface
        /// was resized. The client may use this information to decide
        /// how to adjust its content to the new size (e.g. a scrolling
        /// area might adjust its content position to leave the viewable
        /// content unmoved).
        ///
        /// The client is free to dismiss all but the last configure
        /// event it received.
        ///
        /// The width and height arguments specify the size of the window
        /// in surface-local coordinates.
        ///
        #[derive(Debug)]
        pub struct configure {
            // how the surface was resized
            pub edges: super::resize,
            // new width of the surface
            pub width: i32,
            // new height of the surface
            pub height: i32,
        }
        impl Ser for configure {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.edges.serialize(&mut buf[n..], fds)?;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for configure {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (edges, m) = <super::resize>::deserialize(&buf[n..], fds)?;
                n += m;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((configure { edges, width, height }, n))
            }
        }
        impl Msg<Obj> for configure {
            const OP: u16 = 1;
        }
        impl Event<Obj> for configure {}
        /// popup interaction is done
        ///
        /// The popup_done event is sent out when a popup grab is broken,
        /// that is, when the user clicks a surface that doesn't belong
        /// to the client owning the popup surface.
        ///
        #[derive(Debug)]
        pub struct popup_done {}
        impl Ser for popup_done {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for popup_done {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((popup_done {}, n))
            }
        }
        impl Msg<Obj> for popup_done {
            const OP: u16 = 2;
        }
        impl Event<Obj> for popup_done {}
    }
    /// edge values for resizing
    ///
    /// These values are used to indicate which edge of a surface
    /// is being dragged in a resize operation. The server may
    /// use this information to adapt its behavior, e.g. choose
    /// an appropriate cursor image.
    ///

    #[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct resize {
        flags: u32,
    }
    impl Debug for resize {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            let mut first = true;
            for (name, val) in Self::EACH {
                if val & *self == val {
                    if !first {
                        f.write_str("|")?;
                    }
                    first = false;
                    f.write_str(name)?;
                }
            }
            if first {
                f.write_str("EMPTY")?;
            }
            Ok(())
        }
    }
    impl std::fmt::LowerHex for resize {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            <u32 as std::fmt::LowerHex>::fmt(&self.flags, f)
        }
    }
    impl Ser for resize {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            self.flags.serialize(buf, fds)
        }
    }
    impl Dsr for resize {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (flags, n) = u32::deserialize(buf, fds)?;
            Ok((resize { flags }, n))
        }
    }
    impl std::ops::BitOr for resize {
        type Output = Self;
        fn bitor(self, rhs: Self) -> Self {
            resize { flags: self.flags | rhs.flags }
        }
    }
    impl std::ops::BitAnd for resize {
        type Output = Self;
        fn bitand(self, rhs: Self) -> Self {
            resize { flags: self.flags & rhs.flags }
        }
    }
    impl resize {
        #![allow(non_upper_case_globals)]

        fn contains(&self, rhs: resize) -> bool {
            (self.flags & rhs.flags) == rhs.flags
        }

        /// no edge
        pub const none: resize = resize { flags: 0 };
        /// top edge
        pub const top: resize = resize { flags: 1 };
        /// bottom edge
        pub const bottom: resize = resize { flags: 2 };
        /// left edge
        pub const left: resize = resize { flags: 4 };
        /// top and left edges
        pub const top_left: resize = resize { flags: 5 };
        /// bottom and left edges
        pub const bottom_left: resize = resize { flags: 6 };
        /// right edge
        pub const right: resize = resize { flags: 8 };
        /// top and right edges
        pub const top_right: resize = resize { flags: 9 };
        /// bottom and right edges
        pub const bottom_right: resize = resize { flags: 10 };
        pub const EMPTY: resize = resize { flags: 0 };
        pub const ALL: resize = resize {
            flags: Self::EMPTY.flags
                | Self::none.flags
                | Self::top.flags
                | Self::bottom.flags
                | Self::left.flags
                | Self::top_left.flags
                | Self::bottom_left.flags
                | Self::right.flags
                | Self::top_right.flags
                | Self::bottom_right.flags,
        };
        pub const EACH: [(&'static str, resize); 9] = [
            ("none", Self::none),
            ("top", Self::top),
            ("bottom", Self::bottom),
            ("left", Self::left),
            ("top_left", Self::top_left),
            ("bottom_left", Self::bottom_left),
            ("right", Self::right),
            ("top_right", Self::top_right),
            ("bottom_right", Self::bottom_right),
        ];
    }
    /// details of transient behaviour
    ///
    /// These flags specify details of the expected behaviour
    /// of transient surfaces. Used in the set_transient request.
    ///

    #[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct transient {
        flags: u32,
    }
    impl Debug for transient {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            let mut first = true;
            for (name, val) in Self::EACH {
                if val & *self == val {
                    if !first {
                        f.write_str("|")?;
                    }
                    first = false;
                    f.write_str(name)?;
                }
            }
            if first {
                f.write_str("EMPTY")?;
            }
            Ok(())
        }
    }
    impl std::fmt::LowerHex for transient {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            <u32 as std::fmt::LowerHex>::fmt(&self.flags, f)
        }
    }
    impl Ser for transient {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            self.flags.serialize(buf, fds)
        }
    }
    impl Dsr for transient {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (flags, n) = u32::deserialize(buf, fds)?;
            Ok((transient { flags }, n))
        }
    }
    impl std::ops::BitOr for transient {
        type Output = Self;
        fn bitor(self, rhs: Self) -> Self {
            transient { flags: self.flags | rhs.flags }
        }
    }
    impl std::ops::BitAnd for transient {
        type Output = Self;
        fn bitand(self, rhs: Self) -> Self {
            transient { flags: self.flags & rhs.flags }
        }
    }
    impl transient {
        #![allow(non_upper_case_globals)]

        fn contains(&self, rhs: transient) -> bool {
            (self.flags & rhs.flags) == rhs.flags
        }

        /// do not set keyboard focus
        pub const inactive: transient = transient { flags: 1 };
        pub const EMPTY: transient = transient { flags: 0 };
        pub const ALL: transient = transient { flags: Self::EMPTY.flags | Self::inactive.flags };
        pub const EACH: [(&'static str, transient); 1] = [("inactive", Self::inactive)];
    }
    /// different method to set the surface fullscreen
    ///
    /// Hints to indicate to the compositor how to deal with a conflict
    /// between the dimensions of the surface and the dimensions of the
    /// output. The compositor is free to ignore this parameter.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum fullscreen_method {
        /// no preference, apply default policy
        default = 0,
        /// scale, preserve the surface's aspect ratio and center on output
        scale = 1,
        /// switch output mode to the smallest mode that can fit the surface, add black borders to compensate size mismatch
        driver = 2,
        /// no upscaling, center on output and add black borders to compensate size mismatch
        fill = 3,
    }

    impl From<fullscreen_method> for u32 {
        fn from(v: fullscreen_method) -> u32 {
            match v {
                fullscreen_method::default => 0,
                fullscreen_method::scale => 1,
                fullscreen_method::driver => 2,
                fullscreen_method::fill => 3,
            }
        }
    }

    impl TryFrom<u32> for fullscreen_method {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(fullscreen_method::default),
                1 => Ok(fullscreen_method::scale),
                2 => Ok(fullscreen_method::driver),
                3 => Ok(fullscreen_method::fill),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for fullscreen_method {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for fullscreen_method {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for fullscreen_method")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_shell_surface;
    pub type Obj = wl_shell_surface;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_shell_surface;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = None;
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// an onscreen surface
///
/// A surface is a rectangular area that may be displayed on zero
/// or more outputs, and shown any number of times at the compositor's
/// discretion. They can present wl_buffers, receive user input, and
/// define a local coordinate system.
///
/// The size of a surface (and relative positions on it) is described
/// in surface-local coordinates, which may differ from the buffer
/// coordinates of the pixel content, in case a buffer_transform
/// or a buffer_scale is used.
///
/// A surface without a "role" is fairly useless: a compositor does
/// not know where, when or how to present it. The role is the
/// purpose of a wl_surface. Examples of roles are a cursor for a
/// pointer (as set by wl_pointer.set_cursor), a drag icon
/// (wl_data_device.start_drag), a sub-surface
/// (wl_subcompositor.get_subsurface), and a window as defined by a
/// shell protocol (e.g. wl_shell.get_shell_surface).
///
/// A surface can have only one role at a time. Initially a
/// wl_surface does not have a role. Once a wl_surface is given a
/// role, it is set permanently for the whole lifetime of the
/// wl_surface object. Giving the current role again is allowed,
/// unless explicitly forbidden by the relevant interface
/// specification.
///
/// Surface roles are given by requests in other interfaces such as
/// wl_pointer.set_cursor. The request should explicitly mention
/// that this request gives a role to a wl_surface. Often, this
/// request also creates a new protocol object that represents the
/// role and adds additional functionality to wl_surface. When a
/// client wants to destroy a wl_surface, they must destroy this role
/// object before the wl_surface, otherwise a defunct_role_object error is
/// sent.
///
/// Destroying the role object does not remove the role from the
/// wl_surface, but it may stop the wl_surface from "playing the role".
/// For instance, if a wl_subsurface object is destroyed, the wl_surface
/// it was created for will be unmapped and forget its position and
/// z-order. It is allowed to create a wl_subsurface for the same
/// wl_surface again, but it is not allowed to use the wl_surface as
/// a cursor (cursor is a different role than sub-surface, and role
/// switching is not allowed).
///
pub mod wl_surface {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_surface";
    pub const VERSION: u32 = 6;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// delete surface
        ///
        /// Deletes the surface and invalidates its object ID.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 0;
        }
        impl Request<Obj> for destroy {}
        /// set the surface contents
        ///
        /// Set a buffer as the content of this surface.
        ///
        /// The new size of the surface is calculated based on the buffer
        /// size transformed by the inverse buffer_transform and the
        /// inverse buffer_scale. This means that at commit time the supplied
        /// buffer size must be an integer multiple of the buffer_scale. If
        /// that's not the case, an invalid_size error is sent.
        ///
        /// The x and y arguments specify the location of the new pending
        /// buffer's upper left corner, relative to the current buffer's upper
        /// left corner, in surface-local coordinates. In other words, the
        /// x and y, combined with the new surface size define in which
        /// directions the surface's size changes. Setting anything other than 0
        /// as x and y arguments is discouraged, and should instead be replaced
        /// with using the separate wl_surface.offset request.
        ///
        /// When the bound wl_surface version is 5 or higher, passing any
        /// non-zero x or y is a protocol violation, and will result in an
        /// 'invalid_offset' error being raised. The x and y arguments are ignored
        /// and do not change the pending state. To achieve equivalent semantics,
        /// use wl_surface.offset.
        ///
        /// Surface contents are double-buffered state, see wl_surface.commit.
        ///
        /// The initial surface contents are void; there is no content.
        /// wl_surface.attach assigns the given wl_buffer as the pending
        /// wl_buffer. wl_surface.commit makes the pending wl_buffer the new
        /// surface contents, and the size of the surface becomes the size
        /// calculated from the wl_buffer, as described above. After commit,
        /// there is no pending buffer until the next attach.
        ///
        /// Committing a pending wl_buffer allows the compositor to read the
        /// pixels in the wl_buffer. The compositor may access the pixels at
        /// any time after the wl_surface.commit request. When the compositor
        /// will not access the pixels anymore, it will send the
        /// wl_buffer.release event. Only after receiving wl_buffer.release,
        /// the client may reuse the wl_buffer. A wl_buffer that has been
        /// attached and then replaced by another attach instead of committed
        /// will not receive a release event, and is not used by the
        /// compositor.
        ///
        /// If a pending wl_buffer has been committed to more than one wl_surface,
        /// the delivery of wl_buffer.release events becomes undefined. A well
        /// behaved client should not rely on wl_buffer.release events in this
        /// case. Alternatively, a client could create multiple wl_buffer objects
        /// from the same backing storage or use wp_linux_buffer_release.
        ///
        /// Destroying the wl_buffer after wl_buffer.release does not change
        /// the surface contents. Destroying the wl_buffer before wl_buffer.release
        /// is allowed as long as the underlying buffer storage isn't re-used (this
        /// can happen e.g. on client process termination). However, if the client
        /// destroys the wl_buffer before receiving the wl_buffer.release event and
        /// mutates the underlying buffer storage, the surface contents become
        /// undefined immediately.
        ///
        /// If wl_surface.attach is sent with a NULL wl_buffer, the
        /// following wl_surface.commit will remove the surface content.
        ///
        #[derive(Debug)]
        pub struct attach {
            // buffer of surface contents
            pub buffer: Option<object<wl_buffer::Obj>>,
            // surface-local x coordinate
            pub x: i32,
            // surface-local y coordinate
            pub y: i32,
        }
        impl Ser for attach {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.buffer.serialize(&mut buf[n..], fds)?;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for attach {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (buffer, m) = <Option<object<wl_buffer::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((attach { buffer, x, y }, n))
            }
        }
        impl Msg<Obj> for attach {
            const OP: u16 = 1;
        }
        impl Request<Obj> for attach {}
        /// mark part of the surface damaged
        ///
        /// This request is used to describe the regions where the pending
        /// buffer is different from the current surface contents, and where
        /// the surface therefore needs to be repainted. The compositor
        /// ignores the parts of the damage that fall outside of the surface.
        ///
        /// Damage is double-buffered state, see wl_surface.commit.
        ///
        /// The damage rectangle is specified in surface-local coordinates,
        /// where x and y specify the upper left corner of the damage rectangle.
        ///
        /// The initial value for pending damage is empty: no damage.
        /// wl_surface.damage adds pending damage: the new pending damage
        /// is the union of old pending damage and the given rectangle.
        ///
        /// wl_surface.commit assigns pending damage as the current damage,
        /// and clears pending damage. The server will clear the current
        /// damage as it repaints the surface.
        ///
        /// Note! New clients should not use this request. Instead damage can be
        /// posted with wl_surface.damage_buffer which uses buffer coordinates
        /// instead of surface coordinates.
        ///
        #[derive(Debug)]
        pub struct damage {
            // surface-local x coordinate
            pub x: i32,
            // surface-local y coordinate
            pub y: i32,
            // width of damage rectangle
            pub width: i32,
            // height of damage rectangle
            pub height: i32,
        }
        impl Ser for damage {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for damage {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((damage { x, y, width, height }, n))
            }
        }
        impl Msg<Obj> for damage {
            const OP: u16 = 2;
        }
        impl Request<Obj> for damage {}
        /// request a frame throttling hint
        ///
        /// Request a notification when it is a good time to start drawing a new
        /// frame, by creating a frame callback. This is useful for throttling
        /// redrawing operations, and driving animations.
        ///
        /// When a client is animating on a wl_surface, it can use the 'frame'
        /// request to get notified when it is a good time to draw and commit the
        /// next frame of animation. If the client commits an update earlier than
        /// that, it is likely that some updates will not make it to the display,
        /// and the client is wasting resources by drawing too often.
        ///
        /// The frame request will take effect on the next wl_surface.commit.
        /// The notification will only be posted for one frame unless
        /// requested again. For a wl_surface, the notifications are posted in
        /// the order the frame requests were committed.
        ///
        /// The server must send the notifications so that a client
        /// will not send excessive updates, while still allowing
        /// the highest possible update rate for clients that wait for the reply
        /// before drawing again. The server should give some time for the client
        /// to draw and commit after sending the frame callback events to let it
        /// hit the next output refresh.
        ///
        /// A server should avoid signaling the frame callbacks if the
        /// surface is not visible in any way, e.g. the surface is off-screen,
        /// or completely obscured by other opaque surfaces.
        ///
        /// The object returned by this request will be destroyed by the
        /// compositor after the callback is fired and as such the client must not
        /// attempt to use it after that point.
        ///
        /// The callback_data passed in the callback is the current time, in
        /// milliseconds, with an undefined base.
        ///
        #[derive(Debug)]
        pub struct frame {
            // callback object for the frame request
            pub callback: new_id<wl_callback::Obj>,
        }
        impl Ser for frame {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.callback.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for frame {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (callback, m) = <new_id<wl_callback::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((frame { callback }, n))
            }
        }
        impl Msg<Obj> for frame {
            const OP: u16 = 3;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.callback.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_callback::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_callback::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for frame {}
        /// set opaque region
        ///
        /// This request sets the region of the surface that contains
        /// opaque content.
        ///
        /// The opaque region is an optimization hint for the compositor
        /// that lets it optimize the redrawing of content behind opaque
        /// regions.  Setting an opaque region is not required for correct
        /// behaviour, but marking transparent content as opaque will result
        /// in repaint artifacts.
        ///
        /// The opaque region is specified in surface-local coordinates.
        ///
        /// The compositor ignores the parts of the opaque region that fall
        /// outside of the surface.
        ///
        /// Opaque region is double-buffered state, see wl_surface.commit.
        ///
        /// wl_surface.set_opaque_region changes the pending opaque region.
        /// wl_surface.commit copies the pending region to the current region.
        /// Otherwise, the pending and current regions are never changed.
        ///
        /// The initial value for an opaque region is empty. Setting the pending
        /// opaque region has copy semantics, and the wl_region object can be
        /// destroyed immediately. A NULL wl_region causes the pending opaque
        /// region to be set to empty.
        ///
        #[derive(Debug)]
        pub struct set_opaque_region {
            // opaque region of the surface
            pub region: Option<object<wl_region::Obj>>,
        }
        impl Ser for set_opaque_region {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.region.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_opaque_region {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (region, m) = <Option<object<wl_region::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_opaque_region { region }, n))
            }
        }
        impl Msg<Obj> for set_opaque_region {
            const OP: u16 = 4;
        }
        impl Request<Obj> for set_opaque_region {}
        /// set input region
        ///
        /// This request sets the region of the surface that can receive
        /// pointer and touch events.
        ///
        /// Input events happening outside of this region will try the next
        /// surface in the server surface stack. The compositor ignores the
        /// parts of the input region that fall outside of the surface.
        ///
        /// The input region is specified in surface-local coordinates.
        ///
        /// Input region is double-buffered state, see wl_surface.commit.
        ///
        /// wl_surface.set_input_region changes the pending input region.
        /// wl_surface.commit copies the pending region to the current region.
        /// Otherwise the pending and current regions are never changed,
        /// except cursor and icon surfaces are special cases, see
        /// wl_pointer.set_cursor and wl_data_device.start_drag.
        ///
        /// The initial value for an input region is infinite. That means the
        /// whole surface will accept input. Setting the pending input region
        /// has copy semantics, and the wl_region object can be destroyed
        /// immediately. A NULL wl_region causes the input region to be set
        /// to infinite.
        ///
        #[derive(Debug)]
        pub struct set_input_region {
            // input region of the surface
            pub region: Option<object<wl_region::Obj>>,
        }
        impl Ser for set_input_region {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.region.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_input_region {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (region, m) = <Option<object<wl_region::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_input_region { region }, n))
            }
        }
        impl Msg<Obj> for set_input_region {
            const OP: u16 = 5;
        }
        impl Request<Obj> for set_input_region {}
        /// commit pending surface state
        ///
        /// Surface state (input, opaque, and damage regions, attached buffers,
        /// etc.) is double-buffered. Protocol requests modify the pending state,
        /// as opposed to the current state in use by the compositor. A commit
        /// request atomically applies all pending state, replacing the current
        /// state. After commit, the new pending state is as documented for each
        /// related request.
        ///
        /// On commit, a pending wl_buffer is applied first, and all other state
        /// second. This means that all coordinates in double-buffered state are
        /// relative to the new wl_buffer coming into use, except for
        /// wl_surface.attach itself. If there is no pending wl_buffer, the
        /// coordinates are relative to the current surface contents.
        ///
        /// All requests that need a commit to become effective are documented
        /// to affect double-buffered state.
        ///
        /// Other interfaces may add further double-buffered surface state.
        ///
        #[derive(Debug)]
        pub struct commit {}
        impl Ser for commit {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for commit {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((commit {}, n))
            }
        }
        impl Msg<Obj> for commit {
            const OP: u16 = 6;
        }
        impl Request<Obj> for commit {}
        /// sets the buffer transformation
        ///
        /// This request sets an optional transformation on how the compositor
        /// interprets the contents of the buffer attached to the surface. The
        /// accepted values for the transform parameter are the values for
        /// wl_output.transform.
        ///
        /// Buffer transform is double-buffered state, see wl_surface.commit.
        ///
        /// A newly created surface has its buffer transformation set to normal.
        ///
        /// wl_surface.set_buffer_transform changes the pending buffer
        /// transformation. wl_surface.commit copies the pending buffer
        /// transformation to the current one. Otherwise, the pending and current
        /// values are never changed.
        ///
        /// The purpose of this request is to allow clients to render content
        /// according to the output transform, thus permitting the compositor to
        /// use certain optimizations even if the display is rotated. Using
        /// hardware overlays and scanning out a client buffer for fullscreen
        /// surfaces are examples of such optimizations. Those optimizations are
        /// highly dependent on the compositor implementation, so the use of this
        /// request should be considered on a case-by-case basis.
        ///
        /// Note that if the transform value includes 90 or 270 degree rotation,
        /// the width of the buffer will become the surface height and the height
        /// of the buffer will become the surface width.
        ///
        /// If transform is not one of the values from the
        /// wl_output.transform enum the invalid_transform protocol error
        /// is raised.
        ///
        #[derive(Debug)]
        pub struct set_buffer_transform {
            // transform for interpreting buffer contents
            pub transform: super::wl_output::transform,
        }
        impl Ser for set_buffer_transform {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.transform.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_buffer_transform {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (transform, m) = <super::wl_output::transform>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_buffer_transform { transform }, n))
            }
        }
        impl Msg<Obj> for set_buffer_transform {
            const OP: u16 = 7;
        }
        impl Request<Obj> for set_buffer_transform {}
        /// sets the buffer scaling factor
        ///
        /// This request sets an optional scaling factor on how the compositor
        /// interprets the contents of the buffer attached to the window.
        ///
        /// Buffer scale is double-buffered state, see wl_surface.commit.
        ///
        /// A newly created surface has its buffer scale set to 1.
        ///
        /// wl_surface.set_buffer_scale changes the pending buffer scale.
        /// wl_surface.commit copies the pending buffer scale to the current one.
        /// Otherwise, the pending and current values are never changed.
        ///
        /// The purpose of this request is to allow clients to supply higher
        /// resolution buffer data for use on high resolution outputs. It is
        /// intended that you pick the same buffer scale as the scale of the
        /// output that the surface is displayed on. This means the compositor
        /// can avoid scaling when rendering the surface on that output.
        ///
        /// Note that if the scale is larger than 1, then you have to attach
        /// a buffer that is larger (by a factor of scale in each dimension)
        /// than the desired surface size.
        ///
        /// If scale is not positive the invalid_scale protocol error is
        /// raised.
        ///
        #[derive(Debug)]
        pub struct set_buffer_scale {
            // positive scale for interpreting buffer contents
            pub scale: i32,
        }
        impl Ser for set_buffer_scale {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.scale.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_buffer_scale {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (scale, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_buffer_scale { scale }, n))
            }
        }
        impl Msg<Obj> for set_buffer_scale {
            const OP: u16 = 8;
        }
        impl Request<Obj> for set_buffer_scale {}
        /// mark part of the surface damaged using buffer coordinates
        ///
        /// This request is used to describe the regions where the pending
        /// buffer is different from the current surface contents, and where
        /// the surface therefore needs to be repainted. The compositor
        /// ignores the parts of the damage that fall outside of the surface.
        ///
        /// Damage is double-buffered state, see wl_surface.commit.
        ///
        /// The damage rectangle is specified in buffer coordinates,
        /// where x and y specify the upper left corner of the damage rectangle.
        ///
        /// The initial value for pending damage is empty: no damage.
        /// wl_surface.damage_buffer adds pending damage: the new pending
        /// damage is the union of old pending damage and the given rectangle.
        ///
        /// wl_surface.commit assigns pending damage as the current damage,
        /// and clears pending damage. The server will clear the current
        /// damage as it repaints the surface.
        ///
        /// This request differs from wl_surface.damage in only one way - it
        /// takes damage in buffer coordinates instead of surface-local
        /// coordinates. While this generally is more intuitive than surface
        /// coordinates, it is especially desirable when using wp_viewport
        /// or when a drawing library (like EGL) is unaware of buffer scale
        /// and buffer transform.
        ///
        /// Note: Because buffer transformation changes and damage requests may
        /// be interleaved in the protocol stream, it is impossible to determine
        /// the actual mapping between surface and buffer damage until
        /// wl_surface.commit time. Therefore, compositors wishing to take both
        /// kinds of damage into account will have to accumulate damage from the
        /// two requests separately and only transform from one to the other
        /// after receiving the wl_surface.commit.
        ///
        #[derive(Debug)]
        pub struct damage_buffer {
            // buffer-local x coordinate
            pub x: i32,
            // buffer-local y coordinate
            pub y: i32,
            // width of damage rectangle
            pub width: i32,
            // height of damage rectangle
            pub height: i32,
        }
        impl Ser for damage_buffer {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for damage_buffer {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((damage_buffer { x, y, width, height }, n))
            }
        }
        impl Msg<Obj> for damage_buffer {
            const OP: u16 = 9;
        }
        impl Request<Obj> for damage_buffer {}
        /// set the surface contents offset
        ///
        /// The x and y arguments specify the location of the new pending
        /// buffer's upper left corner, relative to the current buffer's upper
        /// left corner, in surface-local coordinates. In other words, the
        /// x and y, combined with the new surface size define in which
        /// directions the surface's size changes.
        ///
        /// Surface location offset is double-buffered state, see
        /// wl_surface.commit.
        ///
        /// This request is semantically equivalent to and the replaces the x and y
        /// arguments in the wl_surface.attach request in wl_surface versions prior
        /// to 5. See wl_surface.attach for details.
        ///
        #[derive(Debug)]
        pub struct offset {
            // surface-local x coordinate
            pub x: i32,
            // surface-local y coordinate
            pub y: i32,
        }
        impl Ser for offset {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for offset {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((offset { x, y }, n))
            }
        }
        impl Msg<Obj> for offset {
            const OP: u16 = 10;
        }
        impl Request<Obj> for offset {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// surface enters an output
        ///
        /// This is emitted whenever a surface's creation, movement, or resizing
        /// results in some part of it being within the scanout region of an
        /// output.
        ///
        /// Note that a surface may be overlapping with zero or more outputs.
        ///
        #[derive(Debug)]
        pub struct enter {
            // output entered by the surface
            pub output: object<wl_output::Obj>,
        }
        impl Ser for enter {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.output.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for enter {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (output, m) = <object<wl_output::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((enter { output }, n))
            }
        }
        impl Msg<Obj> for enter {
            const OP: u16 = 0;
        }
        impl Event<Obj> for enter {}
        /// surface leaves an output
        ///
        /// This is emitted whenever a surface's creation, movement, or resizing
        /// results in it no longer having any part of it within the scanout region
        /// of an output.
        ///
        /// Clients should not use the number of outputs the surface is on for frame
        /// throttling purposes. The surface might be hidden even if no leave event
        /// has been sent, and the compositor might expect new surface content
        /// updates even if no enter event has been sent. The frame event should be
        /// used instead.
        ///
        #[derive(Debug)]
        pub struct leave {
            // output left by the surface
            pub output: object<wl_output::Obj>,
        }
        impl Ser for leave {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.output.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for leave {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (output, m) = <object<wl_output::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((leave { output }, n))
            }
        }
        impl Msg<Obj> for leave {
            const OP: u16 = 1;
        }
        impl Event<Obj> for leave {}
        /// preferred buffer scale for the surface
        ///
        /// This event indicates the preferred buffer scale for this surface. It is
        /// sent whenever the compositor's preference changes.
        ///
        /// It is intended that scaling aware clients use this event to scale their
        /// content and use wl_surface.set_buffer_scale to indicate the scale they
        /// have rendered with. This allows clients to supply a higher detail
        /// buffer.
        ///
        #[derive(Debug)]
        pub struct preferred_buffer_scale {
            // preferred scaling factor
            pub factor: i32,
        }
        impl Ser for preferred_buffer_scale {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.factor.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for preferred_buffer_scale {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (factor, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((preferred_buffer_scale { factor }, n))
            }
        }
        impl Msg<Obj> for preferred_buffer_scale {
            const OP: u16 = 2;
        }
        impl Event<Obj> for preferred_buffer_scale {}
        /// preferred buffer transform for the surface
        ///
        /// This event indicates the preferred buffer transform for this surface.
        /// It is sent whenever the compositor's preference changes.
        ///
        /// It is intended that transform aware clients use this event to apply the
        /// transform to their content and use wl_surface.set_buffer_transform to
        /// indicate the transform they have rendered with.
        ///
        #[derive(Debug)]
        pub struct preferred_buffer_transform {
            // preferred transform
            pub transform: super::wl_output::transform,
        }
        impl Ser for preferred_buffer_transform {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.transform.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for preferred_buffer_transform {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (transform, m) = <super::wl_output::transform>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((preferred_buffer_transform { transform }, n))
            }
        }
        impl Msg<Obj> for preferred_buffer_transform {
            const OP: u16 = 3;
        }
        impl Event<Obj> for preferred_buffer_transform {}
    }
    /// wl_surface error values
    ///
    /// These errors can be emitted in response to wl_surface requests.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// buffer scale value is invalid
        invalid_scale = 0,
        /// buffer transform value is invalid
        invalid_transform = 1,
        /// buffer size is invalid
        invalid_size = 2,
        /// buffer offset is invalid
        invalid_offset = 3,
        /// surface was destroyed before its role object
        defunct_role_object = 4,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::invalid_scale => 0,
                error::invalid_transform => 1,
                error::invalid_size => 2,
                error::invalid_offset => 3,
                error::defunct_role_object => 4,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::invalid_scale),
                1 => Ok(error::invalid_transform),
                2 => Ok(error::invalid_size),
                3 => Ok(error::invalid_offset),
                4 => Ok(error::defunct_role_object),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_surface;
    pub type Obj = wl_surface;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_surface;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(0);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// group of input devices
///
/// A seat is a group of keyboards, pointer and touch devices. This
/// object is published as a global during start up, or when such a
/// device is hot plugged.  A seat typically has a pointer and
/// maintains a keyboard focus and a pointer focus.
///
pub mod wl_seat {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_seat";
    pub const VERSION: u32 = 9;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// return pointer object
        ///
        /// The ID provided will be initialized to the wl_pointer interface
        /// for this seat.
        ///
        /// This request only takes effect if the seat has the pointer
        /// capability, or has had the pointer capability in the past.
        /// It is a protocol violation to issue this request on a seat that has
        /// never had the pointer capability. The missing_capability error will
        /// be sent in this case.
        ///
        #[derive(Debug)]
        pub struct get_pointer {
            // seat pointer
            pub id: new_id<wl_pointer::Obj>,
        }
        impl Ser for get_pointer {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for get_pointer {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<wl_pointer::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((get_pointer { id }, n))
            }
        }
        impl Msg<Obj> for get_pointer {
            const OP: u16 = 0;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_pointer::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_pointer::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for get_pointer {}
        /// return keyboard object
        ///
        /// The ID provided will be initialized to the wl_keyboard interface
        /// for this seat.
        ///
        /// This request only takes effect if the seat has the keyboard
        /// capability, or has had the keyboard capability in the past.
        /// It is a protocol violation to issue this request on a seat that has
        /// never had the keyboard capability. The missing_capability error will
        /// be sent in this case.
        ///
        #[derive(Debug)]
        pub struct get_keyboard {
            // seat keyboard
            pub id: new_id<wl_keyboard::Obj>,
        }
        impl Ser for get_keyboard {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for get_keyboard {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<wl_keyboard::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((get_keyboard { id }, n))
            }
        }
        impl Msg<Obj> for get_keyboard {
            const OP: u16 = 1;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_keyboard::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_keyboard::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for get_keyboard {}
        /// return touch object
        ///
        /// The ID provided will be initialized to the wl_touch interface
        /// for this seat.
        ///
        /// This request only takes effect if the seat has the touch
        /// capability, or has had the touch capability in the past.
        /// It is a protocol violation to issue this request on a seat that has
        /// never had the touch capability. The missing_capability error will
        /// be sent in this case.
        ///
        #[derive(Debug)]
        pub struct get_touch {
            // seat touch interface
            pub id: new_id<wl_touch::Obj>,
        }
        impl Ser for get_touch {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for get_touch {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<wl_touch::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((get_touch { id }, n))
            }
        }
        impl Msg<Obj> for get_touch {
            const OP: u16 = 2;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_touch::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_touch::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for get_touch {}
        /// release the seat object
        ///
        /// Using this request a client can tell the server that it is not going to
        /// use the seat object anymore.
        ///
        #[derive(Debug)]
        pub struct release {}
        impl Ser for release {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for release {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((release {}, n))
            }
        }
        impl Msg<Obj> for release {
            const OP: u16 = 3;
        }
        impl Request<Obj> for release {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// seat capabilities changed
        ///
        /// This is emitted whenever a seat gains or loses the pointer,
        /// keyboard or touch capabilities.  The argument is a capability
        /// enum containing the complete set of capabilities this seat has.
        ///
        /// When the pointer capability is added, a client may create a
        /// wl_pointer object using the wl_seat.get_pointer request. This object
        /// will receive pointer events until the capability is removed in the
        /// future.
        ///
        /// When the pointer capability is removed, a client should destroy the
        /// wl_pointer objects associated with the seat where the capability was
        /// removed, using the wl_pointer.release request. No further pointer
        /// events will be received on these objects.
        ///
        /// In some compositors, if a seat regains the pointer capability and a
        /// client has a previously obtained wl_pointer object of version 4 or
        /// less, that object may start sending pointer events again. This
        /// behavior is considered a misinterpretation of the intended behavior
        /// and must not be relied upon by the client. wl_pointer objects of
        /// version 5 or later must not send events if created before the most
        /// recent event notifying the client of an added pointer capability.
        ///
        /// The above behavior also applies to wl_keyboard and wl_touch with the
        /// keyboard and touch capabilities, respectively.
        ///
        #[derive(Debug)]
        pub struct capabilities {
            // capabilities of the seat
            pub capabilities: super::capability,
        }
        impl Ser for capabilities {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.capabilities.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for capabilities {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (capabilities, m) = <super::capability>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((capabilities { capabilities }, n))
            }
        }
        impl Msg<Obj> for capabilities {
            const OP: u16 = 0;
        }
        impl Event<Obj> for capabilities {}
        /// unique identifier for this seat
        ///
        /// In a multi-seat configuration the seat name can be used by clients to
        /// help identify which physical devices the seat represents.
        ///
        /// The seat name is a UTF-8 string with no convention defined for its
        /// contents. Each name is unique among all wl_seat globals. The name is
        /// only guaranteed to be unique for the current compositor instance.
        ///
        /// The same seat names are used for all clients. Thus, the name can be
        /// shared across processes to refer to a specific wl_seat global.
        ///
        /// The name event is sent after binding to the seat global. This event is
        /// only sent once per seat object, and the name does not change over the
        /// lifetime of the wl_seat global.
        ///
        /// Compositors may re-use the same seat name if the wl_seat global is
        /// destroyed and re-created later.
        ///
        #[derive(Debug)]
        pub struct name {
            // seat identifier
            pub name: String,
        }
        impl Ser for name {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.name.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for name {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (name, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((name { name }, n))
            }
        }
        impl Msg<Obj> for name {
            const OP: u16 = 1;
        }
        impl Event<Obj> for name {}
    }
    /// seat capability bitmask
    ///
    /// This is a bitmask of capabilities this seat has; if a member is
    /// set, then it is present on the seat.
    ///

    #[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct capability {
        flags: u32,
    }
    impl Debug for capability {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            let mut first = true;
            for (name, val) in Self::EACH {
                if val & *self == val {
                    if !first {
                        f.write_str("|")?;
                    }
                    first = false;
                    f.write_str(name)?;
                }
            }
            if first {
                f.write_str("EMPTY")?;
            }
            Ok(())
        }
    }
    impl std::fmt::LowerHex for capability {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            <u32 as std::fmt::LowerHex>::fmt(&self.flags, f)
        }
    }
    impl Ser for capability {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            self.flags.serialize(buf, fds)
        }
    }
    impl Dsr for capability {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (flags, n) = u32::deserialize(buf, fds)?;
            Ok((capability { flags }, n))
        }
    }
    impl std::ops::BitOr for capability {
        type Output = Self;
        fn bitor(self, rhs: Self) -> Self {
            capability { flags: self.flags | rhs.flags }
        }
    }
    impl std::ops::BitAnd for capability {
        type Output = Self;
        fn bitand(self, rhs: Self) -> Self {
            capability { flags: self.flags & rhs.flags }
        }
    }
    impl capability {
        #![allow(non_upper_case_globals)]

        fn contains(&self, rhs: capability) -> bool {
            (self.flags & rhs.flags) == rhs.flags
        }

        /// the seat has pointer devices
        pub const pointer: capability = capability { flags: 1 };
        /// the seat has one or more keyboards
        pub const keyboard: capability = capability { flags: 2 };
        /// the seat has touch devices
        pub const touch: capability = capability { flags: 4 };
        pub const EMPTY: capability = capability { flags: 0 };
        pub const ALL: capability = capability {
            flags: Self::EMPTY.flags
                | Self::pointer.flags
                | Self::keyboard.flags
                | Self::touch.flags,
        };
        pub const EACH: [(&'static str, capability); 3] =
            [("pointer", Self::pointer), ("keyboard", Self::keyboard), ("touch", Self::touch)];
    }
    /// wl_seat error values
    ///
    /// These errors can be emitted in response to wl_seat requests.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// get_pointer, get_keyboard or get_touch called on seat without the matching capability
        missing_capability = 0,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::missing_capability => 0,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::missing_capability),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_seat;
    pub type Obj = wl_seat;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_seat;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(3);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// pointer input device
///
/// The wl_pointer interface represents one or more input devices,
/// such as mice, which control the pointer location and pointer_focus
/// of a seat.
///
/// The wl_pointer interface generates motion, enter and leave
/// events for the surfaces that the pointer is located over,
/// and button and axis events for button presses, button releases
/// and scrolling.
///
pub mod wl_pointer {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_pointer";
    pub const VERSION: u32 = 9;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// set the pointer surface
        ///
        /// Set the pointer surface, i.e., the surface that contains the
        /// pointer image (cursor). This request gives the surface the role
        /// of a cursor. If the surface already has another role, it raises
        /// a protocol error.
        ///
        /// The cursor actually changes only if the pointer
        /// focus for this device is one of the requesting client's surfaces
        /// or the surface parameter is the current pointer surface. If
        /// there was a previous surface set with this request it is
        /// replaced. If surface is NULL, the pointer image is hidden.
        ///
        /// The parameters hotspot_x and hotspot_y define the position of
        /// the pointer surface relative to the pointer location. Its
        /// top-left corner is always at (x, y) - (hotspot_x, hotspot_y),
        /// where (x, y) are the coordinates of the pointer location, in
        /// surface-local coordinates.
        ///
        /// On surface.attach requests to the pointer surface, hotspot_x
        /// and hotspot_y are decremented by the x and y parameters
        /// passed to the request. Attach must be confirmed by
        /// wl_surface.commit as usual.
        ///
        /// The hotspot can also be updated by passing the currently set
        /// pointer surface to this request with new values for hotspot_x
        /// and hotspot_y.
        ///
        /// The input region is ignored for wl_surfaces with the role of
        /// a cursor. When the use as a cursor ends, the wl_surface is
        /// unmapped.
        ///
        /// The serial parameter must match the latest wl_pointer.enter
        /// serial number sent to the client. Otherwise the request will be
        /// ignored.
        ///
        #[derive(Debug)]
        pub struct set_cursor {
            // serial number of the enter event
            pub serial: u32,
            // pointer surface
            pub surface: Option<object<wl_surface::Obj>>,
            // surface-local x coordinate
            pub hotspot_x: i32,
            // surface-local y coordinate
            pub hotspot_y: i32,
        }
        impl Ser for set_cursor {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.surface.serialize(&mut buf[n..], fds)?;
                n += self.hotspot_x.serialize(&mut buf[n..], fds)?;
                n += self.hotspot_y.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_cursor {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface, m) = <Option<object<wl_surface::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (hotspot_x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (hotspot_y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_cursor { serial, surface, hotspot_x, hotspot_y }, n))
            }
        }
        impl Msg<Obj> for set_cursor {
            const OP: u16 = 0;
        }
        impl Request<Obj> for set_cursor {}
        /// release the pointer object
        ///
        /// Using this request a client can tell the server that it is not going to
        /// use the pointer object anymore.
        ///
        /// This request destroys the pointer proxy object, so clients must not call
        /// wl_pointer_destroy() after using this request.
        ///
        #[derive(Debug)]
        pub struct release {}
        impl Ser for release {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for release {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((release {}, n))
            }
        }
        impl Msg<Obj> for release {
            const OP: u16 = 1;
        }
        impl Request<Obj> for release {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// enter event
        ///
        /// Notification that this seat's pointer is focused on a certain
        /// surface.
        ///
        /// When a seat's focus enters a surface, the pointer image
        /// is undefined and a client should respond to this event by setting
        /// an appropriate pointer image with the set_cursor request.
        ///
        #[derive(Debug)]
        pub struct enter {
            // serial number of the enter event
            pub serial: u32,
            // surface entered by the pointer
            pub surface: object<wl_surface::Obj>,
            // surface-local x coordinate
            pub surface_x: fixed,
            // surface-local y coordinate
            pub surface_y: fixed,
        }
        impl Ser for enter {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.surface.serialize(&mut buf[n..], fds)?;
                n += self.surface_x.serialize(&mut buf[n..], fds)?;
                n += self.surface_y.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for enter {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface_x, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface_y, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((enter { serial, surface, surface_x, surface_y }, n))
            }
        }
        impl Msg<Obj> for enter {
            const OP: u16 = 0;
        }
        impl Event<Obj> for enter {}
        /// leave event
        ///
        /// Notification that this seat's pointer is no longer focused on
        /// a certain surface.
        ///
        /// The leave notification is sent before the enter notification
        /// for the new focus.
        ///
        #[derive(Debug)]
        pub struct leave {
            // serial number of the leave event
            pub serial: u32,
            // surface left by the pointer
            pub surface: object<wl_surface::Obj>,
        }
        impl Ser for leave {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.surface.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for leave {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((leave { serial, surface }, n))
            }
        }
        impl Msg<Obj> for leave {
            const OP: u16 = 1;
        }
        impl Event<Obj> for leave {}
        /// pointer motion event
        ///
        /// Notification of pointer location change. The arguments
        /// surface_x and surface_y are the location relative to the
        /// focused surface.
        ///
        #[derive(Debug)]
        pub struct motion {
            // timestamp with millisecond granularity
            pub time: u32,
            // surface-local x coordinate
            pub surface_x: fixed,
            // surface-local y coordinate
            pub surface_y: fixed,
        }
        impl Ser for motion {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.time.serialize(&mut buf[n..], fds)?;
                n += self.surface_x.serialize(&mut buf[n..], fds)?;
                n += self.surface_y.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for motion {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (time, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface_x, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface_y, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((motion { time, surface_x, surface_y }, n))
            }
        }
        impl Msg<Obj> for motion {
            const OP: u16 = 2;
        }
        impl Event<Obj> for motion {}
        /// pointer button event
        ///
        /// Mouse button click and release notifications.
        ///
        /// The location of the click is given by the last motion or
        /// enter event.
        /// The time argument is a timestamp with millisecond
        /// granularity, with an undefined base.
        ///
        /// The button is a button code as defined in the Linux kernel's
        /// linux/input-event-codes.h header file, e.g. BTN_LEFT.
        ///
        /// Any 16-bit button code value is reserved for future additions to the
        /// kernel's event code list. All other button codes above 0xFFFF are
        /// currently undefined but may be used in future versions of this
        /// protocol.
        ///
        #[derive(Debug)]
        pub struct button {
            // serial number of the button event
            pub serial: u32,
            // timestamp with millisecond granularity
            pub time: u32,
            // button that produced the event
            pub button: u32,
            // physical state of the button
            pub state: super::button_state,
        }
        impl Ser for button {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.time.serialize(&mut buf[n..], fds)?;
                n += self.button.serialize(&mut buf[n..], fds)?;
                n += self.state.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for button {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (time, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (button, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (state, m) = <super::button_state>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((button { serial, time, button, state }, n))
            }
        }
        impl Msg<Obj> for button {
            const OP: u16 = 3;
        }
        impl Event<Obj> for button {}
        /// axis event
        ///
        /// Scroll and other axis notifications.
        ///
        /// For scroll events (vertical and horizontal scroll axes), the
        /// value parameter is the length of a vector along the specified
        /// axis in a coordinate space identical to those of motion events,
        /// representing a relative movement along the specified axis.
        ///
        /// For devices that support movements non-parallel to axes multiple
        /// axis events will be emitted.
        ///
        /// When applicable, for example for touch pads, the server can
        /// choose to emit scroll events where the motion vector is
        /// equivalent to a motion event vector.
        ///
        /// When applicable, a client can transform its content relative to the
        /// scroll distance.
        ///
        #[derive(Debug)]
        pub struct axis {
            // timestamp with millisecond granularity
            pub time: u32,
            // axis type
            pub axis: super::axis,
            // length of vector in surface-local coordinate space
            pub value: fixed,
        }
        impl Ser for axis {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.time.serialize(&mut buf[n..], fds)?;
                n += self.axis.serialize(&mut buf[n..], fds)?;
                n += self.value.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for axis {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (time, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (axis, m) = <super::axis>::deserialize(&buf[n..], fds)?;
                n += m;
                let (value, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((axis { time, axis, value }, n))
            }
        }
        impl Msg<Obj> for axis {
            const OP: u16 = 4;
        }
        impl Event<Obj> for axis {}
        /// end of a pointer event sequence
        ///
        /// Indicates the end of a set of events that logically belong together.
        /// A client is expected to accumulate the data in all events within the
        /// frame before proceeding.
        ///
        /// All wl_pointer events before a wl_pointer.frame event belong
        /// logically together. For example, in a diagonal scroll motion the
        /// compositor will send an optional wl_pointer.axis_source event, two
        /// wl_pointer.axis events (horizontal and vertical) and finally a
        /// wl_pointer.frame event. The client may use this information to
        /// calculate a diagonal vector for scrolling.
        ///
        /// When multiple wl_pointer.axis events occur within the same frame,
        /// the motion vector is the combined motion of all events.
        /// When a wl_pointer.axis and a wl_pointer.axis_stop event occur within
        /// the same frame, this indicates that axis movement in one axis has
        /// stopped but continues in the other axis.
        /// When multiple wl_pointer.axis_stop events occur within the same
        /// frame, this indicates that these axes stopped in the same instance.
        ///
        /// A wl_pointer.frame event is sent for every logical event group,
        /// even if the group only contains a single wl_pointer event.
        /// Specifically, a client may get a sequence: motion, frame, button,
        /// frame, axis, frame, axis_stop, frame.
        ///
        /// The wl_pointer.enter and wl_pointer.leave events are logical events
        /// generated by the compositor and not the hardware. These events are
        /// also grouped by a wl_pointer.frame. When a pointer moves from one
        /// surface to another, a compositor should group the
        /// wl_pointer.leave event within the same wl_pointer.frame.
        /// However, a client must not rely on wl_pointer.leave and
        /// wl_pointer.enter being in the same wl_pointer.frame.
        /// Compositor-specific policies may require the wl_pointer.leave and
        /// wl_pointer.enter event being split across multiple wl_pointer.frame
        /// groups.
        ///
        #[derive(Debug)]
        pub struct frame {}
        impl Ser for frame {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for frame {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((frame {}, n))
            }
        }
        impl Msg<Obj> for frame {
            const OP: u16 = 5;
        }
        impl Event<Obj> for frame {}
        /// axis source event
        ///
        /// Source information for scroll and other axes.
        ///
        /// This event does not occur on its own. It is sent before a
        /// wl_pointer.frame event and carries the source information for
        /// all events within that frame.
        ///
        /// The source specifies how this event was generated. If the source is
        /// wl_pointer.axis_source.finger, a wl_pointer.axis_stop event will be
        /// sent when the user lifts the finger off the device.
        ///
        /// If the source is wl_pointer.axis_source.wheel,
        /// wl_pointer.axis_source.wheel_tilt or
        /// wl_pointer.axis_source.continuous, a wl_pointer.axis_stop event may
        /// or may not be sent. Whether a compositor sends an axis_stop event
        /// for these sources is hardware-specific and implementation-dependent;
        /// clients must not rely on receiving an axis_stop event for these
        /// scroll sources and should treat scroll sequences from these scroll
        /// sources as unterminated by default.
        ///
        /// This event is optional. If the source is unknown for a particular
        /// axis event sequence, no event is sent.
        /// Only one wl_pointer.axis_source event is permitted per frame.
        ///
        /// The order of wl_pointer.axis_discrete and wl_pointer.axis_source is
        /// not guaranteed.
        ///
        #[derive(Debug)]
        pub struct axis_source {
            // source of the axis event
            pub axis_source: super::axis_source,
        }
        impl Ser for axis_source {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.axis_source.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for axis_source {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (axis_source, m) = <super::axis_source>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((axis_source { axis_source }, n))
            }
        }
        impl Msg<Obj> for axis_source {
            const OP: u16 = 6;
        }
        impl Event<Obj> for axis_source {}
        /// axis stop event
        ///
        /// Stop notification for scroll and other axes.
        ///
        /// For some wl_pointer.axis_source types, a wl_pointer.axis_stop event
        /// is sent to notify a client that the axis sequence has terminated.
        /// This enables the client to implement kinetic scrolling.
        /// See the wl_pointer.axis_source documentation for information on when
        /// this event may be generated.
        ///
        /// Any wl_pointer.axis events with the same axis_source after this
        /// event should be considered as the start of a new axis motion.
        ///
        /// The timestamp is to be interpreted identical to the timestamp in the
        /// wl_pointer.axis event. The timestamp value may be the same as a
        /// preceding wl_pointer.axis event.
        ///
        #[derive(Debug)]
        pub struct axis_stop {
            // timestamp with millisecond granularity
            pub time: u32,
            // the axis stopped with this event
            pub axis: super::axis,
        }
        impl Ser for axis_stop {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.time.serialize(&mut buf[n..], fds)?;
                n += self.axis.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for axis_stop {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (time, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (axis, m) = <super::axis>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((axis_stop { time, axis }, n))
            }
        }
        impl Msg<Obj> for axis_stop {
            const OP: u16 = 7;
        }
        impl Event<Obj> for axis_stop {}
        /// axis click event
        ///
        /// Discrete step information for scroll and other axes.
        ///
        /// This event carries the axis value of the wl_pointer.axis event in
        /// discrete steps (e.g. mouse wheel clicks).
        ///
        /// This event is deprecated with wl_pointer version 8 - this event is not
        /// sent to clients supporting version 8 or later.
        ///
        /// This event does not occur on its own, it is coupled with a
        /// wl_pointer.axis event that represents this axis value on a
        /// continuous scale. The protocol guarantees that each axis_discrete
        /// event is always followed by exactly one axis event with the same
        /// axis number within the same wl_pointer.frame. Note that the protocol
        /// allows for other events to occur between the axis_discrete and
        /// its coupled axis event, including other axis_discrete or axis
        /// events. A wl_pointer.frame must not contain more than one axis_discrete
        /// event per axis type.
        ///
        /// This event is optional; continuous scrolling devices
        /// like two-finger scrolling on touchpads do not have discrete
        /// steps and do not generate this event.
        ///
        /// The discrete value carries the directional information. e.g. a value
        /// of -2 is two steps towards the negative direction of this axis.
        ///
        /// The axis number is identical to the axis number in the associated
        /// axis event.
        ///
        /// The order of wl_pointer.axis_discrete and wl_pointer.axis_source is
        /// not guaranteed.
        ///
        #[derive(Debug)]
        pub struct axis_discrete {
            // axis type
            pub axis: super::axis,
            // number of steps
            pub discrete: i32,
        }
        impl Ser for axis_discrete {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.axis.serialize(&mut buf[n..], fds)?;
                n += self.discrete.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for axis_discrete {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (axis, m) = <super::axis>::deserialize(&buf[n..], fds)?;
                n += m;
                let (discrete, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((axis_discrete { axis, discrete }, n))
            }
        }
        impl Msg<Obj> for axis_discrete {
            const OP: u16 = 8;
        }
        impl Event<Obj> for axis_discrete {}
        /// axis high-resolution scroll event
        ///
        /// Discrete high-resolution scroll information.
        ///
        /// This event carries high-resolution wheel scroll information,
        /// with each multiple of 120 representing one logical scroll step
        /// (a wheel detent). For example, an axis_value120 of 30 is one quarter of
        /// a logical scroll step in the positive direction, a value120 of
        /// -240 are two logical scroll steps in the negative direction within the
        /// same hardware event.
        /// Clients that rely on discrete scrolling should accumulate the
        /// value120 to multiples of 120 before processing the event.
        ///
        /// The value120 must not be zero.
        ///
        /// This event replaces the wl_pointer.axis_discrete event in clients
        /// supporting wl_pointer version 8 or later.
        ///
        /// Where a wl_pointer.axis_source event occurs in the same
        /// wl_pointer.frame, the axis source applies to this event.
        ///
        /// The order of wl_pointer.axis_value120 and wl_pointer.axis_source is
        /// not guaranteed.
        ///
        #[derive(Debug)]
        pub struct axis_value120 {
            // axis type
            pub axis: super::axis,
            // scroll distance as fraction of 120
            pub value120: i32,
        }
        impl Ser for axis_value120 {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.axis.serialize(&mut buf[n..], fds)?;
                n += self.value120.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for axis_value120 {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (axis, m) = <super::axis>::deserialize(&buf[n..], fds)?;
                n += m;
                let (value120, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((axis_value120 { axis, value120 }, n))
            }
        }
        impl Msg<Obj> for axis_value120 {
            const OP: u16 = 9;
        }
        impl Event<Obj> for axis_value120 {}
        /// axis relative physical direction event
        ///
        /// Relative directional information of the entity causing the axis
        /// motion.
        ///
        /// For a wl_pointer.axis event, the wl_pointer.axis_relative_direction
        /// event specifies the movement direction of the entity causing the
        /// wl_pointer.axis event. For example:
        /// - if a user's fingers on a touchpad move down and this
        /// causes a wl_pointer.axis vertical_scroll down event, the physical
        /// direction is 'identical'
        /// - if a user's fingers on a touchpad move down and this causes a
        /// wl_pointer.axis vertical_scroll up scroll up event ('natural
        /// scrolling'), the physical direction is 'inverted'.
        ///
        /// A client may use this information to adjust scroll motion of
        /// components. Specifically, enabling natural scrolling causes the
        /// content to change direction compared to traditional scrolling.
        /// Some widgets like volume control sliders should usually match the
        /// physical direction regardless of whether natural scrolling is
        /// active. This event enables clients to match the scroll direction of
        /// a widget to the physical direction.
        ///
        /// This event does not occur on its own, it is coupled with a
        /// wl_pointer.axis event that represents this axis value.
        /// The protocol guarantees that each axis_relative_direction event is
        /// always followed by exactly one axis event with the same
        /// axis number within the same wl_pointer.frame. Note that the protocol
        /// allows for other events to occur between the axis_relative_direction
        /// and its coupled axis event.
        ///
        /// The axis number is identical to the axis number in the associated
        /// axis event.
        ///
        /// The order of wl_pointer.axis_relative_direction,
        /// wl_pointer.axis_discrete and wl_pointer.axis_source is not
        /// guaranteed.
        ///
        #[derive(Debug)]
        pub struct axis_relative_direction {
            // axis type
            pub axis: super::axis,
            // physical direction relative to axis motion
            pub direction: super::axis_relative_direction,
        }
        impl Ser for axis_relative_direction {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.axis.serialize(&mut buf[n..], fds)?;
                n += self.direction.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for axis_relative_direction {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (axis, m) = <super::axis>::deserialize(&buf[n..], fds)?;
                n += m;
                let (direction, m) = <super::axis_relative_direction>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((axis_relative_direction { axis, direction }, n))
            }
        }
        impl Msg<Obj> for axis_relative_direction {
            const OP: u16 = 10;
        }
        impl Event<Obj> for axis_relative_direction {}
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// given wl_surface has another role
        role = 0,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::role => 0,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::role),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }
    /// physical button state
    ///
    /// Describes the physical state of a button that produced the button
    /// event.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum button_state {
        /// the button is not pressed
        released = 0,
        /// the button is pressed
        pressed = 1,
    }

    impl From<button_state> for u32 {
        fn from(v: button_state) -> u32 {
            match v {
                button_state::released => 0,
                button_state::pressed => 1,
            }
        }
    }

    impl TryFrom<u32> for button_state {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(button_state::released),
                1 => Ok(button_state::pressed),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for button_state {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for button_state {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for button_state")?;
            Ok((v, n))
        }
    }
    /// axis types
    ///
    /// Describes the axis types of scroll events.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum axis {
        /// vertical axis
        vertical_scroll = 0,
        /// horizontal axis
        horizontal_scroll = 1,
    }

    impl From<axis> for u32 {
        fn from(v: axis) -> u32 {
            match v {
                axis::vertical_scroll => 0,
                axis::horizontal_scroll => 1,
            }
        }
    }

    impl TryFrom<u32> for axis {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(axis::vertical_scroll),
                1 => Ok(axis::horizontal_scroll),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for axis {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for axis {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for axis")?;
            Ok((v, n))
        }
    }
    /// axis source types
    ///
    /// Describes the source types for axis events. This indicates to the
    /// client how an axis event was physically generated; a client may
    /// adjust the user interface accordingly. For example, scroll events
    /// from a "finger" source may be in a smooth coordinate space with
    /// kinetic scrolling whereas a "wheel" source may be in discrete steps
    /// of a number of lines.
    ///
    /// The "continuous" axis source is a device generating events in a
    /// continuous coordinate space, but using something other than a
    /// finger. One example for this source is button-based scrolling where
    /// the vertical motion of a device is converted to scroll events while
    /// a button is held down.
    ///
    /// The "wheel tilt" axis source indicates that the actual device is a
    /// wheel but the scroll event is not caused by a rotation but a
    /// (usually sideways) tilt of the wheel.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum axis_source {
        /// a physical wheel rotation
        wheel = 0,
        /// finger on a touch surface
        finger = 1,
        /// continuous coordinate space
        continuous = 2,
        /// a physical wheel tilt
        wheel_tilt = 3,
    }

    impl From<axis_source> for u32 {
        fn from(v: axis_source) -> u32 {
            match v {
                axis_source::wheel => 0,
                axis_source::finger => 1,
                axis_source::continuous => 2,
                axis_source::wheel_tilt => 3,
            }
        }
    }

    impl TryFrom<u32> for axis_source {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(axis_source::wheel),
                1 => Ok(axis_source::finger),
                2 => Ok(axis_source::continuous),
                3 => Ok(axis_source::wheel_tilt),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for axis_source {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for axis_source {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for axis_source")?;
            Ok((v, n))
        }
    }
    /// axis relative direction
    ///
    /// This specifies the direction of the physical motion that caused a
    /// wl_pointer.axis event, relative to the wl_pointer.axis direction.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum axis_relative_direction {
        /// physical motion matches axis direction
        identical = 0,
        /// physical motion is the inverse of the axis direction
        inverted = 1,
    }

    impl From<axis_relative_direction> for u32 {
        fn from(v: axis_relative_direction) -> u32 {
            match v {
                axis_relative_direction::identical => 0,
                axis_relative_direction::inverted => 1,
            }
        }
    }

    impl TryFrom<u32> for axis_relative_direction {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(axis_relative_direction::identical),
                1 => Ok(axis_relative_direction::inverted),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for axis_relative_direction {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for axis_relative_direction {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for axis_relative_direction")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_pointer;
    pub type Obj = wl_pointer;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_pointer;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(1);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// keyboard input device
///
/// The wl_keyboard interface represents one or more keyboards
/// associated with a seat.
///
pub mod wl_keyboard {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_keyboard";
    pub const VERSION: u32 = 9;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// release the keyboard object
        #[derive(Debug)]
        pub struct release {}
        impl Ser for release {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for release {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((release {}, n))
            }
        }
        impl Msg<Obj> for release {
            const OP: u16 = 0;
        }
        impl Request<Obj> for release {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// keyboard mapping
        ///
        /// This event provides a file descriptor to the client which can be
        /// memory-mapped in read-only mode to provide a keyboard mapping
        /// description.
        ///
        /// From version 7 onwards, the fd must be mapped with MAP_PRIVATE by
        /// the recipient, as MAP_SHARED may fail.
        ///
        #[derive(Debug)]
        pub struct keymap {
            // keymap format
            pub format: super::keymap_format,
            // keymap file descriptor
            pub fd: OwnedFd,
            // keymap size, in bytes
            pub size: u32,
        }
        impl Ser for keymap {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.format.serialize(&mut buf[n..], fds)?;
                n += self.fd.serialize(&mut buf[n..], fds)?;
                n += self.size.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for keymap {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (format, m) = <super::keymap_format>::deserialize(&buf[n..], fds)?;
                n += m;
                let (fd, m) = <OwnedFd>::deserialize(&buf[n..], fds)?;
                n += m;
                let (size, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((keymap { format, fd, size }, n))
            }
        }
        impl Msg<Obj> for keymap {
            const OP: u16 = 0;
        }
        impl Event<Obj> for keymap {}
        /// enter event
        ///
        /// Notification that this seat's keyboard focus is on a certain
        /// surface.
        ///
        /// The compositor must send the wl_keyboard.modifiers event after this
        /// event.
        ///
        #[derive(Debug)]
        pub struct enter {
            // serial number of the enter event
            pub serial: u32,
            // surface gaining keyboard focus
            pub surface: object<wl_surface::Obj>,
            // the currently pressed keys
            pub keys: Vec<u8>,
        }
        impl Ser for enter {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.surface.serialize(&mut buf[n..], fds)?;
                n += self.keys.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for enter {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (keys, m) = <Vec<u8>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((enter { serial, surface, keys }, n))
            }
        }
        impl Msg<Obj> for enter {
            const OP: u16 = 1;
        }
        impl Event<Obj> for enter {}
        /// leave event
        ///
        /// Notification that this seat's keyboard focus is no longer on
        /// a certain surface.
        ///
        /// The leave notification is sent before the enter notification
        /// for the new focus.
        ///
        /// After this event client must assume that all keys, including modifiers,
        /// are lifted and also it must stop key repeating if there's some going on.
        ///
        #[derive(Debug)]
        pub struct leave {
            // serial number of the leave event
            pub serial: u32,
            // surface that lost keyboard focus
            pub surface: object<wl_surface::Obj>,
        }
        impl Ser for leave {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.surface.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for leave {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((leave { serial, surface }, n))
            }
        }
        impl Msg<Obj> for leave {
            const OP: u16 = 2;
        }
        impl Event<Obj> for leave {}
        /// key event
        ///
        /// A key was pressed or released.
        /// The time argument is a timestamp with millisecond
        /// granularity, with an undefined base.
        ///
        /// The key is a platform-specific key code that can be interpreted
        /// by feeding it to the keyboard mapping (see the keymap event).
        ///
        /// If this event produces a change in modifiers, then the resulting
        /// wl_keyboard.modifiers event must be sent after this event.
        ///
        #[derive(Debug)]
        pub struct key {
            // serial number of the key event
            pub serial: u32,
            // timestamp with millisecond granularity
            pub time: u32,
            // key that produced the event
            pub key: u32,
            // physical state of the key
            pub state: super::key_state,
        }
        impl Ser for key {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.time.serialize(&mut buf[n..], fds)?;
                n += self.key.serialize(&mut buf[n..], fds)?;
                n += self.state.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for key {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (time, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (key, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (state, m) = <super::key_state>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((key { serial, time, key, state }, n))
            }
        }
        impl Msg<Obj> for key {
            const OP: u16 = 3;
        }
        impl Event<Obj> for key {}
        /// modifier and group state
        ///
        /// Notifies clients that the modifier and/or group state has
        /// changed, and it should update its local state.
        ///
        #[derive(Debug)]
        pub struct modifiers {
            // serial number of the modifiers event
            pub serial: u32,
            // depressed modifiers
            pub mods_depressed: u32,
            // latched modifiers
            pub mods_latched: u32,
            // locked modifiers
            pub mods_locked: u32,
            // keyboard layout
            pub group: u32,
        }
        impl Ser for modifiers {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.mods_depressed.serialize(&mut buf[n..], fds)?;
                n += self.mods_latched.serialize(&mut buf[n..], fds)?;
                n += self.mods_locked.serialize(&mut buf[n..], fds)?;
                n += self.group.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for modifiers {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (mods_depressed, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (mods_latched, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (mods_locked, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (group, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((modifiers { serial, mods_depressed, mods_latched, mods_locked, group }, n))
            }
        }
        impl Msg<Obj> for modifiers {
            const OP: u16 = 4;
        }
        impl Event<Obj> for modifiers {}
        /// repeat rate and delay
        ///
        /// Informs the client about the keyboard's repeat rate and delay.
        ///
        /// This event is sent as soon as the wl_keyboard object has been created,
        /// and is guaranteed to be received by the client before any key press
        /// event.
        ///
        /// Negative values for either rate or delay are illegal. A rate of zero
        /// will disable any repeating (regardless of the value of delay).
        ///
        /// This event can be sent later on as well with a new value if necessary,
        /// so clients should continue listening for the event past the creation
        /// of wl_keyboard.
        ///
        #[derive(Debug)]
        pub struct repeat_info {
            // the rate of repeating keys in characters per second
            pub rate: i32,
            // delay in milliseconds since key down until repeating starts
            pub delay: i32,
        }
        impl Ser for repeat_info {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.rate.serialize(&mut buf[n..], fds)?;
                n += self.delay.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for repeat_info {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (rate, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (delay, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((repeat_info { rate, delay }, n))
            }
        }
        impl Msg<Obj> for repeat_info {
            const OP: u16 = 5;
        }
        impl Event<Obj> for repeat_info {}
    }
    /// keyboard mapping format
    ///
    /// This specifies the format of the keymap provided to the
    /// client with the wl_keyboard.keymap event.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum keymap_format {
        /// no keymap; client must understand how to interpret the raw keycode
        no_keymap = 0,
        /// libxkbcommon compatible, null-terminated string; to determine the xkb keycode, clients must add 8 to the key event keycode
        xkb_v1 = 1,
    }

    impl From<keymap_format> for u32 {
        fn from(v: keymap_format) -> u32 {
            match v {
                keymap_format::no_keymap => 0,
                keymap_format::xkb_v1 => 1,
            }
        }
    }

    impl TryFrom<u32> for keymap_format {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(keymap_format::no_keymap),
                1 => Ok(keymap_format::xkb_v1),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for keymap_format {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for keymap_format {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for keymap_format")?;
            Ok((v, n))
        }
    }
    /// physical key state
    ///
    /// Describes the physical state of a key that produced the key event.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum key_state {
        /// key is not pressed
        released = 0,
        /// key is pressed
        pressed = 1,
    }

    impl From<key_state> for u32 {
        fn from(v: key_state) -> u32 {
            match v {
                key_state::released => 0,
                key_state::pressed => 1,
            }
        }
    }

    impl TryFrom<u32> for key_state {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(key_state::released),
                1 => Ok(key_state::pressed),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for key_state {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for key_state {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for key_state")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_keyboard;
    pub type Obj = wl_keyboard;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_keyboard;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(0);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// touchscreen input device
///
/// The wl_touch interface represents a touchscreen
/// associated with a seat.
///
/// Touch interactions can consist of one or more contacts.
/// For each contact, a series of events is generated, starting
/// with a down event, followed by zero or more motion events,
/// and ending with an up event. Events relating to the same
/// contact point can be identified by the ID of the sequence.
///
pub mod wl_touch {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_touch";
    pub const VERSION: u32 = 9;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// release the touch object
        #[derive(Debug)]
        pub struct release {}
        impl Ser for release {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for release {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((release {}, n))
            }
        }
        impl Msg<Obj> for release {
            const OP: u16 = 0;
        }
        impl Request<Obj> for release {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// touch down event and beginning of a touch sequence
        ///
        /// A new touch point has appeared on the surface. This touch point is
        /// assigned a unique ID. Future events from this touch point reference
        /// this ID. The ID ceases to be valid after a touch up event and may be
        /// reused in the future.
        ///
        #[derive(Debug)]
        pub struct down {
            // serial number of the touch down event
            pub serial: u32,
            // timestamp with millisecond granularity
            pub time: u32,
            // surface touched
            pub surface: object<wl_surface::Obj>,
            // the unique ID of this touch point
            pub id: i32,
            // surface-local x coordinate
            pub x: fixed,
            // surface-local y coordinate
            pub y: fixed,
        }
        impl Ser for down {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.time.serialize(&mut buf[n..], fds)?;
                n += self.surface.serialize(&mut buf[n..], fds)?;
                n += self.id.serialize(&mut buf[n..], fds)?;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for down {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (time, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (id, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (x, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((down { serial, time, surface, id, x, y }, n))
            }
        }
        impl Msg<Obj> for down {
            const OP: u16 = 0;
        }
        impl Event<Obj> for down {}
        /// end of a touch event sequence
        ///
        /// The touch point has disappeared. No further events will be sent for
        /// this touch point and the touch point's ID is released and may be
        /// reused in a future touch down event.
        ///
        #[derive(Debug)]
        pub struct up {
            // serial number of the touch up event
            pub serial: u32,
            // timestamp with millisecond granularity
            pub time: u32,
            // the unique ID of this touch point
            pub id: i32,
        }
        impl Ser for up {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.time.serialize(&mut buf[n..], fds)?;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for up {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (time, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (id, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((up { serial, time, id }, n))
            }
        }
        impl Msg<Obj> for up {
            const OP: u16 = 1;
        }
        impl Event<Obj> for up {}
        /// update of touch point coordinates
        ///
        /// A touch point has changed coordinates.
        ///
        #[derive(Debug)]
        pub struct motion {
            // timestamp with millisecond granularity
            pub time: u32,
            // the unique ID of this touch point
            pub id: i32,
            // surface-local x coordinate
            pub x: fixed,
            // surface-local y coordinate
            pub y: fixed,
        }
        impl Ser for motion {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.time.serialize(&mut buf[n..], fds)?;
                n += self.id.serialize(&mut buf[n..], fds)?;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for motion {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (time, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (id, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (x, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((motion { time, id, x, y }, n))
            }
        }
        impl Msg<Obj> for motion {
            const OP: u16 = 2;
        }
        impl Event<Obj> for motion {}
        /// end of touch frame event
        ///
        /// Indicates the end of a set of events that logically belong together.
        /// A client is expected to accumulate the data in all events within the
        /// frame before proceeding.
        ///
        /// A wl_touch.frame terminates at least one event but otherwise no
        /// guarantee is provided about the set of events within a frame. A client
        /// must assume that any state not updated in a frame is unchanged from the
        /// previously known state.
        ///
        #[derive(Debug)]
        pub struct frame {}
        impl Ser for frame {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for frame {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((frame {}, n))
            }
        }
        impl Msg<Obj> for frame {
            const OP: u16 = 3;
        }
        impl Event<Obj> for frame {}
        /// touch session cancelled
        ///
        /// Sent if the compositor decides the touch stream is a global
        /// gesture. No further events are sent to the clients from that
        /// particular gesture. Touch cancellation applies to all touch points
        /// currently active on this client's surface. The client is
        /// responsible for finalizing the touch points, future touch points on
        /// this surface may reuse the touch point ID.
        ///
        #[derive(Debug)]
        pub struct cancel {}
        impl Ser for cancel {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for cancel {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((cancel {}, n))
            }
        }
        impl Msg<Obj> for cancel {
            const OP: u16 = 4;
        }
        impl Event<Obj> for cancel {}
        /// update shape of touch point
        ///
        /// Sent when a touchpoint has changed its shape.
        ///
        /// This event does not occur on its own. It is sent before a
        /// wl_touch.frame event and carries the new shape information for
        /// any previously reported, or new touch points of that frame.
        ///
        /// Other events describing the touch point such as wl_touch.down,
        /// wl_touch.motion or wl_touch.orientation may be sent within the
        /// same wl_touch.frame. A client should treat these events as a single
        /// logical touch point update. The order of wl_touch.shape,
        /// wl_touch.orientation and wl_touch.motion is not guaranteed.
        /// A wl_touch.down event is guaranteed to occur before the first
        /// wl_touch.shape event for this touch ID but both events may occur within
        /// the same wl_touch.frame.
        ///
        /// A touchpoint shape is approximated by an ellipse through the major and
        /// minor axis length. The major axis length describes the longer diameter
        /// of the ellipse, while the minor axis length describes the shorter
        /// diameter. Major and minor are orthogonal and both are specified in
        /// surface-local coordinates. The center of the ellipse is always at the
        /// touchpoint location as reported by wl_touch.down or wl_touch.move.
        ///
        /// This event is only sent by the compositor if the touch device supports
        /// shape reports. The client has to make reasonable assumptions about the
        /// shape if it did not receive this event.
        ///
        #[derive(Debug)]
        pub struct shape {
            // the unique ID of this touch point
            pub id: i32,
            // length of the major axis in surface-local coordinates
            pub major: fixed,
            // length of the minor axis in surface-local coordinates
            pub minor: fixed,
        }
        impl Ser for shape {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                n += self.major.serialize(&mut buf[n..], fds)?;
                n += self.minor.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for shape {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (major, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                let (minor, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((shape { id, major, minor }, n))
            }
        }
        impl Msg<Obj> for shape {
            const OP: u16 = 5;
        }
        impl Event<Obj> for shape {}
        /// update orientation of touch point
        ///
        /// Sent when a touchpoint has changed its orientation.
        ///
        /// This event does not occur on its own. It is sent before a
        /// wl_touch.frame event and carries the new shape information for
        /// any previously reported, or new touch points of that frame.
        ///
        /// Other events describing the touch point such as wl_touch.down,
        /// wl_touch.motion or wl_touch.shape may be sent within the
        /// same wl_touch.frame. A client should treat these events as a single
        /// logical touch point update. The order of wl_touch.shape,
        /// wl_touch.orientation and wl_touch.motion is not guaranteed.
        /// A wl_touch.down event is guaranteed to occur before the first
        /// wl_touch.orientation event for this touch ID but both events may occur
        /// within the same wl_touch.frame.
        ///
        /// The orientation describes the clockwise angle of a touchpoint's major
        /// axis to the positive surface y-axis and is normalized to the -180 to
        /// +180 degree range. The granularity of orientation depends on the touch
        /// device, some devices only support binary rotation values between 0 and
        /// 90 degrees.
        ///
        /// This event is only sent by the compositor if the touch device supports
        /// orientation reports.
        ///
        #[derive(Debug)]
        pub struct orientation {
            // the unique ID of this touch point
            pub id: i32,
            // angle between major axis and positive surface y-axis in degrees
            pub orientation: fixed,
        }
        impl Ser for orientation {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                n += self.orientation.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for orientation {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (orientation, m) = <fixed>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((orientation { id, orientation }, n))
            }
        }
        impl Msg<Obj> for orientation {
            const OP: u16 = 6;
        }
        impl Event<Obj> for orientation {}
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_touch;
    pub type Obj = wl_touch;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_touch;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(0);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// compositor output region
///
/// An output describes part of the compositor geometry.  The
/// compositor works in the 'compositor coordinate system' and an
/// output corresponds to a rectangular area in that space that is
/// actually visible.  This typically corresponds to a monitor that
/// displays part of the compositor space.  This object is published
/// as global during start up, or when a monitor is hotplugged.
///
pub mod wl_output {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_output";
    pub const VERSION: u32 = 4;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// release the output object
        ///
        /// Using this request a client can tell the server that it is not going to
        /// use the output object anymore.
        ///
        #[derive(Debug)]
        pub struct release {}
        impl Ser for release {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for release {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((release {}, n))
            }
        }
        impl Msg<Obj> for release {
            const OP: u16 = 0;
        }
        impl Request<Obj> for release {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// properties of the output
        ///
        /// The geometry event describes geometric properties of the output.
        /// The event is sent when binding to the output object and whenever
        /// any of the properties change.
        ///
        /// The physical size can be set to zero if it doesn't make sense for this
        /// output (e.g. for projectors or virtual outputs).
        ///
        /// The geometry event will be followed by a done event (starting from
        /// version 2).
        ///
        /// Note: wl_output only advertises partial information about the output
        /// position and identification. Some compositors, for instance those not
        /// implementing a desktop-style output layout or those exposing virtual
        /// outputs, might fake this information. Instead of using x and y, clients
        /// should use xdg_output.logical_position. Instead of using make and model,
        /// clients should use name and description.
        ///
        #[derive(Debug)]
        pub struct geometry {
            // x position within the global compositor space
            pub x: i32,
            // y position within the global compositor space
            pub y: i32,
            // width in millimeters of the output
            pub physical_width: i32,
            // height in millimeters of the output
            pub physical_height: i32,
            // subpixel orientation of the output
            pub subpixel: super::subpixel,
            // textual description of the manufacturer
            pub make: String,
            // textual description of the model
            pub model: String,
            // transform that maps framebuffer to output
            pub transform: super::transform,
        }
        impl Ser for geometry {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                n += self.physical_width.serialize(&mut buf[n..], fds)?;
                n += self.physical_height.serialize(&mut buf[n..], fds)?;
                n += self.subpixel.serialize(&mut buf[n..], fds)?;
                n += self.make.serialize(&mut buf[n..], fds)?;
                n += self.model.serialize(&mut buf[n..], fds)?;
                n += self.transform.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for geometry {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (physical_width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (physical_height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (subpixel, m) = <super::subpixel>::deserialize(&buf[n..], fds)?;
                n += m;
                let (make, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                let (model, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                let (transform, m) = <super::transform>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((
                    geometry {
                        x,
                        y,
                        physical_width,
                        physical_height,
                        subpixel,
                        make,
                        model,
                        transform,
                    },
                    n,
                ))
            }
        }
        impl Msg<Obj> for geometry {
            const OP: u16 = 0;
        }
        impl Event<Obj> for geometry {}
        /// advertise available modes for the output
        ///
        /// The mode event describes an available mode for the output.
        ///
        /// The event is sent when binding to the output object and there
        /// will always be one mode, the current mode.  The event is sent
        /// again if an output changes mode, for the mode that is now
        /// current.  In other words, the current mode is always the last
        /// mode that was received with the current flag set.
        ///
        /// Non-current modes are deprecated. A compositor can decide to only
        /// advertise the current mode and never send other modes. Clients
        /// should not rely on non-current modes.
        ///
        /// The size of a mode is given in physical hardware units of
        /// the output device. This is not necessarily the same as
        /// the output size in the global compositor space. For instance,
        /// the output may be scaled, as described in wl_output.scale,
        /// or transformed, as described in wl_output.transform. Clients
        /// willing to retrieve the output size in the global compositor
        /// space should use xdg_output.logical_size instead.
        ///
        /// The vertical refresh rate can be set to zero if it doesn't make
        /// sense for this output (e.g. for virtual outputs).
        ///
        /// The mode event will be followed by a done event (starting from
        /// version 2).
        ///
        /// Clients should not use the refresh rate to schedule frames. Instead,
        /// they should use the wl_surface.frame event or the presentation-time
        /// protocol.
        ///
        /// Note: this information is not always meaningful for all outputs. Some
        /// compositors, such as those exposing virtual outputs, might fake the
        /// refresh rate or the size.
        ///
        #[derive(Debug)]
        pub struct mode {
            // bitfield of mode flags
            pub flags: super::mode,
            // width of the mode in hardware units
            pub width: i32,
            // height of the mode in hardware units
            pub height: i32,
            // vertical refresh rate in mHz
            pub refresh: i32,
        }
        impl Ser for mode {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.flags.serialize(&mut buf[n..], fds)?;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                n += self.refresh.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for mode {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (flags, m) = <super::mode>::deserialize(&buf[n..], fds)?;
                n += m;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (refresh, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((mode { flags, width, height, refresh }, n))
            }
        }
        impl Msg<Obj> for mode {
            const OP: u16 = 1;
        }
        impl Event<Obj> for mode {}
        /// sent all information about output
        ///
        /// This event is sent after all other properties have been
        /// sent after binding to the output object and after any
        /// other property changes done after that. This allows
        /// changes to the output properties to be seen as
        /// atomic, even if they happen via multiple events.
        ///
        #[derive(Debug)]
        pub struct done {}
        impl Ser for done {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for done {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((done {}, n))
            }
        }
        impl Msg<Obj> for done {
            const OP: u16 = 2;
        }
        impl Event<Obj> for done {}
        /// output scaling properties
        ///
        /// This event contains scaling geometry information
        /// that is not in the geometry event. It may be sent after
        /// binding the output object or if the output scale changes
        /// later. If it is not sent, the client should assume a
        /// scale of 1.
        ///
        /// A scale larger than 1 means that the compositor will
        /// automatically scale surface buffers by this amount
        /// when rendering. This is used for very high resolution
        /// displays where applications rendering at the native
        /// resolution would be too small to be legible.
        ///
        /// It is intended that scaling aware clients track the
        /// current output of a surface, and if it is on a scaled
        /// output it should use wl_surface.set_buffer_scale with
        /// the scale of the output. That way the compositor can
        /// avoid scaling the surface, and the client can supply
        /// a higher detail image.
        ///
        /// The scale event will be followed by a done event.
        ///
        #[derive(Debug)]
        pub struct scale {
            // scaling factor of output
            pub factor: i32,
        }
        impl Ser for scale {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.factor.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for scale {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (factor, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((scale { factor }, n))
            }
        }
        impl Msg<Obj> for scale {
            const OP: u16 = 3;
        }
        impl Event<Obj> for scale {}
        /// name of this output
        ///
        /// Many compositors will assign user-friendly names to their outputs, show
        /// them to the user, allow the user to refer to an output, etc. The client
        /// may wish to know this name as well to offer the user similar behaviors.
        ///
        /// The name is a UTF-8 string with no convention defined for its contents.
        /// Each name is unique among all wl_output globals. The name is only
        /// guaranteed to be unique for the compositor instance.
        ///
        /// The same output name is used for all clients for a given wl_output
        /// global. Thus, the name can be shared across processes to refer to a
        /// specific wl_output global.
        ///
        /// The name is not guaranteed to be persistent across sessions, thus cannot
        /// be used to reliably identify an output in e.g. configuration files.
        ///
        /// Examples of names include 'HDMI-A-1', 'WL-1', 'X11-1', etc. However, do
        /// not assume that the name is a reflection of an underlying DRM connector,
        /// X11 connection, etc.
        ///
        /// The name event is sent after binding the output object. This event is
        /// only sent once per output object, and the name does not change over the
        /// lifetime of the wl_output global.
        ///
        /// Compositors may re-use the same output name if the wl_output global is
        /// destroyed and re-created later. Compositors should avoid re-using the
        /// same name if possible.
        ///
        /// The name event will be followed by a done event.
        ///
        #[derive(Debug)]
        pub struct name {
            // output name
            pub name: String,
        }
        impl Ser for name {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.name.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for name {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (name, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((name { name }, n))
            }
        }
        impl Msg<Obj> for name {
            const OP: u16 = 4;
        }
        impl Event<Obj> for name {}
        /// human-readable description of this output
        ///
        /// Many compositors can produce human-readable descriptions of their
        /// outputs. The client may wish to know this description as well, e.g. for
        /// output selection purposes.
        ///
        /// The description is a UTF-8 string with no convention defined for its
        /// contents. The description is not guaranteed to be unique among all
        /// wl_output globals. Examples might include 'Foocorp 11" Display' or
        /// 'Virtual X11 output via :1'.
        ///
        /// The description event is sent after binding the output object and
        /// whenever the description changes. The description is optional, and may
        /// not be sent at all.
        ///
        /// The description event will be followed by a done event.
        ///
        #[derive(Debug)]
        pub struct description {
            // output description
            pub description: String,
        }
        impl Ser for description {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.description.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for description {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (description, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((description { description }, n))
            }
        }
        impl Msg<Obj> for description {
            const OP: u16 = 5;
        }
        impl Event<Obj> for description {}
    }
    /// subpixel geometry information
    ///
    /// This enumeration describes how the physical
    /// pixels on an output are laid out.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum subpixel {
        /// unknown geometry
        unknown = 0,
        /// no geometry
        none = 1,
        /// horizontal RGB
        horizontal_rgb = 2,
        /// horizontal BGR
        horizontal_bgr = 3,
        /// vertical RGB
        vertical_rgb = 4,
        /// vertical BGR
        vertical_bgr = 5,
    }

    impl From<subpixel> for u32 {
        fn from(v: subpixel) -> u32 {
            match v {
                subpixel::unknown => 0,
                subpixel::none => 1,
                subpixel::horizontal_rgb => 2,
                subpixel::horizontal_bgr => 3,
                subpixel::vertical_rgb => 4,
                subpixel::vertical_bgr => 5,
            }
        }
    }

    impl TryFrom<u32> for subpixel {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(subpixel::unknown),
                1 => Ok(subpixel::none),
                2 => Ok(subpixel::horizontal_rgb),
                3 => Ok(subpixel::horizontal_bgr),
                4 => Ok(subpixel::vertical_rgb),
                5 => Ok(subpixel::vertical_bgr),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for subpixel {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for subpixel {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for subpixel")?;
            Ok((v, n))
        }
    }
    /// transform from framebuffer to output
    ///
    /// This describes the transform that a compositor will apply to a
    /// surface to compensate for the rotation or mirroring of an
    /// output device.
    ///
    /// The flipped values correspond to an initial flip around a
    /// vertical axis followed by rotation.
    ///
    /// The purpose is mainly to allow clients to render accordingly and
    /// tell the compositor, so that for fullscreen surfaces, the
    /// compositor will still be able to scan out directly from client
    /// surfaces.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum transform {
        /// no transform
        normal = 0,
        /// 90 degrees counter-clockwise
        N90 = 1,
        /// 180 degrees counter-clockwise
        N180 = 2,
        /// 270 degrees counter-clockwise
        N270 = 3,
        /// 180 degree flip around a vertical axis
        flipped = 4,
        /// flip and rotate 90 degrees counter-clockwise
        flipped_90 = 5,
        /// flip and rotate 180 degrees counter-clockwise
        flipped_180 = 6,
        /// flip and rotate 270 degrees counter-clockwise
        flipped_270 = 7,
    }

    impl From<transform> for u32 {
        fn from(v: transform) -> u32 {
            match v {
                transform::normal => 0,
                transform::N90 => 1,
                transform::N180 => 2,
                transform::N270 => 3,
                transform::flipped => 4,
                transform::flipped_90 => 5,
                transform::flipped_180 => 6,
                transform::flipped_270 => 7,
            }
        }
    }

    impl TryFrom<u32> for transform {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(transform::normal),
                1 => Ok(transform::N90),
                2 => Ok(transform::N180),
                3 => Ok(transform::N270),
                4 => Ok(transform::flipped),
                5 => Ok(transform::flipped_90),
                6 => Ok(transform::flipped_180),
                7 => Ok(transform::flipped_270),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for transform {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for transform {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for transform")?;
            Ok((v, n))
        }
    }
    /// mode information
    ///
    /// These flags describe properties of an output mode.
    /// They are used in the flags bitfield of the mode event.
    ///

    #[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct mode {
        flags: u32,
    }
    impl Debug for mode {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            let mut first = true;
            for (name, val) in Self::EACH {
                if val & *self == val {
                    if !first {
                        f.write_str("|")?;
                    }
                    first = false;
                    f.write_str(name)?;
                }
            }
            if first {
                f.write_str("EMPTY")?;
            }
            Ok(())
        }
    }
    impl std::fmt::LowerHex for mode {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            <u32 as std::fmt::LowerHex>::fmt(&self.flags, f)
        }
    }
    impl Ser for mode {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            self.flags.serialize(buf, fds)
        }
    }
    impl Dsr for mode {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (flags, n) = u32::deserialize(buf, fds)?;
            Ok((mode { flags }, n))
        }
    }
    impl std::ops::BitOr for mode {
        type Output = Self;
        fn bitor(self, rhs: Self) -> Self {
            mode { flags: self.flags | rhs.flags }
        }
    }
    impl std::ops::BitAnd for mode {
        type Output = Self;
        fn bitand(self, rhs: Self) -> Self {
            mode { flags: self.flags & rhs.flags }
        }
    }
    impl mode {
        #![allow(non_upper_case_globals)]

        fn contains(&self, rhs: mode) -> bool {
            (self.flags & rhs.flags) == rhs.flags
        }

        /// indicates this is the current mode
        pub const current: mode = mode { flags: 1 };
        /// indicates this is the preferred mode
        pub const preferred: mode = mode { flags: 2 };
        pub const EMPTY: mode = mode { flags: 0 };
        pub const ALL: mode =
            mode { flags: Self::EMPTY.flags | Self::current.flags | Self::preferred.flags };
        pub const EACH: [(&'static str, mode); 2] =
            [("current", Self::current), ("preferred", Self::preferred)];
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_output;
    pub type Obj = wl_output;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_output;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(0);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// region interface
///
/// A region object describes an area.
///
/// Region objects are used to describe the opaque and input
/// regions of a surface.
///
pub mod wl_region {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_region";
    pub const VERSION: u32 = 1;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// destroy region
        ///
        /// Destroy the region.  This will invalidate the object ID.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 0;
        }
        impl Request<Obj> for destroy {}
        /// add rectangle to region
        ///
        /// Add the specified rectangle to the region.
        ///
        #[derive(Debug)]
        pub struct add {
            // region-local x coordinate
            pub x: i32,
            // region-local y coordinate
            pub y: i32,
            // rectangle width
            pub width: i32,
            // rectangle height
            pub height: i32,
        }
        impl Ser for add {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for add {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((add { x, y, width, height }, n))
            }
        }
        impl Msg<Obj> for add {
            const OP: u16 = 1;
        }
        impl Request<Obj> for add {}
        /// subtract rectangle from region
        ///
        /// Subtract the specified rectangle from the region.
        ///
        #[derive(Debug)]
        pub struct subtract {
            // region-local x coordinate
            pub x: i32,
            // region-local y coordinate
            pub y: i32,
            // rectangle width
            pub width: i32,
            // rectangle height
            pub height: i32,
        }
        impl Ser for subtract {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for subtract {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((subtract { x, y, width, height }, n))
            }
        }
        impl Msg<Obj> for subtract {
            const OP: u16 = 2;
        }
        impl Request<Obj> for subtract {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_region;
    pub type Obj = wl_region;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_region;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(0);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// sub-surface compositing
///
/// The global interface exposing sub-surface compositing capabilities.
/// A wl_surface, that has sub-surfaces associated, is called the
/// parent surface. Sub-surfaces can be arbitrarily nested and create
/// a tree of sub-surfaces.
///
/// The root surface in a tree of sub-surfaces is the main
/// surface. The main surface cannot be a sub-surface, because
/// sub-surfaces must always have a parent.
///
/// A main surface with its sub-surfaces forms a (compound) window.
/// For window management purposes, this set of wl_surface objects is
/// to be considered as a single window, and it should also behave as
/// such.
///
/// The aim of sub-surfaces is to offload some of the compositing work
/// within a window from clients to the compositor. A prime example is
/// a video player with decorations and video in separate wl_surface
/// objects. This should allow the compositor to pass YUV video buffer
/// processing to dedicated overlay hardware when possible.
///
pub mod wl_subcompositor {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_subcompositor";
    pub const VERSION: u32 = 1;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// unbind from the subcompositor interface
        ///
        /// Informs the server that the client will not be using this
        /// protocol object anymore. This does not affect any other
        /// objects, wl_subsurface objects included.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 0;
        }
        impl Request<Obj> for destroy {}
        /// give a surface the role sub-surface
        ///
        /// Create a sub-surface interface for the given surface, and
        /// associate it with the given parent surface. This turns a
        /// plain wl_surface into a sub-surface.
        ///
        /// The to-be sub-surface must not already have another role, and it
        /// must not have an existing wl_subsurface object. Otherwise the
        /// bad_surface protocol error is raised.
        ///
        /// Adding sub-surfaces to a parent is a double-buffered operation on the
        /// parent (see wl_surface.commit). The effect of adding a sub-surface
        /// becomes visible on the next time the state of the parent surface is
        /// applied.
        ///
        /// The parent surface must not be one of the child surface's descendants,
        /// and the parent must be different from the child surface, otherwise the
        /// bad_parent protocol error is raised.
        ///
        /// This request modifies the behaviour of wl_surface.commit request on
        /// the sub-surface, see the documentation on wl_subsurface interface.
        ///
        #[derive(Debug)]
        pub struct get_subsurface {
            // the new sub-surface object ID
            pub id: new_id<wl_subsurface::Obj>,
            // the surface to be turned into a sub-surface
            pub surface: object<wl_surface::Obj>,
            // the parent surface
            pub parent: object<wl_surface::Obj>,
        }
        impl Ser for get_subsurface {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                n += self.surface.serialize(&mut buf[n..], fds)?;
                n += self.parent.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for get_subsurface {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<wl_subsurface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (parent, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((get_subsurface { id, surface, parent }, n))
            }
        }
        impl Msg<Obj> for get_subsurface {
            const OP: u16 = 1;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: wl_subsurface::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<wl_subsurface::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for get_subsurface {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// the to-be sub-surface is invalid
        bad_surface = 0,
        /// the to-be sub-surface parent is invalid
        bad_parent = 1,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::bad_surface => 0,
                error::bad_parent => 1,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::bad_surface),
                1 => Ok(error::bad_parent),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_subcompositor;
    pub type Obj = wl_subcompositor;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_subcompositor;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(0);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// sub-surface interface to a wl_surface
///
/// An additional interface to a wl_surface object, which has been
/// made a sub-surface. A sub-surface has one parent surface. A
/// sub-surface's size and position are not limited to that of the parent.
/// Particularly, a sub-surface is not automatically clipped to its
/// parent's area.
///
/// A sub-surface becomes mapped, when a non-NULL wl_buffer is applied
/// and the parent surface is mapped. The order of which one happens
/// first is irrelevant. A sub-surface is hidden if the parent becomes
/// hidden, or if a NULL wl_buffer is applied. These rules apply
/// recursively through the tree of surfaces.
///
/// The behaviour of a wl_surface.commit request on a sub-surface
/// depends on the sub-surface's mode. The possible modes are
/// synchronized and desynchronized, see methods
/// wl_subsurface.set_sync and wl_subsurface.set_desync. Synchronized
/// mode caches the wl_surface state to be applied when the parent's
/// state gets applied, and desynchronized mode applies the pending
/// wl_surface state directly. A sub-surface is initially in the
/// synchronized mode.
///
/// Sub-surfaces also have another kind of state, which is managed by
/// wl_subsurface requests, as opposed to wl_surface requests. This
/// state includes the sub-surface position relative to the parent
/// surface (wl_subsurface.set_position), and the stacking order of
/// the parent and its sub-surfaces (wl_subsurface.place_above and
/// .place_below). This state is applied when the parent surface's
/// wl_surface state is applied, regardless of the sub-surface's mode.
/// As the exception, set_sync and set_desync are effective immediately.
///
/// The main surface can be thought to be always in desynchronized mode,
/// since it does not have a parent in the sub-surfaces sense.
///
/// Even if a sub-surface is in desynchronized mode, it will behave as
/// in synchronized mode, if its parent surface behaves as in
/// synchronized mode. This rule is applied recursively throughout the
/// tree of surfaces. This means, that one can set a sub-surface into
/// synchronized mode, and then assume that all its child and grand-child
/// sub-surfaces are synchronized, too, without explicitly setting them.
///
/// Destroying a sub-surface takes effect immediately. If you need to
/// synchronize the removal of a sub-surface to the parent surface update,
/// unmap the sub-surface first by attaching a NULL wl_buffer, update parent,
/// and then destroy the sub-surface.
///
/// If the parent wl_surface object is destroyed, the sub-surface is
/// unmapped.
///
pub mod wl_subsurface {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "wl_subsurface";
    pub const VERSION: u32 = 1;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// remove sub-surface interface
        ///
        /// The sub-surface interface is removed from the wl_surface object
        /// that was turned into a sub-surface with a
        /// wl_subcompositor.get_subsurface request. The wl_surface's association
        /// to the parent is deleted. The wl_surface is unmapped immediately.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 0;
        }
        impl Request<Obj> for destroy {}
        /// reposition the sub-surface
        ///
        /// This schedules a sub-surface position change.
        /// The sub-surface will be moved so that its origin (top left
        /// corner pixel) will be at the location x, y of the parent surface
        /// coordinate system. The coordinates are not restricted to the parent
        /// surface area. Negative values are allowed.
        ///
        /// The scheduled coordinates will take effect whenever the state of the
        /// parent surface is applied. When this happens depends on whether the
        /// parent surface is in synchronized mode or not. See
        /// wl_subsurface.set_sync and wl_subsurface.set_desync for details.
        ///
        /// If more than one set_position request is invoked by the client before
        /// the commit of the parent surface, the position of a new request always
        /// replaces the scheduled position from any previous request.
        ///
        /// The initial position is 0, 0.
        ///
        #[derive(Debug)]
        pub struct set_position {
            // x coordinate in the parent surface
            pub x: i32,
            // y coordinate in the parent surface
            pub y: i32,
        }
        impl Ser for set_position {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_position {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_position { x, y }, n))
            }
        }
        impl Msg<Obj> for set_position {
            const OP: u16 = 1;
        }
        impl Request<Obj> for set_position {}
        /// restack the sub-surface
        ///
        /// This sub-surface is taken from the stack, and put back just
        /// above the reference surface, changing the z-order of the sub-surfaces.
        /// The reference surface must be one of the sibling surfaces, or the
        /// parent surface. Using any other surface, including this sub-surface,
        /// will cause a protocol error.
        ///
        /// The z-order is double-buffered. Requests are handled in order and
        /// applied immediately to a pending state. The final pending state is
        /// copied to the active state the next time the state of the parent
        /// surface is applied. When this happens depends on whether the parent
        /// surface is in synchronized mode or not. See wl_subsurface.set_sync and
        /// wl_subsurface.set_desync for details.
        ///
        /// A new sub-surface is initially added as the top-most in the stack
        /// of its siblings and parent.
        ///
        #[derive(Debug)]
        pub struct place_above {
            // the reference surface
            pub sibling: object<wl_surface::Obj>,
        }
        impl Ser for place_above {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.sibling.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for place_above {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (sibling, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((place_above { sibling }, n))
            }
        }
        impl Msg<Obj> for place_above {
            const OP: u16 = 2;
        }
        impl Request<Obj> for place_above {}
        /// restack the sub-surface
        ///
        /// The sub-surface is placed just below the reference surface.
        /// See wl_subsurface.place_above.
        ///
        #[derive(Debug)]
        pub struct place_below {
            // the reference surface
            pub sibling: object<wl_surface::Obj>,
        }
        impl Ser for place_below {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.sibling.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for place_below {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (sibling, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((place_below { sibling }, n))
            }
        }
        impl Msg<Obj> for place_below {
            const OP: u16 = 3;
        }
        impl Request<Obj> for place_below {}
        /// set sub-surface to synchronized mode
        ///
        /// Change the commit behaviour of the sub-surface to synchronized
        /// mode, also described as the parent dependent mode.
        ///
        /// In synchronized mode, wl_surface.commit on a sub-surface will
        /// accumulate the committed state in a cache, but the state will
        /// not be applied and hence will not change the compositor output.
        /// The cached state is applied to the sub-surface immediately after
        /// the parent surface's state is applied. This ensures atomic
        /// updates of the parent and all its synchronized sub-surfaces.
        /// Applying the cached state will invalidate the cache, so further
        /// parent surface commits do not (re-)apply old state.
        ///
        /// See wl_subsurface for the recursive effect of this mode.
        ///
        #[derive(Debug)]
        pub struct set_sync {}
        impl Ser for set_sync {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for set_sync {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((set_sync {}, n))
            }
        }
        impl Msg<Obj> for set_sync {
            const OP: u16 = 4;
        }
        impl Request<Obj> for set_sync {}
        /// set sub-surface to desynchronized mode
        ///
        /// Change the commit behaviour of the sub-surface to desynchronized
        /// mode, also described as independent or freely running mode.
        ///
        /// In desynchronized mode, wl_surface.commit on a sub-surface will
        /// apply the pending state directly, without caching, as happens
        /// normally with a wl_surface. Calling wl_surface.commit on the
        /// parent surface has no effect on the sub-surface's wl_surface
        /// state. This mode allows a sub-surface to be updated on its own.
        ///
        /// If cached state exists when wl_surface.commit is called in
        /// desynchronized mode, the pending state is added to the cached
        /// state, and applied as a whole. This invalidates the cache.
        ///
        /// Note: even if a sub-surface is set to desynchronized, a parent
        /// sub-surface may override it to behave as synchronized. For details,
        /// see wl_subsurface.
        ///
        /// If a surface's parent surface behaves as desynchronized, then
        /// the cached state is applied on set_desync.
        ///
        #[derive(Debug)]
        pub struct set_desync {}
        impl Ser for set_desync {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for set_desync {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((set_desync {}, n))
            }
        }
        impl Msg<Obj> for set_desync {
            const OP: u16 = 5;
        }
        impl Request<Obj> for set_desync {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// wl_surface is not a sibling or the parent
        bad_surface = 0,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::bad_surface => 0,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::bad_surface),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct wl_subsurface;
    pub type Obj = wl_subsurface;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = wl_subsurface;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(0);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// create surfaces that are layers of the desktop
///
/// Clients can use this interface to assign the surface_layer role to
/// wl_surfaces. Such surfaces are assigned to a "layer" of the output and
/// rendered with a defined z-depth respective to each other. They may also be
/// anchored to the edges and corners of a screen and specify input handling
/// semantics. This interface should be suitable for the implementation of
/// many desktop shell components, and a broad number of other applications
/// that interact with the desktop.
///
pub mod zwlr_layer_shell_v1 {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "zwlr_layer_shell_v1";
    pub const VERSION: u32 = 4;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// create a layer_surface from a surface
        ///
        /// Create a layer surface for an existing surface. This assigns the role of
        /// layer_surface, or raises a protocol error if another role is already
        /// assigned.
        ///
        /// Creating a layer surface from a wl_surface which has a buffer attached
        /// or committed is a client error, and any attempts by a client to attach
        /// or manipulate a buffer prior to the first layer_surface.configure call
        /// must also be treated as errors.
        ///
        /// After creating a layer_surface object and setting it up, the client
        /// must perform an initial commit without any buffer attached.
        /// The compositor will reply with a layer_surface.configure event.
        /// The client must acknowledge it and is then allowed to attach a buffer
        /// to map the surface.
        ///
        /// You may pass NULL for output to allow the compositor to decide which
        /// output to use. Generally this will be the one that the user most
        /// recently interacted with.
        ///
        /// Clients can specify a namespace that defines the purpose of the layer
        /// surface.
        ///
        #[derive(Debug)]
        pub struct get_layer_surface {
            //
            pub id: new_id<zwlr_layer_surface_v1::Obj>,
            //
            pub surface: object<wl_surface::Obj>,
            //
            pub output: Option<object<wl_output::Obj>>,
            // layer to add this surface to
            pub layer: super::layer,
            // namespace for the layer surface
            pub namespace: String,
        }
        impl Ser for get_layer_surface {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                n += self.surface.serialize(&mut buf[n..], fds)?;
                n += self.output.serialize(&mut buf[n..], fds)?;
                n += self.layer.serialize(&mut buf[n..], fds)?;
                n += self.namespace.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for get_layer_surface {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<zwlr_layer_surface_v1::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (output, m) = <Option<object<wl_output::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (layer, m) = <super::layer>::deserialize(&buf[n..], fds)?;
                n += m;
                let (namespace, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((get_layer_surface { id, surface, output, layer, namespace }, n))
            }
        }
        impl Msg<Obj> for get_layer_surface {
            const OP: u16 = 0;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: zwlr_layer_surface_v1::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<zwlr_layer_surface_v1::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for get_layer_surface {}
        /// destroy the layer_shell object
        ///
        /// This request indicates that the client will not use the layer_shell
        /// object any more. Objects that have been created through this instance
        /// are not affected.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 1;
        }
        impl Request<Obj> for destroy {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// wl_surface has another role
        role = 0,
        /// layer value is invalid
        invalid_layer = 1,
        /// wl_surface has a buffer attached or committed
        already_constructed = 2,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::role => 0,
                error::invalid_layer => 1,
                error::already_constructed => 2,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::role),
                1 => Ok(error::invalid_layer),
                2 => Ok(error::already_constructed),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }
    /// available layers for surfaces
    ///
    /// These values indicate which layers a surface can be rendered in. They
    /// are ordered by z depth, bottom-most first. Traditional shell surfaces
    /// will typically be rendered between the bottom and top layers.
    /// Fullscreen shell surfaces are typically rendered at the top layer.
    /// Multiple surfaces can share a single layer, and ordering within a
    /// single layer is undefined.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum layer {
        background = 0,
        bottom = 1,
        top = 2,
        overlay = 3,
    }

    impl From<layer> for u32 {
        fn from(v: layer) -> u32 {
            match v {
                layer::background => 0,
                layer::bottom => 1,
                layer::top => 2,
                layer::overlay => 3,
            }
        }
    }

    impl TryFrom<u32> for layer {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(layer::background),
                1 => Ok(layer::bottom),
                2 => Ok(layer::top),
                3 => Ok(layer::overlay),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for layer {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for layer {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for layer")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct zwlr_layer_shell_v1;
    pub type Obj = zwlr_layer_shell_v1;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = zwlr_layer_shell_v1;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(1);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// layer metadata interface
///
/// An interface that may be implemented by a wl_surface, for surfaces that
/// are designed to be rendered as a layer of a stacked desktop-like
/// environment.
///
/// Layer surface state (layer, size, anchor, exclusive zone,
/// margin, interactivity) is double-buffered, and will be applied at the
/// time wl_surface.commit of the corresponding wl_surface is called.
///
/// Attaching a null buffer to a layer surface unmaps it.
///
/// Unmapping a layer_surface means that the surface cannot be shown by the
/// compositor until it is explicitly mapped again. The layer_surface
/// returns to the state it had right after layer_shell.get_layer_surface.
/// The client can re-map the surface by performing a commit without any
/// buffer attached, waiting for a configure event and handling it as usual.
///
pub mod zwlr_layer_surface_v1 {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "zwlr_layer_surface_v1";
    pub const VERSION: u32 = 4;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// sets the size of the surface
        ///
        /// Sets the size of the surface in surface-local coordinates. The
        /// compositor will display the surface centered with respect to its
        /// anchors.
        ///
        /// If you pass 0 for either value, the compositor will assign it and
        /// inform you of the assignment in the configure event. You must set your
        /// anchor to opposite edges in the dimensions you omit; not doing so is a
        /// protocol error. Both values are 0 by default.
        ///
        /// Size is double-buffered, see wl_surface.commit.
        ///
        #[derive(Debug)]
        pub struct set_size {
            //
            pub width: u32,
            //
            pub height: u32,
        }
        impl Ser for set_size {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_size {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (width, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_size { width, height }, n))
            }
        }
        impl Msg<Obj> for set_size {
            const OP: u16 = 0;
        }
        impl Request<Obj> for set_size {}
        /// configures the anchor point of the surface
        ///
        /// Requests that the compositor anchor the surface to the specified edges
        /// and corners. If two orthogonal edges are specified (e.g. 'top' and
        /// 'left'), then the anchor point will be the intersection of the edges
        /// (e.g. the top left corner of the output); otherwise the anchor point
        /// will be centered on that edge, or in the center if none is specified.
        ///
        /// Anchor is double-buffered, see wl_surface.commit.
        ///
        #[derive(Debug)]
        pub struct set_anchor {
            //
            pub anchor: super::anchor,
        }
        impl Ser for set_anchor {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.anchor.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_anchor {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (anchor, m) = <super::anchor>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_anchor { anchor }, n))
            }
        }
        impl Msg<Obj> for set_anchor {
            const OP: u16 = 1;
        }
        impl Request<Obj> for set_anchor {}
        /// configures the exclusive geometry of this surface
        ///
        /// Requests that the compositor avoids occluding an area with other
        /// surfaces. The compositor's use of this information is
        /// implementation-dependent - do not assume that this region will not
        /// actually be occluded.
        ///
        /// A positive value is only meaningful if the surface is anchored to one
        /// edge or an edge and both perpendicular edges. If the surface is not
        /// anchored, anchored to only two perpendicular edges (a corner), anchored
        /// to only two parallel edges or anchored to all edges, a positive value
        /// will be treated the same as zero.
        ///
        /// A positive zone is the distance from the edge in surface-local
        /// coordinates to consider exclusive.
        ///
        /// Surfaces that do not wish to have an exclusive zone may instead specify
        /// how they should interact with surfaces that do. If set to zero, the
        /// surface indicates that it would like to be moved to avoid occluding
        /// surfaces with a positive exclusive zone. If set to -1, the surface
        /// indicates that it would not like to be moved to accommodate for other
        /// surfaces, and the compositor should extend it all the way to the edges
        /// it is anchored to.
        ///
        /// For example, a panel might set its exclusive zone to 10, so that
        /// maximized shell surfaces are not shown on top of it. A notification
        /// might set its exclusive zone to 0, so that it is moved to avoid
        /// occluding the panel, but shell surfaces are shown underneath it. A
        /// wallpaper or lock screen might set their exclusive zone to -1, so that
        /// they stretch below or over the panel.
        ///
        /// The default value is 0.
        ///
        /// Exclusive zone is double-buffered, see wl_surface.commit.
        ///
        #[derive(Debug)]
        pub struct set_exclusive_zone {
            //
            pub zone: i32,
        }
        impl Ser for set_exclusive_zone {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.zone.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_exclusive_zone {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (zone, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_exclusive_zone { zone }, n))
            }
        }
        impl Msg<Obj> for set_exclusive_zone {
            const OP: u16 = 2;
        }
        impl Request<Obj> for set_exclusive_zone {}
        /// sets a margin from the anchor point
        ///
        /// Requests that the surface be placed some distance away from the anchor
        /// point on the output, in surface-local coordinates. Setting this value
        /// for edges you are not anchored to has no effect.
        ///
        /// The exclusive zone includes the margin.
        ///
        /// Margin is double-buffered, see wl_surface.commit.
        ///
        #[derive(Debug)]
        pub struct set_margin {
            //
            pub top: i32,
            //
            pub right: i32,
            //
            pub bottom: i32,
            //
            pub left: i32,
        }
        impl Ser for set_margin {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.top.serialize(&mut buf[n..], fds)?;
                n += self.right.serialize(&mut buf[n..], fds)?;
                n += self.bottom.serialize(&mut buf[n..], fds)?;
                n += self.left.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_margin {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (top, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (right, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (bottom, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (left, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_margin { top, right, bottom, left }, n))
            }
        }
        impl Msg<Obj> for set_margin {
            const OP: u16 = 3;
        }
        impl Request<Obj> for set_margin {}
        /// requests keyboard events
        ///
        /// Set how keyboard events are delivered to this surface. By default,
        /// layer shell surfaces do not receive keyboard events; this request can
        /// be used to change this.
        ///
        /// This setting is inherited by child surfaces set by the get_popup
        /// request.
        ///
        /// Layer surfaces receive pointer, touch, and tablet events normally. If
        /// you do not want to receive them, set the input region on your surface
        /// to an empty region.
        ///
        /// Keyboard interactivity is double-buffered, see wl_surface.commit.
        ///
        #[derive(Debug)]
        pub struct set_keyboard_interactivity {
            //
            pub keyboard_interactivity: super::keyboard_interactivity,
        }
        impl Ser for set_keyboard_interactivity {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.keyboard_interactivity.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_keyboard_interactivity {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (keyboard_interactivity, m) =
                    <super::keyboard_interactivity>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_keyboard_interactivity { keyboard_interactivity }, n))
            }
        }
        impl Msg<Obj> for set_keyboard_interactivity {
            const OP: u16 = 4;
        }
        impl Request<Obj> for set_keyboard_interactivity {}
        /// assign this layer_surface as an xdg_popup parent
        ///
        /// This assigns an xdg_popup's parent to this layer_surface.  This popup
        /// should have been created via xdg_surface::get_popup with the parent set
        /// to NULL, and this request must be invoked before committing the popup's
        /// initial state.
        ///
        /// See the documentation of xdg_popup for more details about what an
        /// xdg_popup is and how it is used.
        ///
        #[derive(Debug)]
        pub struct get_popup {
            //
            pub popup: object<xdg_popup::Obj>,
        }
        impl Ser for get_popup {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.popup.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for get_popup {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (popup, m) = <object<xdg_popup::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((get_popup { popup }, n))
            }
        }
        impl Msg<Obj> for get_popup {
            const OP: u16 = 5;
        }
        impl Request<Obj> for get_popup {}
        /// ack a configure event
        ///
        /// When a configure event is received, if a client commits the
        /// surface in response to the configure event, then the client
        /// must make an ack_configure request sometime before the commit
        /// request, passing along the serial of the configure event.
        ///
        /// If the client receives multiple configure events before it
        /// can respond to one, it only has to ack the last configure event.
        ///
        /// A client is not required to commit immediately after sending
        /// an ack_configure request - it may even ack_configure several times
        /// before its next surface commit.
        ///
        /// A client may send multiple ack_configure requests before committing, but
        /// only the last request sent before a commit indicates which configure
        /// event the client really is responding to.
        ///
        #[derive(Debug)]
        pub struct ack_configure {
            // the serial from the configure event
            pub serial: u32,
        }
        impl Ser for ack_configure {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for ack_configure {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((ack_configure { serial }, n))
            }
        }
        impl Msg<Obj> for ack_configure {
            const OP: u16 = 6;
        }
        impl Request<Obj> for ack_configure {}
        /// destroy the layer_surface
        ///
        /// This request destroys the layer surface.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 7;
        }
        impl Request<Obj> for destroy {}
        /// change the layer of the surface
        ///
        /// Change the layer that the surface is rendered on.
        ///
        /// Layer is double-buffered, see wl_surface.commit.
        ///
        #[derive(Debug)]
        pub struct set_layer {
            // layer to move this surface to
            pub layer: super::zwlr_layer_shell_v1::layer,
        }
        impl Ser for set_layer {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.layer.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_layer {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (layer, m) = <super::zwlr_layer_shell_v1::layer>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_layer { layer }, n))
            }
        }
        impl Msg<Obj> for set_layer {
            const OP: u16 = 8;
        }
        impl Request<Obj> for set_layer {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// suggest a surface change
        ///
        /// The configure event asks the client to resize its surface.
        ///
        /// Clients should arrange their surface for the new states, and then send
        /// an ack_configure request with the serial sent in this configure event at
        /// some point before committing the new surface.
        ///
        /// The client is free to dismiss all but the last configure event it
        /// received.
        ///
        /// The width and height arguments specify the size of the window in
        /// surface-local coordinates.
        ///
        /// The size is a hint, in the sense that the client is free to ignore it if
        /// it doesn't resize, pick a smaller size (to satisfy aspect ratio or
        /// resize in steps of NxM pixels). If the client picks a smaller size and
        /// is anchored to two opposite anchors (e.g. 'top' and 'bottom'), the
        /// surface will be centered on this axis.
        ///
        /// If the width or height arguments are zero, it means the client should
        /// decide its own window dimension.
        ///
        #[derive(Debug)]
        pub struct configure {
            //
            pub serial: u32,
            //
            pub width: u32,
            //
            pub height: u32,
        }
        impl Ser for configure {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for configure {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (width, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((configure { serial, width, height }, n))
            }
        }
        impl Msg<Obj> for configure {
            const OP: u16 = 0;
        }
        impl Event<Obj> for configure {}
        /// surface should be closed
        ///
        /// The closed event is sent by the compositor when the surface will no
        /// longer be shown. The output may have been destroyed or the user may
        /// have asked for it to be removed. Further changes to the surface will be
        /// ignored. The client should destroy the resource after receiving this
        /// event, and create a new surface if they so choose.
        ///
        #[derive(Debug)]
        pub struct closed {}
        impl Ser for closed {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for closed {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((closed {}, n))
            }
        }
        impl Msg<Obj> for closed {
            const OP: u16 = 1;
        }
        impl Event<Obj> for closed {}
    }
    /// types of keyboard interaction possible for a layer shell surface
    ///
    /// Types of keyboard interaction possible for layer shell surfaces. The
    /// rationale for this is twofold: (1) some applications are not interested
    /// in keyboard events and not allowing them to be focused can improve the
    /// desktop experience; (2) some applications will want to take exclusive
    /// keyboard focus.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum keyboard_interactivity {
        /// no keyboard focus is possible
        ///
        /// This value indicates that this surface is not interested in keyboard
        /// events and the compositor should never assign it the keyboard focus.
        ///
        /// This is the default value, set for newly created layer shell surfaces.
        ///
        /// This is useful for e.g. desktop widgets that display information or
        /// only have interaction with non-keyboard input devices.
        ///
        none = 0,
        /// request exclusive keyboard focus
        ///
        /// Request exclusive keyboard focus if this surface is above the shell surface layer.
        ///
        /// For the top and overlay layers, the seat will always give
        /// exclusive keyboard focus to the top-most layer which has keyboard
        /// interactivity set to exclusive. If this layer contains multiple
        /// surfaces with keyboard interactivity set to exclusive, the compositor
        /// determines the one receiving keyboard events in an implementation-
        /// defined manner. In this case, no guarantee is made when this surface
        /// will receive keyboard focus (if ever).
        ///
        /// For the bottom and background layers, the compositor is allowed to use
        /// normal focus semantics.
        ///
        /// This setting is mainly intended for applications that need to ensure
        /// they receive all keyboard events, such as a lock screen or a password
        /// prompt.
        ///
        exclusive = 1,
        /// request regular keyboard focus semantics
        ///
        /// This requests the compositor to allow this surface to be focused and
        /// unfocused by the user in an implementation-defined manner. The user
        /// should be able to unfocus this surface even regardless of the layer
        /// it is on.
        ///
        /// Typically, the compositor will want to use its normal mechanism to
        /// manage keyboard focus between layer shell surfaces with this setting
        /// and regular toplevels on the desktop layer (e.g. click to focus).
        /// Nevertheless, it is possible for a compositor to require a special
        /// interaction to focus or unfocus layer shell surfaces (e.g. requiring
        /// a click even if focus follows the mouse normally, or providing a
        /// keybinding to switch focus between layers).
        ///
        /// This setting is mainly intended for desktop shell components (e.g.
        /// panels) that allow keyboard interaction. Using this option can allow
        /// implementing a desktop shell that can be fully usable without the
        /// mouse.
        ///
        on_demand = 2,
    }

    impl From<keyboard_interactivity> for u32 {
        fn from(v: keyboard_interactivity) -> u32 {
            match v {
                keyboard_interactivity::none => 0,
                keyboard_interactivity::exclusive => 1,
                keyboard_interactivity::on_demand => 2,
            }
        }
    }

    impl TryFrom<u32> for keyboard_interactivity {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(keyboard_interactivity::none),
                1 => Ok(keyboard_interactivity::exclusive),
                2 => Ok(keyboard_interactivity::on_demand),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for keyboard_interactivity {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for keyboard_interactivity {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for keyboard_interactivity")?;
            Ok((v, n))
        }
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// provided surface state is invalid
        invalid_surface_state = 0,
        /// size is invalid
        invalid_size = 1,
        /// anchor bitfield is invalid
        invalid_anchor = 2,
        /// keyboard interactivity is invalid
        invalid_keyboard_interactivity = 3,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::invalid_surface_state => 0,
                error::invalid_size => 1,
                error::invalid_anchor => 2,
                error::invalid_keyboard_interactivity => 3,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::invalid_surface_state),
                1 => Ok(error::invalid_size),
                2 => Ok(error::invalid_anchor),
                3 => Ok(error::invalid_keyboard_interactivity),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct anchor {
        flags: u32,
    }
    impl Debug for anchor {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            let mut first = true;
            for (name, val) in Self::EACH {
                if val & *self == val {
                    if !first {
                        f.write_str("|")?;
                    }
                    first = false;
                    f.write_str(name)?;
                }
            }
            if first {
                f.write_str("EMPTY")?;
            }
            Ok(())
        }
    }
    impl std::fmt::LowerHex for anchor {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            <u32 as std::fmt::LowerHex>::fmt(&self.flags, f)
        }
    }
    impl Ser for anchor {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            self.flags.serialize(buf, fds)
        }
    }
    impl Dsr for anchor {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (flags, n) = u32::deserialize(buf, fds)?;
            Ok((anchor { flags }, n))
        }
    }
    impl std::ops::BitOr for anchor {
        type Output = Self;
        fn bitor(self, rhs: Self) -> Self {
            anchor { flags: self.flags | rhs.flags }
        }
    }
    impl std::ops::BitAnd for anchor {
        type Output = Self;
        fn bitand(self, rhs: Self) -> Self {
            anchor { flags: self.flags & rhs.flags }
        }
    }
    impl anchor {
        #![allow(non_upper_case_globals)]

        fn contains(&self, rhs: anchor) -> bool {
            (self.flags & rhs.flags) == rhs.flags
        }

        /// the top edge of the anchor rectangle
        pub const top: anchor = anchor { flags: 1 };
        /// the bottom edge of the anchor rectangle
        pub const bottom: anchor = anchor { flags: 2 };
        /// the left edge of the anchor rectangle
        pub const left: anchor = anchor { flags: 4 };
        /// the right edge of the anchor rectangle
        pub const right: anchor = anchor { flags: 8 };
        pub const EMPTY: anchor = anchor { flags: 0 };
        pub const ALL: anchor = anchor {
            flags: Self::EMPTY.flags
                | Self::top.flags
                | Self::bottom.flags
                | Self::left.flags
                | Self::right.flags,
        };
        pub const EACH: [(&'static str, anchor); 4] = [
            ("top", Self::top),
            ("bottom", Self::bottom),
            ("left", Self::left),
            ("right", Self::right),
        ];
    }

    #[derive(Debug, Clone, Copy)]
    pub struct zwlr_layer_surface_v1;
    pub type Obj = zwlr_layer_surface_v1;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = zwlr_layer_surface_v1;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(7);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// create desktop-style surfaces
///
/// The xdg_wm_base interface is exposed as a global object enabling clients
/// to turn their wl_surfaces into windows in a desktop environment. It
/// defines the basic functionality needed for clients and the compositor to
/// create windows that can be dragged, resized, maximized, etc, as well as
/// creating transient windows such as popup menus.
///
pub mod xdg_wm_base {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "xdg_wm_base";
    pub const VERSION: u32 = 6;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// destroy xdg_wm_base
        ///
        /// Destroy this xdg_wm_base object.
        ///
        /// Destroying a bound xdg_wm_base object while there are surfaces
        /// still alive created by this xdg_wm_base object instance is illegal
        /// and will result in a defunct_surfaces error.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 0;
        }
        impl Request<Obj> for destroy {}
        /// create a positioner object
        ///
        /// Create a positioner object. A positioner object is used to position
        /// surfaces relative to some parent surface. See the interface description
        /// and xdg_surface.get_popup for details.
        ///
        #[derive(Debug)]
        pub struct create_positioner {
            //
            pub id: new_id<xdg_positioner::Obj>,
        }
        impl Ser for create_positioner {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for create_positioner {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<xdg_positioner::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((create_positioner { id }, n))
            }
        }
        impl Msg<Obj> for create_positioner {
            const OP: u16 = 1;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: xdg_positioner::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<xdg_positioner::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for create_positioner {}
        /// create a shell surface from a surface
        ///
        /// This creates an xdg_surface for the given surface. While xdg_surface
        /// itself is not a role, the corresponding surface may only be assigned
        /// a role extending xdg_surface, such as xdg_toplevel or xdg_popup. It is
        /// illegal to create an xdg_surface for a wl_surface which already has an
        /// assigned role and this will result in a role error.
        ///
        /// This creates an xdg_surface for the given surface. An xdg_surface is
        /// used as basis to define a role to a given surface, such as xdg_toplevel
        /// or xdg_popup. It also manages functionality shared between xdg_surface
        /// based surface roles.
        ///
        /// See the documentation of xdg_surface for more details about what an
        /// xdg_surface is and how it is used.
        ///
        #[derive(Debug)]
        pub struct get_xdg_surface {
            //
            pub id: new_id<xdg_surface::Obj>,
            //
            pub surface: object<wl_surface::Obj>,
        }
        impl Ser for get_xdg_surface {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                n += self.surface.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for get_xdg_surface {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<xdg_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (surface, m) = <object<wl_surface::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((get_xdg_surface { id, surface }, n))
            }
        }
        impl Msg<Obj> for get_xdg_surface {
            const OP: u16 = 2;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: xdg_surface::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<xdg_surface::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for get_xdg_surface {}
        /// respond to a ping event
        ///
        /// A client must respond to a ping event with a pong request or
        /// the client may be deemed unresponsive. See xdg_wm_base.ping
        /// and xdg_wm_base.error.unresponsive.
        ///
        #[derive(Debug)]
        pub struct pong {
            // serial of the ping event
            pub serial: u32,
        }
        impl Ser for pong {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for pong {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((pong { serial }, n))
            }
        }
        impl Msg<Obj> for pong {
            const OP: u16 = 3;
        }
        impl Request<Obj> for pong {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// check if the client is alive
        ///
        /// The ping event asks the client if it's still alive. Pass the
        /// serial specified in the event back to the compositor by sending
        /// a "pong" request back with the specified serial. See xdg_wm_base.pong.
        ///
        /// Compositors can use this to determine if the client is still
        /// alive. It's unspecified what will happen if the client doesn't
        /// respond to the ping request, or in what timeframe. Clients should
        /// try to respond in a reasonable amount of time. The “unresponsive”
        /// error is provided for compositors that wish to disconnect unresponsive
        /// clients.
        ///
        /// A compositor is free to ping in any way it wants, but a client must
        /// always respond to any xdg_wm_base object it created.
        ///
        #[derive(Debug)]
        pub struct ping {
            // pass this to the pong request
            pub serial: u32,
        }
        impl Ser for ping {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for ping {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((ping { serial }, n))
            }
        }
        impl Msg<Obj> for ping {
            const OP: u16 = 0;
        }
        impl Event<Obj> for ping {}
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// given wl_surface has another role
        role = 0,
        /// xdg_wm_base was destroyed before children
        defunct_surfaces = 1,
        /// the client tried to map or destroy a non-topmost popup
        not_the_topmost_popup = 2,
        /// the client specified an invalid popup parent surface
        invalid_popup_parent = 3,
        /// the client provided an invalid surface state
        invalid_surface_state = 4,
        /// the client provided an invalid positioner
        invalid_positioner = 5,
        /// the client didn’t respond to a ping event in time
        unresponsive = 6,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::role => 0,
                error::defunct_surfaces => 1,
                error::not_the_topmost_popup => 2,
                error::invalid_popup_parent => 3,
                error::invalid_surface_state => 4,
                error::invalid_positioner => 5,
                error::unresponsive => 6,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::role),
                1 => Ok(error::defunct_surfaces),
                2 => Ok(error::not_the_topmost_popup),
                3 => Ok(error::invalid_popup_parent),
                4 => Ok(error::invalid_surface_state),
                5 => Ok(error::invalid_positioner),
                6 => Ok(error::unresponsive),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct xdg_wm_base;
    pub type Obj = xdg_wm_base;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = xdg_wm_base;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(0);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// child surface positioner
///
/// The xdg_positioner provides a collection of rules for the placement of a
/// child surface relative to a parent surface. Rules can be defined to ensure
/// the child surface remains within the visible area's borders, and to
/// specify how the child surface changes its position, such as sliding along
/// an axis, or flipping around a rectangle. These positioner-created rules are
/// constrained by the requirement that a child surface must intersect with or
/// be at least partially adjacent to its parent surface.
///
/// See the various requests for details about possible rules.
///
/// At the time of the request, the compositor makes a copy of the rules
/// specified by the xdg_positioner. Thus, after the request is complete the
/// xdg_positioner object can be destroyed or reused; further changes to the
/// object will have no effect on previous usages.
///
/// For an xdg_positioner object to be considered complete, it must have a
/// non-zero size set by set_size, and a non-zero anchor rectangle set by
/// set_anchor_rect. Passing an incomplete xdg_positioner object when
/// positioning a surface raises an invalid_positioner error.
///
pub mod xdg_positioner {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "xdg_positioner";
    pub const VERSION: u32 = 6;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// destroy the xdg_positioner object
        ///
        /// Notify the compositor that the xdg_positioner will no longer be used.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 0;
        }
        impl Request<Obj> for destroy {}
        /// set the size of the to-be positioned rectangle
        ///
        /// Set the size of the surface that is to be positioned with the positioner
        /// object. The size is in surface-local coordinates and corresponds to the
        /// window geometry. See xdg_surface.set_window_geometry.
        ///
        /// If a zero or negative size is set the invalid_input error is raised.
        ///
        #[derive(Debug)]
        pub struct set_size {
            // width of positioned rectangle
            pub width: i32,
            // height of positioned rectangle
            pub height: i32,
        }
        impl Ser for set_size {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_size {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_size { width, height }, n))
            }
        }
        impl Msg<Obj> for set_size {
            const OP: u16 = 1;
        }
        impl Request<Obj> for set_size {}
        /// set the anchor rectangle within the parent surface
        ///
        /// Specify the anchor rectangle within the parent surface that the child
        /// surface will be placed relative to. The rectangle is relative to the
        /// window geometry as defined by xdg_surface.set_window_geometry of the
        /// parent surface.
        ///
        /// When the xdg_positioner object is used to position a child surface, the
        /// anchor rectangle may not extend outside the window geometry of the
        /// positioned child's parent surface.
        ///
        /// If a negative size is set the invalid_input error is raised.
        ///
        #[derive(Debug)]
        pub struct set_anchor_rect {
            // x position of anchor rectangle
            pub x: i32,
            // y position of anchor rectangle
            pub y: i32,
            // width of anchor rectangle
            pub width: i32,
            // height of anchor rectangle
            pub height: i32,
        }
        impl Ser for set_anchor_rect {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_anchor_rect {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_anchor_rect { x, y, width, height }, n))
            }
        }
        impl Msg<Obj> for set_anchor_rect {
            const OP: u16 = 2;
        }
        impl Request<Obj> for set_anchor_rect {}
        /// set anchor rectangle anchor
        ///
        /// Defines the anchor point for the anchor rectangle. The specified anchor
        /// is used derive an anchor point that the child surface will be
        /// positioned relative to. If a corner anchor is set (e.g. 'top_left' or
        /// 'bottom_right'), the anchor point will be at the specified corner;
        /// otherwise, the derived anchor point will be centered on the specified
        /// edge, or in the center of the anchor rectangle if no edge is specified.
        ///
        #[derive(Debug)]
        pub struct set_anchor {
            // anchor
            pub anchor: super::anchor,
        }
        impl Ser for set_anchor {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.anchor.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_anchor {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (anchor, m) = <super::anchor>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_anchor { anchor }, n))
            }
        }
        impl Msg<Obj> for set_anchor {
            const OP: u16 = 3;
        }
        impl Request<Obj> for set_anchor {}
        /// set child surface gravity
        ///
        /// Defines in what direction a surface should be positioned, relative to
        /// the anchor point of the parent surface. If a corner gravity is
        /// specified (e.g. 'bottom_right' or 'top_left'), then the child surface
        /// will be placed towards the specified gravity; otherwise, the child
        /// surface will be centered over the anchor point on any axis that had no
        /// gravity specified. If the gravity is not in the ‘gravity’ enum, an
        /// invalid_input error is raised.
        ///
        #[derive(Debug)]
        pub struct set_gravity {
            // gravity direction
            pub gravity: super::gravity,
        }
        impl Ser for set_gravity {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.gravity.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_gravity {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (gravity, m) = <super::gravity>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_gravity { gravity }, n))
            }
        }
        impl Msg<Obj> for set_gravity {
            const OP: u16 = 4;
        }
        impl Request<Obj> for set_gravity {}
        /// set the adjustment to be done when constrained
        ///
        /// Specify how the window should be positioned if the originally intended
        /// position caused the surface to be constrained, meaning at least
        /// partially outside positioning boundaries set by the compositor. The
        /// adjustment is set by constructing a bitmask describing the adjustment to
        /// be made when the surface is constrained on that axis.
        ///
        /// If no bit for one axis is set, the compositor will assume that the child
        /// surface should not change its position on that axis when constrained.
        ///
        /// If more than one bit for one axis is set, the order of how adjustments
        /// are applied is specified in the corresponding adjustment descriptions.
        ///
        /// The default adjustment is none.
        ///
        #[derive(Debug)]
        pub struct set_constraint_adjustment {
            // bit mask of constraint adjustments
            pub constraint_adjustment: u32,
        }
        impl Ser for set_constraint_adjustment {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.constraint_adjustment.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_constraint_adjustment {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (constraint_adjustment, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_constraint_adjustment { constraint_adjustment }, n))
            }
        }
        impl Msg<Obj> for set_constraint_adjustment {
            const OP: u16 = 5;
        }
        impl Request<Obj> for set_constraint_adjustment {}
        /// set surface position offset
        ///
        /// Specify the surface position offset relative to the position of the
        /// anchor on the anchor rectangle and the anchor on the surface. For
        /// example if the anchor of the anchor rectangle is at (x, y), the surface
        /// has the gravity bottom|right, and the offset is (ox, oy), the calculated
        /// surface position will be (x + ox, y + oy). The offset position of the
        /// surface is the one used for constraint testing. See
        /// set_constraint_adjustment.
        ///
        /// An example use case is placing a popup menu on top of a user interface
        /// element, while aligning the user interface element of the parent surface
        /// with some user interface element placed somewhere in the popup surface.
        ///
        #[derive(Debug)]
        pub struct set_offset {
            // surface position x offset
            pub x: i32,
            // surface position y offset
            pub y: i32,
        }
        impl Ser for set_offset {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_offset {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_offset { x, y }, n))
            }
        }
        impl Msg<Obj> for set_offset {
            const OP: u16 = 6;
        }
        impl Request<Obj> for set_offset {}
        /// continuously reconstrain the surface
        ///
        /// When set reactive, the surface is reconstrained if the conditions used
        /// for constraining changed, e.g. the parent window moved.
        ///
        /// If the conditions changed and the popup was reconstrained, an
        /// xdg_popup.configure event is sent with updated geometry, followed by an
        /// xdg_surface.configure event.
        ///
        #[derive(Debug)]
        pub struct set_reactive {}
        impl Ser for set_reactive {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for set_reactive {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((set_reactive {}, n))
            }
        }
        impl Msg<Obj> for set_reactive {
            const OP: u16 = 7;
        }
        impl Request<Obj> for set_reactive {}
        ///
        /// Set the parent window geometry the compositor should use when
        /// positioning the popup. The compositor may use this information to
        /// determine the future state the popup should be constrained using. If
        /// this doesn't match the dimension of the parent the popup is eventually
        /// positioned against, the behavior is undefined.
        ///
        /// The arguments are given in the surface-local coordinate space.
        ///
        #[derive(Debug)]
        pub struct set_parent_size {
            // future window geometry width of parent
            pub parent_width: i32,
            // future window geometry height of parent
            pub parent_height: i32,
        }
        impl Ser for set_parent_size {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.parent_width.serialize(&mut buf[n..], fds)?;
                n += self.parent_height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_parent_size {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (parent_width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (parent_height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_parent_size { parent_width, parent_height }, n))
            }
        }
        impl Msg<Obj> for set_parent_size {
            const OP: u16 = 8;
        }
        impl Request<Obj> for set_parent_size {}
        /// set parent configure this is a response to
        ///
        /// Set the serial of an xdg_surface.configure event this positioner will be
        /// used in response to. The compositor may use this information together
        /// with set_parent_size to determine what future state the popup should be
        /// constrained using.
        ///
        #[derive(Debug)]
        pub struct set_parent_configure {
            // serial of parent configure event
            pub serial: u32,
        }
        impl Ser for set_parent_configure {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_parent_configure {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_parent_configure { serial }, n))
            }
        }
        impl Msg<Obj> for set_parent_configure {
            const OP: u16 = 9;
        }
        impl Request<Obj> for set_parent_configure {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// invalid input provided
        invalid_input = 0,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::invalid_input => 0,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::invalid_input),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum anchor {
        none = 0,
        top = 1,
        bottom = 2,
        left = 3,
        right = 4,
        top_left = 5,
        bottom_left = 6,
        top_right = 7,
        bottom_right = 8,
    }

    impl From<anchor> for u32 {
        fn from(v: anchor) -> u32 {
            match v {
                anchor::none => 0,
                anchor::top => 1,
                anchor::bottom => 2,
                anchor::left => 3,
                anchor::right => 4,
                anchor::top_left => 5,
                anchor::bottom_left => 6,
                anchor::top_right => 7,
                anchor::bottom_right => 8,
            }
        }
    }

    impl TryFrom<u32> for anchor {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(anchor::none),
                1 => Ok(anchor::top),
                2 => Ok(anchor::bottom),
                3 => Ok(anchor::left),
                4 => Ok(anchor::right),
                5 => Ok(anchor::top_left),
                6 => Ok(anchor::bottom_left),
                7 => Ok(anchor::top_right),
                8 => Ok(anchor::bottom_right),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for anchor {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for anchor {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for anchor")?;
            Ok((v, n))
        }
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum gravity {
        none = 0,
        top = 1,
        bottom = 2,
        left = 3,
        right = 4,
        top_left = 5,
        bottom_left = 6,
        top_right = 7,
        bottom_right = 8,
    }

    impl From<gravity> for u32 {
        fn from(v: gravity) -> u32 {
            match v {
                gravity::none => 0,
                gravity::top => 1,
                gravity::bottom => 2,
                gravity::left => 3,
                gravity::right => 4,
                gravity::top_left => 5,
                gravity::bottom_left => 6,
                gravity::top_right => 7,
                gravity::bottom_right => 8,
            }
        }
    }

    impl TryFrom<u32> for gravity {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(gravity::none),
                1 => Ok(gravity::top),
                2 => Ok(gravity::bottom),
                3 => Ok(gravity::left),
                4 => Ok(gravity::right),
                5 => Ok(gravity::top_left),
                6 => Ok(gravity::bottom_left),
                7 => Ok(gravity::top_right),
                8 => Ok(gravity::bottom_right),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for gravity {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for gravity {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for gravity")?;
            Ok((v, n))
        }
    }
    /// constraint adjustments
    ///
    /// The constraint adjustment value define ways the compositor will adjust
    /// the position of the surface, if the unadjusted position would result
    /// in the surface being partly constrained.
    ///
    /// Whether a surface is considered 'constrained' is left to the compositor
    /// to determine. For example, the surface may be partly outside the
    /// compositor's defined 'work area', thus necessitating the child surface's
    /// position be adjusted until it is entirely inside the work area.
    ///
    /// The adjustments can be combined, according to a defined precedence: 1)
    /// Flip, 2) Slide, 3) Resize.
    ///

    #[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct constraint_adjustment {
        flags: u32,
    }
    impl Debug for constraint_adjustment {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            let mut first = true;
            for (name, val) in Self::EACH {
                if val & *self == val {
                    if !first {
                        f.write_str("|")?;
                    }
                    first = false;
                    f.write_str(name)?;
                }
            }
            if first {
                f.write_str("EMPTY")?;
            }
            Ok(())
        }
    }
    impl std::fmt::LowerHex for constraint_adjustment {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            <u32 as std::fmt::LowerHex>::fmt(&self.flags, f)
        }
    }
    impl Ser for constraint_adjustment {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            self.flags.serialize(buf, fds)
        }
    }
    impl Dsr for constraint_adjustment {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (flags, n) = u32::deserialize(buf, fds)?;
            Ok((constraint_adjustment { flags }, n))
        }
    }
    impl std::ops::BitOr for constraint_adjustment {
        type Output = Self;
        fn bitor(self, rhs: Self) -> Self {
            constraint_adjustment { flags: self.flags | rhs.flags }
        }
    }
    impl std::ops::BitAnd for constraint_adjustment {
        type Output = Self;
        fn bitand(self, rhs: Self) -> Self {
            constraint_adjustment { flags: self.flags & rhs.flags }
        }
    }
    impl constraint_adjustment {
        #![allow(non_upper_case_globals)]

        fn contains(&self, rhs: constraint_adjustment) -> bool {
            (self.flags & rhs.flags) == rhs.flags
        }

        /// don't move the child surface when constrained
        ///
        /// Don't alter the surface position even if it is constrained on some
        /// axis, for example partially outside the edge of an output.
        ///
        pub const none: constraint_adjustment = constraint_adjustment { flags: 0 };
        /// move along the x axis until unconstrained
        ///
        /// Slide the surface along the x axis until it is no longer constrained.
        ///
        /// First try to slide towards the direction of the gravity on the x axis
        /// until either the edge in the opposite direction of the gravity is
        /// unconstrained or the edge in the direction of the gravity is
        /// constrained.
        ///
        /// Then try to slide towards the opposite direction of the gravity on the
        /// x axis until either the edge in the direction of the gravity is
        /// unconstrained or the edge in the opposite direction of the gravity is
        /// constrained.
        ///
        pub const slide_x: constraint_adjustment = constraint_adjustment { flags: 1 };
        /// move along the y axis until unconstrained
        ///
        /// Slide the surface along the y axis until it is no longer constrained.
        ///
        /// First try to slide towards the direction of the gravity on the y axis
        /// until either the edge in the opposite direction of the gravity is
        /// unconstrained or the edge in the direction of the gravity is
        /// constrained.
        ///
        /// Then try to slide towards the opposite direction of the gravity on the
        /// y axis until either the edge in the direction of the gravity is
        /// unconstrained or the edge in the opposite direction of the gravity is
        /// constrained.
        ///
        pub const slide_y: constraint_adjustment = constraint_adjustment { flags: 2 };
        /// invert the anchor and gravity on the x axis
        ///
        /// Invert the anchor and gravity on the x axis if the surface is
        /// constrained on the x axis. For example, if the left edge of the
        /// surface is constrained, the gravity is 'left' and the anchor is
        /// 'left', change the gravity to 'right' and the anchor to 'right'.
        ///
        /// If the adjusted position also ends up being constrained, the resulting
        /// position of the flip_x adjustment will be the one before the
        /// adjustment.
        ///
        pub const flip_x: constraint_adjustment = constraint_adjustment { flags: 4 };
        /// invert the anchor and gravity on the y axis
        ///
        /// Invert the anchor and gravity on the y axis if the surface is
        /// constrained on the y axis. For example, if the bottom edge of the
        /// surface is constrained, the gravity is 'bottom' and the anchor is
        /// 'bottom', change the gravity to 'top' and the anchor to 'top'.
        ///
        /// The adjusted position is calculated given the original anchor
        /// rectangle and offset, but with the new flipped anchor and gravity
        /// values.
        ///
        /// If the adjusted position also ends up being constrained, the resulting
        /// position of the flip_y adjustment will be the one before the
        /// adjustment.
        ///
        pub const flip_y: constraint_adjustment = constraint_adjustment { flags: 8 };
        /// horizontally resize the surface
        ///
        /// Resize the surface horizontally so that it is completely
        /// unconstrained.
        ///
        pub const resize_x: constraint_adjustment = constraint_adjustment { flags: 16 };
        /// vertically resize the surface
        ///
        /// Resize the surface vertically so that it is completely unconstrained.
        ///
        pub const resize_y: constraint_adjustment = constraint_adjustment { flags: 32 };
        pub const EMPTY: constraint_adjustment = constraint_adjustment { flags: 0 };
        pub const ALL: constraint_adjustment = constraint_adjustment {
            flags: Self::EMPTY.flags
                | Self::none.flags
                | Self::slide_x.flags
                | Self::slide_y.flags
                | Self::flip_x.flags
                | Self::flip_y.flags
                | Self::resize_x.flags
                | Self::resize_y.flags,
        };
        pub const EACH: [(&'static str, constraint_adjustment); 7] = [
            ("none", Self::none),
            ("slide_x", Self::slide_x),
            ("slide_y", Self::slide_y),
            ("flip_x", Self::flip_x),
            ("flip_y", Self::flip_y),
            ("resize_x", Self::resize_x),
            ("resize_y", Self::resize_y),
        ];
    }

    #[derive(Debug, Clone, Copy)]
    pub struct xdg_positioner;
    pub type Obj = xdg_positioner;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = xdg_positioner;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(0);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// desktop user interface surface base interface
///
/// An interface that may be implemented by a wl_surface, for
/// implementations that provide a desktop-style user interface.
///
/// It provides a base set of functionality required to construct user
/// interface elements requiring management by the compositor, such as
/// toplevel windows, menus, etc. The types of functionality are split into
/// xdg_surface roles.
///
/// Creating an xdg_surface does not set the role for a wl_surface. In order
/// to map an xdg_surface, the client must create a role-specific object
/// using, e.g., get_toplevel, get_popup. The wl_surface for any given
/// xdg_surface can have at most one role, and may not be assigned any role
/// not based on xdg_surface.
///
/// A role must be assigned before any other requests are made to the
/// xdg_surface object.
///
/// The client must call wl_surface.commit on the corresponding wl_surface
/// for the xdg_surface state to take effect.
///
/// Creating an xdg_surface from a wl_surface which has a buffer attached or
/// committed is a client error, and any attempts by a client to attach or
/// manipulate a buffer prior to the first xdg_surface.configure call must
/// also be treated as errors.
///
/// After creating a role-specific object and setting it up, the client must
/// perform an initial commit without any buffer attached. The compositor
/// will reply with initial wl_surface state such as
/// wl_surface.preferred_buffer_scale followed by an xdg_surface.configure
/// event. The client must acknowledge it and is then allowed to attach a
/// buffer to map the surface.
///
/// Mapping an xdg_surface-based role surface is defined as making it
/// possible for the surface to be shown by the compositor. Note that
/// a mapped surface is not guaranteed to be visible once it is mapped.
///
/// For an xdg_surface to be mapped by the compositor, the following
/// conditions must be met:
/// (1) the client has assigned an xdg_surface-based role to the surface
/// (2) the client has set and committed the xdg_surface state and the
/// role-dependent state to the surface
/// (3) the client has committed a buffer to the surface
///
/// A newly-unmapped surface is considered to have met condition (1) out
/// of the 3 required conditions for mapping a surface if its role surface
/// has not been destroyed, i.e. the client must perform the initial commit
/// again before attaching a buffer.
///
pub mod xdg_surface {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "xdg_surface";
    pub const VERSION: u32 = 6;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// destroy the xdg_surface
        ///
        /// Destroy the xdg_surface object. An xdg_surface must only be destroyed
        /// after its role object has been destroyed, otherwise
        /// a defunct_role_object error is raised.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 0;
        }
        impl Request<Obj> for destroy {}
        /// assign the xdg_toplevel surface role
        ///
        /// This creates an xdg_toplevel object for the given xdg_surface and gives
        /// the associated wl_surface the xdg_toplevel role.
        ///
        /// See the documentation of xdg_toplevel for more details about what an
        /// xdg_toplevel is and how it is used.
        ///
        #[derive(Debug)]
        pub struct get_toplevel {
            //
            pub id: new_id<xdg_toplevel::Obj>,
        }
        impl Ser for get_toplevel {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for get_toplevel {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<xdg_toplevel::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((get_toplevel { id }, n))
            }
        }
        impl Msg<Obj> for get_toplevel {
            const OP: u16 = 1;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: xdg_toplevel::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<xdg_toplevel::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for get_toplevel {}
        /// assign the xdg_popup surface role
        ///
        /// This creates an xdg_popup object for the given xdg_surface and gives
        /// the associated wl_surface the xdg_popup role.
        ///
        /// If null is passed as a parent, a parent surface must be specified using
        /// some other protocol, before committing the initial state.
        ///
        /// See the documentation of xdg_popup for more details about what an
        /// xdg_popup is and how it is used.
        ///
        #[derive(Debug)]
        pub struct get_popup {
            //
            pub id: new_id<xdg_popup::Obj>,
            //
            pub parent: Option<object<xdg_surface::Obj>>,
            //
            pub positioner: object<xdg_positioner::Obj>,
        }
        impl Ser for get_popup {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.id.serialize(&mut buf[n..], fds)?;
                n += self.parent.serialize(&mut buf[n..], fds)?;
                n += self.positioner.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for get_popup {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (id, m) = <new_id<xdg_popup::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (parent, m) = <Option<object<xdg_surface::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (positioner, m) = <object<xdg_positioner::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((get_popup { id, parent, positioner }, n))
            }
        }
        impl Msg<Obj> for get_popup {
            const OP: u16 = 2;
            fn new_id(&self) -> Option<(u32, ObjMeta)> {
                let id = self.id.0;
                let meta = ObjMeta {
                    alive: true,
                    destructor: xdg_popup::Obj::DESTRUCTOR,
                    type_id: TypeId::of::<xdg_popup::Obj>(),
                };
                Some((id, meta))
            }
        }
        impl Request<Obj> for get_popup {}
        /// set the new window geometry
        ///
        /// The window geometry of a surface is its "visible bounds" from the
        /// user's perspective. Client-side decorations often have invisible
        /// portions like drop-shadows which should be ignored for the
        /// purposes of aligning, placing and constraining windows.
        ///
        /// The window geometry is double buffered, and will be applied at the
        /// time wl_surface.commit of the corresponding wl_surface is called.
        ///
        /// When maintaining a position, the compositor should treat the (x, y)
        /// coordinate of the window geometry as the top left corner of the window.
        /// A client changing the (x, y) window geometry coordinate should in
        /// general not alter the position of the window.
        ///
        /// Once the window geometry of the surface is set, it is not possible to
        /// unset it, and it will remain the same until set_window_geometry is
        /// called again, even if a new subsurface or buffer is attached.
        ///
        /// If never set, the value is the full bounds of the surface,
        /// including any subsurfaces. This updates dynamically on every
        /// commit. This unset is meant for extremely simple clients.
        ///
        /// The arguments are given in the surface-local coordinate space of
        /// the wl_surface associated with this xdg_surface, and may extend outside
        /// of the wl_surface itself to mark parts of the subsurface tree as part of
        /// the window geometry.
        ///
        /// When applied, the effective window geometry will be the set window
        /// geometry clamped to the bounding rectangle of the combined
        /// geometry of the surface of the xdg_surface and the associated
        /// subsurfaces.
        ///
        /// The effective geometry will not be recalculated unless a new call to
        /// set_window_geometry is done and the new pending surface state is
        /// subsequently applied.
        ///
        /// The width and height of the effective window geometry must be
        /// greater than zero. Setting an invalid size will raise an
        /// invalid_size error.
        ///
        #[derive(Debug)]
        pub struct set_window_geometry {
            //
            pub x: i32,
            //
            pub y: i32,
            //
            pub width: i32,
            //
            pub height: i32,
        }
        impl Ser for set_window_geometry {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_window_geometry {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_window_geometry { x, y, width, height }, n))
            }
        }
        impl Msg<Obj> for set_window_geometry {
            const OP: u16 = 3;
        }
        impl Request<Obj> for set_window_geometry {}
        /// ack a configure event
        ///
        /// When a configure event is received, if a client commits the
        /// surface in response to the configure event, then the client
        /// must make an ack_configure request sometime before the commit
        /// request, passing along the serial of the configure event.
        ///
        /// For instance, for toplevel surfaces the compositor might use this
        /// information to move a surface to the top left only when the client has
        /// drawn itself for the maximized or fullscreen state.
        ///
        /// If the client receives multiple configure events before it
        /// can respond to one, it only has to ack the last configure event.
        /// Acking a configure event that was never sent raises an invalid_serial
        /// error.
        ///
        /// A client is not required to commit immediately after sending
        /// an ack_configure request - it may even ack_configure several times
        /// before its next surface commit.
        ///
        /// A client may send multiple ack_configure requests before committing, but
        /// only the last request sent before a commit indicates which configure
        /// event the client really is responding to.
        ///
        /// Sending an ack_configure request consumes the serial number sent with
        /// the request, as well as serial numbers sent by all configure events
        /// sent on this xdg_surface prior to the configure event referenced by
        /// the committed serial.
        ///
        /// It is an error to issue multiple ack_configure requests referencing a
        /// serial from the same configure event, or to issue an ack_configure
        /// request referencing a serial from a configure event issued before the
        /// event identified by the last ack_configure request for the same
        /// xdg_surface. Doing so will raise an invalid_serial error.
        ///
        #[derive(Debug)]
        pub struct ack_configure {
            // the serial from the configure event
            pub serial: u32,
        }
        impl Ser for ack_configure {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for ack_configure {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((ack_configure { serial }, n))
            }
        }
        impl Msg<Obj> for ack_configure {
            const OP: u16 = 4;
        }
        impl Request<Obj> for ack_configure {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// suggest a surface change
        ///
        /// The configure event marks the end of a configure sequence. A configure
        /// sequence is a set of one or more events configuring the state of the
        /// xdg_surface, including the final xdg_surface.configure event.
        ///
        /// Where applicable, xdg_surface surface roles will during a configure
        /// sequence extend this event as a latched state sent as events before the
        /// xdg_surface.configure event. Such events should be considered to make up
        /// a set of atomically applied configuration states, where the
        /// xdg_surface.configure commits the accumulated state.
        ///
        /// Clients should arrange their surface for the new states, and then send
        /// an ack_configure request with the serial sent in this configure event at
        /// some point before committing the new surface.
        ///
        /// If the client receives multiple configure events before it can respond
        /// to one, it is free to discard all but the last event it received.
        ///
        #[derive(Debug)]
        pub struct configure {
            // serial of the configure event
            pub serial: u32,
        }
        impl Ser for configure {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for configure {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((configure { serial }, n))
            }
        }
        impl Msg<Obj> for configure {
            const OP: u16 = 0;
        }
        impl Event<Obj> for configure {}
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// Surface was not fully constructed
        not_constructed = 1,
        /// Surface was already constructed
        already_constructed = 2,
        /// Attaching a buffer to an unconfigured surface
        unconfigured_buffer = 3,
        /// Invalid serial number when acking a configure event
        invalid_serial = 4,
        /// Width or height was zero or negative
        invalid_size = 5,
        /// Surface was destroyed before its role object
        defunct_role_object = 6,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::not_constructed => 1,
                error::already_constructed => 2,
                error::unconfigured_buffer => 3,
                error::invalid_serial => 4,
                error::invalid_size => 5,
                error::defunct_role_object => 6,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                1 => Ok(error::not_constructed),
                2 => Ok(error::already_constructed),
                3 => Ok(error::unconfigured_buffer),
                4 => Ok(error::invalid_serial),
                5 => Ok(error::invalid_size),
                6 => Ok(error::defunct_role_object),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct xdg_surface;
    pub type Obj = xdg_surface;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = xdg_surface;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(0);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// toplevel surface
///
/// This interface defines an xdg_surface role which allows a surface to,
/// among other things, set window-like properties such as maximize,
/// fullscreen, and minimize, set application-specific metadata like title and
/// id, and well as trigger user interactive operations such as interactive
/// resize and move.
///
/// Unmapping an xdg_toplevel means that the surface cannot be shown
/// by the compositor until it is explicitly mapped again.
/// All active operations (e.g., move, resize) are canceled and all
/// attributes (e.g. title, state, stacking, ...) are discarded for
/// an xdg_toplevel surface when it is unmapped. The xdg_toplevel returns to
/// the state it had right after xdg_surface.get_toplevel. The client
/// can re-map the toplevel by perfoming a commit without any buffer
/// attached, waiting for a configure event and handling it as usual (see
/// xdg_surface description).
///
/// Attaching a null buffer to a toplevel unmaps the surface.
///
pub mod xdg_toplevel {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "xdg_toplevel";
    pub const VERSION: u32 = 6;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// destroy the xdg_toplevel
        ///
        /// This request destroys the role surface and unmaps the surface;
        /// see "Unmapping" behavior in interface section for details.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 0;
        }
        impl Request<Obj> for destroy {}
        /// set the parent of this surface
        ///
        /// Set the "parent" of this surface. This surface should be stacked
        /// above the parent surface and all other ancestor surfaces.
        ///
        /// Parent surfaces should be set on dialogs, toolboxes, or other
        /// "auxiliary" surfaces, so that the parent is raised when the dialog
        /// is raised.
        ///
        /// Setting a null parent for a child surface unsets its parent. Setting
        /// a null parent for a surface which currently has no parent is a no-op.
        ///
        /// Only mapped surfaces can have child surfaces. Setting a parent which
        /// is not mapped is equivalent to setting a null parent. If a surface
        /// becomes unmapped, its children's parent is set to the parent of
        /// the now-unmapped surface. If the now-unmapped surface has no parent,
        /// its children's parent is unset. If the now-unmapped surface becomes
        /// mapped again, its parent-child relationship is not restored.
        ///
        /// The parent toplevel must not be one of the child toplevel's
        /// descendants, and the parent must be different from the child toplevel,
        /// otherwise the invalid_parent protocol error is raised.
        ///
        #[derive(Debug)]
        pub struct set_parent {
            //
            pub parent: Option<object<xdg_toplevel::Obj>>,
        }
        impl Ser for set_parent {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.parent.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_parent {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (parent, m) = <Option<object<xdg_toplevel::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_parent { parent }, n))
            }
        }
        impl Msg<Obj> for set_parent {
            const OP: u16 = 1;
        }
        impl Request<Obj> for set_parent {}
        /// set surface title
        ///
        /// Set a short title for the surface.
        ///
        /// This string may be used to identify the surface in a task bar,
        /// window list, or other user interface elements provided by the
        /// compositor.
        ///
        /// The string must be encoded in UTF-8.
        ///
        #[derive(Debug)]
        pub struct set_title {
            //
            pub title: String,
        }
        impl Ser for set_title {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.title.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_title {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (title, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_title { title }, n))
            }
        }
        impl Msg<Obj> for set_title {
            const OP: u16 = 2;
        }
        impl Request<Obj> for set_title {}
        /// set application ID
        ///
        /// Set an application identifier for the surface.
        ///
        /// The app ID identifies the general class of applications to which
        /// the surface belongs. The compositor can use this to group multiple
        /// surfaces together, or to determine how to launch a new application.
        ///
        /// For D-Bus activatable applications, the app ID is used as the D-Bus
        /// service name.
        ///
        /// The compositor shell will try to group application surfaces together
        /// by their app ID. As a best practice, it is suggested to select app
        /// ID's that match the basename of the application's .desktop file.
        /// For example, "org.freedesktop.FooViewer" where the .desktop file is
        /// "org.freedesktop.FooViewer.desktop".
        ///
        /// Like other properties, a set_app_id request can be sent after the
        /// xdg_toplevel has been mapped to update the property.
        ///
        /// See the desktop-entry specification [0] for more details on
        /// application identifiers and how they relate to well-known D-Bus
        /// names and .desktop files.
        ///
        /// [0] https://standards.freedesktop.org/desktop-entry-spec/
        ///
        #[derive(Debug)]
        pub struct set_app_id {
            //
            pub app_id: String,
        }
        impl Ser for set_app_id {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.app_id.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_app_id {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (app_id, m) = <String>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_app_id { app_id }, n))
            }
        }
        impl Msg<Obj> for set_app_id {
            const OP: u16 = 3;
        }
        impl Request<Obj> for set_app_id {}
        /// show the window menu
        ///
        /// Clients implementing client-side decorations might want to show
        /// a context menu when right-clicking on the decorations, giving the
        /// user a menu that they can use to maximize or minimize the window.
        ///
        /// This request asks the compositor to pop up such a window menu at
        /// the given position, relative to the local surface coordinates of
        /// the parent surface. There are no guarantees as to what menu items
        /// the window menu contains, or even if a window menu will be drawn
        /// at all.
        ///
        /// This request must be used in response to some sort of user action
        /// like a button press, key press, or touch down event.
        ///
        #[derive(Debug)]
        pub struct show_window_menu {
            // the wl_seat of the user event
            pub seat: object<wl_seat::Obj>,
            // the serial of the user event
            pub serial: u32,
            // the x position to pop up the window menu at
            pub x: i32,
            // the y position to pop up the window menu at
            pub y: i32,
        }
        impl Ser for show_window_menu {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.seat.serialize(&mut buf[n..], fds)?;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for show_window_menu {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (seat, m) = <object<wl_seat::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((show_window_menu { seat, serial, x, y }, n))
            }
        }
        impl Msg<Obj> for show_window_menu {
            const OP: u16 = 4;
        }
        impl Request<Obj> for show_window_menu {}
        /// start an interactive move
        ///
        /// Start an interactive, user-driven move of the surface.
        ///
        /// This request must be used in response to some sort of user action
        /// like a button press, key press, or touch down event. The passed
        /// serial is used to determine the type of interactive move (touch,
        /// pointer, etc).
        ///
        /// The server may ignore move requests depending on the state of
        /// the surface (e.g. fullscreen or maximized), or if the passed serial
        /// is no longer valid.
        ///
        /// If triggered, the surface will lose the focus of the device
        /// (wl_pointer, wl_touch, etc) used for the move. It is up to the
        /// compositor to visually indicate that the move is taking place, such as
        /// updating a pointer cursor, during the move. There is no guarantee
        /// that the device focus will return when the move is completed.
        ///
        #[derive(Debug)]
        pub struct move_ {
            // the wl_seat of the user event
            pub seat: object<wl_seat::Obj>,
            // the serial of the user event
            pub serial: u32,
        }
        impl Ser for move_ {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.seat.serialize(&mut buf[n..], fds)?;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for move_ {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (seat, m) = <object<wl_seat::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((move_ { seat, serial }, n))
            }
        }
        impl Msg<Obj> for move_ {
            const OP: u16 = 5;
        }
        impl Request<Obj> for move_ {}
        /// start an interactive resize
        ///
        /// Start a user-driven, interactive resize of the surface.
        ///
        /// This request must be used in response to some sort of user action
        /// like a button press, key press, or touch down event. The passed
        /// serial is used to determine the type of interactive resize (touch,
        /// pointer, etc).
        ///
        /// The server may ignore resize requests depending on the state of
        /// the surface (e.g. fullscreen or maximized).
        ///
        /// If triggered, the client will receive configure events with the
        /// "resize" state enum value and the expected sizes. See the "resize"
        /// enum value for more details about what is required. The client
        /// must also acknowledge configure events using "ack_configure". After
        /// the resize is completed, the client will receive another "configure"
        /// event without the resize state.
        ///
        /// If triggered, the surface also will lose the focus of the device
        /// (wl_pointer, wl_touch, etc) used for the resize. It is up to the
        /// compositor to visually indicate that the resize is taking place,
        /// such as updating a pointer cursor, during the resize. There is no
        /// guarantee that the device focus will return when the resize is
        /// completed.
        ///
        /// The edges parameter specifies how the surface should be resized, and
        /// is one of the values of the resize_edge enum. Values not matching
        /// a variant of the enum will cause the invalid_resize_edge protocol error.
        /// The compositor may use this information to update the surface position
        /// for example when dragging the top left corner. The compositor may also
        /// use this information to adapt its behavior, e.g. choose an appropriate
        /// cursor image.
        ///
        #[derive(Debug)]
        pub struct resize {
            // the wl_seat of the user event
            pub seat: object<wl_seat::Obj>,
            // the serial of the user event
            pub serial: u32,
            // which edge or corner is being dragged
            pub edges: super::resize_edge,
        }
        impl Ser for resize {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.seat.serialize(&mut buf[n..], fds)?;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                n += self.edges.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for resize {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (seat, m) = <object<wl_seat::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (edges, m) = <super::resize_edge>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((resize { seat, serial, edges }, n))
            }
        }
        impl Msg<Obj> for resize {
            const OP: u16 = 6;
        }
        impl Request<Obj> for resize {}
        /// set the maximum size
        ///
        /// Set a maximum size for the window.
        ///
        /// The client can specify a maximum size so that the compositor does
        /// not try to configure the window beyond this size.
        ///
        /// The width and height arguments are in window geometry coordinates.
        /// See xdg_surface.set_window_geometry.
        ///
        /// Values set in this way are double-buffered. They will get applied
        /// on the next commit.
        ///
        /// The compositor can use this information to allow or disallow
        /// different states like maximize or fullscreen and draw accurate
        /// animations.
        ///
        /// Similarly, a tiling window manager may use this information to
        /// place and resize client windows in a more effective way.
        ///
        /// The client should not rely on the compositor to obey the maximum
        /// size. The compositor may decide to ignore the values set by the
        /// client and request a larger size.
        ///
        /// If never set, or a value of zero in the request, means that the
        /// client has no expected maximum size in the given dimension.
        /// As a result, a client wishing to reset the maximum size
        /// to an unspecified state can use zero for width and height in the
        /// request.
        ///
        /// Requesting a maximum size to be smaller than the minimum size of
        /// a surface is illegal and will result in an invalid_size error.
        ///
        /// The width and height must be greater than or equal to zero. Using
        /// strictly negative values for width or height will result in a
        /// invalid_size error.
        ///
        #[derive(Debug)]
        pub struct set_max_size {
            //
            pub width: i32,
            //
            pub height: i32,
        }
        impl Ser for set_max_size {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_max_size {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_max_size { width, height }, n))
            }
        }
        impl Msg<Obj> for set_max_size {
            const OP: u16 = 7;
        }
        impl Request<Obj> for set_max_size {}
        /// set the minimum size
        ///
        /// Set a minimum size for the window.
        ///
        /// The client can specify a minimum size so that the compositor does
        /// not try to configure the window below this size.
        ///
        /// The width and height arguments are in window geometry coordinates.
        /// See xdg_surface.set_window_geometry.
        ///
        /// Values set in this way are double-buffered. They will get applied
        /// on the next commit.
        ///
        /// The compositor can use this information to allow or disallow
        /// different states like maximize or fullscreen and draw accurate
        /// animations.
        ///
        /// Similarly, a tiling window manager may use this information to
        /// place and resize client windows in a more effective way.
        ///
        /// The client should not rely on the compositor to obey the minimum
        /// size. The compositor may decide to ignore the values set by the
        /// client and request a smaller size.
        ///
        /// If never set, or a value of zero in the request, means that the
        /// client has no expected minimum size in the given dimension.
        /// As a result, a client wishing to reset the minimum size
        /// to an unspecified state can use zero for width and height in the
        /// request.
        ///
        /// Requesting a minimum size to be larger than the maximum size of
        /// a surface is illegal and will result in an invalid_size error.
        ///
        /// The width and height must be greater than or equal to zero. Using
        /// strictly negative values for width and height will result in a
        /// invalid_size error.
        ///
        #[derive(Debug)]
        pub struct set_min_size {
            //
            pub width: i32,
            //
            pub height: i32,
        }
        impl Ser for set_min_size {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_min_size {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_min_size { width, height }, n))
            }
        }
        impl Msg<Obj> for set_min_size {
            const OP: u16 = 8;
        }
        impl Request<Obj> for set_min_size {}
        /// maximize the window
        ///
        /// Maximize the surface.
        ///
        /// After requesting that the surface should be maximized, the compositor
        /// will respond by emitting a configure event. Whether this configure
        /// actually sets the window maximized is subject to compositor policies.
        /// The client must then update its content, drawing in the configured
        /// state. The client must also acknowledge the configure when committing
        /// the new content (see ack_configure).
        ///
        /// It is up to the compositor to decide how and where to maximize the
        /// surface, for example which output and what region of the screen should
        /// be used.
        ///
        /// If the surface was already maximized, the compositor will still emit
        /// a configure event with the "maximized" state.
        ///
        /// If the surface is in a fullscreen state, this request has no direct
        /// effect. It may alter the state the surface is returned to when
        /// unmaximized unless overridden by the compositor.
        ///
        #[derive(Debug)]
        pub struct set_maximized {}
        impl Ser for set_maximized {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for set_maximized {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((set_maximized {}, n))
            }
        }
        impl Msg<Obj> for set_maximized {
            const OP: u16 = 9;
        }
        impl Request<Obj> for set_maximized {}
        /// unmaximize the window
        ///
        /// Unmaximize the surface.
        ///
        /// After requesting that the surface should be unmaximized, the compositor
        /// will respond by emitting a configure event. Whether this actually
        /// un-maximizes the window is subject to compositor policies.
        /// If available and applicable, the compositor will include the window
        /// geometry dimensions the window had prior to being maximized in the
        /// configure event. The client must then update its content, drawing it in
        /// the configured state. The client must also acknowledge the configure
        /// when committing the new content (see ack_configure).
        ///
        /// It is up to the compositor to position the surface after it was
        /// unmaximized; usually the position the surface had before maximizing, if
        /// applicable.
        ///
        /// If the surface was already not maximized, the compositor will still
        /// emit a configure event without the "maximized" state.
        ///
        /// If the surface is in a fullscreen state, this request has no direct
        /// effect. It may alter the state the surface is returned to when
        /// unmaximized unless overridden by the compositor.
        ///
        #[derive(Debug)]
        pub struct unset_maximized {}
        impl Ser for unset_maximized {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for unset_maximized {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((unset_maximized {}, n))
            }
        }
        impl Msg<Obj> for unset_maximized {
            const OP: u16 = 10;
        }
        impl Request<Obj> for unset_maximized {}
        /// set the window as fullscreen on an output
        ///
        /// Make the surface fullscreen.
        ///
        /// After requesting that the surface should be fullscreened, the
        /// compositor will respond by emitting a configure event. Whether the
        /// client is actually put into a fullscreen state is subject to compositor
        /// policies. The client must also acknowledge the configure when
        /// committing the new content (see ack_configure).
        ///
        /// The output passed by the request indicates the client's preference as
        /// to which display it should be set fullscreen on. If this value is NULL,
        /// it's up to the compositor to choose which display will be used to map
        /// this surface.
        ///
        /// If the surface doesn't cover the whole output, the compositor will
        /// position the surface in the center of the output and compensate with
        /// with border fill covering the rest of the output. The content of the
        /// border fill is undefined, but should be assumed to be in some way that
        /// attempts to blend into the surrounding area (e.g. solid black).
        ///
        /// If the fullscreened surface is not opaque, the compositor must make
        /// sure that other screen content not part of the same surface tree (made
        /// up of subsurfaces, popups or similarly coupled surfaces) are not
        /// visible below the fullscreened surface.
        ///
        #[derive(Debug)]
        pub struct set_fullscreen {
            //
            pub output: Option<object<wl_output::Obj>>,
        }
        impl Ser for set_fullscreen {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.output.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for set_fullscreen {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (output, m) = <Option<object<wl_output::Obj>>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((set_fullscreen { output }, n))
            }
        }
        impl Msg<Obj> for set_fullscreen {
            const OP: u16 = 11;
        }
        impl Request<Obj> for set_fullscreen {}
        /// unset the window as fullscreen
        ///
        /// Make the surface no longer fullscreen.
        ///
        /// After requesting that the surface should be unfullscreened, the
        /// compositor will respond by emitting a configure event.
        /// Whether this actually removes the fullscreen state of the client is
        /// subject to compositor policies.
        ///
        /// Making a surface unfullscreen sets states for the surface based on the following:
        /// * the state(s) it may have had before becoming fullscreen
        /// * any state(s) decided by the compositor
        /// * any state(s) requested by the client while the surface was fullscreen
        ///
        /// The compositor may include the previous window geometry dimensions in
        /// the configure event, if applicable.
        ///
        /// The client must also acknowledge the configure when committing the new
        /// content (see ack_configure).
        ///
        #[derive(Debug)]
        pub struct unset_fullscreen {}
        impl Ser for unset_fullscreen {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for unset_fullscreen {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((unset_fullscreen {}, n))
            }
        }
        impl Msg<Obj> for unset_fullscreen {
            const OP: u16 = 12;
        }
        impl Request<Obj> for unset_fullscreen {}
        /// set the window as minimized
        ///
        /// Request that the compositor minimize your surface. There is no
        /// way to know if the surface is currently minimized, nor is there
        /// any way to unset minimization on this surface.
        ///
        /// If you are looking to throttle redrawing when minimized, please
        /// instead use the wl_surface.frame event for this, as this will
        /// also work with live previews on windows in Alt-Tab, Expose or
        /// similar compositor features.
        ///
        #[derive(Debug)]
        pub struct set_minimized {}
        impl Ser for set_minimized {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for set_minimized {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((set_minimized {}, n))
            }
        }
        impl Msg<Obj> for set_minimized {
            const OP: u16 = 13;
        }
        impl Request<Obj> for set_minimized {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// suggest a surface change
        ///
        /// This configure event asks the client to resize its toplevel surface or
        /// to change its state. The configured state should not be applied
        /// immediately. See xdg_surface.configure for details.
        ///
        /// The width and height arguments specify a hint to the window
        /// about how its surface should be resized in window geometry
        /// coordinates. See set_window_geometry.
        ///
        /// If the width or height arguments are zero, it means the client
        /// should decide its own window dimension. This may happen when the
        /// compositor needs to configure the state of the surface but doesn't
        /// have any information about any previous or expected dimension.
        ///
        /// The states listed in the event specify how the width/height
        /// arguments should be interpreted, and possibly how it should be
        /// drawn.
        ///
        /// Clients must send an ack_configure in response to this event. See
        /// xdg_surface.configure and xdg_surface.ack_configure for details.
        ///
        #[derive(Debug)]
        pub struct configure {
            //
            pub width: i32,
            //
            pub height: i32,
            //
            pub states: Vec<u8>,
        }
        impl Ser for configure {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                n += self.states.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for configure {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (states, m) = <Vec<u8>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((configure { width, height, states }, n))
            }
        }
        impl Msg<Obj> for configure {
            const OP: u16 = 0;
        }
        impl Event<Obj> for configure {}
        /// surface wants to be closed
        ///
        /// The close event is sent by the compositor when the user
        /// wants the surface to be closed. This should be equivalent to
        /// the user clicking the close button in client-side decorations,
        /// if your application has any.
        ///
        /// This is only a request that the user intends to close the
        /// window. The client may choose to ignore this request, or show
        /// a dialog to ask the user to save their data, etc.
        ///
        #[derive(Debug)]
        pub struct close {}
        impl Ser for close {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for close {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((close {}, n))
            }
        }
        impl Msg<Obj> for close {
            const OP: u16 = 1;
        }
        impl Event<Obj> for close {}
        /// recommended window geometry bounds
        ///
        /// The configure_bounds event may be sent prior to a xdg_toplevel.configure
        /// event to communicate the bounds a window geometry size is recommended
        /// to constrain to.
        ///
        /// The passed width and height are in surface coordinate space. If width
        /// and height are 0, it means bounds is unknown and equivalent to as if no
        /// configure_bounds event was ever sent for this surface.
        ///
        /// The bounds can for example correspond to the size of a monitor excluding
        /// any panels or other shell components, so that a surface isn't created in
        /// a way that it cannot fit.
        ///
        /// The bounds may change at any point, and in such a case, a new
        /// xdg_toplevel.configure_bounds will be sent, followed by
        /// xdg_toplevel.configure and xdg_surface.configure.
        ///
        #[derive(Debug)]
        pub struct configure_bounds {
            //
            pub width: i32,
            //
            pub height: i32,
        }
        impl Ser for configure_bounds {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for configure_bounds {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((configure_bounds { width, height }, n))
            }
        }
        impl Msg<Obj> for configure_bounds {
            const OP: u16 = 2;
        }
        impl Event<Obj> for configure_bounds {}
        /// compositor capabilities
        ///
        /// This event advertises the capabilities supported by the compositor. If
        /// a capability isn't supported, clients should hide or disable the UI
        /// elements that expose this functionality. For instance, if the
        /// compositor doesn't advertise support for minimized toplevels, a button
        /// triggering the set_minimized request should not be displayed.
        ///
        /// The compositor will ignore requests it doesn't support. For instance,
        /// a compositor which doesn't advertise support for minimized will ignore
        /// set_minimized requests.
        ///
        /// Compositors must send this event once before the first
        /// xdg_surface.configure event. When the capabilities change, compositors
        /// must send this event again and then send an xdg_surface.configure
        /// event.
        ///
        /// The configured state should not be applied immediately. See
        /// xdg_surface.configure for details.
        ///
        /// The capabilities are sent as an array of 32-bit unsigned integers in
        /// native endianness.
        ///
        #[derive(Debug)]
        pub struct wm_capabilities {
            // array of 32-bit capabilities
            pub capabilities: Vec<u8>,
        }
        impl Ser for wm_capabilities {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.capabilities.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for wm_capabilities {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (capabilities, m) = <Vec<u8>>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((wm_capabilities { capabilities }, n))
            }
        }
        impl Msg<Obj> for wm_capabilities {
            const OP: u16 = 3;
        }
        impl Event<Obj> for wm_capabilities {}
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// provided value is
        /// not a valid variant of the resize_edge enum
        invalid_resize_edge = 0,
        /// invalid parent toplevel
        invalid_parent = 1,
        /// client provided an invalid min or max size
        invalid_size = 2,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::invalid_resize_edge => 0,
                error::invalid_parent => 1,
                error::invalid_size => 2,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::invalid_resize_edge),
                1 => Ok(error::invalid_parent),
                2 => Ok(error::invalid_size),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }
    /// edge values for resizing
    ///
    /// These values are used to indicate which edge of a surface
    /// is being dragged in a resize operation.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum resize_edge {
        none = 0,
        top = 1,
        bottom = 2,
        left = 4,
        top_left = 5,
        bottom_left = 6,
        right = 8,
        top_right = 9,
        bottom_right = 10,
    }

    impl From<resize_edge> for u32 {
        fn from(v: resize_edge) -> u32 {
            match v {
                resize_edge::none => 0,
                resize_edge::top => 1,
                resize_edge::bottom => 2,
                resize_edge::left => 4,
                resize_edge::top_left => 5,
                resize_edge::bottom_left => 6,
                resize_edge::right => 8,
                resize_edge::top_right => 9,
                resize_edge::bottom_right => 10,
            }
        }
    }

    impl TryFrom<u32> for resize_edge {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(resize_edge::none),
                1 => Ok(resize_edge::top),
                2 => Ok(resize_edge::bottom),
                4 => Ok(resize_edge::left),
                5 => Ok(resize_edge::top_left),
                6 => Ok(resize_edge::bottom_left),
                8 => Ok(resize_edge::right),
                9 => Ok(resize_edge::top_right),
                10 => Ok(resize_edge::bottom_right),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for resize_edge {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for resize_edge {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for resize_edge")?;
            Ok((v, n))
        }
    }
    /// types of state on the surface
    ///
    /// The different state values used on the surface. This is designed for
    /// state values like maximized, fullscreen. It is paired with the
    /// configure event to ensure that both the client and the compositor
    /// setting the state can be synchronized.
    ///
    /// States set in this way are double-buffered. They will get applied on
    /// the next commit.
    ///
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum state {
        /// the surface is maximized
        ///
        /// The surface is maximized. The window geometry specified in the configure
        /// event must be obeyed by the client, or the xdg_wm_base.invalid_surface_state
        /// error is raised.
        ///
        /// The client should draw without shadow or other
        /// decoration outside of the window geometry.
        ///
        maximized = 1,
        /// the surface is fullscreen
        ///
        /// The surface is fullscreen. The window geometry specified in the
        /// configure event is a maximum; the client cannot resize beyond it. For
        /// a surface to cover the whole fullscreened area, the geometry
        /// dimensions must be obeyed by the client. For more details, see
        /// xdg_toplevel.set_fullscreen.
        ///
        fullscreen = 2,
        /// the surface is being resized
        ///
        /// The surface is being resized. The window geometry specified in the
        /// configure event is a maximum; the client cannot resize beyond it.
        /// Clients that have aspect ratio or cell sizing configuration can use
        /// a smaller size, however.
        ///
        resizing = 3,
        /// the surface is now activated
        ///
        /// Client window decorations should be painted as if the window is
        /// active. Do not assume this means that the window actually has
        /// keyboard or pointer focus.
        ///
        activated = 4,
        /// the surface’s left edge is tiled
        ///
        /// The window is currently in a tiled layout and the left edge is
        /// considered to be adjacent to another part of the tiling grid.
        ///
        tiled_left = 5,
        /// the surface’s right edge is tiled
        ///
        /// The window is currently in a tiled layout and the right edge is
        /// considered to be adjacent to another part of the tiling grid.
        ///
        tiled_right = 6,
        /// the surface’s top edge is tiled
        ///
        /// The window is currently in a tiled layout and the top edge is
        /// considered to be adjacent to another part of the tiling grid.
        ///
        tiled_top = 7,
        /// the surface’s bottom edge is tiled
        ///
        /// The window is currently in a tiled layout and the bottom edge is
        /// considered to be adjacent to another part of the tiling grid.
        ///
        tiled_bottom = 8,
        /// surface repaint is suspended
        ///
        /// The surface is currently not ordinarily being repainted; for
        /// example because its content is occluded by another window, or its
        /// outputs are switched off due to screen locking.
        ///
        suspended = 9,
    }

    impl From<state> for u32 {
        fn from(v: state) -> u32 {
            match v {
                state::maximized => 1,
                state::fullscreen => 2,
                state::resizing => 3,
                state::activated => 4,
                state::tiled_left => 5,
                state::tiled_right => 6,
                state::tiled_top => 7,
                state::tiled_bottom => 8,
                state::suspended => 9,
            }
        }
    }

    impl TryFrom<u32> for state {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                1 => Ok(state::maximized),
                2 => Ok(state::fullscreen),
                3 => Ok(state::resizing),
                4 => Ok(state::activated),
                5 => Ok(state::tiled_left),
                6 => Ok(state::tiled_right),
                7 => Ok(state::tiled_top),
                8 => Ok(state::tiled_bottom),
                9 => Ok(state::suspended),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for state {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for state {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for state")?;
            Ok((v, n))
        }
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum wm_capabilities {
        /// show_window_menu is available
        window_menu = 1,
        /// set_maximized and unset_maximized are available
        maximize = 2,
        /// set_fullscreen and unset_fullscreen are available
        fullscreen = 3,
        /// set_minimized is available
        minimize = 4,
    }

    impl From<wm_capabilities> for u32 {
        fn from(v: wm_capabilities) -> u32 {
            match v {
                wm_capabilities::window_menu => 1,
                wm_capabilities::maximize => 2,
                wm_capabilities::fullscreen => 3,
                wm_capabilities::minimize => 4,
            }
        }
    }

    impl TryFrom<u32> for wm_capabilities {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                1 => Ok(wm_capabilities::window_menu),
                2 => Ok(wm_capabilities::maximize),
                3 => Ok(wm_capabilities::fullscreen),
                4 => Ok(wm_capabilities::minimize),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for wm_capabilities {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for wm_capabilities {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for wm_capabilities")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct xdg_toplevel;
    pub type Obj = xdg_toplevel;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = xdg_toplevel;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(0);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
/// short-lived, popup surfaces for menus
///
/// A popup surface is a short-lived, temporary surface. It can be used to
/// implement for example menus, popovers, tooltips and other similar user
/// interface concepts.
///
/// A popup can be made to take an explicit grab. See xdg_popup.grab for
/// details.
///
/// When the popup is dismissed, a popup_done event will be sent out, and at
/// the same time the surface will be unmapped. See the xdg_popup.popup_done
/// event for details.
///
/// Explicitly destroying the xdg_popup object will also dismiss the popup and
/// unmap the surface. Clients that want to dismiss the popup when another
/// surface of their own is clicked should dismiss the popup using the destroy
/// request.
///
/// A newly created xdg_popup will be stacked on top of all previously created
/// xdg_popup surfaces associated with the same xdg_toplevel.
///
/// The parent of an xdg_popup must be mapped (see the xdg_surface
/// description) before the xdg_popup itself.
///
/// The client must call wl_surface.commit on the corresponding wl_surface
/// for the xdg_popup state to take effect.
///
pub mod xdg_popup {
    #![allow(non_camel_case_types)]
    #[allow(unused_imports)]
    use super::*;
    pub const NAME: &'static str = "xdg_popup";
    pub const VERSION: u32 = 6;
    pub mod request {
        #[allow(unused_imports)]
        use super::*;
        /// remove xdg_popup interface
        ///
        /// This destroys the popup. Explicitly destroying the xdg_popup
        /// object will also dismiss the popup, and unmap the surface.
        ///
        /// If this xdg_popup is not the "topmost" popup, the
        /// xdg_wm_base.not_the_topmost_popup protocol error will be sent.
        ///
        #[derive(Debug)]
        pub struct destroy {}
        impl Ser for destroy {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for destroy {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((destroy {}, n))
            }
        }
        impl Msg<Obj> for destroy {
            const OP: u16 = 0;
        }
        impl Request<Obj> for destroy {}
        /// make the popup take an explicit grab
        ///
        /// This request makes the created popup take an explicit grab. An explicit
        /// grab will be dismissed when the user dismisses the popup, or when the
        /// client destroys the xdg_popup. This can be done by the user clicking
        /// outside the surface, using the keyboard, or even locking the screen
        /// through closing the lid or a timeout.
        ///
        /// If the compositor denies the grab, the popup will be immediately
        /// dismissed.
        ///
        /// This request must be used in response to some sort of user action like a
        /// button press, key press, or touch down event. The serial number of the
        /// event should be passed as 'serial'.
        ///
        /// The parent of a grabbing popup must either be an xdg_toplevel surface or
        /// another xdg_popup with an explicit grab. If the parent is another
        /// xdg_popup it means that the popups are nested, with this popup now being
        /// the topmost popup.
        ///
        /// Nested popups must be destroyed in the reverse order they were created
        /// in, e.g. the only popup you are allowed to destroy at all times is the
        /// topmost one.
        ///
        /// When compositors choose to dismiss a popup, they may dismiss every
        /// nested grabbing popup as well. When a compositor dismisses popups, it
        /// will follow the same dismissing order as required from the client.
        ///
        /// If the topmost grabbing popup is destroyed, the grab will be returned to
        /// the parent of the popup, if that parent previously had an explicit grab.
        ///
        /// If the parent is a grabbing popup which has already been dismissed, this
        /// popup will be immediately dismissed. If the parent is a popup that did
        /// not take an explicit grab, an error will be raised.
        ///
        /// During a popup grab, the client owning the grab will receive pointer
        /// and touch events for all their surfaces as normal (similar to an
        /// "owner-events" grab in X11 parlance), while the top most grabbing popup
        /// will always have keyboard focus.
        ///
        #[derive(Debug)]
        pub struct grab {
            // the wl_seat of the user event
            pub seat: object<wl_seat::Obj>,
            // the serial of the user event
            pub serial: u32,
        }
        impl Ser for grab {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.seat.serialize(&mut buf[n..], fds)?;
                n += self.serial.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for grab {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (seat, m) = <object<wl_seat::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (serial, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((grab { seat, serial }, n))
            }
        }
        impl Msg<Obj> for grab {
            const OP: u16 = 1;
        }
        impl Request<Obj> for grab {}
        /// recalculate the popup's location
        ///
        /// Reposition an already-mapped popup. The popup will be placed given the
        /// details in the passed xdg_positioner object, and a
        /// xdg_popup.repositioned followed by xdg_popup.configure and
        /// xdg_surface.configure will be emitted in response. Any parameters set
        /// by the previous positioner will be discarded.
        ///
        /// The passed token will be sent in the corresponding
        /// xdg_popup.repositioned event. The new popup position will not take
        /// effect until the corresponding configure event is acknowledged by the
        /// client. See xdg_popup.repositioned for details. The token itself is
        /// opaque, and has no other special meaning.
        ///
        /// If multiple reposition requests are sent, the compositor may skip all
        /// but the last one.
        ///
        /// If the popup is repositioned in response to a configure event for its
        /// parent, the client should send an xdg_positioner.set_parent_configure
        /// and possibly an xdg_positioner.set_parent_size request to allow the
        /// compositor to properly constrain the popup.
        ///
        /// If the popup is repositioned together with a parent that is being
        /// resized, but not in response to a configure event, the client should
        /// send an xdg_positioner.set_parent_size request.
        ///
        #[derive(Debug)]
        pub struct reposition {
            //
            pub positioner: object<xdg_positioner::Obj>,
            // reposition request token
            pub token: u32,
        }
        impl Ser for reposition {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.positioner.serialize(&mut buf[n..], fds)?;
                n += self.token.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for reposition {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (positioner, m) = <object<xdg_positioner::Obj>>::deserialize(&buf[n..], fds)?;
                n += m;
                let (token, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((reposition { positioner, token }, n))
            }
        }
        impl Msg<Obj> for reposition {
            const OP: u16 = 2;
        }
        impl Request<Obj> for reposition {}
    }
    pub mod event {
        #[allow(unused_imports)]
        use super::*;
        /// configure the popup surface
        ///
        /// This event asks the popup surface to configure itself given the
        /// configuration. The configured state should not be applied immediately.
        /// See xdg_surface.configure for details.
        ///
        /// The x and y arguments represent the position the popup was placed at
        /// given the xdg_positioner rule, relative to the upper left corner of the
        /// window geometry of the parent surface.
        ///
        /// For version 2 or older, the configure event for an xdg_popup is only
        /// ever sent once for the initial configuration. Starting with version 3,
        /// it may be sent again if the popup is setup with an xdg_positioner with
        /// set_reactive requested, or in response to xdg_popup.reposition requests.
        ///
        #[derive(Debug)]
        pub struct configure {
            // x position relative to parent surface window geometry
            pub x: i32,
            // y position relative to parent surface window geometry
            pub y: i32,
            // window geometry width
            pub width: i32,
            // window geometry height
            pub height: i32,
        }
        impl Ser for configure {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.x.serialize(&mut buf[n..], fds)?;
                n += self.y.serialize(&mut buf[n..], fds)?;
                n += self.width.serialize(&mut buf[n..], fds)?;
                n += self.height.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for configure {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (x, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (y, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (width, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                let (height, m) = <i32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((configure { x, y, width, height }, n))
            }
        }
        impl Msg<Obj> for configure {
            const OP: u16 = 0;
        }
        impl Event<Obj> for configure {}
        /// popup interaction is done
        ///
        /// The popup_done event is sent out when a popup is dismissed by the
        /// compositor. The client should destroy the xdg_popup object at this
        /// point.
        ///
        #[derive(Debug)]
        pub struct popup_done {}
        impl Ser for popup_done {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                Ok(n)
            }
        }
        impl Dsr for popup_done {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                Ok((popup_done {}, n))
            }
        }
        impl Msg<Obj> for popup_done {
            const OP: u16 = 1;
        }
        impl Event<Obj> for popup_done {}
        /// signal the completion of a repositioned request
        ///
        /// The repositioned event is sent as part of a popup configuration
        /// sequence, together with xdg_popup.configure and lastly
        /// xdg_surface.configure to notify the completion of a reposition request.
        ///
        /// The repositioned event is to notify about the completion of a
        /// xdg_popup.reposition request. The token argument is the token passed
        /// in the xdg_popup.reposition request.
        ///
        /// Immediately after this event is emitted, xdg_popup.configure and
        /// xdg_surface.configure will be sent with the updated size and position,
        /// as well as a new configure serial.
        ///
        /// The client should optionally update the content of the popup, but must
        /// acknowledge the new popup configuration for the new position to take
        /// effect. See xdg_surface.ack_configure for details.
        ///
        #[derive(Debug)]
        pub struct repositioned {
            // reposition request token
            pub token: u32,
        }
        impl Ser for repositioned {
            #[allow(unused)]
            fn serialize(
                self,
                buf: &mut [u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<usize, &'static str> {
                let mut n = 0;
                n += self.token.serialize(&mut buf[n..], fds)?;
                Ok(n)
            }
        }
        impl Dsr for repositioned {
            #[allow(unused)]
            fn deserialize(
                buf: &[u8],
                fds: &mut Vec<OwnedFd>,
            ) -> Result<(Self, usize), &'static str> {
                let mut n = 0;
                let (token, m) = <u32>::deserialize(&buf[n..], fds)?;
                n += m;
                Ok((repositioned { token }, n))
            }
        }
        impl Msg<Obj> for repositioned {
            const OP: u16 = 2;
        }
        impl Event<Obj> for repositioned {}
    }
    #[repr(u32)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum error {
        /// tried to grab after being mapped
        invalid_grab = 0,
    }

    impl From<error> for u32 {
        fn from(v: error) -> u32 {
            match v {
                error::invalid_grab => 0,
            }
        }
    }

    impl TryFrom<u32> for error {
        type Error = core::num::TryFromIntError;
        fn try_from(v: u32) -> Result<Self, Self::Error> {
            match v {
                0 => Ok(error::invalid_grab),

                _ => u32::try_from(-1).map(|_| unreachable!())?,
            }
        }
    }

    impl Ser for error {
        fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            u32::from(self).serialize(buf, fds)
        }
    }

    impl Dsr for error {
        fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            let (i, n) = u32::deserialize(buf, fds)?;
            let v = i.try_into().map_err(|_| "invalid value for error")?;
            Ok((v, n))
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct xdg_popup;
    pub type Obj = xdg_popup;

    #[allow(non_upper_case_globals)]
    pub const Obj: Obj = xdg_popup;

    impl Dsr for Obj {
        fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {
            Ok((Obj, 0))
        }
    }

    impl Ser for Obj {
        fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {
            Ok(0)
        }
    }

    impl Object for Obj {
        const DESTRUCTOR: Option<u16> = Some(0);
        fn new_obj() -> Self {
            Obj
        }
        fn new_dyn(version: u32) -> Dyn {
            Dyn { name: NAME.to_string(), version }
        }
    }
}
