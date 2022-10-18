use crate::{str_, Arg, Enum, Interface, Message, MessageVariant, Protocol};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

fn parse_uint(s: &str) -> Result<u32, std::num::ParseIntError> {
    if s.len() > 2 && &s[0..2] == "0x" {
        u32::from_str_radix(&s[2..], 16)
    } else {
        s.parse::<u32>()
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
    let last = parts
        .pop()
        .unwrap_or_else(|| panic!("nonempty split {}", str_(s)));
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
                if req {
                    let gname = tok_id(&to_camelcase(str_(&arg.name)));
                    quote! { #gname }
                } else {
                    quote! { dyn Interface }
                }
            };
            if !req || arg.r#type != b"new_id" {
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

fn gen_opname(m: &Message, ops: &mut Vec<TokenStream>) -> Ident {
    let opname = tok_id(&str_(&m.name).to_uppercase());
    let opval: u16 = ops
        .len()
        .try_into()
        .unwrap_or_else(|_| panic!("opcodes must fit in u16: {}", str_(&m.name)));
    ops.push(quote! { const #opname : u16 = #opval; });
    opname
}

struct ArgTys {
    arg: Vec<Ident>,
    ty: Vec<TokenStream>,
    gen: Vec<Ident>,
    gen_constraints: Vec<TokenStream>,
}

impl ArgTys {
    fn new() -> ArgTys {
        ArgTys {
            arg: vec![],
            ty: vec![],
            gen: vec![],
            gen_constraints: vec![],
        }
    }

    fn push(&mut self, arg: Ident, ty: TokenStream) {
        self.arg.push(arg);
        self.ty.push(ty);
    }
}

impl quote::ToTokens for ArgTys {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        use quote::TokenStreamExt;
        for i in 0..self.arg.len() {
            tokens.append(self.arg[i].clone());
            tokens.append(proc_macro2::Punct::new(':', proc_macro2::Spacing::Alone));
            tokens.append_all(self.ty[i].clone());
            if i != self.arg.len() {
                tokens.append(proc_macro2::Punct::new(',', proc_macro2::Spacing::Alone));
            }
        }
    }
}

struct MsgGen {
    opname: Ident,
    arg_ty: ArgTys,
    req_arg_ty: ArgTys,
    req_new_id: Vec<TokenStream>,
    req_ret_arg_ty: ArgTys,
    ser: Vec<TokenStream>,
    dsr: Vec<TokenStream>,
}

fn gen_args(m: &Message, opname: Ident) -> MsgGen {
    let mut arg_ty = ArgTys::new();
    let mut req_arg_ty = ArgTys::new();
    let mut req_new_id = Vec::with_capacity(m.args.len());
    let mut req_ret_arg_ty = ArgTys::new();
    let mut ser = Vec::with_capacity(m.args.len());
    let mut dsr = Vec::with_capacity(m.args.len());

    req_new_id.push(quote! {
        let cl = match self.cl.upgrade() {
            Some(cl) => cl,
            None => return Err(WlSerError::ClientUnavailable),
        };
        let mut cl = cl.borrow();
    });

    for a in &m.args {
        let name = tok_id(str_(&a.name));
        let ret_name = tok_id(&format!("ret_{}", str_(&a.name)));
        let ty = ty_ident(a, false);
        let req_ty = ty_ident(a, true);

        match (a.r#type.as_slice(), a.allow_null) {
            (b"object" | b"new_id", false) => dsr.push(quote! {
                let (#name, sz): (Option<#ty>, usize) = <Option<#ty> as WlDsr>::dsr(cl, buf, fdbuf)?;
                let #name = #name.unwrap();
                let buf = &mut buf[sz..];
            }),
            _ => dsr.push(quote! {
                let (#name, sz): (#ty, usize) = <#ty as WlDsr>::dsr(cl, buf, fdbuf)?;
                let buf = &mut buf[sz..];
            })
        }

        if a.r#type.as_slice() == b"new_id" && a.interface.is_none() {
            ser.push(quote! { interface.ser(buf, fdbuf)?; });
            ser.push(quote! { version.ser(buf, fdbuf)?; });
        }
        ser.push(quote! { #name.ser(buf, fdbuf)?; });
        req_arg_ty.push(name.clone(), req_ty.clone());
        if &a.r#type == b"new_id" {
            if a.interface.is_some() {
                req_new_id.push(quote! {
                    let #name = cl.new_id(#name);
                    let #ret_name = #name.clone();
                });
            } else {
                let gname = tok_id(&to_camelcase(str_(&a.name)));
                req_arg_ty
                    .gen_constraints
                    .push(quote! { #gname : Interface });
                req_arg_ty
                    .gen_constraints
                    .push(quote! { #gname: InterfaceDesc });

                arg_ty.push(tok_id("interface"), quote! {  WlStr });
                arg_ty.push(tok_id("version"), quote! { u32 });
                req_new_id.push(quote! {
                    let interface : WlStr = <#gname as InterfaceDesc>::NAME.into();
                    let version = <#gname as InterfaceDesc>::VERSION;
                    let #name = cl.new_id(#name).as_dyn();
                    let #ret_name = #name.clone();
                });

                req_arg_ty.gen.push(gname);
            }
            req_ret_arg_ty.push(ret_name.clone(), ty.clone());
        }

        arg_ty.push(name, ty);
    }

    MsgGen {
        opname,
        arg_ty,
        req_arg_ty,
        req_new_id,
        req_ret_arg_ty,
        ser,
        dsr,
    }
}

fn gen_message(
    m: &Message,
    opname: Ident,
    events: &mut Vec<TokenStream>,
    requests: &mut Vec<TokenStream>,
    dsr_cases: &mut Vec<TokenStream>,
    ser_cases: &mut Vec<TokenStream>,
    req_fns: &mut Vec<TokenStream>,
) {
    let name = tok_id(&to_camelcase(str_(&m.name)));

    let MsgGen {
        opname,
        arg_ty,
        req_arg_ty,
        req_new_id,
        req_ret_arg_ty,
        ser,
        dsr,
    } = gen_args(m, opname);

    let desc = str_(&m.description);
    let args = &arg_ty.arg;
    let body = quote! {
        #[doc = #desc]
        #name{ #arg_ty },
    };

    match m.var {
        MessageVariant::Event => {
            events.push(body);
            dsr_cases.push(quote! {
                OpCode::#opname => {
                    #(#dsr)*
                    Ok((Event::#name{ #(#args),* }, sz))
                }
            })
        }

        MessageVariant::Request => {
            requests.push(body);
            ser_cases.push(quote! {
                Request::#name{ #(ref #args),* } => {
                    buf.extend([0u8; 4]);
                    let i = buf.len();
                    #(#ser)*
                    let j = buf.len();
                    let sz : u32 = ((j - i) & 0xffff).try_into().unwrap();
                    let sz = sz + 8;
                    let szop = (sz << 16) | u32::from(OpCode::#opname);
                    buf[i-4..i].copy_from_slice(&szop.to_ne_bytes());
                    Ok(())
                }
            });

            let fnname = match str_(&m.name) {
                id @ ("fn" | "static" | "union" | "struct" | "move" | "mut" | "ref" | "const"
                | "box" | "async" | "await" | "impl" | "trait" | "dyn" | "for" | "in"
                | "let") => Ident::new_raw(id, Span::call_site()),
                id => tok_id(id),
            };
            let (fngen, fnwhere) = if req_arg_ty.gen.is_empty() {
                (quote! {}, quote! {})
            } else {
                let req_gen = req_arg_ty.gen.clone();
                let req_where = req_arg_ty.gen_constraints.clone();
                (quote! { <#(#req_gen),*> }, quote! { where #(#req_where),* })
            };

            let req_ret_val = &req_ret_arg_ty.arg;
            let req_ret_ty = &req_ret_arg_ty.ty;
            req_fns.push(quote! {
                #[doc = #desc]
                pub fn #fnname #fngen (
                    &self,
                    #req_arg_ty
                ) -> Result<(#(#req_ret_ty),*), WlSerError>
                    #fnwhere
                {
                    #(#req_new_id)*
                    let req = Request::#name{ #(#args),* };
                    if wayland_debug_enabled() {
                        println!("[{}] -> {req:?}", timestamp());
                    }
                    cl.send(self, req)?;
                    Ok((#(#req_ret_val),*))
                }
            });
        }
    }
}

fn gen_enum(n: &Enum, enms: &mut Vec<TokenStream>, opts: GenOptions<'_>) {
    let name = opts.rewrite(&n.name);
    if let std::borrow::Cow::Owned(_) = name {
        eprintln!("REWROTE: {} -> {:?}", str_(&n.name), str_(&name));
    }
    let name = tok_id(&to_camelcase(str_(&name)));
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
            fn dsr(cl: &WlClient, buf: &mut [u8], fdbuf: &mut Vec<RawFd>) -> Result<(Self, usize), WlDsrError> {
                let (n, sz) = u32::dsr(cl, buf, fdbuf)?;
                Ok((n.try_into().unwrap(), sz))
            }
        }
        impl WlSer for #name {
            fn ser(&self, buf: &mut Vec<u8>, fdbuf: &mut Vec<RawFd>) -> Result<(), WlSerError> {
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

#[derive(Clone, Copy)]
pub struct GenOptions<'g> {
    pub reused: &'g [(Vec<u8>, Vec<u8>)],
    pub rewrite: &'g [&'g dyn Fn(&[u8], &[u8], &[u8]) -> Option<Vec<u8>>],

    pub cur: (&'g [u8], &'g [u8]),
}

fn default_rewrites(iface: &[u8], version: &[u8], ident: &[u8]) -> Option<Vec<u8>> {
    match (iface, version, ident) {
        (b"wp_content_type_v1", b"1", b"type") => Some(b"content_type".to_vec()),
        _ => None,
    }
}

impl std::default::Default for GenOptions<'_> {
    fn default() -> Self {
        GenOptions {
            reused: &[],
            rewrite: &[&default_rewrites],

            cur: (&[], &[]),
        }
    }
}

impl<'g> GenOptions<'g> {
    fn should_reuse(&self, name: &[u8]) -> bool {
        !self.reused.iter().any(|(n, _)| n == name)
    }

    fn rewrite<'n>(&self, name: &'n [u8]) -> std::borrow::Cow<'n, [u8]> {
        for rw in self.rewrite {
            if let Some(v) = rw(self.cur.0, self.cur.1, name) {
                return std::borrow::Cow::Owned(v);
            }
        }
        std::borrow::Cow::Borrowed(name)
    }
}

pub fn generate(p: &Protocol, options: GenOptions<'_>) -> TokenStream {
    let Protocol { interfaces, .. } = p;
    let mut ifaces: Vec<TokenStream> = Vec::with_capacity(interfaces.len());

    for Interface {
        name,
        version,
        description,
        messages,
        enums,
        ..
    } in interfaces
    {
        let iname = str_(&name);
        let iversion: u32 = str_(&version).parse().unwrap();
        let iobjname = tok_id(&to_camelcase(iname));
        let modname = quote::format_ident!("{iname}_v{iversion}");
        let idoc = str_(&description);

        let mut vops: Vec<TokenStream> = Vec::with_capacity(messages.len());
        let mut rops: Vec<TokenStream> = Vec::with_capacity(messages.len());

        let mut events: Vec<TokenStream> = Vec::with_capacity(messages.len());
        let mut requests: Vec<TokenStream> = Vec::with_capacity(messages.len());
        let mut dsr_cases: Vec<TokenStream> = Vec::with_capacity(messages.len());
        let mut ser_cases: Vec<TokenStream> = Vec::with_capacity(messages.len());
        let mut req_fns: Vec<TokenStream> = Vec::with_capacity(messages.len());
        for m in messages {
            use MessageVariant::*;
            let opname = match m.var {
                Event => gen_opname(m, &mut vops),
                Request => gen_opname(m, &mut rops),
            };

            gen_message(
                m,
                opname,
                &mut events,
                &mut requests,
                &mut dsr_cases,
                &mut ser_cases,
                &mut req_fns,
            );
        }

        let pub_use = if options.should_reuse(name) {
            let iname = tok_id(iname);
            quote! {
                pub use #modname as #iname;
                pub type #iobjname = #modname::Proxy;
            }
        } else {
            quote! {}
        };

        let mut enms: Vec<TokenStream> = Vec::with_capacity(enums.len());
        for n in enums {
            gen_enum(
                n,
                &mut enms,
                GenOptions {
                    cur: (name, version),
                    ..options
                },
            );
        }

        let name_null = format!("{}\0", iname);
        ifaces.push(quote! {
            #[doc = #idoc]
            pub mod #modname {
                use super::*;

                #(#enms)*
                #[derive(Debug)]
                pub enum Event { #(#events)* }
                #[derive(Debug)]
                pub enum Request { #(#requests)* }

                struct OpCode;
                impl OpCode {
                    #(#vops)*
                    #(#rops)*
                }

                type EventHandler<T> = Box<dyn Fn(&T, &WlClient, Event) -> Result<(), WlHandleError>>;
                pub struct Proxy {
                    on_event: Option<EventHandler<Proxy>>,
                }

                impl Proxy {
                    pub fn new(on_event: impl Fn(&Proxy, &WlClient, Event) -> Result<(), WlHandleError> + 'static) -> Proxy {
                        Proxy{ on_event: Some(Box::new(on_event)) }
                    }
                }

                impl InterfaceDesc for Proxy {
                    type Request = Request;
                    type Event = Event;
                    const NAME: &'static std::ffi::CStr = unsafe { std::ffi::CStr::from_bytes_with_nul_unchecked(#name_null.as_bytes()) };
                    const VERSION: u32 = #iversion;
                }

                impl Interface for Proxy {
                    fn handle(&self, cl: &WlClient, id: u32, op: u16, buf: &mut [u8], fdbuf: &mut Vec<RawFd>) -> Result<(), WlHandleError> {
                        let (event, _) = Event::dsr(cl, buf, fdbuf).map_err(WlHandleError::Deser)?;
                        if wayland_debug_enabled() {
                            println!("[{}] {}@{id} <- {event:?}", timestamp(), <Proxy as InterfaceDesc>::NAME.to_str().unwrap());
                        }
                        match &self.on_event {
                            Some(cb) => cb(self, cl, event),
                            None => Err(WlHandleError::Unhandled),
                        }
                    }

                    fn type_id(&self) -> std::any::TypeId {
                        std::any::TypeId::of::<Proxy>()
                    }
                }

                impl WlRef<Proxy> { #(#req_fns)* }

                impl WlDsr for Event {
                    fn dsr(
                        cl: &WlClient,
                        buf: &mut [u8],
                        fdbuf: &mut Vec<RawFd>,
                    ) -> Result<(Self, usize), WlDsrError> {
                        let szop = u32::from_ne_bytes(buf[4..8].try_into().unwrap());
                        let op : u16 = (szop & 0xffff).try_into().unwrap();
                        let sz : usize = (szop >> 16).try_into().unwrap();
                        let buf = &mut buf[8..];
                        match op {
                            #(#dsr_cases)*
                            _ => Err(WlDsrError::UnrecognizedOpcode(op)),
                        }
                    }
                }

                impl WlSer for Request {
                    fn ser(
                        &self,
                        buf: &mut Vec<u8>,
                        fdbuf: &mut Vec<RawFd>
                    ) -> Result<(), WlSerError> {
                        match *self {
                            #(#ser_cases)*
                        }
                    }
                }
            }

            #pub_use
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
            if !m.args.is_empty() {
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
                    parg(a);
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
