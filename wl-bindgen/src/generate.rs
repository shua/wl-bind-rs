use crate::{str_, Arg, Enum, Interface, Message, MessageVariant, Protocol};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

fn parse_uint(s: &str) -> Result<u32, std::num::ParseIntError> {
    if s.len() > 2 && &s[0..2] == "0x" {
        u32::from_str_radix(&s[2..], 16)
    } else {
        u32::from_str_radix(s, 10)
    }
}
fn tok_id(s: &str) -> Ident {
    Ident::new(s, Span::call_site())
}
fn to_camelcase(s: &str) -> String {
    let mut r = String::with_capacity(s.len());
    let mut init = true;
    for c in s.chars() {
        if c == '_' {
            init = true;
            continue;
        }
        if init {
            r.extend(c.to_uppercase());
            init = false;
            continue;
        }
        r.push(c);
    }
    r
}

fn ns_split(s: &[u8]) -> (Option<TokenStream>, &str) {
    let mut parts: Vec<_> = s.split(|&b| b == b'.').map(str_).collect();
    let last = parts.pop().expect(&format!("nonempty split {}", str_(s)));
    let ns = parts.into_iter().fold(None, |acc, id| {
        let id = tok_id(id);
        match acc {
            Some(tl) => Some(quote! { #id :: #tl }),
            None => Some(quote! { #id }),
        }
    });
    (ns, last)
}

fn ty_ident(arg: &Arg, req: bool) -> TokenStream {
    if let Some(ref ename) = arg.r#enum {
        let (ns, ename) = ns_split(ename);
        let ename = tok_id(&to_camelcase(ename));
        if let Some(ns) = ns {
            return quote!(#ns::#ename);
        } else {
            return quote!(#ename);
        }
    }

    match arg.r#type.as_slice() {
        b"object" | b"new_id" => {
            let mut iname = if let Some(ref iname) = arg.interface {
                let (ns, iname) = ns_split(iname);
                let iname = tok_id(&to_camelcase(iname));
                if let Some(ns) = ns {
                    quote! { #ns::#iname }
                } else {
                    quote! { #iname }
                }
            } else {
                quote! { () }
            };
            if !req {
                iname = quote! { WlRef<#iname> };
            }
            if arg.allow_null {
                iname = quote! { Option<#iname> };
            }
            match arg.r#type.as_slice() {
                b"new_id" => iname,
                b"object" => iname,
                _ => unreachable!("already limited cases in outer match"),
            }
        }
        b"int" => quote! { i32 },
        b"uint" => quote! { u32 },
        b"fixed" => quote! { WlFixed },
        b"string" => quote! { WlStr },
        b"array" => quote! { WlArray },
        b"fd" => quote! { WlFd },
        _ => panic!("unexpected arg type: {}", str_(&arg.r#type)),
    }
}

fn gen_message(
    m: &Message,
    opcodes: &mut Vec<TokenStream>,
    events: &mut Vec<TokenStream>,
    requests: &mut Vec<TokenStream>,
    dsr_cases: &mut Vec<TokenStream>,
    ser_cases: &mut Vec<TokenStream>,
    req_fns: &mut Vec<TokenStream>,
) {
    let opname = tok_id(&str_(&m.name).to_uppercase());
    let opval = opcodes.len();
    let opval: u16 = opval
        .try_into()
        .expect(&format!("opcodes must fit in u16: {}", str_(&m.name)));
    opcodes.push(quote! { const #opname : u16 = #opval; });

    let name = tok_id(&to_camelcase(str_(&m.name)));
    let mut arg_ty = Vec::with_capacity(m.args.len());
    let mut req_arg_ty = Vec::with_capacity(m.args.len());
    let mut req_new_id = Vec::with_capacity(m.args.len());
    let mut req_ret_ty = Vec::with_capacity(m.args.len());
    let mut req_ret_val = Vec::with_capacity(m.args.len());
    let mut args = Vec::with_capacity(m.args.len());
    let mut ser = Vec::with_capacity(m.args.len());
    let mut dsr = Vec::with_capacity(m.args.len());

    for a in &m.args {
        let name = tok_id(str_(&a.name));
        let ty = ty_ident(&a, false);
        let req_ty = ty_ident(&a, true);
        req_arg_ty.push(quote! { #name: #req_ty });
        arg_ty.push(quote! { #name: #ty });
        args.push(quote! { #name });
        match (a.r#type.as_slice(), a.allow_null) {
            (b"object" | b"new_id", false) => dsr.push(quote! {
                let (#name, sz): (Option<#ty>, usize) = <Option<#ty> as WlDsr>::dsr(reg, buf, fdbuf).unwrap();
                let #name = #name.unwrap();
                let buf = &mut buf[sz..];
            }),
            _ => dsr.push(quote! {
                let (#name, sz): (#ty, usize) = <#ty as WlDsr>::dsr(reg, buf, fdbuf).unwrap();
                let buf = &mut buf[sz..];
            })
        }
        ser.push(quote! { #name.ser(buf, fdbuf)?; });
        if &a.r#type == b"new_id" {
            req_new_id.push(quote! { let #name = cl.new_id(#name); });
            req_ret_ty.push(ty);
            req_ret_val.push(name);
        }
    }

    let desc = str_(&m.description);
    let fnname = match str_(&m.name) {
        id @ ("move" | "mut" | "ref" | "box" | "async" | "impl" | "for" | "const") => {
            Ident::new_raw(id, Span::call_site())
        }
        id => tok_id(id),
    };
    let body = quote! {
        #[doc = #desc]
        #name{ #(#arg_ty),* },
    };

    match m.var {
        MessageVariant::Event => {
            events.push(body);
            dsr_cases.push(quote! {
                OpCode::#opname => {
                    #(#dsr)*
                    Ok(Event::#name{ #(#args),* })
                }
            })
        }

        MessageVariant::Request => {
            requests.push(body);
            ser_cases.push(quote! {
                Request::#name{ #(#args),* } => {
                    buf.extend_from_slice(&[0u8; 4]);
                    let i = buf.len();
                    #(#ser)*
                    let j = buf.len();
                    let sz_u = ((j - i) & 0xffff);
                    let sz : usize = sz_u.try_into().unwrap();
                    let opsz = (u32::from(OpCode::#opname) << 16) | sz;
                    buf[i-4..i].copy_from_slice(&opsz.to_be_bytes());
                    Ok(sz_u)
                }
            });
            req_fns.push(quote! {
                #[doc = #desc]
                pub fn #fnname(
                    &self,
                    cl: &mut WlClient
                    #(,#req_arg_ty)*
                ) -> Result<(#(#req_ret_ty),*), ()> {
                    #(#req_new_id)*
                    todo!("actually call cl.send with something");
                    Ok((#(#req_ret_val),*))
                }
            });
        }
    }
}

fn gen_enum(n: &Enum, enms: &mut Vec<TokenStream>) {
    let name = tok_id(&to_camelcase(str_(&n.name)));
    let mut entries = Vec::with_capacity(n.entries.len());
    let mut from_cases = Vec::with_capacity(n.entries.len());
    for e in &n.entries {
        let ename = if b"0123456789".contains(&e.name[0]) {
            format!("{}{}", (n.name[0] as char), str_(&e.name))
        } else {
            str_(&e.name).to_string()
        };
        let value = str_(&e.value);
        let value = parse_uint(value).expect("enum value must be uint: {value}");
        if !n.bitfield {
            let ename = tok_id(&to_camelcase(&ename));
            entries.push(quote! { #ename = #value, });
            from_cases.push(quote! { #value => Ok(#name::#ename), });
        } else {
            let ename = tok_id(&ename.to_uppercase());
            entries.push(quote! { const #ename : u32 = #value; });
        }
    }

    let dsr_ser = quote! {
        impl WlDsr for #name {
            type Error = <u32 as WlDsr>::Error;
            fn dsr(reg: &ObjectRegistry, buf: &mut [u8], fdbuf: &mut Vec<RawFd>) -> Result<(Self, usize), Self::Error> {
                let (n, sz) = u32::dsr(reg, buf, fdbuf)?;
                Ok((n.try_into().unwrap(), sz))
            }
        }
        impl WlSer for #name {
            type Error = <u32 as WlSer>::Error;
            fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), Self::Error> {
                let n : u32 = (*self).into();
                n.ser(buf, fdbuf)
            }
        }
    };

    if !n.bitfield {
        enms.push(quote! {
            #[repr(u32)]
            #[derive(Debug, Clone, Copy)]
            pub enum #name { #(#entries)* }
            impl TryFrom<u32> for #name {
                type Error = ();
                fn try_from(n: u32) -> Result<#name, ()> {
                    match n {
                        #(#from_cases)*
                        _ => Err(()),
                    }
                }
            }
            impl From<#name> for u32 {
                fn from(e: #name) -> u32 { e as u32 }
            }
            #dsr_ser
        });
    } else {
        enms.push(quote! {
            #[repr(transparent)]
            #[derive(Debug, Clone, Copy)]
            pub struct #name(u32);
            impl #name { #(#entries)* }
            impl From<u32> for #name {
                fn from(n: u32) -> #name { #name(n) }
            }
            impl From<#name> for u32 {
                fn from(e: #name) -> u32 {
                    e.0
                }
            }
            #dsr_ser
        });
    }
}

pub fn generate(p: &Protocol) -> TokenStream {
    let Protocol { interfaces, .. } = p;
    let mut ifaces: Vec<TokenStream> = Vec::with_capacity(interfaces.len());

    for Interface {
        name,
        description,
        messages,
        enums,
        ..
    } in interfaces
    {
        let name = str_(&name);
        let iobjname = tok_id(&to_camelcase(name));
        let nameid = tok_id(name);
        let idoc = str_(&description);

        let mut opcodes: Vec<TokenStream> = Vec::with_capacity(messages.len());
        let mut events: Vec<TokenStream> = Vec::with_capacity(messages.len());
        let mut requests: Vec<TokenStream> = Vec::with_capacity(messages.len());
        let mut dsr_cases: Vec<TokenStream> = Vec::with_capacity(messages.len());
        let mut ser_cases: Vec<TokenStream> = Vec::with_capacity(messages.len());
        let mut req_fns: Vec<TokenStream> = Vec::with_capacity(messages.len());
        for m in messages {
            gen_message(
                m,
                &mut opcodes,
                &mut events,
                &mut requests,
                &mut dsr_cases,
                &mut ser_cases,
                &mut req_fns,
            );
        }

        let mut enms: Vec<TokenStream> = Vec::with_capacity(enums.len());
        for n in enums {
            gen_enum(n, &mut enms);
        }

        ifaces.push(quote! {
            #[doc = #idoc]
            pub mod #nameid {
                use super::*;

                #(#enms)*
                #[derive(Debug)]
                pub enum Event { #(#events)* }
                pub enum Request { #(#requests)* }

                struct OpCode;
                impl OpCode {
                    #(#opcodes)*
                }

                pub struct Object{
                    pub on_event: Option<Box<dyn Fn(&ObjectRegistry, Event) -> Result<(), WlHandleError>>>,
                }

                impl std::fmt::Debug for WlRef<Object> {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        write!(f, "{}@{}", #name, self.id)
                    }
                }

                impl WlHandler for Object {
                    fn handle(&self, reg: &ObjectRegistry, op: u16, buf: &mut [u8], fdbuf: &mut Vec<RawFd>) -> Result<(), WlHandleError> {
                        let event = match op {
                            #(#dsr_cases)*
                            _ => Err(WlHandleError::UnrecognizedOpcode(op)),
                        }?;
                        if matches!(std::env::var("WAYLAND_DEBUG").as_ref().map(String::as_str), Ok("1")) {
                            println!("ts {event:?}");
                        }
                        match &self.on_event {
                            Some(cb) => cb(reg, event),
                            None => Err(WlHandleError::Unhandled),
                        }
                    }
                    fn type_id(&self) -> std::any::TypeId {
                        std::any::TypeId::of::<Object>()
                    }
                }

                impl WlRef<Object> {
                    #(#req_fns)*
                }

                impl Request {
                    fn ser_request(
                        &self,
                        buf: &mut Vec<u8>,
                        fdbuf: &mut Vec<RawFd>
                    ) -> Result<usize, WlPrimitiveSerError> {
                        match self {
                            #(#ser_cases)*
                            _ => unreachable!("unless you're doing weird stuff with unsafe refs https://github.com/rust-lang/rust/issues/78123"),
                        }
                    }
                }
            }
            pub use #nameid::Object as #iobjname;
        });
    }

    quote! {
        #(#ifaces)*
    }
}

#[allow(unused)]
fn generate_dumb(p: Protocol) {
    println!("protocol {}", str_(&p.name));
    for i in p.interfaces {
        println!("  interface {}", str_(&i.name));
        for (mi, m) in i.messages.iter().enumerate() {
            let var_s = if m.var == MessageVariant::Request {
                "request"
            } else {
                "event  "
            };
            print!("    {} {} {}(", var_s, mi, str_(&m.name));
            if m.args.len() > 0 {
                fn ptype(a: &Arg) {
                    match (a.r#type.as_slice(), &a.interface, &a.r#enum) {
                        (b"new_id", Some(i), _) => print!("new id {}", str_(i)),
                        (b"object", Some(i), _) => print!("{}", str_(i)),
                        (b"int" | b"uint", _, Some(n)) => print!("{}", str_(n)),
                        (t, _, _) => print!("{}", str_(t)),
                    }
                }
                fn parg(a: &Arg) {
                    print!("{}: ", str_(&a.name));
                    ptype(a);
                }
                parg(&m.args[0]);
                for a in m.args[1..].iter() {
                    print!(", ");
                    parg(&a);
                }
            }
            println!(")");
        }

        for n in i.enums {
            println!("    enum   {}", str_(&n.name));
            for e in n.entries {
                println!("      {} = {}", str_(&e.name), str_(&e.value));
            }
        }
    }
}
