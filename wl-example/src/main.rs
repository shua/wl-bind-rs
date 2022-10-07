use wl;

fn main() {
    let mut cl = unsafe { wl::default_client() }.unwrap().borrow_mut();
    let display = cl.display();
    display
        .get_registry(&mut cl, wl::WlRegistry { on_event: None })
        .unwrap();
}
