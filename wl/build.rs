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
    let dst_file = fs::File::create(Path::new(&out_dir).join("binding.rs"))
        .map_err(|e| format!("create binding.rs: {e:?}"))?;
    let mut fmt_cmd = std::process::Command::new("rustfmt")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::from(dst_file))
        .spawn()
        .unwrap();
    write!(fmt_cmd.stdin.as_ref().unwrap(), "{}", protoc_rs).unwrap();
    let status = fmt_cmd.wait().unwrap();
    if !status.success() {
        return Err(format!("rustfmt: exited with {}", status.code().unwrap()));
    }
    println!("cargo:rerun-if-changed=\"src/wayland.xml\"");
    Ok(())
}
