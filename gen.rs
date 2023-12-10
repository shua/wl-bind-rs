#![allow(dead_code)]

use quick_xml::events::Event as xmlEvent;
use std::io::Write;

macro_rules! errexit {
    ($fmt:literal $(, $arg:expr)* $(,)?) => {{ eprintln!($fmt $(, $arg)*); std::process::exit(1); }}
}

macro_rules! emit {
    ($($t:tt)*) => {{
        write!($($t)*).unwrap()
    }};
}
macro_rules! emitln {
    ($($t:tt)*) => {{
        writeln!($($t)*).unwrap()
    }};
}

#[derive(Debug, Clone, Default)]
struct Protocol {
    name: String,
    copyright: String,
    description: Description,
    interfaces: Vec<Interface>,
}

#[derive(Debug, Clone, Default)]
struct Description {
    summary: String,
    body: String,
}

#[derive(Debug, Clone, Default)]
struct Interface {
    name: String,
    version: u32,
    description: Description,
    requests: Vec<Message>,
    events: Vec<Message>,
    enums: Vec<Enum>,
}

#[derive(Debug, Clone, Default)]
struct Message {
    name: String,
    r#type: String,
    description: Description,
    args: Vec<Arg>,
}

#[derive(Debug, Clone, Default)]
struct Enum {
    name: String,
    description: Description,
    bitfield: bool,
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, Default)]
struct Arg {
    name: String,
    description: Description,
    r#type: String,
    r#enum: String,
    interface: String,
    nullable: bool,
}

#[derive(Debug, Clone, Default)]
struct Entry {
    name: String,
    description: Description,
    value: i32,
}

struct Rdr {
    xml: quick_xml::Reader<std::io::BufReader<std::fs::File>>,
    buf: Vec<u8>,
}

fn get_attr(bs: &quick_xml::events::BytesStart, name: &str) -> Option<String> {
    let attr = bs.try_get_attribute(name).ok().flatten();
    attr.map(|a| a.unescape_value().unwrap().to_string())
}

fn read_protocol(rdr: &mut Rdr, name: String) -> Protocol {
    assert!(!name.is_empty(), "protocol must have a name");
    let mut copyright = String::new();
    let description = Description::default();
    let mut interfaces = vec![];
    loop {
        let evt = rdr.xml.read_event_into(&mut rdr.buf);
        let evt = evt.expect("read next xml event");
        match evt {
            xmlEvent::Start(start) => match start.local_name().as_ref() {
                b"copyright" => copyright = read_copyright(rdr),
                b"interface" => {
                    let iname = get_attr(&start, "name").expect("interface must have a name");
                    let iversion =
                        get_attr(&start, "version").expect("interface must have version");
                    interfaces.push(read_interface(rdr, iname, iversion));
                }
                tag => errexit!(
                    "protocol: unexpected tag:{}: {:?}",
                    rdr.xml.buffer_position(),
                    std::str::from_utf8(tag)
                ),
            },
            xmlEvent::End(end) => {
                if end.local_name().as_ref() == b"protocol" {
                    break;
                }
                errexit!(
                    "protocol: unexpected end tag:{}: {:?}",
                    rdr.xml.buffer_position(),
                    std::str::from_utf8(end.local_name().as_ref())
                );
            }
            xmlEvent::Comment(_) => {}
            _ => {}
        }
    }
    Protocol { name, copyright, description, interfaces }
}
fn read_copyright(rdr: &mut Rdr) -> String {
    let mut copyright = String::new();
    loop {
        match rdr.xml.read_event_into(&mut rdr.buf).unwrap() {
            xmlEvent::Comment(_) => {}
            xmlEvent::Text(txt) if txt.iter().all(|b| b.is_ascii_whitespace()) => {}
            xmlEvent::End(end) if end.local_name().as_ref() == b"copyright" => return copyright,
            xmlEvent::Text(txt) => {
                copyright += &txt.unescape().unwrap();
            }
            xmlEvent::CData(txt) => {
                copyright += std::str::from_utf8(txt.as_ref()).unwrap();
            }
            evt => {
                errexit!("copyright: unexpected xml event:{}: {evt:?}", rdr.xml.buffer_position())
            }
        }
    }
}
fn read_interface(rdr: &mut Rdr, name: String, version: String) -> Interface {
    let Ok(version) = u32::from_str_radix(&version, 10) else {
        errexit!("interface:{name}: version must be u32:{}: {version}", rdr.xml.buffer_position())
    };
    let mut description = Description::default();
    let mut events = vec![];
    let mut requests = vec![];
    let mut enums = vec![];
    loop {
        match rdr.xml.read_event_into(&mut rdr.buf).unwrap() {
            xmlEvent::Comment(_) => {}
            xmlEvent::Text(txt) if txt.iter().all(|b| b.is_ascii_whitespace()) => {}
            xmlEvent::Start(start) => match start.local_name().as_ref() {
                b"description" => {
                    let summary = get_attr(&start, "summary");
                    description = read_description(rdr, summary.unwrap_or_default());
                }
                b"event" => {
                    let name = get_attr(&start, "name");
                    let name = name.expect("event must have name");
                    let r#type = get_attr(&start, "type").unwrap_or_default();
                    events.push(read_message(rdr, "event", name, r#type))
                }
                b"request" => {
                    let name = get_attr(&start, "name");
                    let name = name.expect("request must have name");
                    let r#type = get_attr(&start, "type").unwrap_or_default();
                    requests.push(read_message(rdr, "request", name, r#type));
                }
                b"enum" => {
                    let name = get_attr(&start, "name");
                    let name = name.expect("request must have name");
                    let bitfield = get_attr(&start, "bitfield");
                    enums.push(read_enum(rdr, name, bitfield.unwrap_or_default()))
                }
                _ => errexit!(
                    "interface:{name}: unexpected xml event:{}: {start:?}",
                    rdr.xml.buffer_position()
                ),
            },
            xmlEvent::End(end) if end.local_name().as_ref() == b"interface" => {
                return Interface { name, version, description, requests, events, enums }
            }
            evt => errexit!(
                "interface:{name}: unxpected xml event:{}: {evt:?}",
                rdr.xml.buffer_position()
            ),
        }
    }
}
fn read_description(rdr: &mut Rdr, summary: String) -> Description {
    let mut body = String::new();
    loop {
        match rdr.xml.read_event_into(&mut rdr.buf).unwrap() {
            xmlEvent::Comment(_) => {}
            xmlEvent::Text(txt) if txt.iter().all(|b| b.is_ascii_whitespace()) => {}
            xmlEvent::Text(txt) => body += &txt.unescape().unwrap(),
            xmlEvent::CData(cdata) => {
                let txt = std::str::from_utf8(cdata.as_ref()).unwrap();
                body += txt;
            }
            xmlEvent::End(end) if end.local_name().as_ref() == b"description" => {
                return Description { summary, body }
            }
            evt => {
                errexit!("description: unexpected xml event:{}: {evt:?}", rdr.xml.buffer_position())
            }
        }
    }
}
fn read_message(rdr: &mut Rdr, end: &str, name: String, r#type: String) -> Message {
    let mut description = Description::default();
    let mut args = vec![];
    loop {
        match rdr.xml.read_event_into(&mut rdr.buf).unwrap() {
            xmlEvent::Comment(_) => {}
            xmlEvent::Text(txt) if txt.iter().all(|b| b.is_ascii_whitespace()) => {}
            xmlEvent::Start(start) => match start.local_name().as_ref() {
                b"arg" => {
                    let aname = get_attr(&start, "name").expect("arg must have name");
                    let atype = get_attr(&start, "type").expect("arg must have type");
                    let aenum = get_attr(&start, "enum").unwrap_or_default();
                    let aifac = get_attr(&start, "interface").unwrap_or_default();
                    let nullable = get_attr(&start, "allow-null").unwrap_or_default();
                    let summary = get_attr(&start, "summary").unwrap_or_default();
                    args.push(read_arg(rdr, aname, atype, aenum, aifac, nullable, summary))
                }
                b"description" => {
                    let summary = get_attr(&start, "summary");
                    description = read_description(rdr, summary.unwrap_or_default());
                }
                _ => errexit!("{end}:{name}: unexpected xml start: {start:?}"),
            },
            xmlEvent::End(bend) if bend.local_name().as_ref() == end.as_bytes() => {
                return Message { name, r#type, description, args }
            }
            evt => errexit!(
                "{end}:{name}: unexpected xml event:{}: {evt:?}",
                rdr.xml.buffer_position()
            ),
        }
    }
}

fn read_arg(
    rdr: &mut Rdr,
    name: String,
    r#type: String,
    r#enum: String,
    iface: String,
    allow_null: String,
    summary: String,
) -> Arg {
    let description = Description { summary, body: String::new() };
    match r#type.as_str() {
        "int" | "uint" | "fixed" | "object" | "new_id" | "string" | "array" | "fd" | "enum" => {}
        _ => errexit!("arg:{name}: unexpected type: {type}"),
    }
    loop {
        match rdr.xml.read_event_into(&mut rdr.buf).unwrap() {
            xmlEvent::Comment(_) => {}
            xmlEvent::Text(txt) if txt.iter().all(|b| b.is_ascii_whitespace()) => {}
            xmlEvent::End(end) if end.local_name().as_ref() == b"arg" => {
                return Arg {
                    name,
                    description,
                    r#type,
                    r#enum,
                    interface: iface,
                    nullable: allow_null == "true",
                }
            }
            evt => {
                errexit!("arg:{name}: unexpected xml event:{}: {evt:?}", rdr.xml.buffer_position())
            }
        }
    }
}
fn read_enum(rdr: &mut Rdr, name: String, bitfield: String) -> Enum {
    let mut description = Description::default();
    let mut entries = vec![];
    loop {
        match rdr.xml.read_event_into(&mut rdr.buf).unwrap() {
            xmlEvent::Comment(_) => {}
            xmlEvent::Text(txt) if txt.iter().all(|b| b.is_ascii_whitespace()) => {}
            xmlEvent::Start(start) => match start.local_name().as_ref() {
                b"description" => {
                    let summary = get_attr(&start, "summary").unwrap_or_default();
                    description = read_description(rdr, summary);
                }
                b"entry" => {
                    let ename = get_attr(&start, "name").expect("entry must have name");
                    let value = get_attr(&start, "value").expect("entry must have value");
                    let summary = get_attr(&start, "summary").unwrap_or_default();
                    entries.push(read_entry(rdr, ename, value, summary))
                }
                _ => errexit!(
                    "enum:{name}: unexpected xml start event:{}: {start:?}",
                    rdr.xml.buffer_position()
                ),
            },
            xmlEvent::End(end) if end.local_name().as_ref() == b"enum" => {
                return Enum { name, description, bitfield: bitfield == "true", entries }
            }
            evt => {
                errexit!("enum:{name}: unexpected xml event:{}: {evt:?}", rdr.xml.buffer_position())
            }
        }
    }
}
fn read_entry(rdr: &mut Rdr, name: String, value: String, summary: String) -> Entry {
    let ivalue: i32;
    if value.starts_with("0x") {
        ivalue = i32::from_str_radix(&value[2..], 16).unwrap();
    } else {
        ivalue = i32::from_str_radix(&value, 10).unwrap();
    }
    let mut description = Description { summary, body: String::new() };
    loop {
        match rdr.xml.read_event_into(&mut rdr.buf).unwrap() {
            xmlEvent::Comment(_) => {}
            xmlEvent::Text(txt) if txt.iter().all(|b| b.is_ascii_whitespace()) => {}
            xmlEvent::Start(start) if start.local_name().as_ref() == b"description" => {
                let summary = get_attr(&start, "summary").unwrap_or_default();
                description = read_description(rdr, summary);
            }
            xmlEvent::End(end) if end.local_name().as_ref() == b"entry" => {
                return Entry { name, description, value: ivalue }
            }
            evt => errexit!(
                "entry:{name}: unexpected xml event:{}: {evt:?}",
                rdr.xml.buffer_position()
            ),
        }
    }
}

struct Gen<'g> {
    out: &'g mut dyn Write,
}

impl Write for Gen<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.out.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

const HEADER: &'static str = include_str!("base.rs");

fn gen_protocol(g: &mut Gen, p: &Protocol) {
    for iface in p.interfaces.iter() {
        gen_iface(g, iface);
    }
}

fn gen_doccomment(g: &mut Gen, Description { summary, body }: &Description) {
    if !summary.is_empty() {
        for line in summary.lines() {
            emitln!(g, "/// {}", line.trim());
        }
    }
    for line in body.lines() {
        emitln!(g, "/// {}", line.trim());
    }
}

fn gen_iface(
    g: &mut Gen,
    Interface { name, version, description, requests, events, enums }: &Interface,
) {
    gen_doccomment(g, description);
    emitln!(g, "pub mod {name} {{");
    emitln!(g, "#![allow(non_camel_case_types)]");
    emitln!(g, "#[allow(unused_imports)]");
    emitln!(g, "use super::*;");
    emitln!(g, "pub const NAME: &'static str = {name:?};");
    emitln!(g, "pub const VERSION: u32 = {version};");
    emitln!(g, "pub mod request {{");
    emitln!(g, "#[allow(unused_imports)]");
    emitln!(g, "use super::*;");
    for (i, req) in requests.iter().enumerate() {
        gen_message_struct(g, req, i);
        emitln!(g, "impl Request<Obj> for {} {{}}", Ident(&req.name));
    }
    emitln!(g, "}}");
    emitln!(g, "pub mod event {{");
    emitln!(g, "#[allow(unused_imports)]");
    emitln!(g, "use super::*;");
    for (i, evt) in events.iter().enumerate() {
        gen_message_struct(g, evt, i);
        emitln!(g, "impl Event<Obj> for {} {{}}", Ident(&evt.name));
    }
    emitln!(g, "}}");
    for enm in enums {
        if enm.bitfield {
            gen_bitfield(g, enm);
        } else {
            gen_enum(g, enm);
        }
    }

    emitln!(
        g,
        r#"
#[derive(Debug,Clone,Copy)]
pub struct {name};
pub type Obj = {name};

#[allow(non_upper_case_globals)]
pub const Obj: Obj = {name};

impl Dsr for Obj {{
   fn deserialize(_: &[u8], _: &mut Vec<OwnedFd>) -> Result<(Self, usize),  &'static str> {{ Ok((Obj, 0)) }}
}}

impl Ser for Obj {{
    fn serialize(self, _: &mut [u8], _: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {{ Ok(0) }}
}}

impl Object for Obj {{
    const DESTRUCTOR: Option<u16> = {destructor:?};
    fn new_obj() -> Self {{ Obj }}
    fn new_dyn(version: u32) -> Dyn {{ Dyn {{ name: NAME.to_string(), version }} }}
}}
"#,
        name = Ident(name),
        destructor = requests
            .iter()
            .enumerate()
            .find(|(_, msg)| msg.r#type == "destructor")
            .map(|(op, _)| op)
    );

    emitln!(g, "}}");
}

fn gen_ty(g: &mut Gen, r#type: &str, r#enum: &str, interface: &str, nullable: bool) {
    match (nullable, r#type, r#enum, interface) {
        (true, "object" | "new_id", _, _) => {
            emit!(g, "Option<");
            gen_ty(g, r#type, r#enum, interface, false);
            emit!(g, ">");
        }
        (false, "object", _, "") => emit!(g, "object<()>"),
        (false, "new_id", _, "") => emit!(g, "new_id<Dyn>"),
        (false, "object", _, iface) => emit!(g, "object<{iface}::Obj>"),
        (false, "new_id", _, iface) => emit!(g, "new_id<{iface}::Obj>"),
        (_, "int", "", _) => emit!(g, "i32"),
        (_, "uint", "", _) => emit!(g, "u32"),
        (_, "int" | "uint", enm, _) => {
            let mut segs = enm.split(".");
            emit!(g, "super::{}", segs.next().unwrap());
            for seg in segs {
                emit!(g, "::{seg}");
            }
        }
        (_, "fixed", _, _) => emit!(g, "fixed"),
        (_, "fd", _, _) => emit!(g, "OwnedFd"),
        (_, "string", _, _) => emit!(g, "String"),
        (_, "array", _, _) => emit!(g, "Vec<u8>"),
        _ => errexit!("gen_ty: unrecognized type: {type}"),
    }
}

struct Ident<'s, T>(&'s T);

impl<T: AsRef<str>> std::fmt::Debug for Ident<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = self.0.as_ref();
        if name.starts_with(|c: char| c.is_ascii_digit()) {
            write!(f, "N{name}")
        } else if ["move", "box"].contains(&name) {
            write!(f, "{name}_")
        } else {
            write!(f, "{name}")
        }
    }
}

impl<T: AsRef<str>> std::fmt::Display for Ident<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <Self as std::fmt::Debug>::fmt(self, f)
    }
}

fn gen_message_struct(g: &mut Gen, Message { name, description, args, .. }: &Message, op: usize) {
    gen_doccomment(g, description);
    let name_ident = Ident(name);
    emitln!(g, "#[derive(Debug)]");
    emitln!(g, "pub struct {name_ident} {{");
    for Arg { name, description, r#type, r#enum, interface, nullable } in args {
        emitln!(g, "// {}", description.summary.trim());
        emit!(g, "pub {}: ", Ident(name));
        gen_ty(g, r#type, r#enum, interface, *nullable);
        emitln!(g, ",");
    }
    emitln!(g, "}}");

    emitln!(g, "impl Ser for {} {{", Ident(name));
    emitln!(g, "#[allow(unused)]");
    emitln!(g, "fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {{");
    emitln!(g, "let mut n = 0;");
    for Arg { name, .. } in args {
        emit!(g, "n += self.{}.serialize(&mut buf[n..], fds)?;", Ident(name));
    }
    emitln!(g, "Ok(n)");
    emitln!(g, "}}");
    emitln!(g, "}}");

    let name = Ident(name);
    emitln!(g, "impl Dsr for {name} {{");
    emitln!(g, "#[allow(unused)]");
    emitln!(g, "fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {{");
    emitln!(g, "let mut n = 0;");
    for Arg { name, r#type, r#enum, interface, nullable, .. } in args {
        emit!(g, "let ({}, m) = <", Ident(name));
        gen_ty(g, r#type, r#enum, interface, *nullable);
        emitln!(g, ">::deserialize(&buf[n..], fds)?;");
        emitln!(g, "n += m;");
    }
    emit!(g, "Ok(({name}");
    emit!(g, "{{");
    for Arg { name, .. } in args {
        emit!(g, "{}, ", Ident(name));
    }
    emitln!(g, "}}, n))");
    emitln!(g, "}}");
    emitln!(g, "}}");

    emit!(g, "impl Msg<Obj> for {name} {{");
    emitln!(g, "const OP: u16 = {op};");
    if let Some(arg) = args.iter().find(|a| a.r#type == "new_id") {
        emitln!(g, "fn new_id(&self) -> Option<(u32, ObjMeta)> {{");
        emit!(g, "let id = self.{}.0;", Ident(&arg.name));
        if arg.interface != "" {
            let iface = Ident(&arg.interface);
            emitln!(g, "let meta = ObjMeta{{ alive: true, destructor: {iface}::Obj::DESTRUCTOR, type_id: TypeId::of::<{iface}::Obj>() }};");
        } else {
            emitln!(g, "    let meta = ObjMeta{{ alive: true, destructor: None, type_id: TypeId::of::<()>() }};")
        }
        emitln!(g, "    Some((id, meta))");
        emitln!(g, "}}");
    }
    emitln!(g, "}}");
}

fn gen_bitfield(g: &mut Gen, Enum { name, description, entries, .. }: &Enum) {
    gen_doccomment(g, description);
    let name = Ident(name);
    emitln!(
        g,
        r#"
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct {name}{{flags: u32}}
impl Debug for {name} {{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {{
        let mut first = true;
        for (name, val) in Self::EACH {{
            if val & *self == val {{
                if !first {{
                    f.write_str("|")?;
                }}
                first = false;
                f.write_str(name)?;
            }}
        }}
        if first {{
            f.write_str("EMPTY")?;
        }}
        Ok(())
    }}
}}
impl std::fmt::LowerHex for {name} {{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {{
        <u32 as std::fmt::LowerHex>::fmt(&self.flags, f)
    }}
}}
impl Ser for {name} {{
    fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {{
        self.flags.serialize(buf, fds)
    }}
}}
impl Dsr for {name} {{
    fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize), &'static str> {{
        let (flags, n) = u32::deserialize(buf, fds)?;
        Ok(({name}{{flags}}, n))
    }}
}}
impl std::ops::BitOr for {name} {{
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {{
        {name}{{ flags: self.flags | rhs.flags }}
    }}
}}
impl std::ops::BitAnd for {name} {{
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {{
        {name}{{ flags: self.flags & rhs.flags }}
    }}
}}
impl {name} {{
    #![allow(non_upper_case_globals)]

    fn contains(&self, rhs: {name}) -> bool {{
        (self.flags & rhs.flags) == rhs.flags
    }}
"#
    );
    for Entry { name: n, description, value } in entries {
        gen_doccomment(g, description);
        emit!(g, "pub const {}: {name} = {name}{{ flags: {value} }};", Ident(n));
    }
    emitln!(g, "pub const EMPTY: {name} = {name}{{ flags: 0 }};");
    emit!(g, "pub const ALL: {name} = {name}{{ flags: Self::EMPTY.flags ");
    for Entry { name, .. } in entries {
        emit!(g, " | Self::{}.flags", Ident(name));
    }
    emitln!(g, "}};");
    emitln!(g, "pub const EACH: [(&'static str, {name}); {}] = [", entries.len());
    for Entry { name, .. } in entries {
        emit!(g, "(\"{name}\", Self::{name}),", name = Ident(name));
    }
    emitln!(g, "];");
    emitln!(g, "}}");
}

fn gen_enum(g: &mut Gen, Enum { name, description, entries, .. }: &Enum) {
    gen_doccomment(g, description);
    let neg = entries.iter().any(|Entry { value, .. }| *value < 0);
    let repr = if neg { "i32" } else { "u32" };
    emitln!(g, "#[repr({repr})]");
    emitln!(g, "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]");
    emitln!(g, "pub enum {} {{", Ident(name));
    for Entry { name, description, value } in entries {
        gen_doccomment(g, description);
        emitln!(g, "{} = {value},", Ident(name));
    }
    emitln!(g, "}}");

    let name = Ident(name);
    emitln!(
        g,
        r#"
        impl From<{name}> for {repr} {{
            fn from(v: {name}) -> {repr} {{
                match v {{"#
    );
    for Entry { name: n, value, .. } in entries {
        emitln!(g, "{name}::{n} => {value},", n = Ident(n));
    }
    emitln!(
        g,
        r#"
                }}
            }}
        }}"#
    );

    emitln!(
        g,
        r#"
        impl TryFrom<{repr}> for {name} {{
            type Error = core::num::TryFromIntError;
            fn try_from(v: {repr}) -> Result<Self, Self::Error> {{
                match v {{
        "#
    );
    for Entry { name: n, value, .. } in entries {
        emitln!(g, "{value} => Ok({name}::{n}),", n = Ident(n));
    }
    emitln!(
        g,
        r#"
                    _ => u32::try_from(-1).map(|_| unreachable!())?,
                }}
            }}
        }}"#
    );

    emitln!(
        g,
        r#"
        impl Ser for {name} {{
            fn serialize(self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> Result<usize, &'static str> {{
                u32::from(self).serialize(buf, fds)
            }}
        }}

        impl Dsr for {name} {{
            fn deserialize(buf: &[u8], fds: &mut Vec<OwnedFd>) -> Result<(Self, usize),  &'static str> {{
                let (i, n) = u32::deserialize(buf, fds)?;
                let v = i.try_into().map_err(|_| "invalid value for {name}")?;
                Ok((v, n))
            }}
        }}"#
    );
}

fn main() {
    let mut args = std::env::args();
    args.next();
    if args.len() == 0 {
        errexit!("file argument required")
    }

    let mut out = std::io::stdout();
    emit!(out, "{HEADER}");
    for file in args {
        process(&mut out, &file);
    }
}

fn process(mut dst: impl Write, path: &str) {
    let mut rdr = quick_xml::Reader::from_file(path).unwrap();
    rdr.trim_text(false);
    rdr.expand_empty_elements(true);
    let mut rdr = Rdr { xml: rdr, buf: Vec::with_capacity(1024) };
    let protocol: Protocol;
    loop {
        match rdr.xml.read_event_into(&mut rdr.buf).unwrap() {
            xmlEvent::Comment(_) => {}
            xmlEvent::Text(txt) if txt.iter().all(|b| b.is_ascii_whitespace()) => {}
            xmlEvent::Start(start) if start.local_name().as_ref() == b"protocol" => {
                let name = get_attr(&start, "name").expect("protocol must have name");
                protocol = read_protocol(&mut rdr, name);
                break;
            }
            xmlEvent::Decl(_) => {}
            xmlEvent::PI(_) => {}
            xmlEvent::DocType(_) => {}
            evt => errexit!("unexpected xml event:{}: {evt:?}", rdr.xml.buffer_position()),
        }
    }

    let mut g = Gen { out: &mut dst };
    gen_protocol(&mut g, &protocol);
}
