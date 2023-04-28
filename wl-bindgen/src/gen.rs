#![allow(unused)]

use quick_xml::events::Event as XmlEvt;
use std::collections::HashMap;
use std::{borrow::Cow, fmt::Display, io::Write};

enum TyDesc<'b> {
    Int,
    Uint,
    Fixed,
    String,
    Bytes,
    Fd,
    Object { new_id: bool, interface: &'b str },
    Enum(&'b str),
}

type XmlReader<'b> = quick_xml::reader::Reader<&'b [u8]>;
struct Context<'b> {
    r: XmlReader<'b>,
    w: std::io::BufWriter<Box<dyn Write>>,
    registry: HashMap<String, String>,
}

impl<'b> Context<'b> {
    fn skip(&mut self, f: impl Fn(&XmlEvt) -> bool) -> quick_xml::Result<XmlEvt<'b>> {
        let mut evt = self.r.read_event()?;
        while f(&evt) {
            evt = match &evt {
                XmlEvt::Start(b) => {
                    let name = b.name();
                    self.skip(|e| match e {
                        XmlEvt::End(b) => b.name() != name,
                        _ => true,
                    })?
                }
                _ => self.r.read_event()?,
            };
        }
        Ok(evt)
    }
}

fn gen_ser(ctx: &mut Context<'_>, msg: &str, fields: &[(&str, &str)]) -> std::io::Result<()> {
    let s = &mut ctx.w;
    write!(
        s,
        r#"
impl Ser for {msg} {{
    fn ser(&self, s: &mut Serializer) -> SerResult {{
"#
    )?;
    for (name, ty) in fields {
        writeln!(
            s,
            "s.write_{}(self.{name})?;",
            match *ty {
                "int" | "uint" | "string" | "array" | "fd" => ty,
                _ => "any",
            }
        )?;
    }
    write!(
        s,
        r#"Ok(())
    }}
}}
"#
    )
}

struct CamelCase<'d>(&'d str);
impl Display for CamelCase<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut up = true;
        for c in self.0.chars() {
            if c == '_' {
                up == true;
                continue;
            }

            if up {
                write!(f, "{}", c.to_uppercase())?;
            } else {
                write!(f, "{}", c)?;
            }
            up = false;
        }
        Ok(())
    }
}

type Arg<'b> = (&'b str, &'b str, &'b str);

fn gen_dsr(
    ctx: &mut Context<'_>,
    msg_name: &str,
    msgs: &[(&str, bool, &[Arg<'_>])],
) -> std::io::Result<()> {
    write!(
        ctx.w,
        r#"
impl<'dsr> Dsr<'dsr> for {} {{
    type Context = MessageContext;
    fn dsr(d: &mut Deserializer) -> DsrResult<Self> {{
        let msg = match d.ctx.op {{
"#,
        CamelCase(msg_name)
    )?;
    for (op, (name, _, args)) in msgs.iter().enumerate() {
        writeln!(ctx.w, "{op} => {{")?;
        for (name, ty, opts) in *args {
            match (*ty, *opts) {
                ("int" | "uint" | "string" | "array" | "fd", "") => {
                    writeln!(ctx.w, "let {name} = d.read_{ty}()?;")?
                }
                (name @ "fixed", "") | ("int" | "uint", name) => {
                    writeln!(ctx.w, "let {name}: {} = d.read_any()?;", CamelCase(name))?
                }
                ("object", iface) => {
                    writeln!(ctx.w, "let {name}: Id<dyn Any> = d.read_any()?;")?;
                    if iface != "" {
                        let iface = ctx.registry.get(iface).unwrap();
                        writeln!(
                            ctx.w,
                            "let {name}: Id<{iface}> = {name}.downcast().unwrap();",
                        )?;
                    }
                }
                ("new_id", iface) => {
                    let iface = ctx.registry.get(iface).unwrap();
                    writeln!(ctx.w, "let {name}: Id<{iface}> = d.conn.new_id();")?;
                }
                (name, opts) => panic!("unexpected arg type={name} interface={opts:?}"),
            }
        }
        write!(ctx.w, "{}::{}", CamelCase(msg_name), CamelCase(name))?;
        if !args.is_empty() {
            write!(ctx.w, "{{ {}", args[0].0)?;
            for (name, _, _) in &args[1..] {
                write!(ctx.w, ", {name}")?;
            }
            write!(ctx.w, "}}")?;
        }
        writeln!(ctx.w, "\n}}")?;
    }
    write!(
        ctx.w,
        r#"
            op => panic!("unrecognized op ({{op}})"),
        }};
        Ok(msg)
    }}
}}
"#
    )
}

fn gen_ty(ctx: &mut Context, ty: &str, sub: &str, lt: &str) -> std::io::Result<()> {
    match (ty, sub) {
        ("int", "") => write!(ctx.w, "i32"),
        ("uint", "") => write!(ctx.w, "u32"),
        ("fixed", "") => write!(ctx.w, "Fixed"),
        ("string", "") => write!(ctx.w, "&{lt}str"),
        ("array", "") => write!(ctx.w, "&{lt}[u8]"),
        ("fd", "") => write!(ctx.w, "Fd"),
        ("int" | "uint", enm) => {
            let enm = (ctx.registry.get(enm)).expect(&format!("enum ({enm}) not defined"));
            write!(ctx.w, "{enm}")
        }
        ("object" | "new_id", "") => write!(ctx.w, "Id<dyn Any>"),
        ("object" | "new_id", iface) => {
            let iface =
                (ctx.registry.get(iface)).expect(&format!("interface ({iface}) not defined"));
            write!(ctx.w, "Id<{iface}>")
        }
        _ => panic!("unrecognized argument type={ty} interface={sub}"),
    }
}

fn gen_msg_enum(
    ctx: &mut Context,
    vars: &[(&str, bool, Vec<Arg>)],
    lt: &str,
) -> std::io::Result<()> {
    assert!(vars.len() < u16::MAX.into());
    for (op, (name, _, args)) in vars.iter().enumerate() {
        let op = op as u16;
        write!(ctx.w, "{}", CamelCase(name))?;
        if !args.is_empty() {
            let (name, ty, sub) = &args[0];
            write!(ctx.w, "{{ {name}: ",)?;
            gen_ty(ctx, ty, sub, lt)?;
            for (name, ty, sub) in &args[1..] {
                write!(ctx.w, ", {name}: ")?;
                gen_ty(ctx, ty, sub, lt)?;
            }
            write!(ctx.w, "}}")?;
        }
        write!(ctx.w, ",")?;
    }
    Ok(())
}

// -- parsing --

macro_rules! expect {
    ($v:expr , $($p:pat $(if $g:expr)? => $e:expr),+ $(,)?) => {
        match $v {
            $($p $(if $g)? => $e ,)*
            v => panic!("expected one of {:?}, got {v:?}", [$(stringify!($p $(if $g)?)),*]),
        }
    }
}

fn _s<S: AsRef<[u8]> + ?Sized>(b: &S) -> &str {
    std::str::from_utf8(b.as_ref()).expect("valid utf8 string")
}

fn name_matches<'s>(n: &'s str) -> impl Fn(&XmlEvt) -> bool + 's {
    move |e| match e {
        XmlEvt::Start(b) | XmlEvt::Empty(b) => _s(&b.name()) == n,
        XmlEvt::End(b) => _s(&b.name()) == n,
        _ => false,
    }
}

fn get_attr<'b>(b: &quick_xml::events::BytesStart<'b>, name: &str) -> &'b str {
    b.attributes()
        .find_map(|a| match a {
            Ok(a) if _s(&a.key) == name => match a.value {
                // SAFETY: quick_xml ties attribute lifetimes to the event
                // but the underlying data, when borrowed, should only be limited by the
                // top-level buffer.
                Cow::Borrowed(s) => Some(_s(unsafe { std::mem::transmute::<_, &'b [u8]>(s) })),
                Cow::Owned(_) => unreachable!("xml inner is &[u8]"),
            },
            _ => None,
        })
        .unwrap_or("")
}

fn get_attr_as<'b, T>(b: &quick_xml::events::BytesStart<'b>, name: &str) -> Result<T, String>
where
    T: std::str::FromStr + 'b,
    <T as std::str::FromStr>::Err: Display,
{
    let v = get_attr(b, name);
    T::from_str(v).map_err(|e| format!("{e}"))
}

fn protocol<'b>(ctx: &mut Context<'b>) -> quick_xml::Result<()> {
    let evt = ctx.skip(|e| matches!(e, XmlEvt::Decl(_) | XmlEvt::PI(_) | XmlEvt::DocType(_)))?;

    let node = expect! {&evt, XmlEvt::Start(n) if _s(&n.name()) == "protocol" => n};
    let protocol_name = node.name();
    let protocol_name = _s(&protocol_name);
    writeln!(ctx.w, "mod {protocol_name} {{")?;

    let evt = ctx.skip(name_matches("copyright"))?;
    let node = expect! {&evt, XmlEvt::Start(n) if _s(&n.name()) == "interface" => n};

    while _s(&node.name()) == "interface" {
        let iface_name = get_attr(node, "name");
        let iface_version: u32 = get_attr_as(node, "version").unwrap();
        interface(ctx, iface_name, iface_version)?;
    }

    expect! {&evt, XmlEvt::End(n) if _s(&n.name()) == "protocol" => () };
    writeln!(ctx.w, "}}")?;

    Ok(())
}

fn interface<'b>(ctx: &mut Context<'b>, iface_name: &str, version: u32) -> quick_xml::Result<()> {
    let mod_name = format!("{iface_name}_{version}");
    writeln!(ctx.w, "mod {mod_name} {{")?;
    let mut evt = ctx.skip(name_matches("description"))?;

    let mut requests = vec![];
    let mut events = vec![];
    let mut enums = vec![];
    while match &evt {
        XmlEvt::End(n) => _s(&n.name()) != "interface",
        _ => true,
    } {
        let node = expect! {&evt, XmlEvt::Start(n) if ["request", "event", "enum"].contains(&_s(&n.name())) => n };
        let node_name = node.name();
        let node_name = _s(&node_name);

        let name = get_attr(node, "name");
        let ty = get_attr(node, "type");
        ctx.registry.insert(
            format!("{name}"),
            format!("{mod_name}::{}", CamelCase(name)),
        );
        ctx.registry.insert(
            format!("{iface_name}.{name}"),
            format!("{mod_name}::{}", CamelCase(name)),
        );
        match node_name {
            "request" => requests.push(parse_message(ctx, name, ty)?),
            "event" => events.push(parse_message(ctx, name, ty)?),
            "enum" => enums.push(parse_enum(ctx, name)?),
            _ => unreachable!(),
        }

        evt = ctx.r.read_event()?;
    }

    let lt = if requests.iter().any(|(_, _, args)| {
        args.iter()
            .any(|(_, ty, _)| *ty == "string" || *ty == "array")
    }) {
        "'send"
    } else {
        ""
    };
    if lt != "" {
        writeln!(ctx.w, "enum Request<{lt}> {{")?;
    } else {
        writeln!(ctx.w, "enum Request {{")?;
    }
    gen_msg_enum(ctx, &requests, lt)?;
    writeln!(ctx.w, "}}")?;

    let lt = if events.iter().any(|(_, _, args)| {
        args.iter()
            .any(|(_, ty, _)| *ty == "string" || *ty == "array")
    }) {
        "'recv"
    } else {
        ""
    };
    if lt != "" {
        writeln!(ctx.w, "enum Event<{lt}> {{")?;
    } else {
        writeln!(ctx.w, "enum Event {{")?;
    }
    gen_msg_enum(ctx, &events, lt)?;
    writeln!(ctx.w, "}}")?;

    expect! {&evt, XmlEvt::End(n) if _s(&n.name()) == "interface" => n};
    writeln!(ctx.w, "}}")?;

    Ok(())
}

fn parse_message<'b>(
    ctx: &mut Context<'b>,
    name: &str,
    event_type: &str,
) -> quick_xml::Result<(&'b str, bool, Vec<Arg<'b>>)> {
    let mut evt = ctx.skip(name_matches("description"))?;
    let is_destructor = event_type == "destructor";
    let mut args = vec![];

    while match evt {
        XmlEvt::End(_) => !name_matches("event")(&evt),
        _ => false,
    } {
        let (empty, node) = expect! {&evt,
            XmlEvt::Start(n) if name_matches("arg")(&evt) => (false, n),
            XmlEvt::Empty(n) if name_matches("arg")(&evt) => (true, n),
        };

        let name = get_attr(node, "name");
        let ty = get_attr(node, "type");
        let enm = get_attr(node, "enum");
        let iface = get_attr(node, "interface");
        let sub = if iface == "" { enm } else { iface };
        args.push((name, ty, sub));

        if !empty {
            evt = ctx.skip(|e| name_matches("description")(e) || name_matches("summary")(e))?;
            expect! { &evt, XmlEvt::End(_) if name_matches("arg")(&evt) => ()};
        }
        evt = ctx.r.read_event()?;
    }

    todo!();
    Ok((name, is_destructor, args))
}

fn parse_enum<'b>(
    ctx: &mut Context<'b>,
    name: &str,
) -> quick_xml::Result<(&'b str, Vec<(&'b str, u32)>)> {
    todo!()
}
