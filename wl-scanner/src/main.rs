use prettyplease;
use syn;
use wl_bindgen;

fn usage() {
    println!("usage: wl-scanner [-o OUTFILE] INFILES...");
}

macro_rules! perr {
    ($fmt:literal $($rest:tt)*) => { {
        eprintln!($fmt $($rest)*);
        std::process::exit(1);
    } }
}

fn parse<P: AsRef<std::path::Path>>(path: &P, protocols: &mut Vec<wl_bindgen::Protocol>) {
    fn parse_file(
        path: &std::path::Path,
        protocols: &mut Vec<wl_bindgen::Protocol>,
    ) -> std::io::Result<()> {
        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                parse_file(&entry?.path(), protocols)?;
            }
        } else {
            let src = std::fs::read(path)?;
            let proto = wl_bindgen::parse(&src)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
            println!("parse {}", path.display());
            protocols.push(proto);
        }
        Ok(())
    }

    match parse_file(path.as_ref(), protocols) {
        Ok(()) => {}
        Err(e) => perr!("parse {}: {}", path.as_ref().display(), e),
    }
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let mut i = 0;
    let mut outfile = "wl.rs";
    let mut protocols: Vec<wl_bindgen::Protocol> = Vec::with_capacity(args.len());
    while i < args.len() {
        if args[i] == "-o" {
            i += 1;
            outfile = &args[i];
        } else if args[i].starts_with("-") {
            usage();
            return;
        } else {
            parse(&args[i], &mut protocols);
        }
        i += 1;
    }
    let mut outfile = if outfile == "-" {
        use std::os::fd::FromRawFd;
        unsafe { std::fs::File::from_raw_fd(1i32) }
    } else {
        match std::fs::File::create(outfile) {
            Ok(f) => f,
            Err(e) => perr!("create {outfile}: {e}"),
        }
    };

    use std::io::Write;
    let headers = wl_bindgen::generate2::header();
    let src = syn::parse_file(headers.to_string().as_str()).unwrap();
    let src = prettyplease::unparse(&src);
    match outfile.write(src.as_bytes()) {
        Ok(_) => {}
        Err(e) => perr!("write src: {e}"),
    }

    for p in protocols {
        let p = wl_bindgen::generate2::generate(&p);
        let src = syn::parse_file(p.to_string().as_str()).unwrap();
        let src = prettyplease::unparse(&src);
        match outfile.write(src.as_bytes()) {
            Ok(_) => {}
            Err(e) => perr!("write src: {e}"),
        }
    }
}
