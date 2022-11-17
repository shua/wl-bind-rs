use crate::{str_, Arg, Entry, Enum, Interface, Message, MessageVariant, Protocol};
use quick_xml::{events::Event as XEvent, reader::Reader};
use std::borrow::Cow;
use std::fmt::{self, Debug};

fn find_attr_<'e>(e: &'e quick_xml::events::BytesStart, name: &str) -> Option<Cow<'e, [u8]>> {
    e.attributes().find_map(|a| match a {
        Ok(a) => {
            if a.key.as_ref() == name.as_bytes() {
                Some(a.value)
            } else {
                None
            }
        }
        _ => None,
    })
}
fn find_attr<'e>(e: &'e quick_xml::events::BytesStart, name: &str) -> Option<Vec<u8>> {
    find_attr_(e, name).map(|x| x.into_owned())
}
fn get_attr<'e>(e: &'e quick_xml::events::BytesStart, name: &str) -> Vec<u8> {
    find_attr(e, name).unwrap_or_else(|| panic!("{name} not found"))
}

pub enum State {
    Start,
    Protocol(Protocol),
    ProtocolDescription(Protocol),
    Copyright(Protocol),
    Interface(Protocol, Interface),
    InterfaceDescription(Protocol, Interface),
    Message(Protocol, Interface, Message),
    MessageDescription(Protocol, Interface, Message),
    Enum(Protocol, Interface, Enum),
    EnumDescription(Protocol, Interface, Enum),
    EnumEntry(Protocol, Interface, Enum, Entry),
    EnumEntryDescription(Protocol, Interface, Enum, Entry),
}

#[derive(Debug)]
pub enum ParseError {
    XML(quick_xml::Error),
    Unexpected(State, String),
}

impl ParseError {
    fn new<S: ?Sized + AsRef<[u8]>>(st: State, expected: &str, got: &S) -> ParseError {
        let mut s = String::new();
        s.push_str("expected ");
        s.push_str(expected);
        s.push_str(", got <");
        s.push_str(str_(got));
        s.push('>');
        ParseError::Unexpected(st, s)
    }
}

impl From<quick_xml::Error> for ParseError {
    fn from(err: quick_xml::Error) -> ParseError {
        ParseError::XML(err)
    }
}

impl Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use State::*;
        match self {
            Start => write!(f, "Start"),
            Protocol(p) => write!(
                f,
                "Protocol({}:{})",
                str_(&p.name),
                p.interfaces.last().map(|i| str_(&i.name)).unwrap_or("*")
            ),
            ProtocolDescription(p) => write!(f, "Protocol({}:description)", str_(&p.name),),
            Copyright(p) => write!(f, "Protocol({}:copyright)", str_(&p.name)),
            Interface(p, i) => write!(
                f,
                "Interface({}:{}:({},{},{}))",
                str_(&p.name),
                str_(&i.name),
                i.events.last().map(|r| str_(&r.name)).unwrap_or("*"),
                i.requests.last().map(|r| str_(&r.name)).unwrap_or("*"),
                i.enums.last().map(|n| str_(&n.name)).unwrap_or("*"),
            ),
            InterfaceDescription(p, i) => write!(
                f,
                "Interface({}:{}:description)",
                str_(&p.name),
                str_(&i.name)
            ),
            Message(p, i, r) => write!(
                f,
                "Message({}:{}:{}:{})",
                str_(&p.name),
                str_(&i.name),
                str_(&r.name),
                r.args.last().map(|a| str_(&a.name)).unwrap_or("*"),
            ),
            MessageDescription(p, i, r) => write!(
                f,
                "Message({}:{}:{}:description)",
                str_(&p.name),
                str_(&i.name),
                str_(&r.name),
            ),
            Enum(p, i, n) => write!(
                f,
                "Enum({}:{}:{}:{})",
                str_(&p.name),
                str_(&i.name),
                str_(&n.name),
                n.entries.last().map(|e| str_(&e.name)).unwrap_or("*"),
            ),
            EnumDescription(p, i, n) => write!(
                f,
                "Enum({}:{}:{}:description)",
                str_(&p.name),
                str_(&i.name),
                str_(&n.name),
            ),
            EnumEntry(p, i, n, e) => write!(
                f,
                "EnumEntry({}:{}:{}:{})",
                str_(&p.name),
                str_(&i.name),
                str_(&n.name),
                str_(&e.name),
            ),
            EnumEntryDescription(p, i, n, e) => write!(
                f,
                "EnumEntyr({}:{}:{}:{}:description)",
                str_(&p.name),
                str_(&i.name),
                str_(&n.name),
                str_(&e.name),
            ),
        }
    }
}

pub fn parse(xml: &[u8]) -> Result<Protocol, ParseError> {
    macro_rules! perr {
        ($st:expr, $exp:tt , $got:expr) => {
            return Err(ParseError::new($st, &$exp, &$got))
        };
        ($exp:tt , $got:expr) => {
            return Err(ParseError::new(State::Start, &$exp, &$got))
        };
    }
    fn trim_txt(mut txt: &[u8]) -> &[u8] {
        let mut i = txt.len();
        while i > 1 && b"\n \t".contains(&txt[i - 1]) {
            i -= 1;
        }
        txt = &txt[..i];
        i = 0;
        while i < txt.len() && b"\n \t".contains(&txt[i]) {
            i += 1;
        }
        &txt[i..]
    }

    use State::{
        Copyright as SC, Enum as SN, EnumDescription as SND, EnumEntry as SNE,
        EnumEntryDescription as SNED, Interface as SI, InterfaceDescription as SID, Message as SM,
        MessageDescription as SMD, Protocol as SP, ProtocolDescription as SPD, Start as SS,
    };
    use XEvent::{CData, Empty, End, Eof, Start, Text};

    let mut rdr = Reader::from_str(str_(xml));
    let mut buf = vec![];
    let mut state = SS;

    loop {
        state = match (state, rdr.read_event_into(&mut buf)?) {
            (_, Eof) => panic!("unexpected eof"),

            (SS, XEvent::Decl(_)) => SS,
            (SS, Start(e)) => {
                if e.name().as_ref() != b"protocol" {
                    perr!(SS, "<protocol>", &e.name());
                }
                let name = get_attr(&e, "name");
                SP(Protocol {
                    name,
                    ..Default::default()
                })
            }

            (SP(p), Start(e)) => match e.name().as_ref() {
                b"copyright" => SC(p),
                b"interface" => SI(
                    p,
                    Interface {
                        name: get_attr(&e, "name"),
                        version: get_attr(&e, "version"),
                        ..Default::default()
                    },
                ),
                b"description" => SPD(p),
                _ => perr!(
                    SP(p),
                    "<copyright>, <interface>, or <description>",
                    e.name()
                ),
            },
            (SP(mut p), Empty(e)) => match e.name().as_ref() {
                b"description" => {
                    p.description.extend(get_attr(&e, "summary"));
                    SP(p)
                }
                _ => perr!(SP(p), "<description/>", e.name()),
            },
            (SPD(mut p), Text(txt)) => {
                for txt in txt.as_ref().split(|&b| b == b'\n') {
                    p.description.extend(trim_txt(txt));
                    p.description.push(b'\n');
                }
                SPD(p)
            }
            (SPD(p), End(e)) => match e.name().as_ref() {
                b"description" => SP(p),
                _ => perr!(SPD(p), "</description>", e.name()),
            },

            (SC(mut p), Text(txt)) => {
                p.copyright.extend(b"\n");
                p.copyright.extend(txt.as_ref());
                SC(p)
            }
            (SC(mut p), CData(txt)) => {
                p.copyright.extend(b"\n");
                p.copyright.extend(txt.as_ref());
                SC(p)
            }
            (SC(p), End(e)) => match e.name().as_ref() {
                b"copyright" => SP(p),
                _ => perr!(SC(p), "</copyright>", e.name()),
            },

            (SI(p, mut i), Empty(e)) => match e.name().as_ref() {
                b"description" => {
                    i.description.extend(get_attr(&e, "summary"));
                    SI(p, i)
                }
                b"request" => {
                    i.requests.push(Message {
                        name: get_attr(&e, "name"),
                        var: MessageVariant::Request,
                        description: vec![],
                        args: vec![],
                    });
                    SI(p, i)
                }
                b"event" => {
                    i.events.push(Message {
                        name: get_attr(&e, "name"),
                        var: MessageVariant::Event,
                        description: vec![],
                        args: vec![],
                    });
                    SI(p, i)
                }
                _ => perr!(
                    SI(p, i),
                    "<description/>, <request/>, or <event/>",
                    e.name()
                ),
            },
            (SI(p, i), Start(e)) => {
                use MessageVariant::{self as MV, Event as MVE, Request as MVR};
                fn m(name: Vec<u8>, var: MV) -> Message {
                    Message {
                        name,
                        var,
                        description: vec![],
                        args: vec![],
                    }
                }

                match e.name().as_ref() {
                    b"description" => SID(p, i),
                    b"request" => SM(p, i, m(get_attr(&e, "name"), MVR)),
                    b"event" => SM(p, i, m(get_attr(&e, "name"), MVE)),
                    b"enum" => {
                        let name = get_attr(&e, "name");
                        let bitfield = find_attr(&e, "bitfield")
                            .map(|b| b == b"true")
                            .unwrap_or(false);
                        let n = Enum {
                            name,
                            bitfield,
                            ..Default::default()
                        };
                        SN(p, i, n)
                    }
                    _ => perr!(
                        SI(p, i),
                        "<description>, <request>, <event>, or <enum>",
                        e.name()
                    ),
                }
            }
            (SID(p, mut i), Text(txt)) => {
                for txt in txt.as_ref().split(|&b| b == b'\n') {
                    i.description.extend(trim_txt(txt));
                    i.description.push(b'\n');
                }
                SID(p, i)
            }
            (SID(p, i), End(e)) => match e.name().as_ref() {
                b"description" => SI(p, i),
                _ => perr!(SID(p, i), "</description>", e.name()),
            },

            (SM(p, i, r), Start(e)) => match e.name().as_ref() {
                b"description" => SMD(p, i, r),
                _ => perr!(SM(p, i, r), "<description>", e.name()),
            },
            (SM(p, i, mut r), Empty(e)) => match e.name().as_ref() {
                b"description" => {
                    r.description.extend(get_attr(&e, "summary"));
                    SM(p, i, r)
                }
                b"arg" => {
                    let allow_null = find_attr(&e, "allow-null")
                        .map(|s| s == b"true")
                        .unwrap_or(false);
                    r.args.push(Arg {
                        name: get_attr(&e, "name"),
                        r#type: get_attr(&e, "type"),
                        interface: find_attr(&e, "interface"),
                        allow_null,
                        r#enum: find_attr(&e, "enum"),
                        summary: find_attr(&e, "summary"),
                    });
                    SM(p, i, r)
                }
                _ => perr!(SM(p, i, r), "<description>, or <arg>", e.name()),
            },
            (SMD(p, i, mut r), Text(txt)) => {
                for txt in txt.as_ref().split(|&b| b == b'\n') {
                    r.description.extend(trim_txt(txt));
                    r.description.push(b'\n');
                }
                SMD(p, i, r)
            }
            (SMD(p, i, v), End(e)) => match e.name().as_ref() {
                b"description" => SM(p, i, v),
                _ => perr!(SMD(p, i, v), "</description>", e.name()),
            },
            (SM(p, mut i, r), End(e)) if r.var == MessageVariant::Request => {
                match e.name().as_ref() {
                    b"arg" => SM(p, i, r),
                    b"request" if r.var == MessageVariant::Request => {
                        i.requests.push(r);
                        SI(p, i)
                    }
                    b"event" if r.var == MessageVariant::Event => {
                        i.events.push(r);
                        SI(p, i)
                    }
                    _ => perr!(SM(p, i, r), "</arg>, </request>, or </event>", e.name()),
                }
            }
            (SM(p, mut i, v), End(e)) if v.var == MessageVariant::Event => {
                match e.name().as_ref() {
                    b"arg" => SM(p, i, v),
                    b"event" => {
                        i.events.push(v);
                        SI(p, i)
                    }
                    _ => perr!(SM(p, i, v), "</arg>, or </event>", e.name()),
                }
            }

            (SN(p, i, n), Start(e)) => match e.name().as_ref() {
                b"description" => SND(p, i, n),
                b"entry" => SNE(
                    p,
                    i,
                    n,
                    Entry {
                        name: get_attr(&e, "name"),
                        value: get_attr(&e, "value"),
                        summary: find_attr(&e, "summary"),
                        description: vec![],
                    },
                ),
                _ => perr!(SN(p, i, n), "<description>", e.name()),
            },
            (SN(p, i, mut n), Empty(e)) => match e.name().as_ref() {
                b"description" => {
                    n.description.extend(get_attr(&e, "summary"));
                    SN(p, i, n)
                }
                b"entry" => {
                    n.entries.push(Entry {
                        name: get_attr(&e, "name"),
                        value: get_attr(&e, "value"),
                        summary: find_attr(&e, "summary"),
                        description: vec![],
                    });
                    SN(p, i, n)
                }
                _ => perr!(SN(p, i, n), "<description>, or <entry>", e.name()),
            },
            (SND(p, i, mut n), Text(txt)) => {
                for txt in txt.as_ref().split(|&b| b == b'\n') {
                    n.description.extend(trim_txt(txt));
                    n.description.push(b'\n');
                }
                SND(p, i, n)
            }
            (SND(p, i, n), End(e)) => match e.name().as_ref() {
                b"description" => SN(p, i, n),
                _ => perr!(SND(p, i, n), "</description>", e.name()),
            },
            (SN(p, mut i, n), End(e)) => match e.name().as_ref() {
                b"entry" => SN(p, i, n),
                b"enum" => {
                    i.enums.push(n);
                    SI(p, i)
                }
                _ => perr!(SN(p, i, n), "</entry>, or </enum>", e.name()),
            },

            (SNE(p, i, n, entry), Start(e)) => match e.name().as_ref() {
                b"description" => SNED(p, i, n, entry),
                _ => perr!(SNE(p, i, n, entry), "<description>", e.name()),
            },
            (SNED(p, i, n, mut e), Text(txt)) => {
                for txt in txt.as_ref().split(|&b| b == b'\n') {
                    e.description.extend(trim_txt(txt));
                    e.description.push(b'\n');
                }
                SNED(p, i, n, e)
            }
            (SNED(p, i, n, entry), End(e)) => match e.name().as_ref() {
                b"description" => SNE(p, i, n, entry),
                _ => perr!(SNED(p, i, n, entry), "</description>", e.name()),
            },
            (SNE(p, i, mut n, entry), End(e)) => match e.name().as_ref() {
                b"entry" => {
                    n.entries.push(entry);
                    SN(p, i, n)
                }
                _ => perr!(SNE(p, i, n, entry), "</entry>", e.name()),
            },

            (SI(mut p, i), End(e)) => match e.name().as_ref() {
                b"interface" => {
                    p.interfaces.push(i);
                    SP(p)
                }
                _ => perr!(SI(p, i), "</interface>", e.name()),
            },

            (SP(p), End(e)) => match e.name().as_ref() {
                b"protocol" => {
                    return Ok(p);
                }
                _ => perr!(SP(p), "</protocol>", e.name()),
            },

            (state, XEvent::Comment(_)) => state,
            (state, Text(txt)) if txt.as_ref().iter().all(|b| b"\n \t".contains(b)) => state,

            (state, event) => panic!("unexpected: state {:?}, event{:?}", state, event),
        };
    }
}
