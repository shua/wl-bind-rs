use crate::{str_, Arg, Entry, Enum, Interface, Message, Protocol};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, ToTokens};

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

#[repr(transparent)]
struct TokenWrap<T>(T);

impl ToTokens for TokenWrap<&'_ Protocol> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        for interface in self.0.interfaces.iter() {
            TokenWrap(interface).to_tokens(tokens);
        }
    }
}

impl ToTokens for TokenWrap<&'_ Interface> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let name_str = str_(&self.0.name);
        let name = tok_id(str_(&self.0.name));
        let desc = str_(&self.0.description);
        let enums = TokenWrap(self.0.enums.as_slice());
        let events = TokenWrap((true, self.0.events.as_slice()));
        let requests = TokenWrap((false, self.0.requests.as_slice()));
        tokens.extend(quote! {
            #[doc = #desc]
            pub mod #name {
                use super::*;

                #enums
                #events
                #requests

                #[repr(transparent)]
                #[derive(Clone,Copy,PartialEq,Eq)]
                pub struct Handle(pub u32);

                impl std::fmt::Debug for Handle {
                    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                        write!(f, "{}@{}", #name_str, self.0)
                    }
                }

                impl Dsr for Handle {
                    fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
                        u32::dsr(buf).map(Handle)
                    }
                    fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
                        self.0.ser(buf)
                    }
                }
            }
        })
    }
}

impl ToTokens for TokenWrap<&'_ [Enum]> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        for n in self.0 {
            let name_str = to_camelcase(str_(&n.name));
            let name = tok_id(&name_str);
            let desc = str_(&n.description);
            let entries: Vec<_> = n
                .entries
                .iter()
                .map(|n| {
                    let sename = str_(&n.name).to_uppercase();
                    let sename = match sename.chars().next().unwrap() {
                        '0'..='9' => format!("{}{sename}", name_str.chars().next().unwrap()),
                        _ => sename,
                    };
                    let ename = tok_id(&sename);
                    let eval = parse_uint(str_(&n.value)).expect("uint value");
                    (
                        quote! { const #ename : #name = #name(#eval) },
                        quote! { (#sename, #name :: #ename) },
                    )
                })
                .collect();
            let entry = entries.iter().map(|t| &t.0);
            let debug = entries.iter().map(|t| &t.1);

            if n.bitfield {
                tokens.extend(quote! {
                    #[doc = #desc]
                    #[repr(transparent)]
                    #[derive(Clone,Copy,PartialEq,Eq)]
                    struct #name(u32);
                    impl #name {
                        #(#entry ;)*
                    }

                    impl std::fmt::Debug for #name {
                        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                            let vs : Vec<&str> = [#(#debug),*]
                                .iter()
                                .filter(|(_, v)| (*v & *self) == *v)
                                .map(|(s, _)| *s)
                                .collect();
                            write!(f, "{}", vs.join("|"))
                        }
                    }

                    impl From<u32> for #name { fn from(n: u32) -> #name { #name(n) } }
                    impl From<#name> for u32 { fn from(v: #name) -> u32 { v.0 } }
                    impl std::ops::BitOr for #name {
                        type Output = #name;
                        fn bitor(self, rhs: #name) -> #name { #name(self.0 | rhs.0) }
                    }
                    impl std::ops::BitAnd for #name {
                        type Output = #name;
                        fn bitand(self, rhs: #name) -> #name { #name(self.0 & rhs.0) }
                    }
                    impl Dsr for #name {
                        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
                            u32::dsr(buf).map(|n| n.into())
                        }
                        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
                            let v = u32::from(*self);
                            v.ser(buf)
                        }
                    }

                });
            } else {
                let entry: Vec<_> = n
                    .entries
                    .iter()
                    .map(|e| {
                        let ename = to_camelcase(str_(&e.name));
                        let ename0 = name_str.chars().next().unwrap();
                        let ename = tok_id(&match ename.chars().next().unwrap() {
                            '0'..='9' => format!("{ename0}{ename}"),
                            _ => ename,
                        });
                        let eval = parse_uint(str_(&e.value)).expect("enum value is u32");
                        (
                            quote! { #ename = #eval },
                            quote! { #eval => Ok(#name :: #ename) },
                            quote! { #name :: #ename => #eval },
                        )
                    })
                    .collect();

                let (entry, efrom, eto) = (
                    entry.iter().map(|e| &e.0),
                    entry.iter().map(|e| &e.1),
                    entry.iter().map(|e| &e.2),
                );
                tokens.extend(quote! {
                    #[doc = #desc]
                    #[repr(u32)]
                    enum #name { #(#entry ,)* }

                    impl TryFrom<u32> for #name {
                        type Error = ValueOutOfBounds;
                        fn try_from(n: u32) -> Result<#name, ValueOutOfBounds> {
                            match n {
                                #(#efrom ,)*
                                _ => Err(ValueOutOfBounds),
                            }
                        }
                    }
                    impl From<#name> for u32 {
                        fn from(n: #name) -> u32 {
                            match n { #(#eto ,)* }
                        }
                    }
                });
            }
        }
    }
}

impl ToTokens for TokenWrap<(bool, &'_ [Message])> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        if self.0 .1.len() == 0 {
            return;
        }

        fn arg_ty(a: &Arg) -> TokenStream {
            match a.r#type.as_slice() {
                b"object" | b"new_id" => match a.interface.as_ref() {
                    Some(iface) => {
                        let (ns, base) = ns_split(iface);
                        let (ns, base) = (ns.iter(), tok_id(base));
                        quote! { #(#ns ::)* #base :: Handle }
                    }
                    None => quote! { u32 },
                },
                b"int" => quote! { i32 },
                b"uint" => quote! { u32 },
                b"fixed" => quote! { Fixed },
                b"array" => quote! { Vec<u8> },
                b"string" => quote! { String },
                b"fd" => quote! { Fd },
                _ => panic!("unexpected arg type: {}", str_(&a.r#type)),
            }
        }

        let vs = self.0 .1.iter().map(|v| {
            let name = tok_id(&to_camelcase(str_(&v.name)));
            let args = v.args.iter().map(|a| {
                let (aname, aty) = (tok_id(str_(&a.name)), arg_ty(a));
                quote! { #aname : #aty }
            });
            quote! { #name { #(#args ,)* } }
        });
        let dsr = self.0 .1.iter().enumerate().map(|(op, v)| {
            let name = tok_id(&to_camelcase(str_(&v.name)));
            let args = v.args.iter().map(|a| tok_id(str_(&a.name)));
            let args_dsr = v.args.iter().map(|a| {
                let (aname, aty) = (tok_id(str_(&a.name)), arg_ty(a));
                quote! { let #aname = <#aty as Dsr>::dsr(buf)?; }
            });
            let op = op as u16;
            quote! {
                #op => {
                    #(#args_dsr)*
                    #name { #(#args),* }
                }
            }
        });
        let ser = self.0 .1.iter().enumerate().map(|(op, v)| {
            let name = tok_id(&to_camelcase(str_(&v.name)));
            let args: Vec<_> = v.args.iter().map(|a| tok_id(str_(&a.name))).collect();
            let op = op as u32;
            quote! {
                #name { #(#args),* } => {
                    #( #args.ser(buf)?; )*
                    #op
                }
            }
        });
        let mname = if self.0 .0 {
            tok_id("Event")
        } else {
            tok_id("Request")
        };
        tokens.extend(quote! {
            #[derive(Debug,Clone,PartialEq)]
            pub enum #mname { #(#vs,)* }

            impl Dsr for Message<Handle, #mname> {
                fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
                    let sz_start = buf.mark();
                    let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
                    let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
                    use #mname::*;
                    let event = match op {
                        #(#dsr)*
                        _ => panic!("unrecognized opcode: {}", op),
                    };
                    if wayland_debug() && (buf.mark() - sz_start) != 8+len {
                        eprintln!(
                            "actual_sz != expected_sz: {} != {}",
                            (buf.mark() - sz_start), 8+len);
                    }
                    Ok(Message(id, event))
                }
                fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
                    (self.0, 0u32).ser(buf)?;
                    let sz_start = buf.0.len();
                    use #mname::*;
                    let op : u32 = match &self.1 {
                        #(#ser)*
                    };
                    assert!(op < u16::MAX as u32);
                    let len = (buf.0.len() - sz_start) as u32;
                    assert!(len < u16::MAX as u32);
                    let oplen = (op << 16) | len;
                    buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
                    Ok(())
                }
            }
        });
    }
}

pub fn header() -> TokenStream {
    quote! {
        use std::os::fd::RawFd;
        fn wayland_debug() -> bool {
            std::env::var("WAYLAND_DEBUG")
                .map(|v| v == "1")
                .unwrap_or(false)
        }

        pub struct ReadBuf<'b>(&'b mut [u8], usize, &'b mut Vec<RawFd>);
        #[derive(Debug)]
        pub enum DsrError {
            InsufficientSpace { needs: usize },
            InsufficientFds { needs: usize },
            Utf(std::string::FromUtf8Error),
        }
        pub struct WriteBuf<'b>(&'b mut Vec<u8>, &'b mut Vec<RawFd>);
        #[derive(Debug)]
        pub enum SerError {}
        #[derive(Debug)]
        pub struct ValueOutOfBounds;

        impl WriteBuf<'_> {
            pub fn new<'b>(buf: &'b mut Vec<u8>, fds: &'b mut Vec<RawFd>) -> WriteBuf<'b> { WriteBuf(buf, fds) }
        }

        #[repr(transparent)]
        #[derive(Clone, Copy, PartialEq)]
        pub struct Fixed(i32);
        #[repr(transparent)]
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub struct Fd(RawFd);

        pub struct Message<Handle, Body>(pub Handle, pub Body);

        impl<'b> ReadBuf<'b> {
            fn mark(&self) -> usize {
                self.1
            }

            fn advance<const N: usize>(&mut self) -> Option<[u8; N]> {
                let bounds = self.1..(self.1 + N);
                self.0[bounds].try_into().ok()
            }

            fn advance_by(&mut self, n: usize) -> Option<&[u8]> {
                let bounds = self.1..(self.1 + n);
                self.0.get(bounds)
            }

            fn pop_fd(&mut self) -> Option<RawFd> {
                self.2.pop()
            }
        }

        impl std::fmt::Debug for Fixed {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}.{}", self.0 >> 8, "TODO")
            }
        }

        pub trait Dsr: Sized {
            fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError>;
            fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError>;
        }
        impl Dsr for u32 {
            fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
                const SZ: usize = std::mem::size_of::<u32>();
                let buf = buf.advance::<SZ>().map(u32::from_ne_bytes);
                buf.map(Ok)
                    .unwrap_or(Err(DsrError::InsufficientSpace { needs: SZ }))
            }
            fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
                buf.0.extend(&self.to_ne_bytes()[..]);
                Ok(())
            }
        }
        impl Dsr for i32 {
            fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
                const SZ: usize = std::mem::size_of::<i32>();
                let buf = buf.advance::<SZ>().map(i32::from_ne_bytes);
                buf.map(Ok)
                    .unwrap_or(Err(DsrError::InsufficientSpace { needs: SZ }))
            }
            fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
                buf.0.extend(&self.to_ne_bytes()[..]);
                Ok(())
            }
        }
        impl Dsr for Fixed {
            fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
                i32::dsr(buf).map(Fixed)
            }
            fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
                self.0.ser(buf)
            }
        }

        impl Dsr for Vec<u8> {
            fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
                let len = u32::dsr(buf)? as usize;
                let bs = buf.advance_by(len).map(|bs| Ok(bs.to_vec()));
                let pad = (((len + 3) / 4) * 4) - len;
                buf.advance_by(pad);
                bs.unwrap_or(Err(DsrError::InsufficientSpace { needs: len }))
            }
            fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
                let len = self.len() as u32;
                len.ser(buf)?;
                buf.0.extend(self);
                let pad = (((len + 3) / 4) * 4) - len;
                for _ in 0..pad {
                    buf.0.push(0);
                }
                Ok(())
            }
        }
        impl Dsr for String {
            fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
                let bs: Vec<u8> = Vec::dsr(buf)?;
                let s = String::from_utf8(bs).map_err(DsrError::Utf)?;
                Ok(s)
            }
            fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
                let len = self.len() as u32;
                len.ser(buf)?;
                buf.0.extend(self.as_bytes());
                let pad = (((len + 3) / 4) * 4) - len;
                for _ in 0..pad {
                    buf.0.push(0);
                }
                Ok(())
            }
        }

        impl Dsr for Fd {
            fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
                match buf.pop_fd() {
                    Some(fd) => Ok(Fd(fd)),
                    None => Err(DsrError::InsufficientFds { needs: 1 }),
                }
            }
            fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
                buf.1.push(self.0);
                Ok(())
            }
        }

        impl<T0: Dsr, T1: Dsr> Dsr for (T0, T1) {
            fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
                Ok((T0::dsr(buf)?, T1::dsr(buf)?))
            }
            fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
                self.0.ser(buf)?;
                self.1.ser(buf)?;
                Ok(())
            }
        }
    }
}

pub fn generate(p: &Protocol) -> TokenStream {
    let p = TokenWrap(p);
    quote! { #p }
}
