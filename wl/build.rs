use std::env;
use std::fs;
use std::io::Write;
use std::path::Path;

fn main() -> Result<(), String> {
    let protoc_file =
        fs::File::open("src/wayland.xml").map_err(|e| format!("open src/wayland.xml: {e:?}"))?;
    let protoc_rs = wl_bindgen::gen_bindings(protoc_file)
        .map_err(|e| format!("parse src/wayland.xml: {e:?}"))?;
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let mut dst_file = fs::File::create(Path::new(&out_dir).join("binding.rs"))
        .map_err(|e| format!("create binding.rs: {e:?}"))?;

    let protoc_rs = syn::parse_file(&format!("{}", protoc_rs)).unwrap();
    write!(dst_file, "{}", prettyplease::unparse(&protoc_rs)).unwrap();
    println!("cargo:rerun-if-changed=\"src/wayland.xml\"");
    Ok(())
}
