//! Packets in, the game connection's server-to-client byte stream out.
//!
//! `adapters` picks the network adapters to watch and `npcap` captures on them. `tcp` rebuilds
//! each connection's stream, and `pipeline` locks on to the game connection and hands its bytes
//! to the protocol layer.

use std::time::Duration;

pub mod adapters;
pub mod npcap;
pub mod pipeline;
pub mod tcp;

/// One captured Ethernet frame.
pub struct Packet {
    /// Capture time since the Unix epoch.
    pub time: Duration,
    pub data: Vec<u8>,
}
