//! TCP segment parsing and one-direction stream reassembly.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use etherparse::{NetSlice, SlicedPacket, TransportSlice};

/// The TCP fields the pipeline needs from one Ethernet frame.
pub struct Segment<'a> {
    pub src: SocketAddr,
    pub dst: SocketAddr,
    pub seq: u32,
    pub syn: bool,
    pub fin: bool,
    pub rst: bool,
    pub payload: &'a [u8],
}

/// Parses an Ethernet frame. Returns `None` for anything that is not TCP over IP.
pub fn parse_segment(frame: &[u8]) -> Option<Segment<'_>> {
    let packet = SlicedPacket::from_ethernet(frame).ok()?;
    let (src_ip, dst_ip) = match packet.net? {
        NetSlice::Ipv4(ip) => (
            IpAddr::V4(ip.header().source_addr()),
            IpAddr::V4(ip.header().destination_addr()),
        ),
        NetSlice::Ipv6(ip) => (
            IpAddr::V6(ip.header().source_addr()),
            IpAddr::V6(ip.header().destination_addr()),
        ),
        _ => return None,
    };
    let TransportSlice::Tcp(tcp) = packet.transport? else {
        return None;
    };
    Some(Segment {
        src: SocketAddr::new(src_ip, tcp.source_port()),
        dst: SocketAddr::new(dst_ip, tcp.destination_port()),
        seq: tcp.sequence_number(),
        syn: tcp.syn(),
        fin: tcp.fin(),
        rst: tcp.rst(),
        payload: tcp.payload(),
    })
}

/// Output of [`Stream::push`].
#[derive(Debug, PartialEq)]
pub enum StreamEvent {
    /// The next bytes of the stream, in order.
    Data(Vec<u8>),
    /// Bytes were lost; whatever follows does not continue the previous data.
    Gap,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct StreamStats {
    pub segments: u64,
    /// Segments that carried no new bytes (retransmissions).
    pub duplicates: u64,
    /// Segments that arrived ahead of a missing one and were buffered.
    pub out_of_order: u64,
    pub gaps: u64,
}

impl StreamStats {
    pub fn add(&mut self, other: &StreamStats) {
        self.segments += other.segments;
        self.duplicates += other.duplicates;
        self.out_of_order += other.out_of_order;
        self.gaps += other.gaps;
    }
}

/// Buffered out-of-order data above this (see [`Stream::buffered`]) is treated as a permanent
/// gap.
const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;

/// Room for waiting segments kept once none are waiting.
const KEEP_PENDING: usize = 64;

/// How long data may wait behind a missing segment. A segment lost on the network is resent
/// well within this; one the capture missed (but the game received) never is, so after this the
/// missing bytes are given up and decoding resumes after them.
pub const GAP_TIMEOUT: Duration = Duration::from_secs(3);

/// Rebuilds one direction of a TCP connection from segments in arrival order.
/// The first segment seen sets the starting point, so capture can begin mid-connection.
#[derive(Default)]
pub struct Stream {
    next_seq: Option<u32>,
    pending: Vec<(u32, Vec<u8>)>,
    pending_bytes: usize,
    /// Capture time of the oldest data waiting behind a missing segment.
    waiting_since: Option<Duration>,
    pub stats: StreamStats,
}

impl Stream {
    /// Memory held for segments waiting behind a missing one: their bytes (each copied into an
    /// allocation of exactly its size) and the list that holds them, including its spare room.
    /// Many tiny segments cost far more than their bytes.
    pub fn buffered(&self) -> usize {
        self.pending_bytes + self.pending.capacity() * size_of::<(u32, Vec<u8>)>()
    }

    /// Adds a segment captured at `now`.
    pub fn push(
        &mut self,
        now: Duration,
        mut seq: u32,
        syn: bool,
        payload: &[u8],
        out: &mut Vec<StreamEvent>,
    ) {
        if syn {
            if self.next_seq.is_some() {
                out.push(StreamEvent::Gap);
            }
            // SYN uses one sequence number; data starts after it.
            seq = seq.wrapping_add(1);
            self.next_seq = Some(seq);
            self.pending.clear();
            self.pending_bytes = 0;
            self.waiting_since = None;
        }
        if !payload.is_empty() {
            self.stats.segments += 1;
            let next = *self.next_seq.get_or_insert(seq);
            if seq.wrapping_sub(next) as i32 > 0 {
                self.stats.out_of_order += 1;
                self.pending_bytes += payload.len();
                self.pending.push((seq, payload.to_vec()));
                self.waiting_since.get_or_insert(now);
            } else {
                self.deliver(seq, payload, out);
                self.drain(out);
            }
        }
        // Give up on missing bytes once data has waited too long behind them, or too much has.
        while !self.pending.is_empty()
            && (self.buffered() > MAX_PENDING_BYTES
                || self
                    .waiting_since
                    .is_some_and(|t| now.saturating_sub(t) >= GAP_TIMEOUT))
        {
            self.skip_gap(out);
        }
        if self.pending.is_empty() {
            self.waiting_since = None;
            // The list is empty again: don't keep room for a burst that has passed.
            if self.pending.capacity() > KEEP_PENDING {
                self.pending = Vec::new();
            }
        } else {
            self.waiting_since.get_or_insert(now);
        }
    }

    /// Emits the part of a segment at or after `next_seq`. Requires `seq <= next_seq`.
    fn deliver(&mut self, seq: u32, payload: &[u8], out: &mut Vec<StreamEvent>) {
        let next = self.next_seq.expect("stream position set before delivery");
        let already_seen = next.wrapping_sub(seq) as usize;
        if already_seen >= payload.len() {
            self.stats.duplicates += 1;
            return;
        }
        let fresh = &payload[already_seen..];
        out.push(StreamEvent::Data(fresh.to_vec()));
        self.next_seq = Some(next.wrapping_add(fresh.len() as u32));
    }

    /// Delivers buffered segments that have become contiguous. The list is sorted latest first
    /// (by distance from the stream position, so wrapping is handled), and segments are taken
    /// off its end in order while they start at or before the position.
    fn drain(&mut self, out: &mut Vec<StreamEvent>) {
        let next = self.next_seq.expect("stream position set before drain");
        self.pending
            .sort_unstable_by_key(|(seq, _)| std::cmp::Reverse(seq.wrapping_sub(next) as i32));
        while let Some(&(seq, _)) = self.pending.last() {
            let next = self.next_seq.expect("stream position set before drain");
            if seq.wrapping_sub(next) as i32 > 0 {
                break;
            }
            let (seq, data) = self.pending.pop().expect("last exists");
            self.pending_bytes -= data.len();
            self.deliver(seq, &data, out);
        }
    }

    /// Gives up on missing bytes and resumes at the earliest buffered segment.
    fn skip_gap(&mut self, out: &mut Vec<StreamEvent>) {
        let next = self.next_seq.expect("stream position set before gap");
        let Some(earliest) = self
            .pending
            .iter()
            .map(|(seq, _)| *seq)
            .min_by_key(|seq| seq.wrapping_sub(next))
        else {
            return;
        };
        self.stats.gaps += 1;
        out.push(StreamEvent::Gap);
        self.next_seq = Some(earliest);
        self.drain(out);
        // Anything still waiting is behind a later gap; its wait restarts (see `push`).
        self.waiting_since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(stream: &mut Stream, segments: &[(u32, &[u8])]) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        for &(seq, data) in segments {
            stream.push(Duration::ZERO, seq, false, data, &mut out);
        }
        out
    }

    fn data(events: &[StreamEvent]) -> Vec<u8> {
        events
            .iter()
            .flat_map(|e| match e {
                StreamEvent::Data(d) => d.clone(),
                StreamEvent::Gap => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn in_order() {
        let mut s = Stream::default();
        let out = run(&mut s, &[(100, b"abc"), (103, b"def")]);
        assert_eq!(data(&out), b"abcdef");
    }

    #[test]
    fn retransmission_is_dropped() {
        let mut s = Stream::default();
        let out = run(&mut s, &[(100, b"abc"), (100, b"abc"), (103, b"def")]);
        assert_eq!(data(&out), b"abcdef");
        assert_eq!(s.stats.duplicates, 1);
    }

    #[test]
    fn overlap_keeps_only_new_bytes() {
        let mut s = Stream::default();
        let out = run(&mut s, &[(100, b"abc"), (101, b"bcdef")]);
        assert_eq!(data(&out), b"abcdef");
    }

    #[test]
    fn out_of_order_is_buffered() {
        let mut s = Stream::default();
        let out = run(&mut s, &[(100, b"abc"), (106, b"ghi"), (103, b"def")]);
        assert_eq!(data(&out), b"abcdefghi");
        assert_eq!(s.stats.out_of_order, 1);
    }

    #[test]
    fn sequence_wraps() {
        let mut s = Stream::default();
        let out = run(&mut s, &[(u32::MAX - 1, b"ab"), (0, b"cd")]);
        assert_eq!(data(&out), b"abcd");
    }

    #[test]
    fn syn_resets_and_signals_gap() {
        let mut s = Stream::default();
        let mut out = Vec::new();
        s.push(Duration::ZERO, 100, false, b"abc", &mut out);
        s.push(Duration::ZERO, 5000, true, b"", &mut out);
        s.push(Duration::ZERO, 5001, false, b"xyz", &mut out);
        assert_eq!(
            out,
            vec![
                StreamEvent::Data(b"abc".to_vec()),
                StreamEvent::Gap,
                StreamEvent::Data(b"xyz".to_vec()),
            ]
        );
    }

    #[test]
    fn too_much_waiting_data_becomes_gap() {
        let mut s = Stream::default();
        let mut out = Vec::new();
        s.push(Duration::ZERO, 0, false, b"a", &mut out);
        let big = vec![0u8; MAX_PENDING_BYTES + 1];
        s.push(Duration::ZERO, 10, false, &big, &mut out);
        assert_eq!(out[1], StreamEvent::Gap);
        assert_eq!(s.stats.gaps, 1);
    }

    #[test]
    fn tiny_waiting_segments_are_counted_by_their_cost_and_released() {
        let mut s = Stream::default();
        let mut out = Vec::new();
        s.push(Duration::ZERO, 0, false, b"a", &mut out);
        // Byte 1 is missing; 10,000 one-byte segments wait behind it.
        for i in 0..10_000u32 {
            s.push(Duration::ZERO, 2 + i, false, b"x", &mut out);
        }
        let entry = size_of::<(u32, Vec<u8>)>();
        assert!(s.buffered() >= 10_000 * (1 + entry), "{}", s.buffered());
        // The missing byte arrives: everything is delivered and the room is released.
        s.push(Duration::ZERO, 1, false, b"b", &mut out);
        assert_eq!(data(&out).len(), 10_002);
        assert!(s.buffered() <= KEEP_PENDING * entry, "{}", s.buffered());
    }

    #[test]
    fn many_tiny_waiting_segments_become_a_gap() {
        let mut s = Stream::default();
        let mut out = Vec::new();
        s.push(Duration::ZERO, 0, false, b"a", &mut out);
        // Far fewer than 4 MiB of bytes, but their list passes the limit.
        let entry = size_of::<(u32, Vec<u8>)>() as u32;
        for i in 0..(MAX_PENDING_BYTES as u32 / entry) {
            s.push(Duration::ZERO, 2 + i, false, b"x", &mut out);
        }
        assert_eq!(s.stats.gaps, 1);
        assert!(s.buffered() <= MAX_PENDING_BYTES);
    }

    #[test]
    fn segment_missed_by_capture_is_skipped_after_the_timeout() {
        // Byte 1 is never seen; 1,000 later segments arrive over 5 seconds.
        let mut s = Stream::default();
        let mut out = Vec::new();
        s.push(Duration::ZERO, 0, false, b"a", &mut out);
        for i in 0..1000u32 {
            let at = Duration::from_millis(u64::from(i) * 5);
            s.push(at, 2 + i, false, b"x", &mut out);
        }
        assert_eq!(s.stats.gaps, 1);
        // Everything after the missing byte is delivered once the gap is given up.
        assert_eq!(data(&out).len(), 1 + 1000);
        assert!(s.pending.is_empty());
    }

    #[test]
    fn a_late_retransmission_within_the_timeout_fills_the_hole() {
        let mut s = Stream::default();
        let mut out = Vec::new();
        s.push(Duration::ZERO, 0, false, b"a", &mut out);
        s.push(Duration::from_millis(100), 2, false, b"c", &mut out);
        s.push(Duration::from_millis(900), 1, false, b"b", &mut out);
        assert_eq!(data(&out), b"abc");
        assert_eq!(s.stats.gaps, 0);
    }
}
