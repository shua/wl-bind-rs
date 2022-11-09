use std::env;
use std::fs;
use std::io::Write;
use std::path::Path;

fn parse_protoc<Src>(src: Src, reused: &mut Vec<(Vec<u8>, Vec<u8>)>) -> syn::File
where
    Src: AsRef<Path>,
{
    let protoc = wl_bindgen::parse(&fs::read(src.as_ref()).unwrap())
        .expect(&format!("parse {}", src.as_ref().display()));

    let protoc_rs = wl_bindgen::generate(
        &protoc,
        wl_bindgen::GenOptions {
            reused,
            ..wl_bindgen::GenOptions::default()
        },
    );
    let protoc_rs = syn::parse_file(&format!("{}", protoc_rs)).unwrap();
    for i in protoc.interfaces.iter() {
        reused.push((i.name.clone(), i.version.clone()));
    }
    protoc_rs
}

fn main() {
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let mut dst_file =
        fs::File::create(Path::new(&out_dir).join("binding.rs")).expect("create binding.rs");

    let maybe_wayland_xml = std::env::var("WAYLAND_XML");
    println!("cargo:rerun-if-env-changed=WAYLAND_XML");
    let wayland_xml = maybe_wayland_xml
        .as_ref()
        .map(String::as_str)
        .unwrap_or("/usr/share/wayland/wayland.xml");
    let mut used: Vec<(Vec<u8>, Vec<u8>)> = vec![];
    let protoc_rs = parse_protoc(wayland_xml, &mut used);
    write!(dst_file, "{}", prettyplease::unparse(&protoc_rs)).unwrap();
    println!("cargo:rerun-if-changed={wayland_xml}");

    let protoc_dir = "/usr/share/wayland-protocols";
    for subpath in ["stable", "staging", "unstable"] {
        for f in fs::read_dir(format!("{protoc_dir}/{subpath}")).unwrap() {
            let f = f.unwrap();
            for f in fs::read_dir(f.path()).unwrap() {
                let f = f.unwrap();
                let protoc_rs = parse_protoc(f.path(), &mut used);
                write!(dst_file, "{}", prettyplease::unparse(&protoc_rs)).unwrap();
            }
        }
    }
    println!("cargo:rerun-if-changed={protoc_dir}");

    let maybe_extra_protocs = std::env::var("WAYLAND_EXTRA_PROTOCOL_DIR");
    println!("cargo:rerun-if-env-changed=WAYLAND_EXTRA_PROTOCOL_DIR");
    if let Ok(path) = maybe_extra_protocs {
        println!("cargo:rerun-if-changed={path}");
        for f in fs::read_dir(path).unwrap() {
            let f = f.unwrap();
            let protoc_rs = parse_protoc(f.path(), &mut used);
            write!(dst_file, "{}", prettyplease::unparse(&protoc_rs)).unwrap();
        }
    }
}
