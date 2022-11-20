mod wl {
    wl_macro::wayland_protocol!("/usr/share/wayland/wayland.xml");
}

include!(1);

fn main() {
    println!("Hello, world!");
}
