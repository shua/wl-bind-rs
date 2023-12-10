mod wl;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

fn main() {
    let (mut c, display) = wl::Client::connect().expect("connect");

    println!("register globals");
    let registry = c
        .construct(display, |registry| wl::wl_display::request::get_registry { registry })
        .unwrap();
    let cb = c.construct(display, |cb| wl::wl_display::request::sync { callback: cb }).unwrap();
    let mut globals = [None::<(u32, u32)>; 3];
    let (compositorg, shmg, layer_shellg) = loop {
        let mut done = false;
        let recv = (c.recv().unwrap())
            .on(registry, |wl::wl_registry::event::global { name, interface, version }| {
                match interface.as_str() {
                    wl::wl_compositor::NAME => {
                        if version < wl::wl_compositor::VERSION {
                            eprintln!(
                                "version supported by server is less than version supported by us"
                            );
                        }
                        globals[0] = Some((name, version));
                    }
                    wl::wl_shm::NAME => {
                        if version < wl::wl_shm::VERSION {
                            eprintln!(
                                "version supported by server is less than version supported by us"
                            );
                        }
                        globals[1] = Some((name, version));
                    }
                    wl::zwlr_layer_shell_v1::NAME => {
                        if version < wl::zwlr_layer_shell_v1::VERSION {
                            eprintln!(
                                "version supported by server is less than version supported by us"
                            );
                        }
                        globals[2] = Some((name, version));
                    }
                    _ => {}
                }
            })
            .on(cb, |wl::wl_callback::event::done { .. }| done = true);

        if done {
            if let [Some(compositor), Some(shm), Some(lsh)] = globals {
                break (compositor, shm, lsh);
            }
            eprintln!("unable to register necessary globals");
            return;
        }
        if let wl::RecvState::Done(Err(err)) = recv {
            eprintln!("error:globals: {err}");
            return;
        }
        if let wl::RecvState::None = recv {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    };
    let compositor = c
        .construct(registry, |id: wl::new_id<wl::wl_compositor::Obj>| {
            wl::wl_registry::request::bind { name: compositorg.0, id: id.to_dyn(compositorg.1) }
        })
        .unwrap();
    let shm = c
        .construct(registry, |id: wl::new_id<wl::wl_shm::Obj>| wl::wl_registry::request::bind {
            name: shmg.0,
            id: id.to_dyn(shmg.1),
        })
        .unwrap();
    let layer_shell = c
        .construct(registry, |id: wl::new_id<wl::zwlr_layer_shell_v1::Obj>| {
            wl::wl_registry::request::bind { name: layer_shellg.0, id: id.to_dyn(layer_shellg.1) }
        })
        .unwrap();

    let cb = c.construct(display, |cb| wl::wl_display::request::sync { callback: cb }).unwrap();
    let mut shm_fmts = vec![];
    loop {
        let mut done = false;
        let recv = (c.recv().unwrap())
            .on(cb, |wl::wl_callback::event::done { .. }| done = true)
            .on(shm, |wl::wl_shm::event::format { format }| shm_fmts.push(format));
        if done {
            break;
        }
        if let wl::RecvState::Some(recv) = recv {
            eprintln!("unexpected message: {recv:?}");
            return;
        }
        if let wl::RecvState::None = recv {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    let surface =
        c.construct(compositor, |id| wl::wl_compositor::request::create_surface { id }).unwrap();

    let mut pbuf = pixbuf::ShmPixelBuffer::new(&mut c, shm, "/wl-shm", 300, 200);
    {
        let mut buf = pbuf.lock().unwrap();
        for i in 0..buf.width {
            for j in 0..buf.height {
                buf[[i, j]] = 0xddff9900;
            }
        }
    }

    let wlr_surface = c
        .construct(layer_shell, |id| wl::zwlr_layer_shell_v1::request::get_layer_surface {
            id,
            surface,
            output: None,
            layer: wl::zwlr_layer_shell_v1::layer::bottom,
            namespace: "wlmenu".to_string(),
        })
        .unwrap();
    c.send(wlr_surface, wl::zwlr_layer_surface_v1::request::set_size { width: 300, height: 200 })
        .unwrap();
    let configured = Arc::new(Mutex::new(None::<u32>));
    let wconfigured = Arc::downgrade(&configured);
    c.listen(wlr_surface, move |wl::zwlr_layer_surface_v1::event::configure { serial, .. }| {
        if let Some(configured) = wconfigured.upgrade() {
            *configured.lock().unwrap() = Some(serial);
        }
        Ok(())
    });
    let close = Arc::new(AtomicBool::new(false));
    let wclose = Arc::downgrade(&close);
    c.listen(wlr_surface, move |wl::zwlr_layer_surface_v1::event::closed {}| {
        if let Some(close) = wclose.upgrade() {
            close.store(true, Ordering::SeqCst);
        }
        Ok(())
    });
    c.send(surface, wl::wl_surface::request::commit {}).unwrap();
    let committed = Arc::new(AtomicBool::new(false));

    println!("main loop");
    while !close.load(Ordering::SeqCst) {
        if let (Some(serial), false) =
            (*configured.lock().unwrap(), committed.load(Ordering::SeqCst))
        {
            c.send(surface, wl::wl_surface::request::attach { buffer: Some(pbuf.wl), x: 0, y: 0 })
                .unwrap();
            c.send(wlr_surface, wl::zwlr_layer_surface_v1::request::ack_configure { serial })
                .unwrap();
            c.send(surface, wl::wl_surface::request::commit {}).unwrap();
            pbuf.locked.store(true, Ordering::SeqCst);
            committed.store(true, Ordering::SeqCst);
        }

        let recv = c.recv().unwrap();
        if let wl::RecvState::None = recv {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    println!("goodbye");
}

mod shm {
    use std::os::fd::{AsRawFd as _, RawFd};

    #[derive(Debug)]
    pub struct Shm {
        buf: *mut u8,
        len: usize,
        name: std::ffi::CString,
        pub fd: RawFd,
    }
    impl std::ops::Drop for Shm {
        fn drop(&mut self) {
            let fd = self.fd.as_raw_fd();
            if fd != -1 {
                let ret = unsafe { libc::munmap(self.buf.cast(), self.len) };
                if ret < 0 {
                    eprintln!("error: {}", std::io::Error::last_os_error())
                }
                let ret = unsafe { libc::shm_unlink(self.name.as_ptr().cast()) };
                if ret < 0 {
                    eprintln!("error: {}", std::io::Error::last_os_error())
                }
            }
            self.fd = -1;
        }
    }
    impl Shm {
        pub fn open(path: &str, len: usize) -> std::io::Result<Shm> {
            let name = std::ffi::CString::new(path).unwrap();
            let fd = unsafe {
                libc::shm_open(name.as_ptr().cast(), libc::O_RDWR | libc::O_CREAT, 0o600)
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let ret = unsafe { libc::ftruncate(fd, len.try_into().unwrap()) };
            if ret < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let buf = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            };
            if buf == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error());
            }
            let buf = buf.cast();
            Ok(Shm { name, fd, buf, len })
        }

        pub fn as_ref<'b>(&'b self) -> &'b [u32] {
            assert_eq!(self.buf.align_offset(std::mem::align_of::<u32>()), 0, "");
            unsafe {
                std::slice::from_raw_parts(self.buf.cast(), self.len / std::mem::size_of::<u32>())
            }
        }
        pub fn as_mut(&mut self) -> &mut [u32] {
            assert_eq!(self.buf.align_offset(std::mem::align_of::<u32>()), 0);
            unsafe {
                std::slice::from_raw_parts_mut(
                    self.buf.cast(),
                    self.len / std::mem::size_of::<u32>(),
                )
            }
        }
    }
}

mod pixbuf {
    use crate::shm;
    use crate::wl;
    use std::os::fd::FromRawFd;
    use std::os::fd::OwnedFd;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[derive(Debug)]
    pub struct ShmPixelBuffer {
        pub wl: wl::object<wl::wl_buffer::Obj>,
        pub locked: Arc<AtomicBool>,
        pub width: usize,
        pub height: usize,
        shm: crate::shm::Shm,
    }

    impl std::ops::Index<[usize; 2]> for ShmMut<'_> {
        type Output = u32;
        fn index(&self, [x, y]: [usize; 2]) -> &Self::Output {
            if x >= self.width || y >= self.height {
                panic!(
                    "index ({}, {}) out of bounds (0..{}, 0..{})",
                    x, y, self.width, self.height
                );
            }
            let buf: &[u32] = self.shm.as_ref();
            &buf[x + y * self.width]
        }
    }

    impl std::ops::IndexMut<[usize; 2]> for ShmMut<'_> {
        fn index_mut(&mut self, [x, y]: [usize; 2]) -> &mut Self::Output {
            if x >= self.width || y >= self.height {
                panic!(
                    "index ({}, {}) out of bounds (0..{}, 0..{})",
                    x, y, self.width, self.height
                );
            }
            let idx = x + y * self.width;
            let buf: &mut [u32] = self.shm.as_mut();
            &mut buf[idx]
        }
    }

    pub struct ShmMut<'b>(&'b mut ShmPixelBuffer);
    impl<'b> std::ops::Deref for ShmMut<'b> {
        type Target = ShmPixelBuffer;
        fn deref(&self) -> &ShmPixelBuffer {
            &(*self.0)
        }
    }
    impl<'b> std::ops::DerefMut for ShmMut<'b> {
        fn deref_mut(&mut self) -> &mut ShmPixelBuffer {
            &mut (*self.0)
        }
    }
    impl std::ops::Drop for ShmMut<'_> {
        fn drop(&mut self) {
            self.0
                .locked
                .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                .expect("shm should be locked, but wasn't");
        }
    }

    impl ShmPixelBuffer {
        pub fn new(
            conn: &mut wl::Client,
            shm: wl::object<wl::wl_shm::Obj>,
            name: &str,
            width: usize,
            height: usize,
        ) -> ShmPixelBuffer {
            let (pfmt, psz) = (wl::wl_shm::format::argb8888, 4);
            let shmbuf_sz = psz * width * height;
            let shmbuf = shm::Shm::open(name, shmbuf_sz).unwrap();
            let pool = conn.new_id();
            conn.send(
                shm,
                wl::wl_shm::request::create_pool {
                    id: pool,
                    // SAFETY: this fd will be consumed by converting it into raw fd and sending. The field will not be dropped, and the underlying fd will not be closed.
                    fd: unsafe { OwnedFd::from_raw_fd(shmbuf.fd) },
                    size: shmbuf_sz.try_into().unwrap(),
                },
            )
            .unwrap();
            let pool = pool.to_object();

            let pool_buf = conn.new_id();
            conn.send(
                pool,
                wl::wl_shm_pool::request::create_buffer {
                    id: pool_buf,
                    offset: 0,
                    width: width.try_into().unwrap(),
                    height: height.try_into().unwrap(),
                    stride: (width * psz).try_into().unwrap(),
                    format: pfmt.into(),
                },
            )
            .unwrap();
            let pool_buf = pool_buf.to_object();

            let locked = Arc::new(AtomicBool::new(false));
            let wlocked = Arc::downgrade(&locked);
            conn.listen(pool_buf, move |wl::wl_buffer::event::release {}| {
                if let Some(lock) = wlocked.upgrade() {
                    lock.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                        .expect("shm should be locked, but wasn't");
                }
                Ok(())
            });

            conn.send(pool, wl::wl_shm_pool::request::destroy {}).unwrap();

            ShmPixelBuffer { wl: pool_buf, locked, width, height, shm: shmbuf }
        }

        pub fn lock(&mut self) -> Option<ShmMut> {
            if self
                .locked
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return None;
            }
            Some(ShmMut(self))
        }
    }
}
