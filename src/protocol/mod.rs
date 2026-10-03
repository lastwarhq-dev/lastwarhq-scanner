//! The game's SmartFox 2X protocol: `framing` splits the byte stream into frames, `message`
//! decompresses and decodes them, and `sfs` parses the SFSObject inside.

pub mod framing;
pub mod message;
pub mod sfs;
