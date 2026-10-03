//! Splits the server's byte stream into SmartFox 2X frames.
//!
//! Frame layout: flags byte · length (u16 BE, or u32 BE with [`FLAG_BIG_LENGTH`]) ·
//! uncompressed size (u32 BE, only with [`FLAG_SIZE_PREFIX`]) · body.

pub const FLAG_BINARY: u8 = 0x80;
pub const FLAG_ENCRYPTED: u8 = 0x40;
pub const FLAG_COMPRESSED: u8 = 0x20;
/// Seen together with [`FLAG_COMPRESSED`] (header byte `0xb0`): a u32 uncompressed size follows the length.
pub const FLAG_SIZE_PREFIX: u8 = 0x10;
pub const FLAG_BIG_LENGTH: u8 = 0x08;

pub const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const SFS_OBJECT: u8 = 0x12;
const MAX_BODY: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub flags: u8,
    pub uncompressed_len: Option<u32>,
    pub body: Vec<u8>,
    /// Header plus body, as it appeared in the stream.
    pub wire_len: usize,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FramerStats {
    pub frames: u64,
    /// Times the framer lost its place and had to search for a message start.
    pub resyncs: u64,
    /// Bytes discarded while searching.
    pub skipped_bytes: u64,
}

impl FramerStats {
    pub fn add(&mut self, other: &FramerStats) {
        self.frames += other.frames;
        self.resyncs += other.resyncs;
        self.skipped_bytes += other.skipped_bytes;
    }
}

struct Header {
    flags: u8,
    header_len: usize,
    body_len: usize,
    uncompressed_len: Option<u32>,
}

enum Parse<T> {
    Ok(T),
    NeedMore,
    Invalid,
}

fn parse_header(buf: &[u8]) -> Parse<Header> {
    let Some(&flags) = buf.first() else {
        return Parse::NeedMore;
    };
    if flags & FLAG_BINARY == 0 {
        return Parse::Invalid;
    }
    let len_size = if flags & FLAG_BIG_LENGTH != 0 { 4 } else { 2 };
    let size_size = if flags & FLAG_SIZE_PREFIX != 0 { 4 } else { 0 };
    let header_len = 1 + len_size + size_size;
    if buf.len() < header_len {
        return Parse::NeedMore;
    }
    let body_len = match len_size {
        4 => u32::from_be_bytes(buf[1..5].try_into().unwrap()) as usize,
        _ => u16::from_be_bytes(buf[1..3].try_into().unwrap()) as usize,
    };
    if body_len == 0 || body_len > MAX_BODY {
        return Parse::Invalid;
    }
    let uncompressed_len = (size_size == 4).then(|| {
        let at = 1 + len_size;
        u32::from_be_bytes(buf[at..at + 4].try_into().unwrap())
    });
    Parse::Ok(Header {
        flags,
        header_len,
        body_len,
        uncompressed_len,
    })
}

/// While resynchronising: does a believable frame start at `buf[0]`?
/// Returns the frame's total length if so.
fn plausible_frame(buf: &[u8]) -> Parse<usize> {
    let header = match parse_header(buf) {
        Parse::Ok(h) => h,
        Parse::NeedMore => return Parse::NeedMore,
        Parse::Invalid => return Parse::Invalid,
    };
    // Encrypted bodies have nothing recognisable to check.
    if header.flags & FLAG_ENCRYPTED != 0 {
        return Parse::Invalid;
    }
    let body = &buf[header.header_len..];
    let ok = if header.flags & FLAG_COMPRESSED != 0 {
        if body.len() < 4 {
            return Parse::NeedMore;
        }
        body[..4] == ZSTD_MAGIC
    } else {
        // SFSObject: type byte, entry count, then the first key's length.
        if body.len() < 5 {
            return Parse::NeedMore;
        }
        let count = u16::from_be_bytes([body[1], body[2]]);
        let key_len = u16::from_be_bytes([body[3], body[4]]);
        body[0] == SFS_OBJECT && (1..1024).contains(&count) && (1..256).contains(&key_len)
    };
    if ok {
        Parse::Ok(header.header_len + header.body_len)
    } else {
        Parse::Invalid
    }
}

enum Search {
    /// A frame starts at this offset.
    Found(usize),
    /// Nothing before this offset can start a frame; wait for more bytes.
    NeedMore(usize),
}

/// Looks for two consecutive plausible frames, which marks a real boundary.
fn find_start(buf: &[u8]) -> Search {
    for i in 0..buf.len() {
        let total = match plausible_frame(&buf[i..]) {
            Parse::Ok(total) => total,
            Parse::NeedMore => return Search::NeedMore(i),
            Parse::Invalid => continue,
        };
        if buf.len() - i < total {
            return Search::NeedMore(i);
        }
        match plausible_frame(&buf[i + total..]) {
            Parse::Ok(_) => return Search::Found(i),
            Parse::NeedMore => return Search::NeedMore(i),
            Parse::Invalid => continue,
        }
    }
    Search::NeedMore(buf.len())
}

/// Accumulates stream bytes and emits complete frames.
#[derive(Default)]
pub struct Framer {
    buf: Vec<u8>,
    /// Starts false: capture usually begins mid-connection, so the first boundary has to be
    /// found.
    synced: bool,
    pub stats: FramerStats,
}

/// Buffer allocation a framer keeps between frames. Past this, an allocation that is mostly
/// unused is released, so one large frame doesn't hold its memory for the rest of the
/// connection.
const KEEP_CAPACITY: usize = 64 * 1024;

impl Framer {
    /// Memory held for bytes waiting for the rest of a frame: the buffer's whole allocation,
    /// which can be more than the bytes it holds.
    pub fn buffered(&self) -> usize {
        self.buf.capacity()
    }

    /// Discards buffered bytes after a stream gap.
    pub fn gap(&mut self) {
        self.stats.skipped_bytes += self.buf.len() as u64;
        self.buf.clear();
        self.release_spare();
        self.synced = false;
    }

    /// Shrinks the buffer when less than half of a large allocation is in use.
    fn release_spare(&mut self) {
        let keep = self.buf.len().max(KEEP_CAPACITY);
        if self.buf.capacity() > 2 * keep {
            self.buf.shrink_to(keep);
        }
    }

    pub fn push(&mut self, data: &[u8], out: &mut Vec<Frame>) {
        self.buf.extend_from_slice(data);
        let mut start = 0;
        loop {
            if !self.synced {
                match find_start(&self.buf[start..]) {
                    Search::Found(offset) => {
                        self.stats.skipped_bytes += offset as u64;
                        start += offset;
                        self.synced = true;
                    }
                    Search::NeedMore(offset) => {
                        self.stats.skipped_bytes += offset as u64;
                        start += offset;
                        break;
                    }
                }
            }
            let header = match parse_header(&self.buf[start..]) {
                Parse::Ok(h) => h,
                Parse::NeedMore => break,
                Parse::Invalid => {
                    self.synced = false;
                    self.stats.resyncs += 1;
                    continue;
                }
            };
            let total = header.header_len + header.body_len;
            if self.buf.len() - start < total {
                break;
            }
            out.push(Frame {
                flags: header.flags,
                uncompressed_len: header.uncompressed_len,
                body: self.buf[start + header.header_len..start + total].to_vec(),
                wire_len: total,
            });
            self.stats.frames += 1;
            start += total;
        }
        self.buf.drain(..start);
        self.release_spare();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain frame whose body is a one-entry SFSObject {"k": byte}.
    fn plain(value: u8) -> Vec<u8> {
        let body = [SFS_OBJECT, 0, 1, 0, 1, b'k', 2, value];
        let mut f = vec![0x80, 0, body.len() as u8];
        f.extend_from_slice(&body);
        f
    }

    /// A 4-byte-length frame with a `len`-byte body that starts like an SFSObject.
    fn large(len: u32) -> Vec<u8> {
        let mut f = vec![FLAG_BINARY | FLAG_BIG_LENGTH];
        f.extend_from_slice(&len.to_be_bytes());
        f.extend_from_slice(&[SFS_OBJECT, 0, 1, 0, 1, b'k']);
        f.resize(5 + len as usize, 0);
        f
    }

    #[test]
    fn memory_of_a_large_frame_is_released_and_counted() {
        // Half of a 4 MB frame arrives in 1,000-byte pieces, then the stream has a gap.
        let half = &large(4_000_000)[..2_000_000];
        let mut framer = Framer::default();
        let mut out = Vec::new();
        for piece in half.chunks(1000) {
            framer.push(piece, &mut out);
        }
        assert!(out.is_empty());
        assert!(framer.buffered() >= 2_000_000);
        framer.gap();
        assert!(framer.buffered() <= KEEP_CAPACITY, "{}", framer.buffered());

        // A whole large frame is emitted; the small remainder doesn't keep its allocation.
        let mut framer = Framer::default();
        let stream = [large(4_000_000), plain(1), plain(2)[..4].to_vec()].concat();
        for piece in stream.chunks(1000) {
            framer.push(piece, &mut out);
        }
        assert_eq!(out.len(), 2);
        assert!(framer.buffered() <= KEEP_CAPACITY, "{}", framer.buffered());
    }

    fn compressed() -> Vec<u8> {
        let body = [0x28, 0xb5, 0x2f, 0xfd, 1, 2, 3];
        let mut f = vec![0xb0, 0, body.len() as u8, 0, 0, 0, 99];
        f.extend_from_slice(&body);
        f
    }

    fn frames(framer: &mut Framer, chunks: &[&[u8]]) -> Vec<Frame> {
        let mut out = Vec::new();
        for chunk in chunks {
            framer.push(chunk, &mut out);
        }
        out
    }

    #[test]
    fn several_frames_in_one_chunk() {
        let stream = [plain(1), compressed(), plain(2)].concat();
        let out = frames(&mut Framer::default(), &[&stream]);
        assert_eq!(out.len(), 3);
        assert_eq!(out[1].flags, 0xb0);
        assert_eq!(out[1].uncompressed_len, Some(99));
        assert_eq!(out[1].body[..4], ZSTD_MAGIC);
    }

    #[test]
    fn first_frame_waits_for_confirmation() {
        let mut framer = Framer::default();
        assert!(frames(&mut framer, &[&plain(1)]).is_empty());
        assert_eq!(frames(&mut framer, &[&plain(2)]).len(), 2);
    }

    #[test]
    fn frame_split_across_chunks() {
        let stream = [plain(1), plain(2), plain(3)].concat();
        let mut framer = Framer::default();
        let out: Vec<_> = stream
            .chunks(3)
            .flat_map(|c| frames(&mut framer, &[c]))
            .collect();
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].body[7], 3);
    }

    #[test]
    fn resyncs_from_mid_frame() {
        let first = plain(1);
        let stream = [&first[4..], &plain(2)[..], &plain(3)[..]].concat();
        let mut framer = Framer::default();
        let out = frames(&mut framer, &[&stream]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].body[7], 2);
        assert_eq!(framer.stats.skipped_bytes, (first.len() - 4) as u64);
    }

    #[test]
    fn gap_discards_partial_frame() {
        let mut framer = Framer::default();
        let a = plain(1);
        let mut out = Vec::new();
        framer.push(&[&a[..], &a[..], &a[..4]].concat(), &mut out);
        framer.gap();
        framer.push(&[plain(2), plain(3)].concat(), &mut out);
        let values: Vec<u8> = out.iter().map(|f| f.body[7]).collect();
        assert_eq!(values, [1, 1, 2, 3]);
    }
}
