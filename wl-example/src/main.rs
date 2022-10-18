use wl;

fn main() {
    let cl = wl::WlClient::new().unwrap();
    let cl = cl.borrow();

    cl.poll(true).unwrap();
    let shm = cl.bind_global(wl::WlShm::new(|me, cl, e| match e {
        wl::wl_shm::Event::Format { format } => todo!("handle shm format"),
    }));
    cl.poll(true).unwrap();
    cl.poll(true).unwrap();
    cl.poll(true).unwrap();
}
