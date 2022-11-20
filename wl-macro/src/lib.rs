use proc_macro::TokenStream;
use std::fs;
use wl_bindgen::parse;

#[proc_macro]
pub fn wayland_protocol(name: TokenStream) -> TokenStream {
    let mut name = name.into_iter();
    let name0 = match name.next() {
        Some(n) => n,
        None => panic!("must provide at least one argument"),
    };
    let name = match name0 {
        proc_macro::TokenTree::Literal(lit) => lit,
        _ => panic!("argument must be a string literal"),
    };
    let name = name.to_string();
    if !(name.starts_with('"') && name.ends_with('"')) {
        panic!("argument must be a string literal");
    }
    let name = &name[1..(name.len() - 1)];

    let wayland_xml = match fs::read(name) {
        Ok(f) => f,
        Err(err) => panic!("wayland_protocol({}): {:?});", name, err),
    };
    let wayland = parse(&wayland_xml);
    todo!()
}

#[proc_macro]
pub fn wayland_protocol_dir(name: TokenStream) -> TokenStream {
    todo!()
}
