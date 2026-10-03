//! Connects the stages: Ethernet frames in, decoded server messages out.
//!
//! Every connection on a game port is decoded, but only one is trusted at a time: the first to
//! produce a SmartFox message is locked on as the game connection. The server's ping reply every
//! 4 s keeps the lock; when the connection closes or its heartbeat stops, the lock moves to the
//! next connection that decodes (for example after the base teleports to another server).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::ops::RangeInclusive;
use std::time::Duration;

use crate::capture::Packet;
use crate::capture::tcp::{self, Stream, StreamEvent, StreamStats};
use crate::protocol::framing::{Framer, FramerStats};
use crate::protocol::message::Message;

/// Game server TCP ports. The port follows the server the base is on: 10000 + the server number
/// (e.g. server 1234 → port 11234). Connections on these ports that are not SmartFox never frame.
pub const GAME_PORTS: RangeInclusive<u16> = 10000..=19999;

/// Three missed 4-second heartbeats release the lock.
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub packets: u64,
    /// Packets that are not TCP on a game port.
    pub ignored: u64,
    /// Client → server segments with data. Their messages are encrypted and not parsed.
    pub client_segments: u64,
    pub client_bytes: u64,
    pub connections: u64,
    /// Times a connection was locked on as the game connection.
    pub locks: u64,
    /// Messages from connections other than the locked one.
    pub discarded: u64,
    /// Connections dropped to stay within [`MAX_CONNECTIONS`] or [`MAX_BUFFERED`].
    pub evicted: u64,
    pub stream: StreamStats,
    pub framer: FramerStats,
}

impl Stats {
    pub fn add(&mut self, other: &Stats) {
        self.packets += other.packets;
        self.ignored += other.ignored;
        self.client_segments += other.client_segments;
        self.client_bytes += other.client_bytes;
        self.connections += other.connections;
        self.locks += other.locks;
        self.discarded += other.discarded;
        self.evicted += other.evicted;
        self.stream.add(&other.stream);
        self.framer.add(&other.framer);
    }
}

/// The connection currently locked on as the game connection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    pub server: SocketAddr,
    pub client: SocketAddr,
    /// Capture time the lock was taken.
    pub since: Duration,
    pub last_heartbeat: Option<Duration>,
}

impl Link {
    fn key(&self) -> (SocketAddr, SocketAddr) {
        (self.server, self.client)
    }

    fn is_stale(&self, now: Duration) -> bool {
        now.saturating_sub(self.last_heartbeat.unwrap_or(self.since)) > HEARTBEAT_TIMEOUT
    }
}

/// A connection with no packets for this long is forgotten (its close may never have been
/// captured). The game connection is never this quiet: its heartbeat is every 4 s.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Connections tracked at once. A new connection past this replaces the one quiet longest,
/// never the locked game connection.
pub const MAX_CONNECTIONS: usize = 64;

/// Bytes all connections together may hold waiting for a missing segment or the rest of a
/// frame. Past this, the connection holding the most is dropped, never the locked game
/// connection. One connection holds at most about 36 MiB (4 MiB of out-of-order data and
/// two frames of up to 16 MiB while finding a frame boundary), so the game connection always
/// fits.
pub const MAX_BUFFERED: usize = 64 * 1024 * 1024;

#[derive(Default)]
struct Connection {
    stream: Stream,
    framer: Framer,
    last_seen: Duration,
}

impl Connection {
    fn buffered(&self) -> usize {
        self.stream.buffered() + self.framer.buffered()
    }
}

pub struct Pipeline {
    ports: RangeInclusive<u16>,
    /// Server → client direction of each connection, keyed by (server, client).
    connections: HashMap<(SocketAddr, SocketAddr), Connection>,
    link: Option<Link>,
    /// When idle connections were last looked for.
    last_sweep: Duration,
    /// Totals from connections that have closed.
    closed: Stats,
    stats: Stats,
}

impl Pipeline {
    pub fn new(ports: RangeInclusive<u16>) -> Self {
        Self {
            ports,
            connections: HashMap::new(),
            link: None,
            last_sweep: Duration::ZERO,
            closed: Stats::default(),
            stats: Stats::default(),
        }
    }

    pub fn link(&self) -> Option<Link> {
        self.link
    }

    pub fn push(&mut self, packet: &Packet, out: &mut Vec<Message>) {
        self.stats.packets += 1;
        let now = packet.time;
        if let Some(link) = self.link.filter(|link| link.is_stale(now)) {
            // Forget the stalled stream too, so the connection is read afresh if it recovers.
            self.close(link.key());
        }
        if now.saturating_sub(self.last_sweep) >= IDLE_TIMEOUT / 2 {
            self.last_sweep = now;
            let idle: Vec<_> = self
                .connections
                .iter()
                .filter(|(_, c)| now.saturating_sub(c.last_seen) >= IDLE_TIMEOUT)
                .map(|(k, _)| *k)
                .collect();
            for key in idle {
                self.close(key);
            }
        }
        let Some(seg) = tcp::parse_segment(&packet.data) else {
            self.stats.ignored += 1;
            return;
        };
        if self.ports.contains(&seg.dst.port()) {
            if !seg.payload.is_empty() {
                self.stats.client_segments += 1;
                self.stats.client_bytes += seg.payload.len() as u64;
            }
            if seg.fin || seg.rst {
                self.close((seg.dst, seg.src));
            }
            return;
        }
        if !self.ports.contains(&seg.src.port()) {
            self.stats.ignored += 1;
            return;
        }

        let key = (seg.src, seg.dst);
        if !self.connections.contains_key(&key) && self.connections.len() >= MAX_CONNECTIONS {
            let quietest = self
                .unlocked()
                .min_by_key(|(_, c)| c.last_seen)
                .map(|(k, _)| *k);
            if let Some(quietest) = quietest {
                self.evict(quietest);
            }
        }
        let conn = self.connections.entry(key).or_insert_with(|| {
            self.stats.connections += 1;
            Connection::default()
        });
        conn.last_seen = now;
        let mut events = Vec::new();
        conn.stream
            .push(now, seg.seq, seg.syn, seg.payload, &mut events);
        let mut frames = Vec::new();
        for event in events {
            match event {
                StreamEvent::Data(bytes) => conn.framer.push(&bytes, &mut frames),
                StreamEvent::Gap => conn.framer.gap(),
            }
        }
        let messages: Vec<Message> = frames
            .into_iter()
            .map(|f| Message::decode(f, now))
            .collect();

        if self.link.is_none() && messages.iter().any(|m| m.object().is_some()) {
            self.link = Some(Link {
                server: key.0,
                client: key.1,
                since: now,
                last_heartbeat: None,
            });
            self.stats.locks += 1;
        }
        match &mut self.link {
            Some(link) if link.key() == key => {
                if messages.iter().any(Message::is_heartbeat) {
                    link.last_heartbeat = Some(now);
                }
                out.extend(messages);
            }
            _ => self.stats.discarded += messages.len() as u64,
        }

        if seg.fin || seg.rst {
            self.close(key);
        }
        self.limit_buffers();
    }

    /// Connections other than the locked game connection.
    fn unlocked(&self) -> impl Iterator<Item = (&(SocketAddr, SocketAddr), &Connection)> {
        let locked = self.link.map(|link| link.key());
        self.connections
            .iter()
            .filter(move |(key, _)| Some(**key) != locked)
    }

    /// Drops the connections holding the most until all together hold at most
    /// [`MAX_BUFFERED`].
    fn limit_buffers(&mut self) {
        loop {
            let total: usize = self.connections.values().map(Connection::buffered).sum();
            if total <= MAX_BUFFERED {
                return;
            }
            let largest = self
                .unlocked()
                .max_by_key(|(_, c)| c.buffered())
                .map(|(k, _)| *k);
            match largest {
                Some(key) => self.evict(key),
                None => return,
            }
        }
    }

    fn evict(&mut self, key: (SocketAddr, SocketAddr)) {
        self.stats.evicted += 1;
        self.close(key);
    }

    fn close(&mut self, key: (SocketAddr, SocketAddr)) {
        if self.link.is_some_and(|link| link.key() == key) {
            self.link = None;
        }
        if let Some(conn) = self.connections.remove(&key) {
            self.closed.stream.add(&conn.stream.stats);
            self.closed.framer.add(&conn.framer.stats);
        }
    }

    pub fn stats(&self) -> Stats {
        let mut stats = self.stats;
        stats.stream = self.closed.stream;
        stats.framer = self.closed.framer;
        for conn in self.connections.values() {
            stats.stream.add(&conn.stream.stats);
            stats.framer.add(&conn.framer.stats);
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use etherparse::PacketBuilder;

    const CLIENT: [u8; 4] = [10, 0, 1, 116];

    /// A SmartFox frame holding `{c: c, a: a, p: {k: 1}}`.
    fn frame(c: u8, a: i16) -> Vec<u8> {
        let mut body = vec![0x12, 0, 3, 0, 1, b'c', 2, c, 0, 1, b'a', 3];
        body.extend_from_slice(&a.to_be_bytes());
        body.extend_from_slice(&[0, 1, b'p', 0x12, 0, 1, 0, 1, b'k', 2, 1]);
        let mut f = vec![0x80];
        f.extend_from_slice(&(body.len() as u16).to_be_bytes());
        f.extend_from_slice(&body);
        f
    }

    fn heartbeat() -> Vec<u8> {
        frame(0, 29)
    }

    fn panel() -> Vec<u8> {
        frame(1, 13)
    }

    /// Feeds TCP segments from one server port, tracking sequence numbers.
    struct Server {
        ip: [u8; 4],
        port: u16,
        seq: u32,
    }

    impl Server {
        fn new(ip: [u8; 4], port: u16) -> Self {
            Self {
                ip,
                port,
                seq: 1000,
            }
        }

        fn packet(&mut self, secs: u64, payload: &[u8], rst: bool) -> Packet {
            let builder = PacketBuilder::ethernet2([1; 6], [2; 6])
                .ipv4(self.ip, CLIENT, 64)
                .tcp(self.port, 50000, self.seq, 1000);
            let builder = if rst { builder.rst() } else { builder };
            let mut data = Vec::new();
            builder.write(&mut data, payload).unwrap();
            self.seq = self.seq.wrapping_add(payload.len() as u32);
            Packet {
                time: Duration::from_secs(secs),
                data,
            }
        }
    }

    fn push(p: &mut Pipeline, packet: Packet) -> usize {
        let mut out = Vec::new();
        p.push(&packet, &mut out);
        out.len()
    }

    #[test]
    fn locks_on_first_smartfox_connection() {
        let mut p = Pipeline::new(GAME_PORTS);
        let mut other = Server::new([129, 226, 2, 37], 10012);
        let mut game = Server::new([203, 0, 113, 10], 11234);
        push(
            &mut p,
            other.packet(0, b"not smartfox at all, just bytes", false),
        );
        assert_eq!(p.link(), None);
        assert_eq!(
            push(
                &mut p,
                game.packet(1, &[heartbeat(), panel()].concat(), false)
            ),
            2
        );
        let link = p.link().unwrap();
        assert_eq!(link.server.port(), 11234);
        assert_eq!(link.last_heartbeat, Some(Duration::from_secs(1)));
    }

    #[test]
    fn ignores_other_game_connection_while_locked() {
        let mut p = Pipeline::new(GAME_PORTS);
        let mut a = Server::new([203, 0, 113, 10], 11234);
        let mut b = Server::new([198, 51, 100, 20], 11233);
        push(&mut p, a.packet(0, &[heartbeat(), panel()].concat(), false));
        assert_eq!(
            push(&mut p, b.packet(1, &[heartbeat(), panel()].concat(), false)),
            0
        );
        assert_eq!(p.link().unwrap().server.port(), 11234);
        assert_eq!(p.stats().discarded, 2);
    }

    #[test]
    fn moves_to_new_connection_after_close() {
        let mut p = Pipeline::new(GAME_PORTS);
        let mut a = Server::new([203, 0, 113, 10], 11234);
        let mut b = Server::new([198, 51, 100, 20], 11233);
        push(&mut p, a.packet(0, &[heartbeat(), panel()].concat(), false));
        push(&mut p, a.packet(1, &[], true));
        assert_eq!(p.link(), None);
        assert_eq!(
            push(&mut p, b.packet(2, &[heartbeat(), panel()].concat(), false)),
            2
        );
        assert_eq!(p.link().unwrap().server.port(), 11233);
        assert_eq!(p.stats().locks, 2);
    }

    #[test]
    fn moves_to_new_connection_when_heartbeat_stops() {
        let mut p = Pipeline::new(GAME_PORTS);
        let mut a = Server::new([203, 0, 113, 10], 11234);
        let mut b = Server::new([198, 51, 100, 20], 11233);
        push(&mut p, a.packet(0, &[heartbeat(), panel()].concat(), false));
        push(&mut p, a.packet(4, &heartbeat(), false));
        // Within the timeout the old lock holds.
        assert_eq!(
            push(
                &mut p,
                b.packet(10, &[heartbeat(), panel()].concat(), false)
            ),
            0
        );
        // 13 s after the last heartbeat it is released and the new connection takes over.
        assert_eq!(push(&mut p, b.packet(17, &heartbeat(), false)), 1);
        assert_eq!(p.link().unwrap().server.port(), 11233);
    }

    #[test]
    fn decoding_resumes_after_a_segment_the_capture_missed() {
        let mut p = Pipeline::new(GAME_PORTS);
        let mut a = Server::new([203, 0, 113, 10], 11234);
        assert_eq!(
            push(&mut p, a.packet(0, &[heartbeat(), panel()].concat(), false)),
            2
        );
        // 30 bytes go by without being captured.
        a.seq = a.seq.wrapping_add(30);
        let mut decoded = 0;
        for t in 1..=5 {
            decoded += push(&mut p, a.packet(t, &[heartbeat(), panel()].concat(), false));
        }
        assert!(
            decoded > 0,
            "messages after the hole are decoded within a few seconds"
        );
        assert_eq!(p.stats().stream.gaps, 1);
        assert_eq!(p.link().unwrap().server.port(), 11234);
    }

    #[test]
    fn idle_connections_are_forgotten() {
        let mut p = Pipeline::new(GAME_PORTS);
        let mut game = Server::new([203, 0, 113, 10], 11234);
        let mut other = Server::new([198, 51, 100, 20], 10012);
        push(&mut p, other.packet(0, b"some other service", false));
        push(
            &mut p,
            game.packet(0, &[heartbeat(), panel()].concat(), false),
        );
        assert_eq!(p.connections.len(), 2);
        // The game keeps talking; the other connection goes quiet without a captured close.
        for t in (4..=200).step_by(4) {
            push(&mut p, game.packet(t, &heartbeat(), false));
        }
        assert_eq!(p.connections.len(), 1);
        assert_eq!(p.link().unwrap().server.port(), 11234);
    }

    #[test]
    fn connections_are_capped_without_losing_the_game() {
        let mut p = Pipeline::new(GAME_PORTS);
        let mut game = Server::new([203, 0, 113, 10], 11234);
        push(
            &mut p,
            game.packet(0, &[heartbeat(), panel()].concat(), false),
        );
        for i in 0..200u16 {
            let mut other = Server::new([198, 51, 100, (i % 250) as u8], 10000 + i);
            push(&mut p, other.packet(1, b"other", false));
        }
        assert_eq!(p.connections.len(), MAX_CONNECTIONS);
        assert_eq!(p.stats().evicted, 200 + 1 - MAX_CONNECTIONS as u64);
        assert_eq!(push(&mut p, game.packet(2, &heartbeat(), false)), 1);
        assert_eq!(p.link().unwrap().server.port(), 11234);
    }

    #[test]
    fn buffered_bytes_are_capped_without_losing_the_game() {
        let mut p = Pipeline::new(GAME_PORTS);
        let mut game = Server::new([203, 0, 113, 10], 11234);
        push(
            &mut p,
            game.packet(0, &[heartbeat(), panel()].concat(), false),
        );
        // Each connection skips one byte, then buffers 65 × 60,000 bytes behind the hole, all
        // within the gap timeout: 3.9 MB each, 78 MB for 20.
        let chunk = vec![0u8; 60_000];
        for i in 0..20u16 {
            let mut other = Server::new([198, 51, 100, i as u8], 10000 + i);
            push(&mut p, other.packet(1, b"x", false));
            other.seq = other.seq.wrapping_add(1);
            for _ in 0..65 {
                push(&mut p, other.packet(1, &chunk, false));
            }
        }
        let total: usize = p.connections.values().map(Connection::buffered).sum();
        assert!(total <= MAX_BUFFERED, "{total} bytes buffered");
        assert!(p.stats().evicted > 0);
        assert_eq!(push(&mut p, game.packet(2, &heartbeat(), false)), 1);
        assert_eq!(p.link().unwrap().server.port(), 11234);
    }

    #[test]
    fn partial_frames_count_toward_the_memory_limit() {
        let mut p = Pipeline::new(GAME_PORTS);
        let mut game = Server::new([203, 0, 113, 10], 11234);
        push(
            &mut p,
            game.packet(0, &[heartbeat(), panel()].concat(), false),
        );
        // Each connection sends 3.9 MB of a 4 MB frame, in order: the framer holds it all.
        let mut frame = vec![0x88];
        frame.extend_from_slice(&4_000_000u32.to_be_bytes());
        frame.extend_from_slice(&[0x12, 0, 1, 0, 1, b'k']);
        frame.resize(3_900_000, 0);
        for i in 0..20u16 {
            let mut other = Server::new([198, 51, 100, i as u8], 10000 + i);
            for piece in frame.chunks(60_000) {
                push(&mut p, other.packet(1, piece, false));
            }
        }
        let total: usize = p.connections.values().map(Connection::buffered).sum();
        assert!(total <= MAX_BUFFERED, "{total} bytes buffered");
        assert!(p.stats().evicted > 0);
        assert_eq!(push(&mut p, game.packet(2, &heartbeat(), false)), 1);
    }
}
