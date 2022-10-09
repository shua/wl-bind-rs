use wl;

fn main() {
    let mut cl = wl::WlClient::new().unwrap();

    loop {
        while let Ok(None) = cl.poll() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        while let Ok(Some(_)) = cl.poll() {}

        let shm = cl.bind_global(wl::WlShm::new(|req, e| match e {
            wl::wl_shm::Event::Format { format } => todo!(),
        }));
    }
}
