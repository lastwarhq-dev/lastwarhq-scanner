//! The game's SmartFox 2X protocol: `framing` splits the byte stream into frames, `message`
//! decompresses and decodes them, and `sfs` parses the SFSObject inside. `proto` reads the
//! protobuf messages some of them carry as byte arrays.

pub mod framing;
pub mod message;
pub mod proto;
pub mod sfs;
