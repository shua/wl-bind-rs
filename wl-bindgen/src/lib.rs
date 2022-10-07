use proc_macro2::TokenStream;

mod parse;
pub use parse::parse;
mod generate;
pub use generate::generate;

fn str_<'s, S: ?Sized + AsRef<[u8]> + 's>(bs: &'s S) -> &'s str {
    unsafe { std::str::from_utf8_unchecked(bs.as_ref()) }
}

#[derive(Default)]
pub struct Protocol {
    name: Vec<u8>,
    copyright: Vec<u8>,
    interfaces: Vec<Interface>,
}
#[derive(Default)]
pub struct Interface {
    name: Vec<u8>,
    version: Vec<u8>,
    description: Vec<u8>,
    messages: Vec<Message>,
    enums: Vec<Enum>,
}
#[derive(Default)]
pub struct Arg {
    name: Vec<u8>,
    r#type: Vec<u8>,
    interface: Option<Vec<u8>>,
    allow_null: bool,
    r#enum: Option<Vec<u8>>,
    summary: Option<Vec<u8>>,
}
#[derive(PartialEq, Eq)]
pub enum MessageVariant {
    Request,
    Event,
}
pub struct Message {
    var: MessageVariant,
    name: Vec<u8>,
    description: Vec<u8>,
    args: Vec<Arg>,
}
#[derive(Default)]
pub struct Enum {
    name: Vec<u8>,
    description: Vec<u8>,
    entries: Vec<Entry>,
    bitfield: bool,
}
#[derive(Default)]
pub struct Entry {
    name: Vec<u8>,
    value: Vec<u8>,
    summary: Option<Vec<u8>>,
}

pub fn gen_bindings<R: std::io::Read>(mut r: R) -> Result<TokenStream, parse::ParseError> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf).unwrap();
    let protoc = parse(buf.as_slice())?;
    Ok(generate(&protoc))
}
