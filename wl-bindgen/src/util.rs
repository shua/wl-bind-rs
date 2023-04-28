use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;

fn cerrstr<T: From<i8> + PartialOrd>(ret: T) -> Result<T, String> {
    // try to read errno as early as possible
    let errno = unsafe { libc::__error().as_ref().map(|n| *n) };
    if ret > T::from(0i8) {
        return Ok(ret);
    }
    let errno = match errno {
        Some(n) => n,
        None => return Err(String::from("unknown error occurred")),
    };
    let buf = &mut [0u8; 128][..];
    let ret = unsafe { libc::strerror_r(errno, buf.as_mut_ptr().cast(), buf.len()) };
    assert!(ret >= 0, "error while trying to handle error ({errno})");
    let len = unsafe { libc::strnlen(buf.as_ptr().cast(), buf.len()) };
    let s = std::str::from_utf8(&buf[..len]).expect("error string is not valid utf8");
    Err(s.to_string())
}

fn recvmsg<'d>(
    sock: &mut UnixStream,
    data: &'d mut [u8],
    fd: &'d mut [RawFd],
) -> Result<(&'d mut [u8], &'d mut [RawFd]), String> {
    const RAWFD_SZ: usize = std::mem::size_of::<RawFd>();
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut libc::iovec {
        iov_base: data.as_mut_ptr().cast(),
        iov_len: data.len(),
    };
    msg.msg_iovlen = 1;
    if fd.len() > 0 {
        msg.msg_control = fd.as_mut_ptr().cast();
        msg.msg_controllen = (fd.len() * RAWFD_SZ) as u32;
    }
    cerrstr(unsafe { libc::recvmsg(sock.as_raw_fd(), &mut msg, 0) })?;

    let mut resdata = &mut data[..0];
    if !msg.msg_iov.is_null() && msg.msg_iovlen > 0 {
        let iov = unsafe { msg.msg_iov.as_mut() }.unwrap();
        let base: *mut u8 = iov.iov_base.cast();
        if !base.is_null() && iov.iov_len > 0 {
            // SAFETY: base must outlive 'd, as it is a pointer into data
            resdata = unsafe { std::slice::from_raw_parts_mut(base, iov.iov_len) };
        }
    }

    let mut resfd = &mut fd[..0];
    let mut cmsg_ptr = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    while let Some(cmsg) = unsafe { cmsg_ptr.as_ref() } {
        if cmsg.cmsg_level == libc::SOL_SOCKET && cmsg.cmsg_type == libc::SCM_RIGHTS {
            let buf: *mut RawFd = unsafe { libc::CMSG_DATA(cmsg_ptr) }.cast();
            if let (Some(buf), true) = (unsafe { buf.as_mut() }, cmsg.cmsg_len > 0) {
                let n = cmsg.cmsg_len as usize / RAWFD_SZ;
                // SAFETY: buf must outlive 'd, as it is a pointer into fd
                resfd = unsafe { std::slice::from_raw_parts_mut(buf, n) };
                break;
            }
        }
        cmsg_ptr = unsafe { libc::CMSG_NXTHDR(&msg, cmsg) };
    }
    Ok((resdata, resfd))
}

const fn sendmsg_fd_cap(len: usize) -> usize {
    let len = len * std::mem::size_of::<RawFd>();
    unsafe { libc::CMSG_SPACE(len as u32) as usize }
}

fn sendmsg<'d>(sock: &mut UnixStream, data: &[u8], fd: &mut Vec<RawFd>) -> Result<(), String> {
    const RAWFD_SZ: usize = std::mem::size_of::<RawFd>();
    let fdlen = fd.len();

    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut libc::iovec {
        iov_base: data.as_ptr().cast_mut().cast(),
        iov_len: data.len(),
    };
    msg.msg_iovlen = 1;

    let mut fd_offset: Option<usize> = None;
    if fdlen > 0 {
        let cmsg_space = unsafe { libc::CMSG_SPACE((fdlen * RAWFD_SZ) as u32) };
        if fd.capacity() < cmsg_space as usize {
            return Err(format!("not enough space in fd buffer"));
        }

        msg.msg_control = fd.as_ptr().cast_mut().cast();
        msg.msg_controllen = cmsg_space;

        let cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        assert!(!cmsg.is_null(), "msg_control is not null, how is cmsg?");
        let data_ptr: *const RawFd = unsafe { libc::CMSG_DATA(cmsg) }.cast();
        // SAFETY: for offset_from
        // * Both the starting and other pointer must be either in bounds or one
        //   byte past the end of the same [allocated object].
        // * Both pointers must be *derived from* a pointer to the same object.
        //   > cmsg is derived from msg.msg_control, which is derived from fd
        // * The distance between the pointers, in bytes, must be an exact multiple
        //   of the size of `T`.
        //   > RawFd is 4 bytes, cmsg internal fields are all 4 bytes
        // * The distance between the pointers, **in bytes**, cannot overflow an `isize`.
        // * The distance being in bounds cannot rely on "wrapping around" the address space.
        let offset = unsafe { data_ptr.offset_from(fd.as_ptr()) }
            .try_into()
            .expect("subslice should have address greater than slice");

        // SAFETY: we reset the len before we exit (modulo panic).
        // Even if we panic, the memory should no longer be uninit it's written
        // with values that can be interpreted as (invalid) RawFds
        unsafe { fd.set_len(fdlen + offset) };
        fd_offset = Some(offset);
        fd.rotate_right(offset);

        *(unsafe { cmsg.as_mut() }).unwrap() = libc::cmsghdr {
            cmsg_len: unsafe { libc::CMSG_LEN((fdlen * RAWFD_SZ) as u32) },
            cmsg_level: libc::SOL_SOCKET,
            cmsg_type: libc::SCM_RIGHTS,
        };
    }

    // SAFETY: even with *mut _ inside msghdr struct, sendmsg should not change any of them
    let ret = cerrstr(unsafe { libc::sendmsg(sock.as_raw_fd(), &msg, 0) });

    if let Some(offset) = fd_offset {
        fd.rotate_left(offset);
        fd.truncate(fd.len() - offset);
    }

    ret.map(|_| ())
}

fn shm_open(name: &str) -> Result<RawFd, String> {
    static SHM_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let name = if name == "" {
        let n = SHM_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("/dev/shm/wl_{n}\0",)
    } else {
        format!("/dev/shm/wl_{name}\0")
    };
    let fd = cerrstr(unsafe { libc::shm_open(name.as_ptr().cast(), libc::O_RDWR) })?;
    Ok(fd)
}

struct Conn {}
type Fd = std::os::fd::RawFd;
struct Fixed(u32);

struct Serializer {}
enum SerError {}
type SerResult = Result<(), SerError>;
trait Ser {
    fn ser(&self, s: &mut Serializer) -> SerResult;
}
impl Serializer {
    pub fn write_int(&mut self, val: i32) -> SerResult {
        todo!()
    }
    pub fn write_uint(&mut self, val: u32) -> SerResult {
        todo!()
    }
    pub fn write_string(&mut self, val: &str) -> SerResult {
        todo!()
    }
    pub fn write_array(&mut self, val: &[u8]) -> SerResult {
        todo!()
    }
    pub fn write_fd(&mut self, val: Fd) -> SerResult {
        todo!()
    }
    pub fn write_any(&mut self, val: &dyn Ser) -> SerResult {
        val.ser(self)
    }
}

struct Deserializer<'dsr, Ctx> {
    data: &'dsr [u8],
    pub ctx: Ctx,
}
enum DsrError {}
type DsrResult<T> = Result<T, DsrError>;
trait Dsr<'dsr>: Sized + 'dsr {
    type Context: ?Sized;
    fn dsr<Ctx: AsRef<Self::Context>>(s: &mut Deserializer<'dsr, Ctx>) -> DsrResult<Self>;
}
impl<'dsr, Ctx> Deserializer<'dsr, Ctx> {
    pub fn read_int(&mut self) -> DsrResult<i32> {
        todo!()
    }
    pub fn read_uint(&mut self) -> DsrResult<u32> {
        todo!()
    }
    pub fn read_string(&mut self) -> DsrResult<&'dsr str> {
        todo!()
    }
    pub fn read_array(&mut self) -> DsrResult<&'dsr [u8]> {
        todo!()
    }
    pub fn read_fd(&mut self) -> DsrResult<Fd> {
        todo!()
    }
    pub fn read_any<T: Dsr<'dsr>>(&mut self) -> DsrResult<T>
    where
        Ctx: AsRef<T::Context>,
    {
        T::dsr(self)
    }
}

struct MessageContext {
    conn: Arc<Conn>,
    op: u16,
}
impl AsRef<Arc<Conn>> for MessageContext {
    fn as_ref(&self) -> &Arc<Conn> {
        &self.conn
    }
}
