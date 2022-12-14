use crate::{str_, Arg, Enum, Interface, Message, Protocol};
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

impl Arg {
    fn ty_ident(&self, req: bool, ret: bool) -> TokenStream {
        if let Some(ref ename) = self.r#enum {
            let (ns, ename) = ns_split(ename);
            let ename = tok_id(&to_camelcase(ename));
            if let Some(ns) = ns {
                return quote!(#ns::#ename);
            } else {
                return quote!(#ename);
            }
        }

        match self.r#type.as_slice() {
            b"object" | b"new_id" => {
                let mut iname = if let Some(ref iname) = self.interface {
                    let (ns, iname) = ns_split(iname);
                    let iname = tok_id(&to_camelcase(iname));
                    if let Some(ns) = ns {
                        quote! { #ns::#iname }
                    } else {
                        quote! { #iname }
                    }
                } else {
                    let gname = tok_id(&to_camelcase(str_(&self.name)));
                    match (req, ret) {
                        (true, false) => {
                            quote! { (#gname, u32) }
                        }
                        (true, true) => {
                            quote! { #gname }
                        }
                        (false, _) => {
                            quote! { dyn Interface }
                        }
                    }
                };
                if !req || ret || self.r#type != b"new_id" {
                    iname = quote! { WlRef<#iname> };
                }
                if self.allow_null {
                    iname = quote! { Option<#iname> };
                }
                match self.r#type.as_slice() {
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
            _ => panic!("unexpected arg type: {}", str_(&self.r#type)),
        }
    }
    fn struct_ty_ident(&self) -> TokenStream {
        self.ty_ident(false, false)
    }

    fn req_ty_ident(&self) -> TokenStream {
        self.ty_ident(true, false)
    }

    fn req_ret_ty_ident(&self) -> TokenStream {
        self.ty_ident(true, true)
    }
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

fn gen_opname(m: &Message, ops: &mut Vec<TokenStream>) -> Ident {
    let opname = tok_id(&str_(&m.name).to_uppercase());
    let opval: u16 = ops
        .len()
        .try_into()
        .unwrap_or_else(|_| panic!("opcodes must fit in u16: {}", str_(&m.name)));
    ops.push(quote! { const #opname : u16 = #opval; });
    opname
}

#[derive(Default)]
struct ArgTy {
    arg: Vec<Ident>,
    ty: Vec<TokenStream>,
}

#[derive(Default)]
struct ArgTys {
    enm: ArgTy,
    req: ArgTy,
    ret: ArgTy,
    gen: ArgTy,
}

impl ArgTys {
    fn new() -> ArgTys {
        ArgTys::default()
    }
}

impl ArgTy {
    fn is_empty(&self) -> bool {
        self.arg.is_empty() && self.ty.is_empty()
    }
    fn push(&mut self, arg: Ident, ty: TokenStream) {
        self.arg.push(arg);
        self.ty.push(ty);
    }
}

impl quote::ToTokens for ArgTy {
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
    req_new_id: Vec<TokenStream>,
    ser: Vec<TokenStream>,
    dsr: Vec<TokenStream>,
}

fn gen_args(m: &Message, opname: Ident) -> MsgGen {
    let mut arg_ty = ArgTys::new();
    let mut req_new_id = Vec::with_capacity(m.args.len());
    let mut ser = Vec::with_capacity(m.args.len());
    let mut dsr = Vec::with_capacity(m.args.len());

    for a in &m.args {
        let name = tok_id(str_(&a.name));
        let ty = a.struct_ty_ident();

        match (a.r#type.as_slice(), a.allow_null) {
            (b"object" | b"new_id", false) => dsr.push(quote! {
                let (#name, sz): (Option<#ty>, usize) = <Option<#ty> as WlDsr>::dsr(ids, write_stream, buf, fdbuf)?;
                let #name = #name.expect("non-null object id");
                let buf = &mut buf[sz..];
            }),
            _ => dsr.push(quote! {
                let (#name, sz): (#ty, usize) = <#ty as WlDsr>::dsr(ids, write_stream, buf, fdbuf)?;
                let buf = &mut buf[sz..];
            })
        }

        arg_ty.req.push(name.clone(), a.req_ty_ident());

        if &a.r#type == b"new_id" {
            let ret_name = tok_id(&format!("ret_{}", str_(&a.name)));
            if a.interface.is_some() {
                req_new_id.push(quote! {
                    let #name = ids.new_id(#name, self.wr.clone());
                    let #ret_name = #name.clone();
                });
            } else {
                let gname = tok_id(&to_camelcase(str_(&a.name)));
                let name_i = tok_id(&format!("{}_interface", str_(&a.name)));
                let name_v = tok_id(&format!("{}_version", str_(&a.name)));

                ser.push(quote! { #name_i.ser(buf, fdbuf).unwrap(); });
                ser.push(quote! { #name_v.ser(buf, fdbuf).unwrap(); });

                req_new_id.push(quote! {
                    let #name_i = #name.0.name().into();
                    let (#ret_name, #name_v) = (
                        ids.new_id(#name.0, self.wr.clone()),
                        #name.1,
                    );
                    let #name = #ret_name.clone().as_dyn();
                });

                arg_ty.enm.push(name_i, quote! { WlStr });
                arg_ty.enm.push(name_v, quote! { u32 });

                arg_ty.gen.push(gname, quote! { Interface });
            }
            arg_ty.ret.push(ret_name.clone(), a.req_ret_ty_ident());
        }

        ser.push(quote! { #name.ser(buf, fdbuf).unwrap(); });
        arg_ty.enm.push(name, ty);
    }

    MsgGen {
        opname,
        arg_ty,
        req_new_id,
        ser,
        dsr,
    }
}

fn gen_dsr(
    dsr_cases: &mut Vec<TokenStream>,
    opname: &Ident,
    name: &Ident,
    args: &[Ident],
    dsr: &[TokenStream],
) {
    dsr_cases.push(quote! {
        OpCode::#opname => {
            #(#dsr)*
            Ok((Event::#name{ #(#args),* }, sz))
        }
    })
}

fn gen_ser(
    ser_cases: &mut Vec<TokenStream>,
    opname: &Ident,
    name: &Ident,
    args: &[Ident],
    ser: &[TokenStream],
) {
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
}

fn gen_reqfn(
    req_fns: &mut Vec<TokenStream>,
    m: &Message,
    name: &Ident,
    desc: &str,
    arg_tys: &ArgTys,
    req_new_id: &[TokenStream],
) {
    let fnname = match str_(&m.name) {
        id @ "move" => Ident::new_raw(id, Span::call_site()),
        id => tok_id(id),
    };
    let fngen = if arg_tys.gen.is_empty() {
        quote! {}
    } else {
        let gen_arg_ty = &arg_tys.gen;
        quote! { <#gen_arg_ty> }
    };

    let args = &arg_tys.enm.arg;
    let req_arg_ty = &arg_tys.req;
    let req_ret_val = &arg_tys.ret.arg;
    let req_ret_ty = &arg_tys.ret.ty;
    req_fns.push(quote! {
        #[doc = #desc]
        pub fn #fnname #fngen (
            &self,
            #req_arg_ty
        ) -> (#(#req_ret_ty),*)
        {
            let ids = self.ids.upgrade().expect("ids unavailable");
            #(#req_new_id)*
            let req = Request::#name{ #(#args),* };

            let wr = self.wr.upgrade().expect("write buf unavailable");
            let mut wr = wr.borrow_mut();
            WlClient::send(&mut *wr, self, req).expect("send request");
            (#(#req_ret_val),*)
        }
    });
}

fn gen_message(msgs: &mut Vec<TokenStream>, m: &Message, opcode: Ident) -> (Ident, MsgGen) {
    let mgen = gen_args(m, opcode);
    let name = tok_id(&to_camelcase(str_(&m.name)));
    let desc = str_(&m.description);
    let enm_arg_ty = &mgen.arg_ty.enm;
    msgs.push(quote! {
        #[doc = #desc]
        #name{ #enm_arg_ty },
    });
    (name, mgen)
}

fn gen_enum(n: &Enum, enms: &mut Vec<TokenStream>, opts: GenOptions<'_>) {
    let name = opts.rewrite(&n.name);
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
            fn dsr(
                ids: &IdSpace,
                write_stream: &std::rc::Weak<RefCell<BufStream>>,
                buf: &mut [u8],
                fdbuf: &mut Vec<RawFd>
            ) -> Result<(Self, usize), WlDsrError> {
                let (n, sz) = u32::dsr(ids, write_stream, buf, fdbuf)?;
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
            #[derive(Debug, Clone, Copy, PartialEq)]
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
        events,
        requests,
        enums,
        ..
    } in interfaces
    {
        let iname = str_(&name);
        let name_ty = tok_id(&to_camelcase(iname));
        let iversion: u32 = str_(&version).parse().unwrap();
        let modname = quote::format_ident!("{iname}_v{iversion}");
        let idoc = str_(&description);

        let mut vops: Vec<TokenStream> = Vec::with_capacity(events.len());

        let mut gen_events: Vec<TokenStream> = Vec::with_capacity(events.len());
        let mut dsr_cases: Vec<TokenStream> = Vec::with_capacity(events.len());
        for v in events {
            let opname = gen_opname(v, &mut vops);
            let (name, mgen) = gen_message(&mut gen_events, v, opname);

            gen_dsr(
                &mut dsr_cases,
                &mgen.opname,
                &name,
                &mgen.arg_ty.enm.arg,
                &mgen.dsr,
            );
        }

        let mut rops: Vec<TokenStream> = Vec::with_capacity(requests.len());
        let mut gen_requests: Vec<TokenStream> = Vec::with_capacity(requests.len());
        let mut ser_cases: Vec<TokenStream> = Vec::with_capacity(requests.len());
        let mut req_fns: Vec<TokenStream> = Vec::with_capacity(requests.len());
        for r in requests {
            let opname = gen_opname(r, &mut rops);
            let (name, mgen) = gen_message(&mut gen_requests, r, opname);

            gen_ser(
                &mut ser_cases,
                &mgen.opname,
                &name,
                &mgen.arg_ty.enm.arg,
                &mgen.ser,
            );
            gen_reqfn(
                &mut req_fns,
                r,
                &name,
                str_(&r.description),
                &mgen.arg_ty,
                &mgen.req_new_id,
            );
        }

        let pub_use = if options.should_reuse(name) {
            let iname = tok_id(iname);
            quote! {
                pub use #modname as #iname;
                pub use #modname::#name_ty;
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
                pub enum Event { #(#gen_events)* }
                #[derive(Debug)]
                pub enum Request { #(#gen_requests)* }

                struct OpCode;
                impl OpCode {
                    #(#vops)*
                    #(#rops)*
                }

                pub struct #name_ty (
                    Box<dyn FnMut(WlRef<#name_ty>, Event) -> Result<(), WlHandleError>>,
                );

                impl #name_ty {
                    pub fn new(on_event: impl FnMut(WlRef<#name_ty>, Event) -> Result<(), WlHandleError> + 'static) -> #name_ty {
                        #name_ty(Box::new(on_event))
                    }
                }

                impl InterfaceDesc for #name_ty {
                    type Request = Request;
                    type Event = Event;
                    const NAME: &'static std::ffi::CStr = unsafe { std::ffi::CStr::from_bytes_with_nul_unchecked(#name_null.as_bytes()) };
                    const VERSION: u32 = #iversion;
                }

                impl Interface for #name_ty {
                    fn handle_event(&mut self, cl: &WlClient, id: u32, buf: &mut [u8], fdbuf: &mut Vec<RawFd>) -> Result<(), WlHandleError> {
                        let wr = Rc::downgrade(&cl.write);
                        let (event, _) = Event::dsr(&cl.ids, &wr, buf, fdbuf)
                            .map_err(WlHandleError::Deser)?;
                        if wayland_debug_enabled() {
                            println!("[{}] {}@{id} <- {event:?}", timestamp(), self.name().to_str().unwrap());
                        }
                        (self.0)(
                            WlClient::static_make_ref(&cl.ids, wr, id).unwrap(),
                            event,
                        )
                    }

                    fn type_id(&self) -> std::any::TypeId {
                        std::any::TypeId::of::<#name_ty>()
                    }

                    fn name(&self) -> &'static std::ffi::CStr {
                        #name_ty::NAME
                    }
                    fn version(&self) -> u32 {
                        #name_ty::VERSION
                    }
                }

                impl WlRef<#name_ty> { #(#req_fns)* }

                impl WlDsr for Event {
                    fn dsr(
                        ids: &IdSpace,
                        write_stream: &std::rc::Weak<RefCell<BufStream>>,
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
