use std::cell::RefCell;
use std::rc::Rc;
use wl;

fn main() {
    let cl = wl::WlClient::new().unwrap();

    let shm_fmts = Rc::new(RefCell::new(vec![]));
    let cb_shm_fmts = shm_fmts.clone();
    let globals = Rc::new(RefCell::new((
        None::<wl::WlRef<wl::WlCompositor>>,
        None::<wl::WlRef<wl::XdgWmBase>>,
        None::<wl::WlRef<wl::WlShm>>,
    )));
    let cb_globals = globals.clone();
    cl.display
        .get_registry(wl::WlRegistry::new(move |reg, e| match e {
            wl::wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                use wl::InterfaceDesc;
                let mut gs = cb_globals.borrow_mut();
                if interface.as_c_str() == wl::WlCompositor::NAME {
                    gs.0.replace(reg.bind(name, (wl::WlCompositor::new(|_, _| Ok(())), version)));
                } else if interface.as_c_str() == wl::XdgWmBase::NAME {
                    gs.1.replace(reg.bind(
                        name,
                        (
                            wl::XdgWmBase::new(|base, e| match e {
                                wl::xdg_wm_base::Event::Ping { serial } => {
                                    base.pong(serial);
                                    Ok(())
                                }
                            }),
                            version,
                        ),
                    ));
                } else if interface.as_c_str() == wl::WlShm::NAME {
                    let cb_shm_fmts = cb_shm_fmts.clone();
                    gs.2.replace(reg.bind(
                        name,
                        (
                            wl::WlShm::new(move |_, e| match e {
                                wl::wl_shm::Event::Format { format } => {
                                    cb_shm_fmts.borrow_mut().push(format);
                                    Ok(())
                                }
                            }),
                            version,
                        ),
                    ));
                }
                Ok(())
            }
            _ => Ok(()),
        }));

    cl.sync();

    let (compositor, wm_base, shm) = {
        let mut gs = globals.borrow_mut();
        (
            gs.0.take().expect("compositor is required"),
            gs.1.take().expect("wm_base is required"),
            gs.2.take().expect("shm is required"),
        )
    };

    if !shm_fmts.borrow().contains(&wl::wl_shm::Format::Argb8888) {
        panic!("server must support arg8888");
    }

    let pbuf = pixbuf::ShmPixelBuffer::new(shm, 300, 200);

    let wl_surface = compositor.create_surface(wl::WlSurface::new(|_, e| match e {
        wl::wl_surface::Event::Enter { output } => Ok(()),
        wl::wl_surface::Event::Leave { output } => Ok(()),
    }));
    let surface = wm_base.get_xdg_surface(
        wl::XdgSurface::new(move |me, e| match e {
            wl::xdg_surface::Event::Configure { serial, .. } => {
                me.ack_configure(serial);
                Ok(())
            }
        }),
        wl_surface.clone(),
    );

    let pos = wm_base.create_positioner(wl::XdgPositioner::new(|_, e| match e {}));
    pos.set_gravity(wl::xdg_positioner::Gravity::TopRight);
    pos.set_size(100, 50);
    pos.set_anchor_rect(0, 0, 100, 50);

    let close = Rc::new(RefCell::new(false));
    let cb_close = close.clone();
    let popup = surface.get_popup(
        wl::XdgPopup::new(move |_, e| match e {
            wl::xdg_popup::Event::Configure { .. } => Ok(()),
            wl::xdg_popup::Event::PopupDone {} => {
                *cb_close.borrow_mut() = true;
                Ok(())
            }
            wl::xdg_popup::Event::Repositioned { .. } => Ok(()),
        }),
        None,
        pos.clone(),
    );
    wl_surface.commit();
    cl.sync();
    pos.destroy();

    while !*close.borrow() {
        cl.poll(true);
    }
}

mod pixbuf {
    use super::{Rc, RefCell};
    use std::os::unix::io::RawFd;

    #[derive(Debug)]
    pub struct ShmPixelBuffer {
        pub wl: wl::WlRef<wl::WlBuffer>,
        pub locked: bool,
        pub width: usize,
        pub height: usize,
        addr: *mut u32,
    }

    impl std::ops::Index<(usize, usize)> for ShmPixelBuffer {
        type Output = u32;
        fn index(&self, (x, y): (usize, usize)) -> &Self::Output {
            if x >= self.width || y >= self.height {
                panic!(
                    "index ({}, {}) out of bounds (0..{}, 0..{})",
                    x, y, self.width, self.height
                );
            }
            unsafe {
                self.addr
                    .offset((x + y * self.width) as isize)
                    .as_ref()
                    .unwrap()
            }
        }
    }

    impl std::ops::IndexMut<(usize, usize)> for ShmPixelBuffer {
        fn index_mut(&mut self, (x, y): (usize, usize)) -> &mut Self::Output {
            if x >= self.width || y >= self.height {
                panic!(
                    "index ({}, {}) out of bounds (0..{}, 0..{})",
                    x, y, self.width, self.height
                );
            }
            unsafe {
                self.addr
                    .offset((x + y * self.width) as isize)
                    .as_mut()
                    .unwrap()
            }
        }
    }

    fn shmbuf(buf_len: usize) -> (RawFd, *mut std::ffi::c_void) {
        // SAFETY: constant has nul byte
        let tmpname = unsafe {
            std::ffi::CString::from_vec_with_nul_unchecked(
                b"/dev/shm/wl_example_buf_XXXXXX\0".to_vec(),
            )
        };
        // SAFETY: tmpname is heap-alloc'd mutable value
        let tmpfd = unsafe { libc::mkstemp(tmpname.as_ptr() as *mut i8) };
        if tmpfd < 0 {
            // SAFETY: just C ffi
            unsafe {
                libc::perror(b"mkstemp\0".as_ptr() as *const i8);
                libc::unlink(tmpname.as_ptr());
            }
            panic!("mkstemp failed");
        }
        // SAFETY: just C ffi
        if unsafe { libc::unlink(tmpname.as_ptr()) } < 0 {
            unsafe { libc::perror(b"unlink\0".as_ptr() as *const i8) };
            panic!("unlink failed");
        }

        // SAFETY: just C ffi, tmpfd is valid fd from mkstemp
        if unsafe { libc::ftruncate(tmpfd, buf_len.try_into().unwrap()) } < 0 {
            unsafe { libc::perror(b"ftruncate\0".as_ptr() as *const i8) };
            panic!("ftruncate failed");
        }

        // SAFETY: just C ffi, tmpfd is valid fd from mkstemp
        let shmbuf = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                buf_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                tmpfd,
                0,
            )
        };
        if shmbuf == libc::MAP_FAILED {
            // SAFETY: just C ffi
            unsafe {
                libc::perror(b"mmap\0".as_ptr() as *const i8);
                libc::close(tmpfd);
            }
            panic!("mmap failed");
        }

        (RawFd::from(tmpfd), shmbuf)
    }

    impl ShmPixelBuffer {
        pub fn new(
            shm: wl::WlRef<wl::WlShm>,
            width: usize,
            height: usize,
        ) -> Rc<RefCell<ShmPixelBuffer>> {
            let (pfmt, psz) = (wl::wl_shm::Format::Argb8888, 4);
            let shmbuf_sz = psz * width * height;
            let (shmfd, shmbuf) = shmbuf(shmbuf_sz);
            let pool = shm.create_pool(
                wl::WlShmPool::new(|_, e| match e {}),
                shmfd.into(),
                shmbuf_sz.try_into().unwrap(),
            );

            let ret = Rc::new_cyclic(|rc| {
                let rc = rc.clone();
                let pool_buf = pool.create_buffer(
                    wl::WlBuffer::new(move |_, e| match e {
                        wl::wl_buffer::Event::Release {} => {
                            let rc: Option<Rc<RefCell<ShmPixelBuffer>>> = rc.upgrade();
                            if let Some(buf) = rc {
                                buf.borrow_mut().locked = false;
                            }
                            Ok(())
                        }
                    }),
                    0,
                    width.try_into().unwrap(),
                    height.try_into().unwrap(),
                    (width * psz).try_into().unwrap(),
                    pfmt,
                );

                RefCell::new(ShmPixelBuffer {
                    wl: pool_buf,
                    locked: false,
                    width,
                    height,
                    addr: shmbuf as *mut u32,
                })
            });
            pool.destroy();

            ret
        }
    }
}
