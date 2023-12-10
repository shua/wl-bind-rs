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
