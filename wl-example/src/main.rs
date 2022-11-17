use std::cell::RefCell;
use std::rc::Rc;
use wl;

struct InitData<const GLOBALS: usize> {
    versions: [(&'static str, u32, u32, u32); GLOBALS],
    shm_has_argb8888: bool,
    has_pointer_caps: bool,
}
impl<const GLOBALS: usize> InitData<GLOBALS> {
    fn new(versions: [(&'static str, u32, u32, u32); GLOBALS]) -> Self {
        InitData {
            versions,
            shm_has_argb8888: false,
            has_pointer_caps: false,
        }
    }
}

impl<const GLOBALS: usize> wl::wl_registry::EventHandler for InitData<GLOBALS> {
    fn on_global(
        &mut self,
        _: wl::WlRef<wl::WlRegistry>,
        name: u32,
        interface: String,
        version: u32,
    ) -> Result<(), wl::WlHandleError> {
        if let Some((_, n, v, _)) = self
            .versions
            .iter_mut()
            .find(|(i, _, vl, vr)| *i == interface && (*vl..=*vr).contains(&version))
        {
            *n = name;
            *v = version;
        }
        Ok(())
    }
}

impl<const GLOBALS: usize> wl::wl_shm::EventHandler for InitData<GLOBALS> {
    fn on_format(
        &mut self,
        _: wl::WlRef<wl::WlShm>,
        format: wl::wl_shm::Format,
    ) -> Result<(), wl::WlHandleError> {
        if format == wl::wl_shm::Format::Argb8888 {
            self.shm_has_argb8888 = true;
        }
        Ok(())
    }
}

impl<const GLOBALS: usize> wl::wl_seat::EventHandler for InitData<GLOBALS> {
    fn on_capabilities(
        &mut self,
        _: wl::WlRef<wl::WlSeat>,
        capabilities: wl::wl_seat::Capability,
    ) -> Result<(), wl::WlHandleError> {
        if (wl::wl_seat::Capability::POINTER & u32::from(capabilities)) != 0 {
            self.has_pointer_caps = true;
        }
        Ok(())
    }
}

#[derive(Default)]
struct App {
    done: bool,
    surface_configured: bool,
    dirty: bool,
}

fn main() {
    let cl = wl::WlClient::new().unwrap();

    use wl::InterfaceDesc;
    let idata = cl.ids.new_resource(InitData::new([
        (wl::WlCompositor::NAME, 0, 4, 4),
        (wl::WlSeat::NAME, 0, 7, 7),
        (wl::WlShm::NAME, 0, 1, 1),
        (wl::ZwlrLayerShellV1::NAME, 0, 2, 5),
    ]));
    let reg = wl::WlRegistry::new(idata);
    let reg = cl.display.get_registry(&cl.ids, reg);

    cl.sync();
    let (compositor, seat, shm, layer_shell) = {
        let gs = &cl.ids.resource(idata).versions;
        let (mut compositor, mut seat, mut shm, mut layer_shell) = (None, None, None, None);
        for (iface, name, version, _) in gs.iter().filter(|(_, n, _, _)| *n != 0) {
            match *iface {
                wl::WlCompositor::NAME => {
                    compositor = Some(reg.bind(&cl.ids, *name, (wl::WlCompositor, *version)));
                }
                wl::WlSeat::NAME => {
                    seat = Some(reg.bind(&cl.ids, *name, (wl::WlSeat::new(idata), *version)));
                }
                wl::WlShm::NAME => {
                    shm = Some(reg.bind(&cl.ids, *name, (wl::WlShm::new(idata), *version)));
                }
                wl::ZwlrLayerShellV1::NAME => {
                    layer_shell = Some(reg.bind(&cl.ids, *name, (wl::ZwlrLayerShellV1, *version)));
                }
                _ => {}
            }
        }
        (
            compositor.expect("no suitable wl_compositor found"),
            seat.expect("no suitable wl_seat found"),
            shm.expect("no suitable wl_shm found"),
            layer_shell.expect("no suitable zwlr_layer_shell found"),
        )
    };

    cl.sync();
    assert!(
        cl.ids.resource(idata).shm_has_argb8888,
        "server must support argb8888"
    );
    let pbuf = pixbuf::ShmPixelBuffer::new(&cl.ids, shm, 300, 200);
    cl.ids.resource_mut(pbuf).clear(0xffff9900);
    let app = cl.ids.new_resource(App::default());

    let pointer = seat.get_pointer(
        &cl.ids,
        wl::WlPointer::from_fn(move |_, ids, e| match e {
            wl::wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                ids.resource_mut(pbuf).clear(0xffff9900);
                ids.resource_mut(app).dirty = true;
                Ok(())
            }
            wl::wl_pointer::Event::Leave { serial, surface } => {
                ids.resource_mut(pbuf).clear(0xff444444);
                ids.resource_mut(app).dirty = true;
                Ok(())
            }
            _ => Ok(()),
        }),
    );
    let wl_surface = compositor.create_surface(
        &cl.ids,
        wl::WlSurface::from_fn(move |_, ids, e| match e {
            wl::wl_surface::Event::Enter { .. } => {
                ids.resource_mut(pbuf).clear(0xffff9900);
                ids.resource_mut(app).dirty = true;
                Ok(())
            }
            wl::wl_surface::Event::Leave { .. } => {
                ids.resource_mut(pbuf).clear(0xff444444);
                ids.resource_mut(app).dirty = true;
                Ok(())
            }
        }),
    );
    let layer_surface = layer_shell.get_layer_surface(
        &cl.ids,
        wl::ZwlrLayerSurfaceV1::from_fn(move |sh, ids, e| match e {
            wl::zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                ids.resource_mut(app).surface_configured = true;
                sh.ack_configure(ids, serial);
                Ok(())
            }
            wl::zwlr_layer_surface_v1::Event::Closed {} => {
                ids.resource_mut(app).done = true;
                Ok(())
            }
        }),
        wl_surface.clone(),
        None,
        wl::zwlr_layer_shell_v1::Layer::Overlay,
        wl::WlStr::from("ns"),
    );
    layer_surface.set_size(&cl.ids, 300, 200);
    wl_surface.commit(&cl.ids);
    cl.sync();
    assert!(
        cl.ids.resource(app).surface_configured,
        "layer should have configured"
    );
    wl_surface.attach(&cl.ids, Some(pbuf.borrow(&cl.ids).wl.clone()), 0, 0);
    wl_surface.commit(&cl.ids);
    cl.sync();

    while !cl.ids.resource(app).done {
        if cl.ids.resource(app).dirty {
            wl_surface.commit(&cl.ids);
        }
        cl.poll(true);
    }
}

mod pixbuf {
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

    impl wl::wl_buffer::EventHandler for ShmPixelBuffer {
        fn on_release(&mut self, me: wl::WlRef<wl::WlBuffer>) -> Result<(), wl::WlHandleError> {
            self.locked = false;
            Ok(())
        }
    }

    impl ShmPixelBuffer {
        pub fn new(
            ids: &wl::IdSpace,
            shm: wl::WlRef<wl::WlShm>,
            width: usize,
            height: usize,
        ) -> wl::IdResource<ShmPixelBuffer> {
            let (pfmt, psz) = (wl::wl_shm::Format::Argb8888, 4);
            let shmbuf_sz = psz * width * height;
            let (shmfd, shmbuf) = shmbuf(shmbuf_sz);
            let pool = shm.create_pool(
                ids,
                wl::WlShmPool,
                shmfd.into(),
                shmbuf_sz.try_into().unwrap(),
            );

            let ret = ids.new_resource_cyclic(|buf: wl::IdResource<ShmPixelBuffer>| {
                let pool_buf = pool.create_buffer(
                    ids,
                    wl::WlBuffer::new(buf),
                    0,
                    width.try_into().unwrap(),
                    height.try_into().unwrap(),
                    (width * psz).try_into().unwrap(),
                    pfmt,
                );

                let ret = ShmPixelBuffer {
                    wl: pool_buf,
                    locked: false,
                    width,
                    height,
                    addr: shmbuf as *mut u32,
                };
                ret
            });
            pool.destroy(ids);

            ret
        }

        pub fn clear(&mut self, color: u32) {
            for i in 0..(self.width * self.height) {
                unsafe {
                    *self.addr.offset(i.try_into().unwrap()).as_mut().unwrap() = color;
                }
            }
        }
    }
}
