mod wl;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;

const FDBUF_MAX: usize =
    unsafe { libc::CMSG_SPACE((253 * std::mem::size_of::<RawFd>()) as u32) as usize };

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

    // SAFETY: will be <= capacity, and we promise not to access the uninitialized
    // parts until after we write into them with recvmsg
    unsafe { buf.set_len(buf.capacity()) };
    unsafe { fdbuf.set_len(fdbuf.capacity()) };

    let iov_base = (&mut buf[buf_len..]).as_mut_ptr() as *mut libc::c_void;
    //let msg_control = (&mut fdbuf[fdbuf_len..]).as_mut_ptr() as *mut libc::c_void;
    let mut msg_control: [u8; FDBUF_MAX] = [0; FDBUF_MAX];

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
        iov_base: buf.as_ptr() as _,
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

fn main() {
    use wl::Dsr;

    let mut conn = find_socket().unwrap();
    let display = wl::wl_display::Handle(1);
    let (mut buf, mut fds) = (vec![], vec![]);
    wl::Message(
        display,
        wl::wl_display::Request::GetRegistry {
            registry: wl::wl_registry::Handle(2),
        },
    )
    .ser(&mut wl::WriteBuf::new(&mut buf, &mut fds))
    .unwrap();
    sendmsg(&mut conn, &buf, &fds);
}
