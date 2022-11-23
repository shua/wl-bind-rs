use std::os::fd::RawFd;
fn wayland_debug() -> bool {
    std::env::var("WAYLAND_DEBUG").map(|v| v == "1").unwrap_or(false)
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
    pub fn new<'b>(buf: &'b mut Vec<u8>, fds: &'b mut Vec<RawFd>) -> WriteBuf<'b> {
        WriteBuf(buf, fds)
    }
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
            .unwrap_or(
                Err(DsrError::InsufficientSpace {
                    needs: SZ,
                }),
            )
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
            .unwrap_or(
                Err(DsrError::InsufficientSpace {
                    needs: SZ,
                }),
            )
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
        bs.unwrap_or(
            Err(DsrError::InsufficientSpace {
                needs: len,
            }),
        )
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
            None => {
                Err(DsrError::InsufficientFds {
                    needs: 1,
                })
            }
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
/**
The core global object.  This is a special singleton object.  It
is used for internal Wayland protocol features.

*/
pub mod wl_display {
    use super::*;
    /**
These errors are global and can be emitted in response to any
server request.

*/
    #[repr(u32)]
    enum Error {
        InvalidObject = 0u32,
        InvalidMethod = 1u32,
        NoMemory = 2u32,
        Implementation = 3u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::InvalidObject),
                1u32 => Ok(Error::InvalidMethod),
                2u32 => Ok(Error::NoMemory),
                3u32 => Ok(Error::Implementation),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::InvalidObject => 0u32,
                Error::InvalidMethod => 1u32,
                Error::NoMemory => 2u32,
                Error::Implementation => 3u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Error { object_id: u32, code: u32, message: String },
        DeleteId { id: u32 },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let object_id = <u32 as Dsr>::dsr(buf)?;
                    let code = <u32 as Dsr>::dsr(buf)?;
                    let message = <String as Dsr>::dsr(buf)?;
                    Error { object_id, code, message }
                }
                1u16 => {
                    let id = <u32 as Dsr>::dsr(buf)?;
                    DeleteId { id }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Error { object_id, code, message } => {
                    object_id.ser(buf)?;
                    code.ser(buf)?;
                    message.ser(buf)?;
                    0u32
                }
                DeleteId { id } => {
                    id.ser(buf)?;
                    1u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Sync { callback: wl_callback::Handle },
        GetRegistry { registry: wl_registry::Handle },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let callback = <wl_callback::Handle as Dsr>::dsr(buf)?;
                    Sync { callback }
                }
                1u16 => {
                    let registry = <wl_registry::Handle as Dsr>::dsr(buf)?;
                    GetRegistry { registry }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Sync { callback } => {
                    callback.ser(buf)?;
                    0u32
                }
                GetRegistry { registry } => {
                    registry.ser(buf)?;
                    1u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_display", self.0)
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
/**
The singleton global registry object.  The server has a number of
global objects that are available to all clients.  These objects
typically represent an actual object in the server (for example,
an input device) or they are singleton objects that provide
extension functionality.

When a client creates a registry object, the registry object
will emit a global event for each global currently in the
registry.  Globals come and go as a result of device or
monitor hotplugs, reconfiguration or other events, and the
registry will send out global and global_remove events to
keep the client up to date with the changes.  To mark the end
of the initial burst of events, the client can use the
wl_display.sync request immediately after calling
wl_display.get_registry.

A client can bind to a global object by using the bind
request.  This creates a client-side handle that lets the object
emit events to the client and lets the client invoke requests on
the object.

*/
pub mod wl_registry {
    use super::*;
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Global { name: u32, interface: String, version: u32 },
        GlobalRemove { name: u32 },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let name = <u32 as Dsr>::dsr(buf)?;
                    let interface = <String as Dsr>::dsr(buf)?;
                    let version = <u32 as Dsr>::dsr(buf)?;
                    Global { name, interface, version }
                }
                1u16 => {
                    let name = <u32 as Dsr>::dsr(buf)?;
                    GlobalRemove { name }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Global { name, interface, version } => {
                    name.ser(buf)?;
                    interface.ser(buf)?;
                    version.ser(buf)?;
                    0u32
                }
                GlobalRemove { name } => {
                    name.ser(buf)?;
                    1u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Bind { name: u32, id: u32 },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let name = <u32 as Dsr>::dsr(buf)?;
                    let id = <u32 as Dsr>::dsr(buf)?;
                    Bind { name, id }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Bind { name, id } => {
                    name.ser(buf)?;
                    id.ser(buf)?;
                    0u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_registry", self.0)
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
/**
Clients can handle the 'done' event to get notified when
the related request is done.

*/
pub mod wl_callback {
    use super::*;
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Done { callback_data: u32 },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let callback_data = <u32 as Dsr>::dsr(buf)?;
                    Done { callback_data }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Done { callback_data } => {
                    callback_data.ser(buf)?;
                    0u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_callback", self.0)
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
/**
A compositor.  This object is a singleton global.  The
compositor is in charge of combining the contents of multiple
surfaces into one displayable output.

*/
pub mod wl_compositor {
    use super::*;
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        CreateSurface { id: wl_surface::Handle },
        CreateRegion { id: wl_region::Handle },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let id = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    CreateSurface { id }
                }
                1u16 => {
                    let id = <wl_region::Handle as Dsr>::dsr(buf)?;
                    CreateRegion { id }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                CreateSurface { id } => {
                    id.ser(buf)?;
                    0u32
                }
                CreateRegion { id } => {
                    id.ser(buf)?;
                    1u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_compositor", self.0)
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
/**
The wl_shm_pool object encapsulates a piece of memory shared
between the compositor and client.  Through the wl_shm_pool
object, the client can allocate shared memory wl_buffer objects.
All objects created through the same pool share the same
underlying mapped memory. Reusing the mapped memory avoids the
setup/teardown overhead and is useful when interactively resizing
a surface or for many small buffers.

*/
pub mod wl_shm_pool {
    use super::*;
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        CreateBuffer {
            id: wl_buffer::Handle,
            offset: i32,
            width: i32,
            height: i32,
            stride: i32,
            format: u32,
        },
        Destroy {},
        Resize { size: i32 },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let id = <wl_buffer::Handle as Dsr>::dsr(buf)?;
                    let offset = <i32 as Dsr>::dsr(buf)?;
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    let stride = <i32 as Dsr>::dsr(buf)?;
                    let format = <u32 as Dsr>::dsr(buf)?;
                    CreateBuffer {
                        id,
                        offset,
                        width,
                        height,
                        stride,
                        format,
                    }
                }
                1u16 => Destroy {},
                2u16 => {
                    let size = <i32 as Dsr>::dsr(buf)?;
                    Resize { size }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                CreateBuffer { id, offset, width, height, stride, format } => {
                    id.ser(buf)?;
                    offset.ser(buf)?;
                    width.ser(buf)?;
                    height.ser(buf)?;
                    stride.ser(buf)?;
                    format.ser(buf)?;
                    0u32
                }
                Destroy {} => 1u32,
                Resize { size } => {
                    size.ser(buf)?;
                    2u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_shm_pool", self.0)
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
/**
A singleton global object that provides support for shared
memory.

Clients can create wl_shm_pool objects using the create_pool
request.

On binding the wl_shm object one or more format events
are emitted to inform clients about the valid pixel formats
that can be used for buffers.

*/
pub mod wl_shm {
    use super::*;
    /**
These errors can be emitted in response to wl_shm requests.

*/
    #[repr(u32)]
    enum Error {
        InvalidFormat = 0u32,
        InvalidStride = 1u32,
        InvalidFd = 2u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::InvalidFormat),
                1u32 => Ok(Error::InvalidStride),
                2u32 => Ok(Error::InvalidFd),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::InvalidFormat => 0u32,
                Error::InvalidStride => 1u32,
                Error::InvalidFd => 2u32,
            }
        }
    }
    /**
This describes the memory layout of an individual pixel.

All renderers should support argb8888 and xrgb8888 but any other
formats are optional and may not be supported by the particular
renderer in use.

The drm format codes match the macros defined in drm_fourcc.h, except
argb8888 and xrgb8888. The formats actually supported by the compositor
will be reported by the format event.

For all wl_shm formats and unless specified in another protocol
extension, pre-multiplied alpha is used for pixel values.

*/
    #[repr(u32)]
    enum Format {
        Argb8888 = 0u32,
        Xrgb8888 = 1u32,
        C8 = 538982467u32,
        Rgb332 = 943867730u32,
        Bgr233 = 944916290u32,
        Xrgb4444 = 842093144u32,
        Xbgr4444 = 842089048u32,
        Rgbx4444 = 842094674u32,
        Bgrx4444 = 842094658u32,
        Argb4444 = 842093121u32,
        Abgr4444 = 842089025u32,
        Rgba4444 = 842088786u32,
        Bgra4444 = 842088770u32,
        Xrgb1555 = 892424792u32,
        Xbgr1555 = 892420696u32,
        Rgbx5551 = 892426322u32,
        Bgrx5551 = 892426306u32,
        Argb1555 = 892424769u32,
        Abgr1555 = 892420673u32,
        Rgba5551 = 892420434u32,
        Bgra5551 = 892420418u32,
        Rgb565 = 909199186u32,
        Bgr565 = 909199170u32,
        Rgb888 = 875710290u32,
        Bgr888 = 875710274u32,
        Xbgr8888 = 875709016u32,
        Rgbx8888 = 875714642u32,
        Bgrx8888 = 875714626u32,
        Abgr8888 = 875708993u32,
        Rgba8888 = 875708754u32,
        Bgra8888 = 875708738u32,
        Xrgb2101010 = 808669784u32,
        Xbgr2101010 = 808665688u32,
        Rgbx1010102 = 808671314u32,
        Bgrx1010102 = 808671298u32,
        Argb2101010 = 808669761u32,
        Abgr2101010 = 808665665u32,
        Rgba1010102 = 808665426u32,
        Bgra1010102 = 808665410u32,
        Yuyv = 1448695129u32,
        Yvyu = 1431918169u32,
        Uyvy = 1498831189u32,
        Vyuy = 1498765654u32,
        Ayuv = 1448433985u32,
        Nv12 = 842094158u32,
        Nv21 = 825382478u32,
        Nv16 = 909203022u32,
        Nv61 = 825644622u32,
        Yuv410 = 961959257u32,
        Yvu410 = 961893977u32,
        Yuv411 = 825316697u32,
        Yvu411 = 825316953u32,
        Yuv420 = 842093913u32,
        Yvu420 = 842094169u32,
        Yuv422 = 909202777u32,
        Yvu422 = 909203033u32,
        Yuv444 = 875713881u32,
        Yvu444 = 875714137u32,
        R8 = 538982482u32,
        R16 = 540422482u32,
        Rg88 = 943212370u32,
        Gr88 = 943215175u32,
        Rg1616 = 842221394u32,
        Gr1616 = 842224199u32,
        Xrgb16161616f = 1211388504u32,
        Xbgr16161616f = 1211384408u32,
        Argb16161616f = 1211388481u32,
        Abgr16161616f = 1211384385u32,
        Xyuv8888 = 1448434008u32,
        Vuy888 = 875713878u32,
        Vuy101010 = 808670550u32,
        Y210 = 808530521u32,
        Y212 = 842084953u32,
        Y216 = 909193817u32,
        Y410 = 808531033u32,
        Y412 = 842085465u32,
        Y416 = 909194329u32,
        Xvyu2101010 = 808670808u32,
        Xvyu1216161616 = 909334104u32,
        Xvyu16161616 = 942954072u32,
        Y0l0 = 810299481u32,
        X0l0 = 810299480u32,
        Y0l2 = 843853913u32,
        X0l2 = 843853912u32,
        Yuv4208bit = 942691673u32,
        Yuv42010bit = 808539481u32,
        Xrgb8888A8 = 943805016u32,
        Xbgr8888A8 = 943800920u32,
        Rgbx8888A8 = 943806546u32,
        Bgrx8888A8 = 943806530u32,
        Rgb888A8 = 943798354u32,
        Bgr888A8 = 943798338u32,
        Rgb565A8 = 943797586u32,
        Bgr565A8 = 943797570u32,
        Nv24 = 875714126u32,
        Nv42 = 842290766u32,
        P210 = 808530512u32,
        P010 = 808530000u32,
        P012 = 842084432u32,
        P016 = 909193296u32,
        Axbxgxrx106106106106 = 808534593u32,
        Nv15 = 892425806u32,
        Q410 = 808531025u32,
        Q401 = 825242705u32,
        Xrgb16161616 = 942953048u32,
        Xbgr16161616 = 942948952u32,
        Argb16161616 = 942953025u32,
        Abgr16161616 = 942948929u32,
    }
    impl TryFrom<u32> for Format {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Format, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Format::Argb8888),
                1u32 => Ok(Format::Xrgb8888),
                538982467u32 => Ok(Format::C8),
                943867730u32 => Ok(Format::Rgb332),
                944916290u32 => Ok(Format::Bgr233),
                842093144u32 => Ok(Format::Xrgb4444),
                842089048u32 => Ok(Format::Xbgr4444),
                842094674u32 => Ok(Format::Rgbx4444),
                842094658u32 => Ok(Format::Bgrx4444),
                842093121u32 => Ok(Format::Argb4444),
                842089025u32 => Ok(Format::Abgr4444),
                842088786u32 => Ok(Format::Rgba4444),
                842088770u32 => Ok(Format::Bgra4444),
                892424792u32 => Ok(Format::Xrgb1555),
                892420696u32 => Ok(Format::Xbgr1555),
                892426322u32 => Ok(Format::Rgbx5551),
                892426306u32 => Ok(Format::Bgrx5551),
                892424769u32 => Ok(Format::Argb1555),
                892420673u32 => Ok(Format::Abgr1555),
                892420434u32 => Ok(Format::Rgba5551),
                892420418u32 => Ok(Format::Bgra5551),
                909199186u32 => Ok(Format::Rgb565),
                909199170u32 => Ok(Format::Bgr565),
                875710290u32 => Ok(Format::Rgb888),
                875710274u32 => Ok(Format::Bgr888),
                875709016u32 => Ok(Format::Xbgr8888),
                875714642u32 => Ok(Format::Rgbx8888),
                875714626u32 => Ok(Format::Bgrx8888),
                875708993u32 => Ok(Format::Abgr8888),
                875708754u32 => Ok(Format::Rgba8888),
                875708738u32 => Ok(Format::Bgra8888),
                808669784u32 => Ok(Format::Xrgb2101010),
                808665688u32 => Ok(Format::Xbgr2101010),
                808671314u32 => Ok(Format::Rgbx1010102),
                808671298u32 => Ok(Format::Bgrx1010102),
                808669761u32 => Ok(Format::Argb2101010),
                808665665u32 => Ok(Format::Abgr2101010),
                808665426u32 => Ok(Format::Rgba1010102),
                808665410u32 => Ok(Format::Bgra1010102),
                1448695129u32 => Ok(Format::Yuyv),
                1431918169u32 => Ok(Format::Yvyu),
                1498831189u32 => Ok(Format::Uyvy),
                1498765654u32 => Ok(Format::Vyuy),
                1448433985u32 => Ok(Format::Ayuv),
                842094158u32 => Ok(Format::Nv12),
                825382478u32 => Ok(Format::Nv21),
                909203022u32 => Ok(Format::Nv16),
                825644622u32 => Ok(Format::Nv61),
                961959257u32 => Ok(Format::Yuv410),
                961893977u32 => Ok(Format::Yvu410),
                825316697u32 => Ok(Format::Yuv411),
                825316953u32 => Ok(Format::Yvu411),
                842093913u32 => Ok(Format::Yuv420),
                842094169u32 => Ok(Format::Yvu420),
                909202777u32 => Ok(Format::Yuv422),
                909203033u32 => Ok(Format::Yvu422),
                875713881u32 => Ok(Format::Yuv444),
                875714137u32 => Ok(Format::Yvu444),
                538982482u32 => Ok(Format::R8),
                540422482u32 => Ok(Format::R16),
                943212370u32 => Ok(Format::Rg88),
                943215175u32 => Ok(Format::Gr88),
                842221394u32 => Ok(Format::Rg1616),
                842224199u32 => Ok(Format::Gr1616),
                1211388504u32 => Ok(Format::Xrgb16161616f),
                1211384408u32 => Ok(Format::Xbgr16161616f),
                1211388481u32 => Ok(Format::Argb16161616f),
                1211384385u32 => Ok(Format::Abgr16161616f),
                1448434008u32 => Ok(Format::Xyuv8888),
                875713878u32 => Ok(Format::Vuy888),
                808670550u32 => Ok(Format::Vuy101010),
                808530521u32 => Ok(Format::Y210),
                842084953u32 => Ok(Format::Y212),
                909193817u32 => Ok(Format::Y216),
                808531033u32 => Ok(Format::Y410),
                842085465u32 => Ok(Format::Y412),
                909194329u32 => Ok(Format::Y416),
                808670808u32 => Ok(Format::Xvyu2101010),
                909334104u32 => Ok(Format::Xvyu1216161616),
                942954072u32 => Ok(Format::Xvyu16161616),
                810299481u32 => Ok(Format::Y0l0),
                810299480u32 => Ok(Format::X0l0),
                843853913u32 => Ok(Format::Y0l2),
                843853912u32 => Ok(Format::X0l2),
                942691673u32 => Ok(Format::Yuv4208bit),
                808539481u32 => Ok(Format::Yuv42010bit),
                943805016u32 => Ok(Format::Xrgb8888A8),
                943800920u32 => Ok(Format::Xbgr8888A8),
                943806546u32 => Ok(Format::Rgbx8888A8),
                943806530u32 => Ok(Format::Bgrx8888A8),
                943798354u32 => Ok(Format::Rgb888A8),
                943798338u32 => Ok(Format::Bgr888A8),
                943797586u32 => Ok(Format::Rgb565A8),
                943797570u32 => Ok(Format::Bgr565A8),
                875714126u32 => Ok(Format::Nv24),
                842290766u32 => Ok(Format::Nv42),
                808530512u32 => Ok(Format::P210),
                808530000u32 => Ok(Format::P010),
                842084432u32 => Ok(Format::P012),
                909193296u32 => Ok(Format::P016),
                808534593u32 => Ok(Format::Axbxgxrx106106106106),
                892425806u32 => Ok(Format::Nv15),
                808531025u32 => Ok(Format::Q410),
                825242705u32 => Ok(Format::Q401),
                942953048u32 => Ok(Format::Xrgb16161616),
                942948952u32 => Ok(Format::Xbgr16161616),
                942953025u32 => Ok(Format::Argb16161616),
                942948929u32 => Ok(Format::Abgr16161616),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Format> for u32 {
        fn from(n: Format) -> u32 {
            match n {
                Format::Argb8888 => 0u32,
                Format::Xrgb8888 => 1u32,
                Format::C8 => 538982467u32,
                Format::Rgb332 => 943867730u32,
                Format::Bgr233 => 944916290u32,
                Format::Xrgb4444 => 842093144u32,
                Format::Xbgr4444 => 842089048u32,
                Format::Rgbx4444 => 842094674u32,
                Format::Bgrx4444 => 842094658u32,
                Format::Argb4444 => 842093121u32,
                Format::Abgr4444 => 842089025u32,
                Format::Rgba4444 => 842088786u32,
                Format::Bgra4444 => 842088770u32,
                Format::Xrgb1555 => 892424792u32,
                Format::Xbgr1555 => 892420696u32,
                Format::Rgbx5551 => 892426322u32,
                Format::Bgrx5551 => 892426306u32,
                Format::Argb1555 => 892424769u32,
                Format::Abgr1555 => 892420673u32,
                Format::Rgba5551 => 892420434u32,
                Format::Bgra5551 => 892420418u32,
                Format::Rgb565 => 909199186u32,
                Format::Bgr565 => 909199170u32,
                Format::Rgb888 => 875710290u32,
                Format::Bgr888 => 875710274u32,
                Format::Xbgr8888 => 875709016u32,
                Format::Rgbx8888 => 875714642u32,
                Format::Bgrx8888 => 875714626u32,
                Format::Abgr8888 => 875708993u32,
                Format::Rgba8888 => 875708754u32,
                Format::Bgra8888 => 875708738u32,
                Format::Xrgb2101010 => 808669784u32,
                Format::Xbgr2101010 => 808665688u32,
                Format::Rgbx1010102 => 808671314u32,
                Format::Bgrx1010102 => 808671298u32,
                Format::Argb2101010 => 808669761u32,
                Format::Abgr2101010 => 808665665u32,
                Format::Rgba1010102 => 808665426u32,
                Format::Bgra1010102 => 808665410u32,
                Format::Yuyv => 1448695129u32,
                Format::Yvyu => 1431918169u32,
                Format::Uyvy => 1498831189u32,
                Format::Vyuy => 1498765654u32,
                Format::Ayuv => 1448433985u32,
                Format::Nv12 => 842094158u32,
                Format::Nv21 => 825382478u32,
                Format::Nv16 => 909203022u32,
                Format::Nv61 => 825644622u32,
                Format::Yuv410 => 961959257u32,
                Format::Yvu410 => 961893977u32,
                Format::Yuv411 => 825316697u32,
                Format::Yvu411 => 825316953u32,
                Format::Yuv420 => 842093913u32,
                Format::Yvu420 => 842094169u32,
                Format::Yuv422 => 909202777u32,
                Format::Yvu422 => 909203033u32,
                Format::Yuv444 => 875713881u32,
                Format::Yvu444 => 875714137u32,
                Format::R8 => 538982482u32,
                Format::R16 => 540422482u32,
                Format::Rg88 => 943212370u32,
                Format::Gr88 => 943215175u32,
                Format::Rg1616 => 842221394u32,
                Format::Gr1616 => 842224199u32,
                Format::Xrgb16161616f => 1211388504u32,
                Format::Xbgr16161616f => 1211384408u32,
                Format::Argb16161616f => 1211388481u32,
                Format::Abgr16161616f => 1211384385u32,
                Format::Xyuv8888 => 1448434008u32,
                Format::Vuy888 => 875713878u32,
                Format::Vuy101010 => 808670550u32,
                Format::Y210 => 808530521u32,
                Format::Y212 => 842084953u32,
                Format::Y216 => 909193817u32,
                Format::Y410 => 808531033u32,
                Format::Y412 => 842085465u32,
                Format::Y416 => 909194329u32,
                Format::Xvyu2101010 => 808670808u32,
                Format::Xvyu1216161616 => 909334104u32,
                Format::Xvyu16161616 => 942954072u32,
                Format::Y0l0 => 810299481u32,
                Format::X0l0 => 810299480u32,
                Format::Y0l2 => 843853913u32,
                Format::X0l2 => 843853912u32,
                Format::Yuv4208bit => 942691673u32,
                Format::Yuv42010bit => 808539481u32,
                Format::Xrgb8888A8 => 943805016u32,
                Format::Xbgr8888A8 => 943800920u32,
                Format::Rgbx8888A8 => 943806546u32,
                Format::Bgrx8888A8 => 943806530u32,
                Format::Rgb888A8 => 943798354u32,
                Format::Bgr888A8 => 943798338u32,
                Format::Rgb565A8 => 943797586u32,
                Format::Bgr565A8 => 943797570u32,
                Format::Nv24 => 875714126u32,
                Format::Nv42 => 842290766u32,
                Format::P210 => 808530512u32,
                Format::P010 => 808530000u32,
                Format::P012 => 842084432u32,
                Format::P016 => 909193296u32,
                Format::Axbxgxrx106106106106 => 808534593u32,
                Format::Nv15 => 892425806u32,
                Format::Q410 => 808531025u32,
                Format::Q401 => 825242705u32,
                Format::Xrgb16161616 => 942953048u32,
                Format::Xbgr16161616 => 942948952u32,
                Format::Argb16161616 => 942953025u32,
                Format::Abgr16161616 => 942948929u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Format { format: u32 },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let format = <u32 as Dsr>::dsr(buf)?;
                    Format { format }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Format { format } => {
                    format.ser(buf)?;
                    0u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        CreatePool { id: wl_shm_pool::Handle, fd: Fd, size: i32 },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let id = <wl_shm_pool::Handle as Dsr>::dsr(buf)?;
                    let fd = <Fd as Dsr>::dsr(buf)?;
                    let size = <i32 as Dsr>::dsr(buf)?;
                    CreatePool { id, fd, size }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                CreatePool { id, fd, size } => {
                    id.ser(buf)?;
                    fd.ser(buf)?;
                    size.ser(buf)?;
                    0u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_shm", self.0)
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
/**
A buffer provides the content for a wl_surface. Buffers are
created through factory interfaces such as wl_shm, wp_linux_buffer_params
(from the linux-dmabuf protocol extension) or similar. It has a width and
a height and can be attached to a wl_surface, but the mechanism by which a
client provides and updates the contents is defined by the buffer factory
interface.

If the buffer uses a format that has an alpha channel, the alpha channel
is assumed to be premultiplied in the color channels unless otherwise
specified.

*/
pub mod wl_buffer {
    use super::*;
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Release {},
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => Release {},
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Release {} => 0u32,
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Destroy {},
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => Destroy {},
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Destroy {} => 0u32,
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_buffer", self.0)
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
/**
A wl_data_offer represents a piece of data offered for transfer
by another client (the source client).  It is used by the
copy-and-paste and drag-and-drop mechanisms.  The offer
describes the different mime types that the data can be
converted to and provides the mechanism for transferring the
data directly from the source client.

*/
pub mod wl_data_offer {
    use super::*;
    ///
    #[repr(u32)]
    enum Error {
        InvalidFinish = 0u32,
        InvalidActionMask = 1u32,
        InvalidAction = 2u32,
        InvalidOffer = 3u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::InvalidFinish),
                1u32 => Ok(Error::InvalidActionMask),
                2u32 => Ok(Error::InvalidAction),
                3u32 => Ok(Error::InvalidOffer),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::InvalidFinish => 0u32,
                Error::InvalidActionMask => 1u32,
                Error::InvalidAction => 2u32,
                Error::InvalidOffer => 3u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Offer { mime_type: String },
        SourceActions { source_actions: u32 },
        Action { dnd_action: u32 },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let mime_type = <String as Dsr>::dsr(buf)?;
                    Offer { mime_type }
                }
                1u16 => {
                    let source_actions = <u32 as Dsr>::dsr(buf)?;
                    SourceActions { source_actions }
                }
                2u16 => {
                    let dnd_action = <u32 as Dsr>::dsr(buf)?;
                    Action { dnd_action }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Offer { mime_type } => {
                    mime_type.ser(buf)?;
                    0u32
                }
                SourceActions { source_actions } => {
                    source_actions.ser(buf)?;
                    1u32
                }
                Action { dnd_action } => {
                    dnd_action.ser(buf)?;
                    2u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Accept { serial: u32, mime_type: String },
        Receive { mime_type: String, fd: Fd },
        Destroy {},
        Finish {},
        SetActions { dnd_actions: u32, preferred_action: u32 },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let mime_type = <String as Dsr>::dsr(buf)?;
                    Accept { serial, mime_type }
                }
                1u16 => {
                    let mime_type = <String as Dsr>::dsr(buf)?;
                    let fd = <Fd as Dsr>::dsr(buf)?;
                    Receive { mime_type, fd }
                }
                2u16 => Destroy {},
                3u16 => Finish {},
                4u16 => {
                    let dnd_actions = <u32 as Dsr>::dsr(buf)?;
                    let preferred_action = <u32 as Dsr>::dsr(buf)?;
                    SetActions {
                        dnd_actions,
                        preferred_action,
                    }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Accept { serial, mime_type } => {
                    serial.ser(buf)?;
                    mime_type.ser(buf)?;
                    0u32
                }
                Receive { mime_type, fd } => {
                    mime_type.ser(buf)?;
                    fd.ser(buf)?;
                    1u32
                }
                Destroy {} => 2u32,
                Finish {} => 3u32,
                SetActions { dnd_actions, preferred_action } => {
                    dnd_actions.ser(buf)?;
                    preferred_action.ser(buf)?;
                    4u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_data_offer", self.0)
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
/**
The wl_data_source object is the source side of a wl_data_offer.
It is created by the source client in a data transfer and
provides a way to describe the offered data and a way to respond
to requests to transfer the data.

*/
pub mod wl_data_source {
    use super::*;
    ///
    #[repr(u32)]
    enum Error {
        InvalidActionMask = 0u32,
        InvalidSource = 1u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::InvalidActionMask),
                1u32 => Ok(Error::InvalidSource),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::InvalidActionMask => 0u32,
                Error::InvalidSource => 1u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Target { mime_type: String },
        Send { mime_type: String, fd: Fd },
        Cancelled {},
        DndDropPerformed {},
        DndFinished {},
        Action { dnd_action: u32 },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let mime_type = <String as Dsr>::dsr(buf)?;
                    Target { mime_type }
                }
                1u16 => {
                    let mime_type = <String as Dsr>::dsr(buf)?;
                    let fd = <Fd as Dsr>::dsr(buf)?;
                    Send { mime_type, fd }
                }
                2u16 => Cancelled {},
                3u16 => DndDropPerformed {},
                4u16 => DndFinished {},
                5u16 => {
                    let dnd_action = <u32 as Dsr>::dsr(buf)?;
                    Action { dnd_action }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Target { mime_type } => {
                    mime_type.ser(buf)?;
                    0u32
                }
                Send { mime_type, fd } => {
                    mime_type.ser(buf)?;
                    fd.ser(buf)?;
                    1u32
                }
                Cancelled {} => 2u32,
                DndDropPerformed {} => 3u32,
                DndFinished {} => 4u32,
                Action { dnd_action } => {
                    dnd_action.ser(buf)?;
                    5u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Offer { mime_type: String },
        Destroy {},
        SetActions { dnd_actions: u32 },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let mime_type = <String as Dsr>::dsr(buf)?;
                    Offer { mime_type }
                }
                1u16 => Destroy {},
                2u16 => {
                    let dnd_actions = <u32 as Dsr>::dsr(buf)?;
                    SetActions { dnd_actions }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Offer { mime_type } => {
                    mime_type.ser(buf)?;
                    0u32
                }
                Destroy {} => 1u32,
                SetActions { dnd_actions } => {
                    dnd_actions.ser(buf)?;
                    2u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_data_source", self.0)
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
/**
There is one wl_data_device per seat which can be obtained
from the global wl_data_device_manager singleton.

A wl_data_device provides access to inter-client data transfer
mechanisms such as copy-and-paste and drag-and-drop.

*/
pub mod wl_data_device {
    use super::*;
    ///
    #[repr(u32)]
    enum Error {
        Role = 0u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::Role),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::Role => 0u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        DataOffer { id: wl_data_offer::Handle },
        Enter {
            serial: u32,
            surface: wl_surface::Handle,
            x: Fixed,
            y: Fixed,
            id: wl_data_offer::Handle,
        },
        Leave {},
        Motion { time: u32, x: Fixed, y: Fixed },
        Drop {},
        Selection { id: wl_data_offer::Handle },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let id = <wl_data_offer::Handle as Dsr>::dsr(buf)?;
                    DataOffer { id }
                }
                1u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let surface = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    let x = <Fixed as Dsr>::dsr(buf)?;
                    let y = <Fixed as Dsr>::dsr(buf)?;
                    let id = <wl_data_offer::Handle as Dsr>::dsr(buf)?;
                    Enter { serial, surface, x, y, id }
                }
                2u16 => Leave {},
                3u16 => {
                    let time = <u32 as Dsr>::dsr(buf)?;
                    let x = <Fixed as Dsr>::dsr(buf)?;
                    let y = <Fixed as Dsr>::dsr(buf)?;
                    Motion { time, x, y }
                }
                4u16 => Drop {},
                5u16 => {
                    let id = <wl_data_offer::Handle as Dsr>::dsr(buf)?;
                    Selection { id }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                DataOffer { id } => {
                    id.ser(buf)?;
                    0u32
                }
                Enter { serial, surface, x, y, id } => {
                    serial.ser(buf)?;
                    surface.ser(buf)?;
                    x.ser(buf)?;
                    y.ser(buf)?;
                    id.ser(buf)?;
                    1u32
                }
                Leave {} => 2u32,
                Motion { time, x, y } => {
                    time.ser(buf)?;
                    x.ser(buf)?;
                    y.ser(buf)?;
                    3u32
                }
                Drop {} => 4u32,
                Selection { id } => {
                    id.ser(buf)?;
                    5u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        StartDrag {
            source: wl_data_source::Handle,
            origin: wl_surface::Handle,
            icon: wl_surface::Handle,
            serial: u32,
        },
        SetSelection { source: wl_data_source::Handle, serial: u32 },
        Release {},
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let source = <wl_data_source::Handle as Dsr>::dsr(buf)?;
                    let origin = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    let icon = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    StartDrag {
                        source,
                        origin,
                        icon,
                        serial,
                    }
                }
                1u16 => {
                    let source = <wl_data_source::Handle as Dsr>::dsr(buf)?;
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    SetSelection { source, serial }
                }
                2u16 => Release {},
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                StartDrag { source, origin, icon, serial } => {
                    source.ser(buf)?;
                    origin.ser(buf)?;
                    icon.ser(buf)?;
                    serial.ser(buf)?;
                    0u32
                }
                SetSelection { source, serial } => {
                    source.ser(buf)?;
                    serial.ser(buf)?;
                    1u32
                }
                Release {} => 2u32,
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_data_device", self.0)
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
/**
The wl_data_device_manager is a singleton global object that
provides access to inter-client data transfer mechanisms such as
copy-and-paste and drag-and-drop.  These mechanisms are tied to
a wl_seat and this interface lets a client get a wl_data_device
corresponding to a wl_seat.

Depending on the version bound, the objects created from the bound
wl_data_device_manager object will have different requirements for
functioning properly. See wl_data_source.set_actions,
wl_data_offer.accept and wl_data_offer.finish for details.

*/
pub mod wl_data_device_manager {
    use super::*;
    /**
This is a bitmask of the available/preferred actions in a
drag-and-drop operation.

In the compositor, the selected action is a result of matching the
actions offered by the source and destination sides.  "action" events
with a "none" action will be sent to both source and destination if
there is no match. All further checks will effectively happen on
(source actions ∩ destination actions).

In addition, compositors may also pick different actions in
reaction to key modifiers being pressed. One common design that
is used in major toolkits (and the behavior recommended for
compositors) is:

- If no modifiers are pressed, the first match (in bit order)
will be used.
- Pressing Shift selects "move", if enabled in the mask.
- Pressing Control selects "copy", if enabled in the mask.

Behavior beyond that is considered implementation-dependent.
Compositors may for example bind other modifiers (like Alt/Meta)
or drags initiated with other buttons than BTN_LEFT to specific
actions (e.g. "ask").

*/
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct DndAction(u32);
    impl DndAction {
        const NONE: DndAction = DndAction(0u32);
        const COPY: DndAction = DndAction(1u32);
        const MOVE: DndAction = DndAction(2u32);
        const ASK: DndAction = DndAction(4u32);
    }
    impl std::fmt::Debug for DndAction {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            let vs: Vec<&str> = [
                ("NONE", DndAction::NONE),
                ("COPY", DndAction::COPY),
                ("MOVE", DndAction::MOVE),
                ("ASK", DndAction::ASK),
            ]
                .iter()
                .filter(|(_, v)| (*v & *self) == *v)
                .map(|(s, _)| *s)
                .collect();
            write!(f, "{}", vs.join("|"))
        }
    }
    impl From<u32> for DndAction {
        fn from(n: u32) -> DndAction {
            DndAction(n)
        }
    }
    impl From<DndAction> for u32 {
        fn from(v: DndAction) -> u32 {
            v.0
        }
    }
    impl std::ops::BitOr for DndAction {
        type Output = DndAction;
        fn bitor(self, rhs: DndAction) -> DndAction {
            DndAction(self.0 | rhs.0)
        }
    }
    impl std::ops::BitAnd for DndAction {
        type Output = DndAction;
        fn bitand(self, rhs: DndAction) -> DndAction {
            DndAction(self.0 & rhs.0)
        }
    }
    impl Dsr for DndAction {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            u32::dsr(buf).map(|n| n.into())
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            let v = u32::from(*self);
            v.ser(buf)
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        CreateDataSource { id: wl_data_source::Handle },
        GetDataDevice { id: wl_data_device::Handle, seat: wl_seat::Handle },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let id = <wl_data_source::Handle as Dsr>::dsr(buf)?;
                    CreateDataSource { id }
                }
                1u16 => {
                    let id = <wl_data_device::Handle as Dsr>::dsr(buf)?;
                    let seat = <wl_seat::Handle as Dsr>::dsr(buf)?;
                    GetDataDevice { id, seat }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                CreateDataSource { id } => {
                    id.ser(buf)?;
                    0u32
                }
                GetDataDevice { id, seat } => {
                    id.ser(buf)?;
                    seat.ser(buf)?;
                    1u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_data_device_manager", self.0)
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
/**
This interface is implemented by servers that provide
desktop-style user interfaces.

It allows clients to associate a wl_shell_surface with
a basic surface.

Note! This protocol is deprecated and not intended for production use.
For desktop-style user interfaces, use xdg_shell. Compositors and clients
should not implement this interface.

*/
pub mod wl_shell {
    use super::*;
    ///
    #[repr(u32)]
    enum Error {
        Role = 0u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::Role),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::Role => 0u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        GetShellSurface { id: wl_shell_surface::Handle, surface: wl_surface::Handle },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let id = <wl_shell_surface::Handle as Dsr>::dsr(buf)?;
                    let surface = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    GetShellSurface { id, surface }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                GetShellSurface { id, surface } => {
                    id.ser(buf)?;
                    surface.ser(buf)?;
                    0u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_shell", self.0)
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
/**
An interface that may be implemented by a wl_surface, for
implementations that provide a desktop-style user interface.

It provides requests to treat surfaces like toplevel, fullscreen
or popup windows, move, resize or maximize them, associate
metadata like title and class, etc.

On the server side the object is automatically destroyed when
the related wl_surface is destroyed. On the client side,
wl_shell_surface_destroy() must be called before destroying
the wl_surface object.

*/
pub mod wl_shell_surface {
    use super::*;
    /**
These values are used to indicate which edge of a surface
is being dragged in a resize operation. The server may
use this information to adapt its behavior, e.g. choose
an appropriate cursor image.

*/
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Resize(u32);
    impl Resize {
        const NONE: Resize = Resize(0u32);
        const TOP: Resize = Resize(1u32);
        const BOTTOM: Resize = Resize(2u32);
        const LEFT: Resize = Resize(4u32);
        const TOP_LEFT: Resize = Resize(5u32);
        const BOTTOM_LEFT: Resize = Resize(6u32);
        const RIGHT: Resize = Resize(8u32);
        const TOP_RIGHT: Resize = Resize(9u32);
        const BOTTOM_RIGHT: Resize = Resize(10u32);
    }
    impl std::fmt::Debug for Resize {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            let vs: Vec<&str> = [
                ("NONE", Resize::NONE),
                ("TOP", Resize::TOP),
                ("BOTTOM", Resize::BOTTOM),
                ("LEFT", Resize::LEFT),
                ("TOP_LEFT", Resize::TOP_LEFT),
                ("BOTTOM_LEFT", Resize::BOTTOM_LEFT),
                ("RIGHT", Resize::RIGHT),
                ("TOP_RIGHT", Resize::TOP_RIGHT),
                ("BOTTOM_RIGHT", Resize::BOTTOM_RIGHT),
            ]
                .iter()
                .filter(|(_, v)| (*v & *self) == *v)
                .map(|(s, _)| *s)
                .collect();
            write!(f, "{}", vs.join("|"))
        }
    }
    impl From<u32> for Resize {
        fn from(n: u32) -> Resize {
            Resize(n)
        }
    }
    impl From<Resize> for u32 {
        fn from(v: Resize) -> u32 {
            v.0
        }
    }
    impl std::ops::BitOr for Resize {
        type Output = Resize;
        fn bitor(self, rhs: Resize) -> Resize {
            Resize(self.0 | rhs.0)
        }
    }
    impl std::ops::BitAnd for Resize {
        type Output = Resize;
        fn bitand(self, rhs: Resize) -> Resize {
            Resize(self.0 & rhs.0)
        }
    }
    impl Dsr for Resize {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            u32::dsr(buf).map(|n| n.into())
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            let v = u32::from(*self);
            v.ser(buf)
        }
    }
    /**
These flags specify details of the expected behaviour
of transient surfaces. Used in the set_transient request.

*/
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Transient(u32);
    impl Transient {
        const INACTIVE: Transient = Transient(1u32);
    }
    impl std::fmt::Debug for Transient {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            let vs: Vec<&str> = [("INACTIVE", Transient::INACTIVE)]
                .iter()
                .filter(|(_, v)| (*v & *self) == *v)
                .map(|(s, _)| *s)
                .collect();
            write!(f, "{}", vs.join("|"))
        }
    }
    impl From<u32> for Transient {
        fn from(n: u32) -> Transient {
            Transient(n)
        }
    }
    impl From<Transient> for u32 {
        fn from(v: Transient) -> u32 {
            v.0
        }
    }
    impl std::ops::BitOr for Transient {
        type Output = Transient;
        fn bitor(self, rhs: Transient) -> Transient {
            Transient(self.0 | rhs.0)
        }
    }
    impl std::ops::BitAnd for Transient {
        type Output = Transient;
        fn bitand(self, rhs: Transient) -> Transient {
            Transient(self.0 & rhs.0)
        }
    }
    impl Dsr for Transient {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            u32::dsr(buf).map(|n| n.into())
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            let v = u32::from(*self);
            v.ser(buf)
        }
    }
    /**
Hints to indicate to the compositor how to deal with a conflict
between the dimensions of the surface and the dimensions of the
output. The compositor is free to ignore this parameter.

*/
    #[repr(u32)]
    enum FullscreenMethod {
        Default = 0u32,
        Scale = 1u32,
        Driver = 2u32,
        Fill = 3u32,
    }
    impl TryFrom<u32> for FullscreenMethod {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<FullscreenMethod, ValueOutOfBounds> {
            match n {
                0u32 => Ok(FullscreenMethod::Default),
                1u32 => Ok(FullscreenMethod::Scale),
                2u32 => Ok(FullscreenMethod::Driver),
                3u32 => Ok(FullscreenMethod::Fill),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<FullscreenMethod> for u32 {
        fn from(n: FullscreenMethod) -> u32 {
            match n {
                FullscreenMethod::Default => 0u32,
                FullscreenMethod::Scale => 1u32,
                FullscreenMethod::Driver => 2u32,
                FullscreenMethod::Fill => 3u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Ping { serial: u32 },
        Configure { edges: u32, width: i32, height: i32 },
        PopupDone {},
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    Ping { serial }
                }
                1u16 => {
                    let edges = <u32 as Dsr>::dsr(buf)?;
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    Configure { edges, width, height }
                }
                2u16 => PopupDone {},
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Ping { serial } => {
                    serial.ser(buf)?;
                    0u32
                }
                Configure { edges, width, height } => {
                    edges.ser(buf)?;
                    width.ser(buf)?;
                    height.ser(buf)?;
                    1u32
                }
                PopupDone {} => 2u32,
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Pong { serial: u32 },
        Move { seat: wl_seat::Handle, serial: u32 },
        Resize { seat: wl_seat::Handle, serial: u32, edges: u32 },
        SetToplevel {},
        SetTransient { parent: wl_surface::Handle, x: i32, y: i32, flags: u32 },
        SetFullscreen { method: u32, framerate: u32, output: wl_output::Handle },
        SetPopup {
            seat: wl_seat::Handle,
            serial: u32,
            parent: wl_surface::Handle,
            x: i32,
            y: i32,
            flags: u32,
        },
        SetMaximized { output: wl_output::Handle },
        SetTitle { title: String },
        SetClass { class_: String },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    Pong { serial }
                }
                1u16 => {
                    let seat = <wl_seat::Handle as Dsr>::dsr(buf)?;
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    Move { seat, serial }
                }
                2u16 => {
                    let seat = <wl_seat::Handle as Dsr>::dsr(buf)?;
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let edges = <u32 as Dsr>::dsr(buf)?;
                    Resize { seat, serial, edges }
                }
                3u16 => SetToplevel {},
                4u16 => {
                    let parent = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    let flags = <u32 as Dsr>::dsr(buf)?;
                    SetTransient {
                        parent,
                        x,
                        y,
                        flags,
                    }
                }
                5u16 => {
                    let method = <u32 as Dsr>::dsr(buf)?;
                    let framerate = <u32 as Dsr>::dsr(buf)?;
                    let output = <wl_output::Handle as Dsr>::dsr(buf)?;
                    SetFullscreen {
                        method,
                        framerate,
                        output,
                    }
                }
                6u16 => {
                    let seat = <wl_seat::Handle as Dsr>::dsr(buf)?;
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let parent = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    let flags = <u32 as Dsr>::dsr(buf)?;
                    SetPopup {
                        seat,
                        serial,
                        parent,
                        x,
                        y,
                        flags,
                    }
                }
                7u16 => {
                    let output = <wl_output::Handle as Dsr>::dsr(buf)?;
                    SetMaximized { output }
                }
                8u16 => {
                    let title = <String as Dsr>::dsr(buf)?;
                    SetTitle { title }
                }
                9u16 => {
                    let class_ = <String as Dsr>::dsr(buf)?;
                    SetClass { class_ }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Pong { serial } => {
                    serial.ser(buf)?;
                    0u32
                }
                Move { seat, serial } => {
                    seat.ser(buf)?;
                    serial.ser(buf)?;
                    1u32
                }
                Resize { seat, serial, edges } => {
                    seat.ser(buf)?;
                    serial.ser(buf)?;
                    edges.ser(buf)?;
                    2u32
                }
                SetToplevel {} => 3u32,
                SetTransient { parent, x, y, flags } => {
                    parent.ser(buf)?;
                    x.ser(buf)?;
                    y.ser(buf)?;
                    flags.ser(buf)?;
                    4u32
                }
                SetFullscreen { method, framerate, output } => {
                    method.ser(buf)?;
                    framerate.ser(buf)?;
                    output.ser(buf)?;
                    5u32
                }
                SetPopup { seat, serial, parent, x, y, flags } => {
                    seat.ser(buf)?;
                    serial.ser(buf)?;
                    parent.ser(buf)?;
                    x.ser(buf)?;
                    y.ser(buf)?;
                    flags.ser(buf)?;
                    6u32
                }
                SetMaximized { output } => {
                    output.ser(buf)?;
                    7u32
                }
                SetTitle { title } => {
                    title.ser(buf)?;
                    8u32
                }
                SetClass { class_ } => {
                    class_.ser(buf)?;
                    9u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_shell_surface", self.0)
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
/**
A surface is a rectangular area that may be displayed on zero
or more outputs, and shown any number of times at the compositor's
discretion. They can present wl_buffers, receive user input, and
define a local coordinate system.

The size of a surface (and relative positions on it) is described
in surface-local coordinates, which may differ from the buffer
coordinates of the pixel content, in case a buffer_transform
or a buffer_scale is used.

A surface without a "role" is fairly useless: a compositor does
not know where, when or how to present it. The role is the
purpose of a wl_surface. Examples of roles are a cursor for a
pointer (as set by wl_pointer.set_cursor), a drag icon
(wl_data_device.start_drag), a sub-surface
(wl_subcompositor.get_subsurface), and a window as defined by a
shell protocol (e.g. wl_shell.get_shell_surface).

A surface can have only one role at a time. Initially a
wl_surface does not have a role. Once a wl_surface is given a
role, it is set permanently for the whole lifetime of the
wl_surface object. Giving the current role again is allowed,
unless explicitly forbidden by the relevant interface
specification.

Surface roles are given by requests in other interfaces such as
wl_pointer.set_cursor. The request should explicitly mention
that this request gives a role to a wl_surface. Often, this
request also creates a new protocol object that represents the
role and adds additional functionality to wl_surface. When a
client wants to destroy a wl_surface, they must destroy this 'role
object' before the wl_surface.

Destroying the role object does not remove the role from the
wl_surface, but it may stop the wl_surface from "playing the role".
For instance, if a wl_subsurface object is destroyed, the wl_surface
it was created for will be unmapped and forget its position and
z-order. It is allowed to create a wl_subsurface for the same
wl_surface again, but it is not allowed to use the wl_surface as
a cursor (cursor is a different role than sub-surface, and role
switching is not allowed).

*/
pub mod wl_surface {
    use super::*;
    /**
These errors can be emitted in response to wl_surface requests.

*/
    #[repr(u32)]
    enum Error {
        InvalidScale = 0u32,
        InvalidTransform = 1u32,
        InvalidSize = 2u32,
        InvalidOffset = 3u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::InvalidScale),
                1u32 => Ok(Error::InvalidTransform),
                2u32 => Ok(Error::InvalidSize),
                3u32 => Ok(Error::InvalidOffset),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::InvalidScale => 0u32,
                Error::InvalidTransform => 1u32,
                Error::InvalidSize => 2u32,
                Error::InvalidOffset => 3u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Enter { output: wl_output::Handle },
        Leave { output: wl_output::Handle },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let output = <wl_output::Handle as Dsr>::dsr(buf)?;
                    Enter { output }
                }
                1u16 => {
                    let output = <wl_output::Handle as Dsr>::dsr(buf)?;
                    Leave { output }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Enter { output } => {
                    output.ser(buf)?;
                    0u32
                }
                Leave { output } => {
                    output.ser(buf)?;
                    1u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Destroy {},
        Attach { buffer: wl_buffer::Handle, x: i32, y: i32 },
        Damage { x: i32, y: i32, width: i32, height: i32 },
        Frame { callback: wl_callback::Handle },
        SetOpaqueRegion { region: wl_region::Handle },
        SetInputRegion { region: wl_region::Handle },
        Commit {},
        SetBufferTransform { transform: i32 },
        SetBufferScale { scale: i32 },
        DamageBuffer { x: i32, y: i32, width: i32, height: i32 },
        Offset { x: i32, y: i32 },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => Destroy {},
                1u16 => {
                    let buffer = <wl_buffer::Handle as Dsr>::dsr(buf)?;
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    Attach { buffer, x, y }
                }
                2u16 => {
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    Damage { x, y, width, height }
                }
                3u16 => {
                    let callback = <wl_callback::Handle as Dsr>::dsr(buf)?;
                    Frame { callback }
                }
                4u16 => {
                    let region = <wl_region::Handle as Dsr>::dsr(buf)?;
                    SetOpaqueRegion { region }
                }
                5u16 => {
                    let region = <wl_region::Handle as Dsr>::dsr(buf)?;
                    SetInputRegion { region }
                }
                6u16 => Commit {},
                7u16 => {
                    let transform = <i32 as Dsr>::dsr(buf)?;
                    SetBufferTransform { transform }
                }
                8u16 => {
                    let scale = <i32 as Dsr>::dsr(buf)?;
                    SetBufferScale { scale }
                }
                9u16 => {
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    DamageBuffer {
                        x,
                        y,
                        width,
                        height,
                    }
                }
                10u16 => {
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    Offset { x, y }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Destroy {} => 0u32,
                Attach { buffer, x, y } => {
                    buffer.ser(buf)?;
                    x.ser(buf)?;
                    y.ser(buf)?;
                    1u32
                }
                Damage { x, y, width, height } => {
                    x.ser(buf)?;
                    y.ser(buf)?;
                    width.ser(buf)?;
                    height.ser(buf)?;
                    2u32
                }
                Frame { callback } => {
                    callback.ser(buf)?;
                    3u32
                }
                SetOpaqueRegion { region } => {
                    region.ser(buf)?;
                    4u32
                }
                SetInputRegion { region } => {
                    region.ser(buf)?;
                    5u32
                }
                Commit {} => 6u32,
                SetBufferTransform { transform } => {
                    transform.ser(buf)?;
                    7u32
                }
                SetBufferScale { scale } => {
                    scale.ser(buf)?;
                    8u32
                }
                DamageBuffer { x, y, width, height } => {
                    x.ser(buf)?;
                    y.ser(buf)?;
                    width.ser(buf)?;
                    height.ser(buf)?;
                    9u32
                }
                Offset { x, y } => {
                    x.ser(buf)?;
                    y.ser(buf)?;
                    10u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_surface", self.0)
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
/**
A seat is a group of keyboards, pointer and touch devices. This
object is published as a global during start up, or when such a
device is hot plugged.  A seat typically has a pointer and
maintains a keyboard focus and a pointer focus.

*/
pub mod wl_seat {
    use super::*;
    /**
This is a bitmask of capabilities this seat has; if a member is
set, then it is present on the seat.

*/
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Capability(u32);
    impl Capability {
        const POINTER: Capability = Capability(1u32);
        const KEYBOARD: Capability = Capability(2u32);
        const TOUCH: Capability = Capability(4u32);
    }
    impl std::fmt::Debug for Capability {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            let vs: Vec<&str> = [
                ("POINTER", Capability::POINTER),
                ("KEYBOARD", Capability::KEYBOARD),
                ("TOUCH", Capability::TOUCH),
            ]
                .iter()
                .filter(|(_, v)| (*v & *self) == *v)
                .map(|(s, _)| *s)
                .collect();
            write!(f, "{}", vs.join("|"))
        }
    }
    impl From<u32> for Capability {
        fn from(n: u32) -> Capability {
            Capability(n)
        }
    }
    impl From<Capability> for u32 {
        fn from(v: Capability) -> u32 {
            v.0
        }
    }
    impl std::ops::BitOr for Capability {
        type Output = Capability;
        fn bitor(self, rhs: Capability) -> Capability {
            Capability(self.0 | rhs.0)
        }
    }
    impl std::ops::BitAnd for Capability {
        type Output = Capability;
        fn bitand(self, rhs: Capability) -> Capability {
            Capability(self.0 & rhs.0)
        }
    }
    impl Dsr for Capability {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            u32::dsr(buf).map(|n| n.into())
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            let v = u32::from(*self);
            v.ser(buf)
        }
    }
    /**
These errors can be emitted in response to wl_seat requests.

*/
    #[repr(u32)]
    enum Error {
        MissingCapability = 0u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::MissingCapability),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::MissingCapability => 0u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Capabilities { capabilities: u32 },
        Name { name: String },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let capabilities = <u32 as Dsr>::dsr(buf)?;
                    Capabilities { capabilities }
                }
                1u16 => {
                    let name = <String as Dsr>::dsr(buf)?;
                    Name { name }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Capabilities { capabilities } => {
                    capabilities.ser(buf)?;
                    0u32
                }
                Name { name } => {
                    name.ser(buf)?;
                    1u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        GetPointer { id: wl_pointer::Handle },
        GetKeyboard { id: wl_keyboard::Handle },
        GetTouch { id: wl_touch::Handle },
        Release {},
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let id = <wl_pointer::Handle as Dsr>::dsr(buf)?;
                    GetPointer { id }
                }
                1u16 => {
                    let id = <wl_keyboard::Handle as Dsr>::dsr(buf)?;
                    GetKeyboard { id }
                }
                2u16 => {
                    let id = <wl_touch::Handle as Dsr>::dsr(buf)?;
                    GetTouch { id }
                }
                3u16 => Release {},
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                GetPointer { id } => {
                    id.ser(buf)?;
                    0u32
                }
                GetKeyboard { id } => {
                    id.ser(buf)?;
                    1u32
                }
                GetTouch { id } => {
                    id.ser(buf)?;
                    2u32
                }
                Release {} => 3u32,
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_seat", self.0)
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
/**
The wl_pointer interface represents one or more input devices,
such as mice, which control the pointer location and pointer_focus
of a seat.

The wl_pointer interface generates motion, enter and leave
events for the surfaces that the pointer is located over,
and button and axis events for button presses, button releases
and scrolling.

*/
pub mod wl_pointer {
    use super::*;
    ///
    #[repr(u32)]
    enum Error {
        Role = 0u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::Role),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::Role => 0u32,
            }
        }
    }
    /**
Describes the physical state of a button that produced the button
event.

*/
    #[repr(u32)]
    enum ButtonState {
        Released = 0u32,
        Pressed = 1u32,
    }
    impl TryFrom<u32> for ButtonState {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<ButtonState, ValueOutOfBounds> {
            match n {
                0u32 => Ok(ButtonState::Released),
                1u32 => Ok(ButtonState::Pressed),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<ButtonState> for u32 {
        fn from(n: ButtonState) -> u32 {
            match n {
                ButtonState::Released => 0u32,
                ButtonState::Pressed => 1u32,
            }
        }
    }
    /**
Describes the axis types of scroll events.

*/
    #[repr(u32)]
    enum Axis {
        VerticalScroll = 0u32,
        HorizontalScroll = 1u32,
    }
    impl TryFrom<u32> for Axis {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Axis, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Axis::VerticalScroll),
                1u32 => Ok(Axis::HorizontalScroll),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Axis> for u32 {
        fn from(n: Axis) -> u32 {
            match n {
                Axis::VerticalScroll => 0u32,
                Axis::HorizontalScroll => 1u32,
            }
        }
    }
    /**
Describes the source types for axis events. This indicates to the
client how an axis event was physically generated; a client may
adjust the user interface accordingly. For example, scroll events
from a "finger" source may be in a smooth coordinate space with
kinetic scrolling whereas a "wheel" source may be in discrete steps
of a number of lines.

The "continuous" axis source is a device generating events in a
continuous coordinate space, but using something other than a
finger. One example for this source is button-based scrolling where
the vertical motion of a device is converted to scroll events while
a button is held down.

The "wheel tilt" axis source indicates that the actual device is a
wheel but the scroll event is not caused by a rotation but a
(usually sideways) tilt of the wheel.

*/
    #[repr(u32)]
    enum AxisSource {
        Wheel = 0u32,
        Finger = 1u32,
        Continuous = 2u32,
        WheelTilt = 3u32,
    }
    impl TryFrom<u32> for AxisSource {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<AxisSource, ValueOutOfBounds> {
            match n {
                0u32 => Ok(AxisSource::Wheel),
                1u32 => Ok(AxisSource::Finger),
                2u32 => Ok(AxisSource::Continuous),
                3u32 => Ok(AxisSource::WheelTilt),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<AxisSource> for u32 {
        fn from(n: AxisSource) -> u32 {
            match n {
                AxisSource::Wheel => 0u32,
                AxisSource::Finger => 1u32,
                AxisSource::Continuous => 2u32,
                AxisSource::WheelTilt => 3u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Enter {
            serial: u32,
            surface: wl_surface::Handle,
            surface_x: Fixed,
            surface_y: Fixed,
        },
        Leave { serial: u32, surface: wl_surface::Handle },
        Motion { time: u32, surface_x: Fixed, surface_y: Fixed },
        Button { serial: u32, time: u32, button: u32, state: u32 },
        Axis { time: u32, axis: u32, value: Fixed },
        Frame {},
        AxisSource { axis_source: u32 },
        AxisStop { time: u32, axis: u32 },
        AxisDiscrete { axis: u32, discrete: i32 },
        AxisValue120 { axis: u32, value120: i32 },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let surface = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    let surface_x = <Fixed as Dsr>::dsr(buf)?;
                    let surface_y = <Fixed as Dsr>::dsr(buf)?;
                    Enter {
                        serial,
                        surface,
                        surface_x,
                        surface_y,
                    }
                }
                1u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let surface = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    Leave { serial, surface }
                }
                2u16 => {
                    let time = <u32 as Dsr>::dsr(buf)?;
                    let surface_x = <Fixed as Dsr>::dsr(buf)?;
                    let surface_y = <Fixed as Dsr>::dsr(buf)?;
                    Motion {
                        time,
                        surface_x,
                        surface_y,
                    }
                }
                3u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let time = <u32 as Dsr>::dsr(buf)?;
                    let button = <u32 as Dsr>::dsr(buf)?;
                    let state = <u32 as Dsr>::dsr(buf)?;
                    Button {
                        serial,
                        time,
                        button,
                        state,
                    }
                }
                4u16 => {
                    let time = <u32 as Dsr>::dsr(buf)?;
                    let axis = <u32 as Dsr>::dsr(buf)?;
                    let value = <Fixed as Dsr>::dsr(buf)?;
                    Axis { time, axis, value }
                }
                5u16 => Frame {},
                6u16 => {
                    let axis_source = <u32 as Dsr>::dsr(buf)?;
                    AxisSource { axis_source }
                }
                7u16 => {
                    let time = <u32 as Dsr>::dsr(buf)?;
                    let axis = <u32 as Dsr>::dsr(buf)?;
                    AxisStop { time, axis }
                }
                8u16 => {
                    let axis = <u32 as Dsr>::dsr(buf)?;
                    let discrete = <i32 as Dsr>::dsr(buf)?;
                    AxisDiscrete { axis, discrete }
                }
                9u16 => {
                    let axis = <u32 as Dsr>::dsr(buf)?;
                    let value120 = <i32 as Dsr>::dsr(buf)?;
                    AxisValue120 { axis, value120 }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Enter { serial, surface, surface_x, surface_y } => {
                    serial.ser(buf)?;
                    surface.ser(buf)?;
                    surface_x.ser(buf)?;
                    surface_y.ser(buf)?;
                    0u32
                }
                Leave { serial, surface } => {
                    serial.ser(buf)?;
                    surface.ser(buf)?;
                    1u32
                }
                Motion { time, surface_x, surface_y } => {
                    time.ser(buf)?;
                    surface_x.ser(buf)?;
                    surface_y.ser(buf)?;
                    2u32
                }
                Button { serial, time, button, state } => {
                    serial.ser(buf)?;
                    time.ser(buf)?;
                    button.ser(buf)?;
                    state.ser(buf)?;
                    3u32
                }
                Axis { time, axis, value } => {
                    time.ser(buf)?;
                    axis.ser(buf)?;
                    value.ser(buf)?;
                    4u32
                }
                Frame {} => 5u32,
                AxisSource { axis_source } => {
                    axis_source.ser(buf)?;
                    6u32
                }
                AxisStop { time, axis } => {
                    time.ser(buf)?;
                    axis.ser(buf)?;
                    7u32
                }
                AxisDiscrete { axis, discrete } => {
                    axis.ser(buf)?;
                    discrete.ser(buf)?;
                    8u32
                }
                AxisValue120 { axis, value120 } => {
                    axis.ser(buf)?;
                    value120.ser(buf)?;
                    9u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        SetCursor {
            serial: u32,
            surface: wl_surface::Handle,
            hotspot_x: i32,
            hotspot_y: i32,
        },
        Release {},
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let surface = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    let hotspot_x = <i32 as Dsr>::dsr(buf)?;
                    let hotspot_y = <i32 as Dsr>::dsr(buf)?;
                    SetCursor {
                        serial,
                        surface,
                        hotspot_x,
                        hotspot_y,
                    }
                }
                1u16 => Release {},
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                SetCursor { serial, surface, hotspot_x, hotspot_y } => {
                    serial.ser(buf)?;
                    surface.ser(buf)?;
                    hotspot_x.ser(buf)?;
                    hotspot_y.ser(buf)?;
                    0u32
                }
                Release {} => 1u32,
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_pointer", self.0)
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
/**
The wl_keyboard interface represents one or more keyboards
associated with a seat.

*/
pub mod wl_keyboard {
    use super::*;
    /**
This specifies the format of the keymap provided to the
client with the wl_keyboard.keymap event.

*/
    #[repr(u32)]
    enum KeymapFormat {
        NoKeymap = 0u32,
        XkbV1 = 1u32,
    }
    impl TryFrom<u32> for KeymapFormat {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<KeymapFormat, ValueOutOfBounds> {
            match n {
                0u32 => Ok(KeymapFormat::NoKeymap),
                1u32 => Ok(KeymapFormat::XkbV1),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<KeymapFormat> for u32 {
        fn from(n: KeymapFormat) -> u32 {
            match n {
                KeymapFormat::NoKeymap => 0u32,
                KeymapFormat::XkbV1 => 1u32,
            }
        }
    }
    /**
Describes the physical state of a key that produced the key event.

*/
    #[repr(u32)]
    enum KeyState {
        Released = 0u32,
        Pressed = 1u32,
    }
    impl TryFrom<u32> for KeyState {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<KeyState, ValueOutOfBounds> {
            match n {
                0u32 => Ok(KeyState::Released),
                1u32 => Ok(KeyState::Pressed),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<KeyState> for u32 {
        fn from(n: KeyState) -> u32 {
            match n {
                KeyState::Released => 0u32,
                KeyState::Pressed => 1u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Keymap { format: u32, fd: Fd, size: u32 },
        Enter { serial: u32, surface: wl_surface::Handle, keys: Vec<u8> },
        Leave { serial: u32, surface: wl_surface::Handle },
        Key { serial: u32, time: u32, key: u32, state: u32 },
        Modifiers {
            serial: u32,
            mods_depressed: u32,
            mods_latched: u32,
            mods_locked: u32,
            group: u32,
        },
        RepeatInfo { rate: i32, delay: i32 },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let format = <u32 as Dsr>::dsr(buf)?;
                    let fd = <Fd as Dsr>::dsr(buf)?;
                    let size = <u32 as Dsr>::dsr(buf)?;
                    Keymap { format, fd, size }
                }
                1u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let surface = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    let keys = <Vec<u8> as Dsr>::dsr(buf)?;
                    Enter { serial, surface, keys }
                }
                2u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let surface = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    Leave { serial, surface }
                }
                3u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let time = <u32 as Dsr>::dsr(buf)?;
                    let key = <u32 as Dsr>::dsr(buf)?;
                    let state = <u32 as Dsr>::dsr(buf)?;
                    Key { serial, time, key, state }
                }
                4u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let mods_depressed = <u32 as Dsr>::dsr(buf)?;
                    let mods_latched = <u32 as Dsr>::dsr(buf)?;
                    let mods_locked = <u32 as Dsr>::dsr(buf)?;
                    let group = <u32 as Dsr>::dsr(buf)?;
                    Modifiers {
                        serial,
                        mods_depressed,
                        mods_latched,
                        mods_locked,
                        group,
                    }
                }
                5u16 => {
                    let rate = <i32 as Dsr>::dsr(buf)?;
                    let delay = <i32 as Dsr>::dsr(buf)?;
                    RepeatInfo { rate, delay }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Keymap { format, fd, size } => {
                    format.ser(buf)?;
                    fd.ser(buf)?;
                    size.ser(buf)?;
                    0u32
                }
                Enter { serial, surface, keys } => {
                    serial.ser(buf)?;
                    surface.ser(buf)?;
                    keys.ser(buf)?;
                    1u32
                }
                Leave { serial, surface } => {
                    serial.ser(buf)?;
                    surface.ser(buf)?;
                    2u32
                }
                Key { serial, time, key, state } => {
                    serial.ser(buf)?;
                    time.ser(buf)?;
                    key.ser(buf)?;
                    state.ser(buf)?;
                    3u32
                }
                Modifiers {
                    serial,
                    mods_depressed,
                    mods_latched,
                    mods_locked,
                    group,
                } => {
                    serial.ser(buf)?;
                    mods_depressed.ser(buf)?;
                    mods_latched.ser(buf)?;
                    mods_locked.ser(buf)?;
                    group.ser(buf)?;
                    4u32
                }
                RepeatInfo { rate, delay } => {
                    rate.ser(buf)?;
                    delay.ser(buf)?;
                    5u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Release {},
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => Release {},
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Release {} => 0u32,
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_keyboard", self.0)
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
/**
The wl_touch interface represents a touchscreen
associated with a seat.

Touch interactions can consist of one or more contacts.
For each contact, a series of events is generated, starting
with a down event, followed by zero or more motion events,
and ending with an up event. Events relating to the same
contact point can be identified by the ID of the sequence.

*/
pub mod wl_touch {
    use super::*;
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Down {
            serial: u32,
            time: u32,
            surface: wl_surface::Handle,
            id: i32,
            x: Fixed,
            y: Fixed,
        },
        Up { serial: u32, time: u32, id: i32 },
        Motion { time: u32, id: i32, x: Fixed, y: Fixed },
        Frame {},
        Cancel {},
        Shape { id: i32, major: Fixed, minor: Fixed },
        Orientation { id: i32, orientation: Fixed },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let time = <u32 as Dsr>::dsr(buf)?;
                    let surface = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    let id = <i32 as Dsr>::dsr(buf)?;
                    let x = <Fixed as Dsr>::dsr(buf)?;
                    let y = <Fixed as Dsr>::dsr(buf)?;
                    Down {
                        serial,
                        time,
                        surface,
                        id,
                        x,
                        y,
                    }
                }
                1u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let time = <u32 as Dsr>::dsr(buf)?;
                    let id = <i32 as Dsr>::dsr(buf)?;
                    Up { serial, time, id }
                }
                2u16 => {
                    let time = <u32 as Dsr>::dsr(buf)?;
                    let id = <i32 as Dsr>::dsr(buf)?;
                    let x = <Fixed as Dsr>::dsr(buf)?;
                    let y = <Fixed as Dsr>::dsr(buf)?;
                    Motion { time, id, x, y }
                }
                3u16 => Frame {},
                4u16 => Cancel {},
                5u16 => {
                    let id = <i32 as Dsr>::dsr(buf)?;
                    let major = <Fixed as Dsr>::dsr(buf)?;
                    let minor = <Fixed as Dsr>::dsr(buf)?;
                    Shape { id, major, minor }
                }
                6u16 => {
                    let id = <i32 as Dsr>::dsr(buf)?;
                    let orientation = <Fixed as Dsr>::dsr(buf)?;
                    Orientation { id, orientation }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Down { serial, time, surface, id, x, y } => {
                    serial.ser(buf)?;
                    time.ser(buf)?;
                    surface.ser(buf)?;
                    id.ser(buf)?;
                    x.ser(buf)?;
                    y.ser(buf)?;
                    0u32
                }
                Up { serial, time, id } => {
                    serial.ser(buf)?;
                    time.ser(buf)?;
                    id.ser(buf)?;
                    1u32
                }
                Motion { time, id, x, y } => {
                    time.ser(buf)?;
                    id.ser(buf)?;
                    x.ser(buf)?;
                    y.ser(buf)?;
                    2u32
                }
                Frame {} => 3u32,
                Cancel {} => 4u32,
                Shape { id, major, minor } => {
                    id.ser(buf)?;
                    major.ser(buf)?;
                    minor.ser(buf)?;
                    5u32
                }
                Orientation { id, orientation } => {
                    id.ser(buf)?;
                    orientation.ser(buf)?;
                    6u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Release {},
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => Release {},
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Release {} => 0u32,
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_touch", self.0)
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
/**
An output describes part of the compositor geometry.  The
compositor works in the 'compositor coordinate system' and an
output corresponds to a rectangular area in that space that is
actually visible.  This typically corresponds to a monitor that
displays part of the compositor space.  This object is published
as global during start up, or when a monitor is hotplugged.

*/
pub mod wl_output {
    use super::*;
    /**
This enumeration describes how the physical
pixels on an output are laid out.

*/
    #[repr(u32)]
    enum Subpixel {
        Unknown = 0u32,
        None = 1u32,
        HorizontalRgb = 2u32,
        HorizontalBgr = 3u32,
        VerticalRgb = 4u32,
        VerticalBgr = 5u32,
    }
    impl TryFrom<u32> for Subpixel {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Subpixel, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Subpixel::Unknown),
                1u32 => Ok(Subpixel::None),
                2u32 => Ok(Subpixel::HorizontalRgb),
                3u32 => Ok(Subpixel::HorizontalBgr),
                4u32 => Ok(Subpixel::VerticalRgb),
                5u32 => Ok(Subpixel::VerticalBgr),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Subpixel> for u32 {
        fn from(n: Subpixel) -> u32 {
            match n {
                Subpixel::Unknown => 0u32,
                Subpixel::None => 1u32,
                Subpixel::HorizontalRgb => 2u32,
                Subpixel::HorizontalBgr => 3u32,
                Subpixel::VerticalRgb => 4u32,
                Subpixel::VerticalBgr => 5u32,
            }
        }
    }
    /**
This describes the transform that a compositor will apply to a
surface to compensate for the rotation or mirroring of an
output device.

The flipped values correspond to an initial flip around a
vertical axis followed by rotation.

The purpose is mainly to allow clients to render accordingly and
tell the compositor, so that for fullscreen surfaces, the
compositor will still be able to scan out directly from client
surfaces.

*/
    #[repr(u32)]
    enum Transform {
        Normal = 0u32,
        T90 = 1u32,
        T180 = 2u32,
        T270 = 3u32,
        Flipped = 4u32,
        Flipped90 = 5u32,
        Flipped180 = 6u32,
        Flipped270 = 7u32,
    }
    impl TryFrom<u32> for Transform {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Transform, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Transform::Normal),
                1u32 => Ok(Transform::T90),
                2u32 => Ok(Transform::T180),
                3u32 => Ok(Transform::T270),
                4u32 => Ok(Transform::Flipped),
                5u32 => Ok(Transform::Flipped90),
                6u32 => Ok(Transform::Flipped180),
                7u32 => Ok(Transform::Flipped270),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Transform> for u32 {
        fn from(n: Transform) -> u32 {
            match n {
                Transform::Normal => 0u32,
                Transform::T90 => 1u32,
                Transform::T180 => 2u32,
                Transform::T270 => 3u32,
                Transform::Flipped => 4u32,
                Transform::Flipped90 => 5u32,
                Transform::Flipped180 => 6u32,
                Transform::Flipped270 => 7u32,
            }
        }
    }
    /**
These flags describe properties of an output mode.
They are used in the flags bitfield of the mode event.

*/
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Mode(u32);
    impl Mode {
        const CURRENT: Mode = Mode(1u32);
        const PREFERRED: Mode = Mode(2u32);
    }
    impl std::fmt::Debug for Mode {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            let vs: Vec<&str> = [
                ("CURRENT", Mode::CURRENT),
                ("PREFERRED", Mode::PREFERRED),
            ]
                .iter()
                .filter(|(_, v)| (*v & *self) == *v)
                .map(|(s, _)| *s)
                .collect();
            write!(f, "{}", vs.join("|"))
        }
    }
    impl From<u32> for Mode {
        fn from(n: u32) -> Mode {
            Mode(n)
        }
    }
    impl From<Mode> for u32 {
        fn from(v: Mode) -> u32 {
            v.0
        }
    }
    impl std::ops::BitOr for Mode {
        type Output = Mode;
        fn bitor(self, rhs: Mode) -> Mode {
            Mode(self.0 | rhs.0)
        }
    }
    impl std::ops::BitAnd for Mode {
        type Output = Mode;
        fn bitand(self, rhs: Mode) -> Mode {
            Mode(self.0 & rhs.0)
        }
    }
    impl Dsr for Mode {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            u32::dsr(buf).map(|n| n.into())
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            let v = u32::from(*self);
            v.ser(buf)
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Geometry {
            x: i32,
            y: i32,
            physical_width: i32,
            physical_height: i32,
            subpixel: i32,
            make: String,
            model: String,
            transform: i32,
        },
        Mode { flags: u32, width: i32, height: i32, refresh: i32 },
        Done {},
        Scale { factor: i32 },
        Name { name: String },
        Description { description: String },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    let physical_width = <i32 as Dsr>::dsr(buf)?;
                    let physical_height = <i32 as Dsr>::dsr(buf)?;
                    let subpixel = <i32 as Dsr>::dsr(buf)?;
                    let make = <String as Dsr>::dsr(buf)?;
                    let model = <String as Dsr>::dsr(buf)?;
                    let transform = <i32 as Dsr>::dsr(buf)?;
                    Geometry {
                        x,
                        y,
                        physical_width,
                        physical_height,
                        subpixel,
                        make,
                        model,
                        transform,
                    }
                }
                1u16 => {
                    let flags = <u32 as Dsr>::dsr(buf)?;
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    let refresh = <i32 as Dsr>::dsr(buf)?;
                    Mode {
                        flags,
                        width,
                        height,
                        refresh,
                    }
                }
                2u16 => Done {},
                3u16 => {
                    let factor = <i32 as Dsr>::dsr(buf)?;
                    Scale { factor }
                }
                4u16 => {
                    let name = <String as Dsr>::dsr(buf)?;
                    Name { name }
                }
                5u16 => {
                    let description = <String as Dsr>::dsr(buf)?;
                    Description { description }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Geometry {
                    x,
                    y,
                    physical_width,
                    physical_height,
                    subpixel,
                    make,
                    model,
                    transform,
                } => {
                    x.ser(buf)?;
                    y.ser(buf)?;
                    physical_width.ser(buf)?;
                    physical_height.ser(buf)?;
                    subpixel.ser(buf)?;
                    make.ser(buf)?;
                    model.ser(buf)?;
                    transform.ser(buf)?;
                    0u32
                }
                Mode { flags, width, height, refresh } => {
                    flags.ser(buf)?;
                    width.ser(buf)?;
                    height.ser(buf)?;
                    refresh.ser(buf)?;
                    1u32
                }
                Done {} => 2u32,
                Scale { factor } => {
                    factor.ser(buf)?;
                    3u32
                }
                Name { name } => {
                    name.ser(buf)?;
                    4u32
                }
                Description { description } => {
                    description.ser(buf)?;
                    5u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Release {},
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => Release {},
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Release {} => 0u32,
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_output", self.0)
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
/**
A region object describes an area.

Region objects are used to describe the opaque and input
regions of a surface.

*/
pub mod wl_region {
    use super::*;
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Destroy {},
        Add { x: i32, y: i32, width: i32, height: i32 },
        Subtract { x: i32, y: i32, width: i32, height: i32 },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => Destroy {},
                1u16 => {
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    Add { x, y, width, height }
                }
                2u16 => {
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    Subtract { x, y, width, height }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Destroy {} => 0u32,
                Add { x, y, width, height } => {
                    x.ser(buf)?;
                    y.ser(buf)?;
                    width.ser(buf)?;
                    height.ser(buf)?;
                    1u32
                }
                Subtract { x, y, width, height } => {
                    x.ser(buf)?;
                    y.ser(buf)?;
                    width.ser(buf)?;
                    height.ser(buf)?;
                    2u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_region", self.0)
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
/**
The global interface exposing sub-surface compositing capabilities.
A wl_surface, that has sub-surfaces associated, is called the
parent surface. Sub-surfaces can be arbitrarily nested and create
a tree of sub-surfaces.

The root surface in a tree of sub-surfaces is the main
surface. The main surface cannot be a sub-surface, because
sub-surfaces must always have a parent.

A main surface with its sub-surfaces forms a (compound) window.
For window management purposes, this set of wl_surface objects is
to be considered as a single window, and it should also behave as
such.

The aim of sub-surfaces is to offload some of the compositing work
within a window from clients to the compositor. A prime example is
a video player with decorations and video in separate wl_surface
objects. This should allow the compositor to pass YUV video buffer
processing to dedicated overlay hardware when possible.

*/
pub mod wl_subcompositor {
    use super::*;
    ///
    #[repr(u32)]
    enum Error {
        BadSurface = 0u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::BadSurface),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::BadSurface => 0u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Destroy {},
        GetSubsurface {
            id: wl_subsurface::Handle,
            surface: wl_surface::Handle,
            parent: wl_surface::Handle,
        },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => Destroy {},
                1u16 => {
                    let id = <wl_subsurface::Handle as Dsr>::dsr(buf)?;
                    let surface = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    let parent = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    GetSubsurface {
                        id,
                        surface,
                        parent,
                    }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Destroy {} => 0u32,
                GetSubsurface { id, surface, parent } => {
                    id.ser(buf)?;
                    surface.ser(buf)?;
                    parent.ser(buf)?;
                    1u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_subcompositor", self.0)
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
/**
An additional interface to a wl_surface object, which has been
made a sub-surface. A sub-surface has one parent surface. A
sub-surface's size and position are not limited to that of the parent.
Particularly, a sub-surface is not automatically clipped to its
parent's area.

A sub-surface becomes mapped, when a non-NULL wl_buffer is applied
and the parent surface is mapped. The order of which one happens
first is irrelevant. A sub-surface is hidden if the parent becomes
hidden, or if a NULL wl_buffer is applied. These rules apply
recursively through the tree of surfaces.

The behaviour of a wl_surface.commit request on a sub-surface
depends on the sub-surface's mode. The possible modes are
synchronized and desynchronized, see methods
wl_subsurface.set_sync and wl_subsurface.set_desync. Synchronized
mode caches the wl_surface state to be applied when the parent's
state gets applied, and desynchronized mode applies the pending
wl_surface state directly. A sub-surface is initially in the
synchronized mode.

Sub-surfaces also have another kind of state, which is managed by
wl_subsurface requests, as opposed to wl_surface requests. This
state includes the sub-surface position relative to the parent
surface (wl_subsurface.set_position), and the stacking order of
the parent and its sub-surfaces (wl_subsurface.place_above and
.place_below). This state is applied when the parent surface's
wl_surface state is applied, regardless of the sub-surface's mode.
As the exception, set_sync and set_desync are effective immediately.

The main surface can be thought to be always in desynchronized mode,
since it does not have a parent in the sub-surfaces sense.

Even if a sub-surface is in desynchronized mode, it will behave as
in synchronized mode, if its parent surface behaves as in
synchronized mode. This rule is applied recursively throughout the
tree of surfaces. This means, that one can set a sub-surface into
synchronized mode, and then assume that all its child and grand-child
sub-surfaces are synchronized, too, without explicitly setting them.

If the wl_surface associated with the wl_subsurface is destroyed, the
wl_subsurface object becomes inert. Note, that destroying either object
takes effect immediately. If you need to synchronize the removal
of a sub-surface to the parent surface update, unmap the sub-surface
first by attaching a NULL wl_buffer, update parent, and then destroy
the sub-surface.

If the parent wl_surface object is destroyed, the sub-surface is
unmapped.

*/
pub mod wl_subsurface {
    use super::*;
    ///
    #[repr(u32)]
    enum Error {
        BadSurface = 0u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::BadSurface),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::BadSurface => 0u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Destroy {},
        SetPosition { x: i32, y: i32 },
        PlaceAbove { sibling: wl_surface::Handle },
        PlaceBelow { sibling: wl_surface::Handle },
        SetSync {},
        SetDesync {},
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => Destroy {},
                1u16 => {
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    SetPosition { x, y }
                }
                2u16 => {
                    let sibling = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    PlaceAbove { sibling }
                }
                3u16 => {
                    let sibling = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    PlaceBelow { sibling }
                }
                4u16 => SetSync {},
                5u16 => SetDesync {},
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Destroy {} => 0u32,
                SetPosition { x, y } => {
                    x.ser(buf)?;
                    y.ser(buf)?;
                    1u32
                }
                PlaceAbove { sibling } => {
                    sibling.ser(buf)?;
                    2u32
                }
                PlaceBelow { sibling } => {
                    sibling.ser(buf)?;
                    3u32
                }
                SetSync {} => 4u32,
                SetDesync {} => 5u32,
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "wl_subsurface", self.0)
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
/**
The xdg_wm_base interface is exposed as a global object enabling clients
to turn their wl_surfaces into windows in a desktop environment. It
defines the basic functionality needed for clients and the compositor to
create windows that can be dragged, resized, maximized, etc, as well as
creating transient windows such as popup menus.

*/
pub mod xdg_wm_base {
    use super::*;
    ///
    #[repr(u32)]
    enum Error {
        Role = 0u32,
        DefunctSurfaces = 1u32,
        NotTheTopmostPopup = 2u32,
        InvalidPopupParent = 3u32,
        InvalidSurfaceState = 4u32,
        InvalidPositioner = 5u32,
        Unresponsive = 6u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::Role),
                1u32 => Ok(Error::DefunctSurfaces),
                2u32 => Ok(Error::NotTheTopmostPopup),
                3u32 => Ok(Error::InvalidPopupParent),
                4u32 => Ok(Error::InvalidSurfaceState),
                5u32 => Ok(Error::InvalidPositioner),
                6u32 => Ok(Error::Unresponsive),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::Role => 0u32,
                Error::DefunctSurfaces => 1u32,
                Error::NotTheTopmostPopup => 2u32,
                Error::InvalidPopupParent => 3u32,
                Error::InvalidSurfaceState => 4u32,
                Error::InvalidPositioner => 5u32,
                Error::Unresponsive => 6u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Ping { serial: u32 },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    Ping { serial }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Ping { serial } => {
                    serial.ser(buf)?;
                    0u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Destroy {},
        CreatePositioner { id: xdg_positioner::Handle },
        GetXdgSurface { id: xdg_surface::Handle, surface: wl_surface::Handle },
        Pong { serial: u32 },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => Destroy {},
                1u16 => {
                    let id = <xdg_positioner::Handle as Dsr>::dsr(buf)?;
                    CreatePositioner { id }
                }
                2u16 => {
                    let id = <xdg_surface::Handle as Dsr>::dsr(buf)?;
                    let surface = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    GetXdgSurface { id, surface }
                }
                3u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    Pong { serial }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Destroy {} => 0u32,
                CreatePositioner { id } => {
                    id.ser(buf)?;
                    1u32
                }
                GetXdgSurface { id, surface } => {
                    id.ser(buf)?;
                    surface.ser(buf)?;
                    2u32
                }
                Pong { serial } => {
                    serial.ser(buf)?;
                    3u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "xdg_wm_base", self.0)
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
/**
The xdg_positioner provides a collection of rules for the placement of a
child surface relative to a parent surface. Rules can be defined to ensure
the child surface remains within the visible area's borders, and to
specify how the child surface changes its position, such as sliding along
an axis, or flipping around a rectangle. These positioner-created rules are
constrained by the requirement that a child surface must intersect with or
be at least partially adjacent to its parent surface.

See the various requests for details about possible rules.

At the time of the request, the compositor makes a copy of the rules
specified by the xdg_positioner. Thus, after the request is complete the
xdg_positioner object can be destroyed or reused; further changes to the
object will have no effect on previous usages.

For an xdg_positioner object to be considered complete, it must have a
non-zero size set by set_size, and a non-zero anchor rectangle set by
set_anchor_rect. Passing an incomplete xdg_positioner object when
positioning a surface raises an invalid_positioner error.

*/
pub mod xdg_positioner {
    use super::*;
    ///
    #[repr(u32)]
    enum Error {
        InvalidInput = 0u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::InvalidInput),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::InvalidInput => 0u32,
            }
        }
    }
    ///
    #[repr(u32)]
    enum Anchor {
        None = 0u32,
        Top = 1u32,
        Bottom = 2u32,
        Left = 3u32,
        Right = 4u32,
        TopLeft = 5u32,
        BottomLeft = 6u32,
        TopRight = 7u32,
        BottomRight = 8u32,
    }
    impl TryFrom<u32> for Anchor {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Anchor, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Anchor::None),
                1u32 => Ok(Anchor::Top),
                2u32 => Ok(Anchor::Bottom),
                3u32 => Ok(Anchor::Left),
                4u32 => Ok(Anchor::Right),
                5u32 => Ok(Anchor::TopLeft),
                6u32 => Ok(Anchor::BottomLeft),
                7u32 => Ok(Anchor::TopRight),
                8u32 => Ok(Anchor::BottomRight),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Anchor> for u32 {
        fn from(n: Anchor) -> u32 {
            match n {
                Anchor::None => 0u32,
                Anchor::Top => 1u32,
                Anchor::Bottom => 2u32,
                Anchor::Left => 3u32,
                Anchor::Right => 4u32,
                Anchor::TopLeft => 5u32,
                Anchor::BottomLeft => 6u32,
                Anchor::TopRight => 7u32,
                Anchor::BottomRight => 8u32,
            }
        }
    }
    ///
    #[repr(u32)]
    enum Gravity {
        None = 0u32,
        Top = 1u32,
        Bottom = 2u32,
        Left = 3u32,
        Right = 4u32,
        TopLeft = 5u32,
        BottomLeft = 6u32,
        TopRight = 7u32,
        BottomRight = 8u32,
    }
    impl TryFrom<u32> for Gravity {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Gravity, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Gravity::None),
                1u32 => Ok(Gravity::Top),
                2u32 => Ok(Gravity::Bottom),
                3u32 => Ok(Gravity::Left),
                4u32 => Ok(Gravity::Right),
                5u32 => Ok(Gravity::TopLeft),
                6u32 => Ok(Gravity::BottomLeft),
                7u32 => Ok(Gravity::TopRight),
                8u32 => Ok(Gravity::BottomRight),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Gravity> for u32 {
        fn from(n: Gravity) -> u32 {
            match n {
                Gravity::None => 0u32,
                Gravity::Top => 1u32,
                Gravity::Bottom => 2u32,
                Gravity::Left => 3u32,
                Gravity::Right => 4u32,
                Gravity::TopLeft => 5u32,
                Gravity::BottomLeft => 6u32,
                Gravity::TopRight => 7u32,
                Gravity::BottomRight => 8u32,
            }
        }
    }
    /**
The constraint adjustment value define ways the compositor will adjust
the position of the surface, if the unadjusted position would result
in the surface being partly constrained.

Whether a surface is considered 'constrained' is left to the compositor
to determine. For example, the surface may be partly outside the
compositor's defined 'work area', thus necessitating the child surface's
position be adjusted until it is entirely inside the work area.

The adjustments can be combined, according to a defined precedence: 1)
Flip, 2) Slide, 3) Resize.

*/
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct ConstraintAdjustment(u32);
    impl ConstraintAdjustment {
        const NONE: ConstraintAdjustment = ConstraintAdjustment(0u32);
        const SLIDE_X: ConstraintAdjustment = ConstraintAdjustment(1u32);
        const SLIDE_Y: ConstraintAdjustment = ConstraintAdjustment(2u32);
        const FLIP_X: ConstraintAdjustment = ConstraintAdjustment(4u32);
        const FLIP_Y: ConstraintAdjustment = ConstraintAdjustment(8u32);
        const RESIZE_X: ConstraintAdjustment = ConstraintAdjustment(16u32);
        const RESIZE_Y: ConstraintAdjustment = ConstraintAdjustment(32u32);
    }
    impl std::fmt::Debug for ConstraintAdjustment {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            let vs: Vec<&str> = [
                ("NONE", ConstraintAdjustment::NONE),
                ("SLIDE_X", ConstraintAdjustment::SLIDE_X),
                ("SLIDE_Y", ConstraintAdjustment::SLIDE_Y),
                ("FLIP_X", ConstraintAdjustment::FLIP_X),
                ("FLIP_Y", ConstraintAdjustment::FLIP_Y),
                ("RESIZE_X", ConstraintAdjustment::RESIZE_X),
                ("RESIZE_Y", ConstraintAdjustment::RESIZE_Y),
            ]
                .iter()
                .filter(|(_, v)| (*v & *self) == *v)
                .map(|(s, _)| *s)
                .collect();
            write!(f, "{}", vs.join("|"))
        }
    }
    impl From<u32> for ConstraintAdjustment {
        fn from(n: u32) -> ConstraintAdjustment {
            ConstraintAdjustment(n)
        }
    }
    impl From<ConstraintAdjustment> for u32 {
        fn from(v: ConstraintAdjustment) -> u32 {
            v.0
        }
    }
    impl std::ops::BitOr for ConstraintAdjustment {
        type Output = ConstraintAdjustment;
        fn bitor(self, rhs: ConstraintAdjustment) -> ConstraintAdjustment {
            ConstraintAdjustment(self.0 | rhs.0)
        }
    }
    impl std::ops::BitAnd for ConstraintAdjustment {
        type Output = ConstraintAdjustment;
        fn bitand(self, rhs: ConstraintAdjustment) -> ConstraintAdjustment {
            ConstraintAdjustment(self.0 & rhs.0)
        }
    }
    impl Dsr for ConstraintAdjustment {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            u32::dsr(buf).map(|n| n.into())
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            let v = u32::from(*self);
            v.ser(buf)
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Destroy {},
        SetSize { width: i32, height: i32 },
        SetAnchorRect { x: i32, y: i32, width: i32, height: i32 },
        SetAnchor { anchor: u32 },
        SetGravity { gravity: u32 },
        SetConstraintAdjustment { constraint_adjustment: u32 },
        SetOffset { x: i32, y: i32 },
        SetReactive {},
        SetParentSize { parent_width: i32, parent_height: i32 },
        SetParentConfigure { serial: u32 },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => Destroy {},
                1u16 => {
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    SetSize { width, height }
                }
                2u16 => {
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    SetAnchorRect {
                        x,
                        y,
                        width,
                        height,
                    }
                }
                3u16 => {
                    let anchor = <u32 as Dsr>::dsr(buf)?;
                    SetAnchor { anchor }
                }
                4u16 => {
                    let gravity = <u32 as Dsr>::dsr(buf)?;
                    SetGravity { gravity }
                }
                5u16 => {
                    let constraint_adjustment = <u32 as Dsr>::dsr(buf)?;
                    SetConstraintAdjustment {
                        constraint_adjustment,
                    }
                }
                6u16 => {
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    SetOffset { x, y }
                }
                7u16 => SetReactive {},
                8u16 => {
                    let parent_width = <i32 as Dsr>::dsr(buf)?;
                    let parent_height = <i32 as Dsr>::dsr(buf)?;
                    SetParentSize {
                        parent_width,
                        parent_height,
                    }
                }
                9u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    SetParentConfigure { serial }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Destroy {} => 0u32,
                SetSize { width, height } => {
                    width.ser(buf)?;
                    height.ser(buf)?;
                    1u32
                }
                SetAnchorRect { x, y, width, height } => {
                    x.ser(buf)?;
                    y.ser(buf)?;
                    width.ser(buf)?;
                    height.ser(buf)?;
                    2u32
                }
                SetAnchor { anchor } => {
                    anchor.ser(buf)?;
                    3u32
                }
                SetGravity { gravity } => {
                    gravity.ser(buf)?;
                    4u32
                }
                SetConstraintAdjustment { constraint_adjustment } => {
                    constraint_adjustment.ser(buf)?;
                    5u32
                }
                SetOffset { x, y } => {
                    x.ser(buf)?;
                    y.ser(buf)?;
                    6u32
                }
                SetReactive {} => 7u32,
                SetParentSize { parent_width, parent_height } => {
                    parent_width.ser(buf)?;
                    parent_height.ser(buf)?;
                    8u32
                }
                SetParentConfigure { serial } => {
                    serial.ser(buf)?;
                    9u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "xdg_positioner", self.0)
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
/**
An interface that may be implemented by a wl_surface, for
implementations that provide a desktop-style user interface.

It provides a base set of functionality required to construct user
interface elements requiring management by the compositor, such as
toplevel windows, menus, etc. The types of functionality are split into
xdg_surface roles.

Creating an xdg_surface does not set the role for a wl_surface. In order
to map an xdg_surface, the client must create a role-specific object
using, e.g., get_toplevel, get_popup. The wl_surface for any given
xdg_surface can have at most one role, and may not be assigned any role
not based on xdg_surface.

A role must be assigned before any other requests are made to the
xdg_surface object.

The client must call wl_surface.commit on the corresponding wl_surface
for the xdg_surface state to take effect.

Creating an xdg_surface from a wl_surface which has a buffer attached or
committed is a client error, and any attempts by a client to attach or
manipulate a buffer prior to the first xdg_surface.configure call must
also be treated as errors.

After creating a role-specific object and setting it up, the client must
perform an initial commit without any buffer attached. The compositor
will reply with an xdg_surface.configure event. The client must
acknowledge it and is then allowed to attach a buffer to map the surface.

Mapping an xdg_surface-based role surface is defined as making it
possible for the surface to be shown by the compositor. Note that
a mapped surface is not guaranteed to be visible once it is mapped.

For an xdg_surface to be mapped by the compositor, the following
conditions must be met:
(1) the client has assigned an xdg_surface-based role to the surface
(2) the client has set and committed the xdg_surface state and the
role-dependent state to the surface
(3) the client has committed a buffer to the surface

A newly-unmapped surface is considered to have met condition (1) out
of the 3 required conditions for mapping a surface if its role surface
has not been destroyed, i.e. the client must perform the initial commit
again before attaching a buffer.

*/
pub mod xdg_surface {
    use super::*;
    ///
    #[repr(u32)]
    enum Error {
        NotConstructed = 1u32,
        AlreadyConstructed = 2u32,
        UnconfiguredBuffer = 3u32,
        InvalidSerial = 4u32,
        InvalidSize = 5u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                1u32 => Ok(Error::NotConstructed),
                2u32 => Ok(Error::AlreadyConstructed),
                3u32 => Ok(Error::UnconfiguredBuffer),
                4u32 => Ok(Error::InvalidSerial),
                5u32 => Ok(Error::InvalidSize),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::NotConstructed => 1u32,
                Error::AlreadyConstructed => 2u32,
                Error::UnconfiguredBuffer => 3u32,
                Error::InvalidSerial => 4u32,
                Error::InvalidSize => 5u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Configure { serial: u32 },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    Configure { serial }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Configure { serial } => {
                    serial.ser(buf)?;
                    0u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Destroy {},
        GetToplevel { id: xdg_toplevel::Handle },
        GetPopup {
            id: xdg_popup::Handle,
            parent: xdg_surface::Handle,
            positioner: xdg_positioner::Handle,
        },
        SetWindowGeometry { x: i32, y: i32, width: i32, height: i32 },
        AckConfigure { serial: u32 },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => Destroy {},
                1u16 => {
                    let id = <xdg_toplevel::Handle as Dsr>::dsr(buf)?;
                    GetToplevel { id }
                }
                2u16 => {
                    let id = <xdg_popup::Handle as Dsr>::dsr(buf)?;
                    let parent = <xdg_surface::Handle as Dsr>::dsr(buf)?;
                    let positioner = <xdg_positioner::Handle as Dsr>::dsr(buf)?;
                    GetPopup { id, parent, positioner }
                }
                3u16 => {
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    SetWindowGeometry {
                        x,
                        y,
                        width,
                        height,
                    }
                }
                4u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    AckConfigure { serial }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Destroy {} => 0u32,
                GetToplevel { id } => {
                    id.ser(buf)?;
                    1u32
                }
                GetPopup { id, parent, positioner } => {
                    id.ser(buf)?;
                    parent.ser(buf)?;
                    positioner.ser(buf)?;
                    2u32
                }
                SetWindowGeometry { x, y, width, height } => {
                    x.ser(buf)?;
                    y.ser(buf)?;
                    width.ser(buf)?;
                    height.ser(buf)?;
                    3u32
                }
                AckConfigure { serial } => {
                    serial.ser(buf)?;
                    4u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "xdg_surface", self.0)
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
/**
This interface defines an xdg_surface role which allows a surface to,
among other things, set window-like properties such as maximize,
fullscreen, and minimize, set application-specific metadata like title and
id, and well as trigger user interactive operations such as interactive
resize and move.

Unmapping an xdg_toplevel means that the surface cannot be shown
by the compositor until it is explicitly mapped again.
All active operations (e.g., move, resize) are canceled and all
attributes (e.g. title, state, stacking, ...) are discarded for
an xdg_toplevel surface when it is unmapped. The xdg_toplevel returns to
the state it had right after xdg_surface.get_toplevel. The client
can re-map the toplevel by perfoming a commit without any buffer
attached, waiting for a configure event and handling it as usual (see
xdg_surface description).

Attaching a null buffer to a toplevel unmaps the surface.

*/
pub mod xdg_toplevel {
    use super::*;
    ///
    #[repr(u32)]
    enum Error {
        InvalidResizeEdge = 0u32,
        InvalidParent = 1u32,
        InvalidSize = 2u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::InvalidResizeEdge),
                1u32 => Ok(Error::InvalidParent),
                2u32 => Ok(Error::InvalidSize),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::InvalidResizeEdge => 0u32,
                Error::InvalidParent => 1u32,
                Error::InvalidSize => 2u32,
            }
        }
    }
    /**
These values are used to indicate which edge of a surface
is being dragged in a resize operation.

*/
    #[repr(u32)]
    enum ResizeEdge {
        None = 0u32,
        Top = 1u32,
        Bottom = 2u32,
        Left = 4u32,
        TopLeft = 5u32,
        BottomLeft = 6u32,
        Right = 8u32,
        TopRight = 9u32,
        BottomRight = 10u32,
    }
    impl TryFrom<u32> for ResizeEdge {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<ResizeEdge, ValueOutOfBounds> {
            match n {
                0u32 => Ok(ResizeEdge::None),
                1u32 => Ok(ResizeEdge::Top),
                2u32 => Ok(ResizeEdge::Bottom),
                4u32 => Ok(ResizeEdge::Left),
                5u32 => Ok(ResizeEdge::TopLeft),
                6u32 => Ok(ResizeEdge::BottomLeft),
                8u32 => Ok(ResizeEdge::Right),
                9u32 => Ok(ResizeEdge::TopRight),
                10u32 => Ok(ResizeEdge::BottomRight),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<ResizeEdge> for u32 {
        fn from(n: ResizeEdge) -> u32 {
            match n {
                ResizeEdge::None => 0u32,
                ResizeEdge::Top => 1u32,
                ResizeEdge::Bottom => 2u32,
                ResizeEdge::Left => 4u32,
                ResizeEdge::TopLeft => 5u32,
                ResizeEdge::BottomLeft => 6u32,
                ResizeEdge::Right => 8u32,
                ResizeEdge::TopRight => 9u32,
                ResizeEdge::BottomRight => 10u32,
            }
        }
    }
    /**
The different state values used on the surface. This is designed for
state values like maximized, fullscreen. It is paired with the
configure event to ensure that both the client and the compositor
setting the state can be synchronized.

States set in this way are double-buffered. They will get applied on
the next commit.

*/
    #[repr(u32)]
    enum State {
        Maximized = 1u32,
        Fullscreen = 2u32,
        Resizing = 3u32,
        Activated = 4u32,
        TiledLeft = 5u32,
        TiledRight = 6u32,
        TiledTop = 7u32,
        TiledBottom = 8u32,
    }
    impl TryFrom<u32> for State {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<State, ValueOutOfBounds> {
            match n {
                1u32 => Ok(State::Maximized),
                2u32 => Ok(State::Fullscreen),
                3u32 => Ok(State::Resizing),
                4u32 => Ok(State::Activated),
                5u32 => Ok(State::TiledLeft),
                6u32 => Ok(State::TiledRight),
                7u32 => Ok(State::TiledTop),
                8u32 => Ok(State::TiledBottom),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<State> for u32 {
        fn from(n: State) -> u32 {
            match n {
                State::Maximized => 1u32,
                State::Fullscreen => 2u32,
                State::Resizing => 3u32,
                State::Activated => 4u32,
                State::TiledLeft => 5u32,
                State::TiledRight => 6u32,
                State::TiledTop => 7u32,
                State::TiledBottom => 8u32,
            }
        }
    }
    ///
    #[repr(u32)]
    enum WmCapabilities {
        WindowMenu = 1u32,
        Maximize = 2u32,
        Fullscreen = 3u32,
        Minimize = 4u32,
    }
    impl TryFrom<u32> for WmCapabilities {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<WmCapabilities, ValueOutOfBounds> {
            match n {
                1u32 => Ok(WmCapabilities::WindowMenu),
                2u32 => Ok(WmCapabilities::Maximize),
                3u32 => Ok(WmCapabilities::Fullscreen),
                4u32 => Ok(WmCapabilities::Minimize),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<WmCapabilities> for u32 {
        fn from(n: WmCapabilities) -> u32 {
            match n {
                WmCapabilities::WindowMenu => 1u32,
                WmCapabilities::Maximize => 2u32,
                WmCapabilities::Fullscreen => 3u32,
                WmCapabilities::Minimize => 4u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Configure { width: i32, height: i32, states: Vec<u8> },
        Close {},
        ConfigureBounds { width: i32, height: i32 },
        WmCapabilities { capabilities: Vec<u8> },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    let states = <Vec<u8> as Dsr>::dsr(buf)?;
                    Configure { width, height, states }
                }
                1u16 => Close {},
                2u16 => {
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    ConfigureBounds { width, height }
                }
                3u16 => {
                    let capabilities = <Vec<u8> as Dsr>::dsr(buf)?;
                    WmCapabilities { capabilities }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Configure { width, height, states } => {
                    width.ser(buf)?;
                    height.ser(buf)?;
                    states.ser(buf)?;
                    0u32
                }
                Close {} => 1u32,
                ConfigureBounds { width, height } => {
                    width.ser(buf)?;
                    height.ser(buf)?;
                    2u32
                }
                WmCapabilities { capabilities } => {
                    capabilities.ser(buf)?;
                    3u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Destroy {},
        SetParent { parent: xdg_toplevel::Handle },
        SetTitle { title: String },
        SetAppId { app_id: String },
        ShowWindowMenu { seat: wl_seat::Handle, serial: u32, x: i32, y: i32 },
        Move { seat: wl_seat::Handle, serial: u32 },
        Resize { seat: wl_seat::Handle, serial: u32, edges: u32 },
        SetMaxSize { width: i32, height: i32 },
        SetMinSize { width: i32, height: i32 },
        SetMaximized {},
        UnsetMaximized {},
        SetFullscreen { output: wl_output::Handle },
        UnsetFullscreen {},
        SetMinimized {},
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => Destroy {},
                1u16 => {
                    let parent = <xdg_toplevel::Handle as Dsr>::dsr(buf)?;
                    SetParent { parent }
                }
                2u16 => {
                    let title = <String as Dsr>::dsr(buf)?;
                    SetTitle { title }
                }
                3u16 => {
                    let app_id = <String as Dsr>::dsr(buf)?;
                    SetAppId { app_id }
                }
                4u16 => {
                    let seat = <wl_seat::Handle as Dsr>::dsr(buf)?;
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    ShowWindowMenu {
                        seat,
                        serial,
                        x,
                        y,
                    }
                }
                5u16 => {
                    let seat = <wl_seat::Handle as Dsr>::dsr(buf)?;
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    Move { seat, serial }
                }
                6u16 => {
                    let seat = <wl_seat::Handle as Dsr>::dsr(buf)?;
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let edges = <u32 as Dsr>::dsr(buf)?;
                    Resize { seat, serial, edges }
                }
                7u16 => {
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    SetMaxSize { width, height }
                }
                8u16 => {
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    SetMinSize { width, height }
                }
                9u16 => SetMaximized {},
                10u16 => UnsetMaximized {},
                11u16 => {
                    let output = <wl_output::Handle as Dsr>::dsr(buf)?;
                    SetFullscreen { output }
                }
                12u16 => UnsetFullscreen {},
                13u16 => SetMinimized {},
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Destroy {} => 0u32,
                SetParent { parent } => {
                    parent.ser(buf)?;
                    1u32
                }
                SetTitle { title } => {
                    title.ser(buf)?;
                    2u32
                }
                SetAppId { app_id } => {
                    app_id.ser(buf)?;
                    3u32
                }
                ShowWindowMenu { seat, serial, x, y } => {
                    seat.ser(buf)?;
                    serial.ser(buf)?;
                    x.ser(buf)?;
                    y.ser(buf)?;
                    4u32
                }
                Move { seat, serial } => {
                    seat.ser(buf)?;
                    serial.ser(buf)?;
                    5u32
                }
                Resize { seat, serial, edges } => {
                    seat.ser(buf)?;
                    serial.ser(buf)?;
                    edges.ser(buf)?;
                    6u32
                }
                SetMaxSize { width, height } => {
                    width.ser(buf)?;
                    height.ser(buf)?;
                    7u32
                }
                SetMinSize { width, height } => {
                    width.ser(buf)?;
                    height.ser(buf)?;
                    8u32
                }
                SetMaximized {} => 9u32,
                UnsetMaximized {} => 10u32,
                SetFullscreen { output } => {
                    output.ser(buf)?;
                    11u32
                }
                UnsetFullscreen {} => 12u32,
                SetMinimized {} => 13u32,
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "xdg_toplevel", self.0)
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
/**
A popup surface is a short-lived, temporary surface. It can be used to
implement for example menus, popovers, tooltips and other similar user
interface concepts.

A popup can be made to take an explicit grab. See xdg_popup.grab for
details.

When the popup is dismissed, a popup_done event will be sent out, and at
the same time the surface will be unmapped. See the xdg_popup.popup_done
event for details.

Explicitly destroying the xdg_popup object will also dismiss the popup and
unmap the surface. Clients that want to dismiss the popup when another
surface of their own is clicked should dismiss the popup using the destroy
request.

A newly created xdg_popup will be stacked on top of all previously created
xdg_popup surfaces associated with the same xdg_toplevel.

The parent of an xdg_popup must be mapped (see the xdg_surface
description) before the xdg_popup itself.

The client must call wl_surface.commit on the corresponding wl_surface
for the xdg_popup state to take effect.

*/
pub mod xdg_popup {
    use super::*;
    ///
    #[repr(u32)]
    enum Error {
        InvalidGrab = 0u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::InvalidGrab),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::InvalidGrab => 0u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Configure { x: i32, y: i32, width: i32, height: i32 },
        PopupDone {},
        Repositioned { token: u32 },
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let x = <i32 as Dsr>::dsr(buf)?;
                    let y = <i32 as Dsr>::dsr(buf)?;
                    let width = <i32 as Dsr>::dsr(buf)?;
                    let height = <i32 as Dsr>::dsr(buf)?;
                    Configure { x, y, width, height }
                }
                1u16 => PopupDone {},
                2u16 => {
                    let token = <u32 as Dsr>::dsr(buf)?;
                    Repositioned { token }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Configure { x, y, width, height } => {
                    x.ser(buf)?;
                    y.ser(buf)?;
                    width.ser(buf)?;
                    height.ser(buf)?;
                    0u32
                }
                PopupDone {} => 1u32,
                Repositioned { token } => {
                    token.ser(buf)?;
                    2u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        Destroy {},
        Grab { seat: wl_seat::Handle, serial: u32 },
        Reposition { positioner: xdg_positioner::Handle, token: u32 },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => Destroy {},
                1u16 => {
                    let seat = <wl_seat::Handle as Dsr>::dsr(buf)?;
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    Grab { seat, serial }
                }
                2u16 => {
                    let positioner = <xdg_positioner::Handle as Dsr>::dsr(buf)?;
                    let token = <u32 as Dsr>::dsr(buf)?;
                    Reposition { positioner, token }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                Destroy {} => 0u32,
                Grab { seat, serial } => {
                    seat.ser(buf)?;
                    serial.ser(buf)?;
                    1u32
                }
                Reposition { positioner, token } => {
                    positioner.ser(buf)?;
                    token.ser(buf)?;
                    2u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "xdg_popup", self.0)
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
/**
Clients can use this interface to assign the surface_layer role to
wl_surfaces. Such surfaces are assigned to a "layer" of the output and
rendered with a defined z-depth respective to each other. They may also be
anchored to the edges and corners of a screen and specify input handling
semantics. This interface should be suitable for the implementation of
many desktop shell components, and a broad number of other applications
that interact with the desktop.

*/
pub mod zwlr_layer_shell_v1 {
    use super::*;
    ///
    #[repr(u32)]
    enum Error {
        Role = 0u32,
        InvalidLayer = 1u32,
        AlreadyConstructed = 2u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::Role),
                1u32 => Ok(Error::InvalidLayer),
                2u32 => Ok(Error::AlreadyConstructed),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::Role => 0u32,
                Error::InvalidLayer => 1u32,
                Error::AlreadyConstructed => 2u32,
            }
        }
    }
    /**
These values indicate which layers a surface can be rendered in. They
are ordered by z depth, bottom-most first. Traditional shell surfaces
will typically be rendered between the bottom and top layers.
Fullscreen shell surfaces are typically rendered at the top layer.
Multiple surfaces can share a single layer, and ordering within a
single layer is undefined.

*/
    #[repr(u32)]
    enum Layer {
        Background = 0u32,
        Bottom = 1u32,
        Top = 2u32,
        Overlay = 3u32,
    }
    impl TryFrom<u32> for Layer {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Layer, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Layer::Background),
                1u32 => Ok(Layer::Bottom),
                2u32 => Ok(Layer::Top),
                3u32 => Ok(Layer::Overlay),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Layer> for u32 {
        fn from(n: Layer) -> u32 {
            match n {
                Layer::Background => 0u32,
                Layer::Bottom => 1u32,
                Layer::Top => 2u32,
                Layer::Overlay => 3u32,
            }
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        GetLayerSurface {
            id: zwlr_layer_surface_v1::Handle,
            surface: wl_surface::Handle,
            output: wl_output::Handle,
            layer: u32,
            namespace: String,
        },
        Destroy {},
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let id = <zwlr_layer_surface_v1::Handle as Dsr>::dsr(buf)?;
                    let surface = <wl_surface::Handle as Dsr>::dsr(buf)?;
                    let output = <wl_output::Handle as Dsr>::dsr(buf)?;
                    let layer = <u32 as Dsr>::dsr(buf)?;
                    let namespace = <String as Dsr>::dsr(buf)?;
                    GetLayerSurface {
                        id,
                        surface,
                        output,
                        layer,
                        namespace,
                    }
                }
                1u16 => Destroy {},
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                GetLayerSurface { id, surface, output, layer, namespace } => {
                    id.ser(buf)?;
                    surface.ser(buf)?;
                    output.ser(buf)?;
                    layer.ser(buf)?;
                    namespace.ser(buf)?;
                    0u32
                }
                Destroy {} => 1u32,
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "zwlr_layer_shell_v1", self.0)
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
/**
An interface that may be implemented by a wl_surface, for surfaces that
are designed to be rendered as a layer of a stacked desktop-like
environment.

Layer surface state (layer, size, anchor, exclusive zone,
margin, interactivity) is double-buffered, and will be applied at the
time wl_surface.commit of the corresponding wl_surface is called.

Attaching a null buffer to a layer surface unmaps it.

Unmapping a layer_surface means that the surface cannot be shown by the
compositor until it is explicitly mapped again. The layer_surface
returns to the state it had right after layer_shell.get_layer_surface.
The client can re-map the surface by performing a commit without any
buffer attached, waiting for a configure event and handling it as usual.

*/
pub mod zwlr_layer_surface_v1 {
    use super::*;
    /**
Types of keyboard interaction possible for layer shell surfaces. The
rationale for this is twofold: (1) some applications are not interested
in keyboard events and not allowing them to be focused can improve the
desktop experience; (2) some applications will want to take exclusive
keyboard focus.

*/
    #[repr(u32)]
    enum KeyboardInteractivity {
        None = 0u32,
        Exclusive = 1u32,
        OnDemand = 2u32,
    }
    impl TryFrom<u32> for KeyboardInteractivity {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<KeyboardInteractivity, ValueOutOfBounds> {
            match n {
                0u32 => Ok(KeyboardInteractivity::None),
                1u32 => Ok(KeyboardInteractivity::Exclusive),
                2u32 => Ok(KeyboardInteractivity::OnDemand),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<KeyboardInteractivity> for u32 {
        fn from(n: KeyboardInteractivity) -> u32 {
            match n {
                KeyboardInteractivity::None => 0u32,
                KeyboardInteractivity::Exclusive => 1u32,
                KeyboardInteractivity::OnDemand => 2u32,
            }
        }
    }
    ///
    #[repr(u32)]
    enum Error {
        InvalidSurfaceState = 0u32,
        InvalidSize = 1u32,
        InvalidAnchor = 2u32,
        InvalidKeyboardInteractivity = 3u32,
    }
    impl TryFrom<u32> for Error {
        type Error = ValueOutOfBounds;
        fn try_from(n: u32) -> Result<Error, ValueOutOfBounds> {
            match n {
                0u32 => Ok(Error::InvalidSurfaceState),
                1u32 => Ok(Error::InvalidSize),
                2u32 => Ok(Error::InvalidAnchor),
                3u32 => Ok(Error::InvalidKeyboardInteractivity),
                _ => Err(ValueOutOfBounds),
            }
        }
    }
    impl From<Error> for u32 {
        fn from(n: Error) -> u32 {
            match n {
                Error::InvalidSurfaceState => 0u32,
                Error::InvalidSize => 1u32,
                Error::InvalidAnchor => 2u32,
                Error::InvalidKeyboardInteractivity => 3u32,
            }
        }
    }
    ///
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Anchor(u32);
    impl Anchor {
        const TOP: Anchor = Anchor(1u32);
        const BOTTOM: Anchor = Anchor(2u32);
        const LEFT: Anchor = Anchor(4u32);
        const RIGHT: Anchor = Anchor(8u32);
    }
    impl std::fmt::Debug for Anchor {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            let vs: Vec<&str> = [
                ("TOP", Anchor::TOP),
                ("BOTTOM", Anchor::BOTTOM),
                ("LEFT", Anchor::LEFT),
                ("RIGHT", Anchor::RIGHT),
            ]
                .iter()
                .filter(|(_, v)| (*v & *self) == *v)
                .map(|(s, _)| *s)
                .collect();
            write!(f, "{}", vs.join("|"))
        }
    }
    impl From<u32> for Anchor {
        fn from(n: u32) -> Anchor {
            Anchor(n)
        }
    }
    impl From<Anchor> for u32 {
        fn from(v: Anchor) -> u32 {
            v.0
        }
    }
    impl std::ops::BitOr for Anchor {
        type Output = Anchor;
        fn bitor(self, rhs: Anchor) -> Anchor {
            Anchor(self.0 | rhs.0)
        }
    }
    impl std::ops::BitAnd for Anchor {
        type Output = Anchor;
        fn bitand(self, rhs: Anchor) -> Anchor {
            Anchor(self.0 & rhs.0)
        }
    }
    impl Dsr for Anchor {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            u32::dsr(buf).map(|n| n.into())
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            let v = u32::from(*self);
            v.ser(buf)
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Configure { serial: u32, width: u32, height: u32 },
        Closed {},
    }
    impl Dsr for Message<Handle, Event> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Event::*;
            let event = match op {
                0u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    let width = <u32 as Dsr>::dsr(buf)?;
                    let height = <u32 as Dsr>::dsr(buf)?;
                    Configure { serial, width, height }
                }
                1u16 => Closed {},
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Event::*;
            let op: u32 = match &self.1 {
                Configure { serial, width, height } => {
                    serial.ser(buf)?;
                    width.ser(buf)?;
                    height.ser(buf)?;
                    0u32
                }
                Closed {} => 1u32,
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum Request {
        SetSize { width: u32, height: u32 },
        SetAnchor { anchor: u32 },
        SetExclusiveZone { zone: i32 },
        SetMargin { top: i32, right: i32, bottom: i32, left: i32 },
        SetKeyboardInteractivity { keyboard_interactivity: u32 },
        GetPopup { popup: xdg_popup::Handle },
        AckConfigure { serial: u32 },
        Destroy {},
        SetLayer { layer: u32 },
    }
    impl Dsr for Message<Handle, Request> {
        fn dsr(buf: &mut ReadBuf) -> Result<Self, DsrError> {
            let sz_start = buf.mark();
            let (id, oplen) = <(Handle, u32)>::dsr(buf)?;
            let (op, len) = ((oplen >> 16) as u16, (oplen & 0xffff) as usize);
            use Request::*;
            let event = match op {
                0u16 => {
                    let width = <u32 as Dsr>::dsr(buf)?;
                    let height = <u32 as Dsr>::dsr(buf)?;
                    SetSize { width, height }
                }
                1u16 => {
                    let anchor = <u32 as Dsr>::dsr(buf)?;
                    SetAnchor { anchor }
                }
                2u16 => {
                    let zone = <i32 as Dsr>::dsr(buf)?;
                    SetExclusiveZone { zone }
                }
                3u16 => {
                    let top = <i32 as Dsr>::dsr(buf)?;
                    let right = <i32 as Dsr>::dsr(buf)?;
                    let bottom = <i32 as Dsr>::dsr(buf)?;
                    let left = <i32 as Dsr>::dsr(buf)?;
                    SetMargin {
                        top,
                        right,
                        bottom,
                        left,
                    }
                }
                4u16 => {
                    let keyboard_interactivity = <u32 as Dsr>::dsr(buf)?;
                    SetKeyboardInteractivity {
                        keyboard_interactivity,
                    }
                }
                5u16 => {
                    let popup = <xdg_popup::Handle as Dsr>::dsr(buf)?;
                    GetPopup { popup }
                }
                6u16 => {
                    let serial = <u32 as Dsr>::dsr(buf)?;
                    AckConfigure { serial }
                }
                7u16 => Destroy {},
                8u16 => {
                    let layer = <u32 as Dsr>::dsr(buf)?;
                    SetLayer { layer }
                }
                _ => panic!("unrecognized opcode: {}", op),
            };
            if wayland_debug() && (buf.mark() - sz_start) != 8 + len {
                eprintln!(
                    "actual_sz != expected_sz: {} != {}", (buf.mark() - sz_start), 8 +
                    len
                );
            }
            Ok(Message(id, event))
        }
        fn ser(&self, buf: &mut WriteBuf) -> Result<(), SerError> {
            (self.0, 0u32).ser(buf)?;
            let sz_start = buf.0.len();
            use Request::*;
            let op: u32 = match &self.1 {
                SetSize { width, height } => {
                    width.ser(buf)?;
                    height.ser(buf)?;
                    0u32
                }
                SetAnchor { anchor } => {
                    anchor.ser(buf)?;
                    1u32
                }
                SetExclusiveZone { zone } => {
                    zone.ser(buf)?;
                    2u32
                }
                SetMargin { top, right, bottom, left } => {
                    top.ser(buf)?;
                    right.ser(buf)?;
                    bottom.ser(buf)?;
                    left.ser(buf)?;
                    3u32
                }
                SetKeyboardInteractivity { keyboard_interactivity } => {
                    keyboard_interactivity.ser(buf)?;
                    4u32
                }
                GetPopup { popup } => {
                    popup.ser(buf)?;
                    5u32
                }
                AckConfigure { serial } => {
                    serial.ser(buf)?;
                    6u32
                }
                Destroy {} => 7u32,
                SetLayer { layer } => {
                    layer.ser(buf)?;
                    8u32
                }
            };
            assert!(op < u16::MAX as u32);
            let len = (buf.0.len() - sz_start) as u32;
            assert!(len < u16::MAX as u32);
            let oplen = (op << 16) | len;
            buf.0[(sz_start - 4)..sz_start].copy_from_slice(&oplen.to_ne_bytes()[..]);
            Ok(())
        }
    }
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Handle(pub u32);
    impl std::fmt::Debug for Handle {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}@{}", "zwlr_layer_surface_v1", self.0)
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
