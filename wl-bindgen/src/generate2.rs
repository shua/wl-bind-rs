use crate::{str_, Arg, Enum, Interface, Message, Protocol};
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

impl<T> AsRef<[TokenWrap<T>]> for TokenWrap<&'_ [T]> {
    fn as_ref(&self) -> &[TokenWrap<T>] {
        // SAFETY: TokenWrap is repr(transparent)
        unsafe { std::mem::transmute(self.0) }
    }
}

impl ToTokens for TokenWrap<&'_ Interface> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let name = tok_id(str_(&self.0.name));
        let enums = TokenWrap(self.0.enums.as_slice());
        let events = TokenWrap(self.0.events.as_slice());
        let requests = TokenWrap(self.0.requests.as_slice());
        tokens.extend(
            quote! {

                mod #name {
                    #enums
                    #events
                    #requests
                    #[repr(transparent)]
                    #[Derive(Clone,Copy,PartialEq,Eq)]
                    struct Handle(u32);
                }
            }
            .into_iter(),
        )
    }
}

impl ToTokens for TokenWrap<&'_ [Enum]> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        todo!()
    }
}

impl ToTokens for TokenWrap<&'_ [Message]> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        todo!()
    }
}

fn generate(p: &Protocol) -> TokenStream {
    todo!()
}
