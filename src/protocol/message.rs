//! Turns frames into decoded messages: decompress, then parse the SFSObject.

use std::io::Read;
use std::time::Duration;

use ruzstd::decoding::StreamingDecoder;

use crate::protocol::framing::{FLAG_COMPRESSED, FLAG_ENCRYPTED, Frame};
use crate::protocol::sfs::{self, Value};

/// Upper bound on a decompressed body, to stop a bad frame exhausting memory.
const MAX_UNPACKED: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum Body {
    Object(Value),
    /// Counted and skipped; never decrypted.
    Encrypted,
    Undecodable(String),
}

#[derive(Debug)]
pub struct Message {
    /// Capture time since the Unix epoch.
    pub time: Duration,
    pub flags: u8,
    pub wire_len: usize,
    /// Size of the SFSObject bytes after decompression.
    pub unpacked_len: usize,
    pub body: Body,
}

impl Message {
    pub fn decode(frame: Frame, time: Duration) -> Message {
        let mut message = Message {
            time,
            flags: frame.flags,
            wire_len: frame.wire_len,
            unpacked_len: 0,
            body: Body::Encrypted,
        };
        if frame.flags & FLAG_ENCRYPTED != 0 {
            return message;
        }
        let bytes = if frame.flags & FLAG_COMPRESSED != 0 {
            match decompress(&frame) {
                Ok(bytes) => bytes,
                Err(reason) => {
                    message.body = Body::Undecodable(reason);
                    return message;
                }
            }
        } else {
            frame.body
        };
        message.unpacked_len = bytes.len();
        message.body = match sfs::decode_object(&bytes) {
            Ok(value) => Body::Object(value),
            Err(err) => Body::Undecodable(err.to_string()),
        };
        message
    }

    pub fn object(&self) -> Option<&Value> {
        match &self.body {
            Body::Object(v) => Some(v),
            _ => None,
        }
    }

    /// SmartFox ping reply (`c: 0`, `a: 29`), which the game server sends every 4 s.
    pub fn is_heartbeat(&self) -> bool {
        let Some(object) = self.object() else {
            return false;
        };
        object.get("c").and_then(Value::as_i64) == Some(0)
            && object.get("a").and_then(Value::as_i64) == Some(29)
    }

    /// The game server's clock, from a ping reply's `p.serverTime`. It is a Unix time; its
    /// unit (milliseconds or seconds) is told by its size.
    pub fn server_time(&self) -> Option<Duration> {
        if !self.is_heartbeat() {
            return None;
        }
        let t = self.object()?.get("p")?.get("serverTime")?.as_i64()?;
        match t {
            1_000_000_000_000.. => Some(Duration::from_millis(t as u64)),
            1_000_000_000.. => Some(Duration::from_secs(t as u64)),
            _ => None,
        }
    }

    /// The command name at `p.c` in the message envelope.
    pub fn command(&self) -> Option<&str> {
        self.object()?.get("p")?.get("c")?.as_str()
    }

    /// The command's data at `p.p` in the message envelope.
    pub fn data(&self) -> Option<&Value> {
        self.object()?.get("p")?.get("p")
    }

    /// A decoded command message, as the game would send it, captured `secs` after the epoch.
    #[cfg(test)]
    pub(crate) fn command_for_test(command: &str, data: Value, secs: u64) -> Message {
        let object = |entries: Vec<(&str, Value)>| {
            Value::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
        };
        let envelope = object(vec![(
            "p",
            object(vec![("c", Value::Str(command.into())), ("p", data)]),
        )]);
        Message {
            time: Duration::from_secs(secs),
            flags: 0x80,
            wire_len: 0,
            unpacked_len: 0,
            body: Body::Object(envelope),
        }
    }

    /// A ping reply carrying `server_time`, captured `secs` after the epoch.
    #[cfg(test)]
    pub(crate) fn ping_for_test(server_time: i64, secs: u64) -> Message {
        let object = |entries: Vec<(&str, Value)>| {
            Value::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
        };
        let envelope = object(vec![
            ("c", Value::Byte(0)),
            ("a", Value::Short(29)),
            (
                "p",
                object(vec![
                    ("serverTime", Value::Long(server_time)),
                    ("clientTime", Value::Long(0)),
                ]),
            ),
        ]);
        Message {
            time: Duration::from_secs(secs),
            flags: 0x80,
            wire_len: 0,
            unpacked_len: 0,
            body: Body::Object(envelope),
        }
    }
}

/// Decompresses a zstd body. The size the frame claims is checked against [`MAX_UNPACKED`]
/// before any memory is reserved, and output past the limit is rejected, so a corrupt or
/// misread frame cannot exhaust memory.
fn decompress(frame: &Frame) -> Result<Vec<u8>, String> {
    let expected = frame.uncompressed_len.map(u64::from);
    if let Some(n) = expected
        && n > MAX_UNPACKED
    {
        return Err(format!(
            "frame claims {n} unpacked bytes, over the {MAX_UNPACKED}-byte limit"
        ));
    }
    let decoder =
        StreamingDecoder::new(&frame.body[..]).map_err(|e| format!("zstd header: {e}"))?;
    let mut out = Vec::with_capacity(expected.unwrap_or(0) as usize);
    // One byte past the limit shows whether the data went over it.
    decoder
        .take(MAX_UNPACKED + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("zstd data: {e}"))?;
    if out.len() as u64 > MAX_UNPACKED {
        return Err(format!(
            "unpacked data exceeds the {MAX_UNPACKED}-byte limit"
        ));
    }
    if let Some(n) = expected
        && out.len() as u64 != n
    {
        return Err(format!("unpacked {} bytes, header says {n}", out.len()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_time_from_ping_replies() {
        let ping = |t| Message::ping_for_test(t, 0).server_time();
        assert_eq!(
            ping(1_791_021_600_123),
            Some(Duration::from_millis(1_791_021_600_123))
        );
        assert_eq!(
            ping(1_791_021_600),
            Some(Duration::from_secs(1_791_021_600))
        );
        assert_eq!(ping(12_345), None, "too small to be a Unix time");
        let panel = Message::command_for_test("al.rank", Value::Object(vec![]), 0);
        assert_eq!(panel.server_time(), None);
    }

    #[test]
    fn oversized_claim_is_rejected_before_allocating() {
        // A tiny body claiming almost 4 GiB: must fail cleanly rather than reserve it.
        let frame = Frame {
            flags: 0xb0,
            uncompressed_len: Some(u32::MAX),
            body: vec![0x28, 0xb5, 0x2f, 0xfd, 0, 0],
            wire_len: 13,
        };
        let message = Message::decode(frame, Duration::ZERO);
        let Body::Undecodable(reason) = message.body else {
            panic!("expected an error")
        };
        assert!(reason.contains("limit"), "{reason}");
    }

    #[test]
    fn garbage_compressed_body_is_an_error() {
        let frame = Frame {
            flags: 0xb0,
            uncompressed_len: Some(10),
            body: vec![1, 2, 3, 4],
            wire_len: 11,
        };
        assert!(matches!(
            Message::decode(frame, Duration::ZERO).body,
            Body::Undecodable(_)
        ));
    }
}
