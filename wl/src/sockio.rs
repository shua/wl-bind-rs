use crate::{WlSer, WlSerError};
use std::os::unix::{io::AsRawFd, io::RawFd, net::UnixStream};

const FDBUF_MAX: usize =
    unsafe { libc::CMSG_SPACE((253 * std::mem::size_of::<RawFd>()) as u32) as usize };

const WORD_SZ: usize = std::mem::size_of::<u32>();
const HDR_SZ: usize = WORD_SZ * 2;

pub struct BufStream {
    pub sock: UnixStream,
    buf: Vec<u8>,
    fdbuf: Vec<RawFd>,
}

pub struct BufStreamMessage<'buf> {
    buf: &'buf mut Vec<u8>,
    fdbuf: &'buf mut Vec<RawFd>,
    pub id: u32,
    pub op: u16,
    sz: usize,
}

impl BufStreamMessage<'_> {
    pub fn bufs(&mut self) -> (&mut [u8], &mut Vec<RawFd>) {
        (&mut self.buf[..self.sz], &mut self.fdbuf)
    }
}

impl Drop for BufStreamMessage<'_> {
    fn drop(&mut self) {
        // move unhandled bytes to front of buf, truncate
        self.buf.copy_within(self.sz.., 0);
        self.buf.truncate(self.buf.len() - self.sz);
    }
}

impl BufStream {
    pub fn new(sock: UnixStream) -> BufStream {
        BufStream {
            sock,
            buf: Vec::with_capacity(1024),
            fdbuf: Vec::with_capacity(4),
        }
    }

    pub fn recv(&mut self) -> Option<BufStreamMessage> {
        if self.buf.len() < HDR_SZ {
            let (bufn, fdbufn) =
                recvmsg(&mut self.sock, &mut self.buf, &mut self.fdbuf).expect("recvmsg");
            if bufn == 0 && fdbufn == 0 {
                return None;
            }
        }
        if self.buf.len() < HDR_SZ {
            eprintln!("buf len < HDR_SZ {} {HDR_SZ}", self.buf.len());
            return None;
        }

        let id = u32::from_ne_bytes(self.buf[..WORD_SZ].try_into().unwrap());
        let (sz, op): (usize, u16) = {
            let szop = u32::from_ne_bytes(self.buf[WORD_SZ..HDR_SZ].try_into().unwrap());
            (
                (szop >> 16).try_into().unwrap(),
                (szop & 0xffff).try_into().unwrap(),
            )
        };

        if self.buf.len() < sz {
            recvmsg(&mut self.sock, &mut self.buf, &mut self.fdbuf).expect("recvmsg");
            if self.buf.len() < sz {
                return None;
            }
        }

        Some(BufStreamMessage {
            buf: &mut self.buf,
            fdbuf: &mut self.fdbuf,
            id,
            op,
            sz,
        })
    }

    pub fn send<T: WlSer>(&mut self, id: u32, val: T) -> Result<(), WlSerError> {
        self.buf.truncate(0);
        self.buf.extend(id.to_ne_bytes());
        val.ser(&mut self.buf, &mut self.fdbuf)?;
        let n = sendmsg(&mut self.sock, &self.buf, &self.fdbuf);
        self.buf.truncate(0);
        self.fdbuf.truncate(0);
        if n < self.buf.len() {
            // not even sure we can retry the message here?
            // TODO: check wayland docs for what we can do on socket errors
            panic!("buffer underwrite");
        }
        Ok(())
    }
}

fn recvmsg(
    sock: &mut UnixStream,
    buf: &mut Vec<u8>,
    fdbuf: &mut Vec<RawFd>,
) -> Result<(usize, usize), ()> {
    let buf_len = buf.len();
    let buf_cap = buf.capacity() - buf_len;
    if buf_cap < std::mem::size_of::<u32>() * 2 {
        eprintln!("not enough memory for wl message header: {buf_len}");
        return Err(());
    }
    let fdbuf_len = fdbuf.len();
    let fdbuf_cap = fdbuf.capacity() - fdbuf_len;

    // SAFETY: will be <= capacity, and we promise not to access the uninitialized
    // parts until after we write into them with recvmsg
    unsafe { buf.set_len(buf.capacity()) };
    unsafe { fdbuf.set_len(fdbuf.capacity()) };

    let iov_base = (&mut buf[buf_len..]).as_mut_ptr() as *mut libc::c_void;
    //let msg_control = (&mut fdbuf[fdbuf_len..]).as_mut_ptr() as *mut libc::c_void;
    let mut msg_control: [u8; FDBUF_MAX] = [0; FDBUF_MAX];
    let cmsg_inner_len = fdbuf.len() * std::mem::size_of::<RawFd>();

    let mut iov = [libc::iovec {
        iov_base,
        iov_len: buf_cap,
    }; 1];
    let mut msg = libc::msghdr {
        msg_name: 0 as _,
        msg_namelen: 0,
        msg_iov: &mut iov as _,
        msg_iovlen: 1,
        msg_control: msg_control.as_mut_ptr() as _,
        msg_controllen: FDBUF_MAX,
        msg_flags: 0,
    };

    let n = unsafe { libc::recvmsg(sock.as_raw_fd(), &mut msg as _, 0) };
    if n < 0 {
        let errno = unsafe { *libc::__errno_location() };
        if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK {
            unsafe {
                buf.set_len(buf_len);
                fdbuf.set_len(fdbuf_len);
            }
            return Ok((0, 0));
        } else {
            unsafe { libc::perror(b"recvmsg\0".as_ptr() as _) };
            return Err(());
        }
    }

    // SAFETY: recvmsg will not read more than iov_len == buf.capacity() bytes
    // and will return number of bytes read as n
    unsafe { buf.set_len(buf_len + n as usize) };
    if (msg.msg_flags & libc::MSG_CTRUNC) != 0 {
        eprintln!("ancillary data truncated");
        // can't really be certain of the data, so discarding it
        unsafe { fdbuf.set_len(fdbuf_len) };
        return Err(());
    }

    let mut fdbuf_extend = 0;
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg as _);
        while !cmsg.is_null() {
            //
            // > When sending ancillary data with sendmsg(2), only one item of  each  of
            // > the above types may be included in the sent message.
            // > -- unix(7)
            //
            // I take the above quote to mean that if SCM_RIGHTS is sent, it
            // is sent with only a single header, and no other ancillary data is sent.
            // I don't care about any other header types, so I'm going to discard,
            // the rest of the cmsg data, and memmove the fds to the beginning of the
            // buffer.
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let n = (*cmsg).cmsg_len;
                libc::memmove(
                    fdbuf.as_mut_ptr() as *mut libc::c_void,
                    libc::CMSG_DATA(cmsg) as _,
                    n,
                );
                fdbuf_extend = n / std::mem::size_of::<RawFd>();
                break;
            }

            cmsg = libc::CMSG_NXTHDR(&msg as _, cmsg);
        }
    }

    // SAFETY: we passed capacity to recvmsg, so fdbuf_extend won't put us past that,
    // and we initialized that memory using memmove to move valid fds into it.
    unsafe { fdbuf.set_len(fdbuf_len + fdbuf_extend) };

    Ok((n.try_into().unwrap(), fdbuf_extend))
}

fn sendmsg(sock: &mut UnixStream, buf: &[u8], fds: &[RawFd]) -> usize {
    let mut msg_control: [u8; FDBUF_MAX] = [0; FDBUF_MAX];
    let cmsg_inner_len = fds.len() * std::mem::size_of::<RawFd>();

    let mut iov = [libc::iovec {
        iov_base: unsafe { buf.as_ptr() as _ },
        iov_len: buf.len(),
    }; 1];
    let mut msg = libc::msghdr {
        msg_name: 0 as _,
        msg_namelen: 0,
        msg_iov: iov.as_mut_ptr(),
        msg_iovlen: iov.len(),
        msg_control: msg_control.as_mut_ptr() as _,
        msg_controllen: unsafe { libc::CMSG_SPACE(cmsg_inner_len as u32) as usize },
        msg_flags: 0,
    };

    if fds.len() == 0 {
        msg.msg_control = std::ptr::null_mut();
        msg.msg_controllen = 0;
    } else {
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            let ptr = libc::memcpy(
                libc::CMSG_DATA(cmsg) as _,
                fds.as_ptr() as _,
                cmsg_inner_len,
            );
            if ptr.is_null() {
                libc::perror(b"memcpy\0".as_ptr() as _);
                panic!();
            }
            (*cmsg).cmsg_len = libc::CMSG_LEN(cmsg_inner_len as u32) as usize;
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        }
    }

    let n = unsafe { libc::sendmsg(sock.as_raw_fd(), &msg, 0) };
    if n < 0 {
        unsafe { libc::perror(b"sendmsg\0".as_ptr() as _) };
        panic!();
    }
    n as usize
}
