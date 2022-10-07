use std::os::unix::{io::RawFd, net::UnixStream};

pub fn recvmsg(sock: &mut UnixStream, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), ()> {
    use std::os::unix::io::AsRawFd;

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
    let msg_control = (&mut fdbuf[fdbuf_len..]).as_mut_ptr() as *mut libc::c_void;

    let mut iov = libc::iovec {
        iov_base,
        iov_len: buf_cap,
    };
    let mut msg = libc::msghdr {
        msg_name: 0 as _,
        msg_namelen: 0,
        msg_iov: &mut iov as _,
        msg_iovlen: 1,
        msg_control,
        msg_controllen: std::mem::size_of::<RawFd>() * fdbuf_cap,
        msg_flags: 0,
    };

    let n = unsafe { libc::recvmsg(sock.as_raw_fd(), &mut msg as _, 0) };
    if n < 0 {
        unsafe { libc::perror(b"recvmsg\0".as_ptr() as _) };
        return Err(());
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
        while cmsg != std::ptr::null_mut() {
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
                libc::memmove(msg_control, libc::CMSG_DATA(cmsg) as _, n);
                fdbuf_extend = n / std::mem::size_of::<RawFd>();
                break;
            }

            cmsg = libc::CMSG_NXTHDR(&msg as _, cmsg);
        }
    }

    // SAFETY: we passed capacity to recvmsg, so fdbuf_extend won't put us past that,
    // and we initialized that memory using memmove to move valid fds into it.
    unsafe { fdbuf.set_len(fdbuf_len + fdbuf_extend) };

    Ok(())
}

pub fn sendmsg(sock: &mut UnixStream, buf: &[u8], fds: &[RawFd]) -> usize {
    use std::os::unix::io::AsRawFd;
    const FDBUF_MAX: usize =
        unsafe { libc::CMSG_SPACE((253 * std::mem::size_of::<RawFd>()) as u32) as usize };
    let mut msg_control: [u8; FDBUF_MAX] = [0; FDBUF_MAX];
    let cmsg_inner_len = fds.len() * std::mem::size_of::<RawFd>();
    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR((&mut msg_control).as_mut_ptr() as _);
        let ptr = libc::memcpy(
            libc::CMSG_DATA(cmsg) as _,
            fds.as_ptr() as _,
            cmsg_inner_len,
        );
        if ptr == std::ptr::null_mut() {
            libc::perror(b"memcpy\0".as_ptr() as _);
            panic!();
        }
        (*cmsg).cmsg_len = libc::CMSG_LEN(cmsg_inner_len as u32) as usize;
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
    }

    let mut iov = [libc::iovec {
        iov_base: unsafe { buf.as_ptr() as _ },
        iov_len: buf.len(),
    }; 1];
    let msg = libc::msghdr {
        msg_name: 0 as _,
        msg_namelen: 0,
        msg_iov: iov.as_mut_ptr(),
        msg_iovlen: iov.len(),
        msg_control: msg_control.as_mut_ptr() as _,
        msg_controllen: unsafe { libc::CMSG_SPACE(cmsg_inner_len as u32) as usize },
        msg_flags: 0,
    };

    let n = unsafe { libc::sendmsg(sock.as_raw_fd(), &msg, 0) };
    if n < 0 {
        unsafe { libc::perror(b"sendmsg\0".as_ptr() as _) };
        panic!();
    }
    n as usize
}
