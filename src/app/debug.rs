//! The debug log: the messages the game server sends, each with its method and its data as
//! indented JSON, for working out what a panel carries. Messages are recorded only while the
//! log window is open, and kept in memory only, within [`MAX_ENTRIES`] and [`MAX_BYTES`].

use std::collections::VecDeque;
use std::fmt::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::protocol::message::{Body, Message};
use crate::protocol::sfs::Value;
use crate::util::json::escape;
use crate::util::time::utc_iso;

/// At most this many entries are kept...
const MAX_ENTRIES: usize = 10_000;
/// ...holding at most this much text together. The oldest go first.
const MAX_BYTES: usize = 64 * 1024 * 1024;
/// One entry's text is cut at this length.
const MAX_ENTRY: usize = 4 * 1024 * 1024;

/// The game's login messages, which are never logged.
const LOGIN: [&str; 3] = ["login.ext", "login.other", "init"];

/// Whether messages are being recorded. Read by the capture thread before it takes the state
/// lock, so messages are only formatted while recording, and outside the lock.
static RECORDING: AtomicBool = AtomicBool::new(false);

pub fn recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

pub fn set_recording(on: bool) {
    RECORDING.store(on, Ordering::Relaxed);
}

/// One message in the log.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Counts up from 1 for the life of the log, clearing included.
    pub id: u64,
    /// Capture time, on the game server's clock once a ping reply has shown it.
    pub time: Duration,
    /// The method (`p.c`), or what kind of message this is if it has none.
    pub method: String,
    /// Bytes on the wire.
    pub size: usize,
    /// The data (`p.p`) as indented JSON; the whole message for one without a method.
    pub text: String,
}

impl Entry {
    /// The entry for a message. `None` for what is never logged: the game's login messages
    /// ([`LOGIN`]) and SmartFox's own (`c: 0`: the handshake and login replies, which carry
    /// the session, and the ping replies every 4 s), and encrypted messages, which can't be
    /// read.
    pub fn from_message(message: &Message) -> Option<Entry> {
        let (method, text) = match &message.body {
            Body::Encrypted => return None,
            Body::Undecodable(why) => ("(undecodable)".to_string(), why.clone()),
            Body::Object(object) if object.get("c").and_then(Value::as_i64) == Some(0) => {
                return None;
            }
            Body::Object(_) if message.command().is_some_and(|c| LOGIN.contains(&c)) => {
                return None;
            }
            Body::Object(object) => match (message.command(), message.data()) {
                (Some(method), Some(data)) => (method.to_string(), pretty(data)),
                (Some(method), None) => (method.to_string(), "(no data)".to_string()),
                (None, _) => ("(no method)".to_string(), pretty(object)),
            },
        };
        Some(Entry {
            id: 0,
            time: message.time,
            method,
            size: message.wire_len,
            text: cut(text),
        })
    }

    /// Whether the method or the data contains `filter`, ignoring ASCII case. An empty filter
    /// matches everything.
    fn matches(&self, filter: &str) -> bool {
        contains(&self.method, filter) || contains(&self.text, filter)
    }

    /// The list line: time, size and method, in columns for a fixed-width font.
    fn label(&self) -> String {
        let secs = self.time.as_secs() % 86_400;
        format!(
            "{:02}:{:02}:{:02}.{:03} {:>9}  {}",
            secs / 3600,
            secs / 60 % 60,
            secs % 60,
            self.time.subsec_millis(),
            size(self.size),
            self.method
        )
    }
}

/// A line of the log window's list.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub id: u64,
    pub label: String,
}

#[derive(Debug, Default)]
pub struct DebugLog {
    entries: VecDeque<Entry>,
    bytes: usize,
    last_id: u64,
}

impl DebugLog {
    /// Adds an entry, numbering it, and drops the oldest past the limits.
    pub fn push(&mut self, mut entry: Entry) {
        self.last_id += 1;
        entry.id = self.last_id;
        self.bytes += entry.text.len();
        self.entries.push_back(entry);
        while self.entries.len() > MAX_ENTRIES || self.bytes > MAX_BYTES {
            let Some(old) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= old.text.len();
        }
    }

    /// The lines for the entries after `after` that match `filter`, and the newest id so far,
    /// to ask from next time.
    pub fn lines(&self, after: u64, filter: &str) -> (Vec<Line>, u64) {
        let lines = self
            .entries
            .iter()
            .filter(|e| e.id > after && e.matches(filter))
            .map(|e| Line {
                id: e.id,
                label: e.label(),
            })
            .collect();
        (lines, self.last_id.max(after))
    }

    /// The oldest entry still kept; the ones before it were dropped or cleared.
    pub fn oldest(&self) -> Option<u64> {
        self.entries.front().map(|e| e.id)
    }

    /// An entry in full: method, time and size, then its data.
    pub fn detail(&self, id: u64) -> Option<String> {
        let entry = self.entries.iter().find(|e| e.id == id)?;
        Some(format!(
            "method: {}\ntime: {} (game server)\nsize: {} bytes on the wire\n\n{}",
            entry.method,
            utc_iso(entry.time),
            entry.size,
            entry.text
        ))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

/// `value` as indented JSON. Arrays of plain values from the typed array types go on one line;
/// a byte array is a string of hex digits.
pub fn pretty(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, 0, &mut out);
    out
}

fn write_value(value: &Value, indent: usize, out: &mut String) {
    fn list<T>(items: &[T], out: &mut String, item: impl Fn(&T) -> String) {
        let items: Vec<String> = items.iter().map(item).collect();
        out.push('[');
        out.push_str(&items.join(", "));
        out.push(']');
    }
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
        Value::Byte(v) => out.push_str(&v.to_string()),
        Value::Short(v) => out.push_str(&v.to_string()),
        Value::Int(v) => out.push_str(&v.to_string()),
        Value::Long(v) => out.push_str(&v.to_string()),
        Value::Float(v) => out.push_str(&number(f64::from(*v))),
        Value::Double(v) => out.push_str(&number(*v)),
        Value::Str(s) | Value::Text(s) => out.push_str(&escape(s)),
        Value::BoolArray(items) => list(items, out, bool::to_string),
        Value::ByteArray(bytes) => {
            let mut hex = String::with_capacity(bytes.len() * 2);
            for b in bytes {
                let _ = write!(hex, "{b:02x}");
            }
            out.push_str(&escape(&hex));
        }
        Value::ShortArray(items) => list(items, out, i16::to_string),
        Value::IntArray(items) => list(items, out, i32::to_string),
        Value::LongArray(items) => list(items, out, i64::to_string),
        Value::FloatArray(items) => list(items, out, |v| number(f64::from(*v))),
        Value::DoubleArray(items) => list(items, out, |v| number(*v)),
        Value::StrArray(items) => list(items, out, |s| escape(s)),
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                push_indent(indent + 1, out);
                write_value(item, indent + 1, out);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            push_indent(indent, out);
            out.push(']');
        }
        Value::Object(entries) if entries.is_empty() => out.push_str("{}"),
        Value::Object(entries) => {
            out.push_str("{\n");
            for (i, (key, item)) in entries.iter().enumerate() {
                push_indent(indent + 1, out);
                out.push_str(&escape(key));
                out.push_str(": ");
                write_value(item, indent + 1, out);
                out.push_str(if i + 1 < entries.len() { ",\n" } else { "\n" });
            }
            push_indent(indent, out);
            out.push('}');
        }
    }
}

fn push_indent(indent: usize, out: &mut String) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

/// A number as JSON: `null` for the values JSON can't hold.
fn number(v: f64) -> String {
    if v.is_finite() {
        v.to_string()
    } else {
        "null".into()
    }
}

/// `text`, cut at [`MAX_ENTRY`] bytes with a note saying so.
fn cut(mut text: String) -> String {
    if text.len() <= MAX_ENTRY {
        return text;
    }
    let total = text.len();
    let mut end = MAX_ENTRY;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    let _ = write!(
        text,
        "\n… cut at {} MiB of {total} bytes",
        MAX_ENTRY / 1024 / 1024
    );
    text
}

/// Whether `text` contains `part`, ignoring ASCII case.
fn contains(text: &str, part: &str) -> bool {
    let (text, part) = (text.as_bytes(), part.as_bytes());
    part.is_empty()
        || text
            .windows(part.len())
            .any(|w| w.eq_ignore_ascii_case(part))
}

/// A byte count for the list: `512 B`, `1.5 KB`, `2.0 MB`.
fn size(bytes: usize) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(entries: Vec<(&str, Value)>) -> Value {
        Value::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    fn entry(method: &str, text: &str) -> Entry {
        Entry {
            id: 0,
            time: Duration::from_millis(1_791_021_600_250),
            method: method.into(),
            size: 2048,
            text: text.into(),
        }
    }

    #[test]
    fn data_is_shown_as_indented_json() {
        let data = object(vec![
            ("name", Value::Str("a\"b".into())),
            ("level", Value::Int(3)),
            ("ratio", Value::Float(0.5)),
            ("slots", Value::IntArray(vec![1, 2])),
            ("raw", Value::ByteArray(vec![0x0a, 0xff])),
            ("empty", Value::Array(vec![])),
            (
                "list",
                Value::Array(vec![object(vec![("x", Value::Long(7))]), Value::Null]),
            ),
        ]);
        assert_eq!(
            pretty(&data),
            r#"{
  "name": "a\"b",
  "level": 3,
  "ratio": 0.5,
  "slots": [1, 2],
  "raw": "0aff",
  "empty": [],
  "list": [
    {
      "x": 7
    },
    null
  ]
}"#
        );
        assert_eq!(pretty(&Value::Double(f64::NAN)), "null");
    }

    #[test]
    fn messages_become_entries_except_pings() {
        let data = object(vec![("x", Value::Int(1))]);
        let message = Message::command_for_test("map.example", data, 1_791_021_600);
        let e = Entry::from_message(&message).unwrap();
        assert_eq!(e.method, "map.example");
        assert_eq!(e.text, "{\n  \"x\": 1\n}");
        assert_eq!(Entry::from_message(&Message::ping_for_test(0, 1)), None);
        for login in LOGIN {
            let message = Message::command_for_test(login, object(vec![]), 1_791_021_600);
            assert_eq!(Entry::from_message(&message), None, "{login}");
        }
    }

    #[test]
    fn the_list_shows_time_size_and_method() {
        assert_eq!(
            entry("map.example", "").label(),
            "10:00:00.250    2.0 KB  map.example"
        );
        assert_eq!(size(512), "512 B");
        assert_eq!(size(3 * 1_048_576), "3.0 MB");
    }

    #[test]
    fn the_filter_matches_method_or_data_in_any_case() {
        let mut log = DebugLog::default();
        log.push(entry("map.example", "{\"Kind\": 1}"));
        log.push(entry("al.rank", "{}"));
        let ids = |filter: &str| -> Vec<u64> {
            log.lines(0, filter).0.into_iter().map(|l| l.id).collect()
        };
        assert_eq!(ids(""), [1, 2]);
        assert_eq!(ids("MAP."), [1]);
        assert_eq!(ids("kind"), [1]);
        assert_eq!(ids("rank"), [2]);
        assert_eq!(ids("nothing"), Vec::<u64>::new());
        // Asking again from the newest id gives only what came since.
        let (_, newest) = log.lines(0, "");
        log.push(entry("al.rank", "{}"));
        let (lines, newest) = log.lines(newest, "map");
        assert!(lines.is_empty());
        assert_eq!(newest, 3);
    }

    #[test]
    fn the_oldest_entries_go_past_the_limits() {
        let mut log = DebugLog::default();
        for _ in 0..MAX_ENTRIES + 5 {
            log.push(entry("x", "{}"));
        }
        assert_eq!(log.len(), MAX_ENTRIES);
        assert_eq!(log.oldest(), Some(6));
        log.clear();
        assert!(log.is_empty());
        log.push(entry("x", "{}"));
        assert_eq!(
            log.oldest(),
            Some(MAX_ENTRIES as u64 + 6),
            "ids keep counting"
        );

        let mut log = DebugLog::default();
        let big = "x".repeat(MAX_ENTRY);
        for _ in 0..MAX_BYTES / MAX_ENTRY + 2 {
            log.push(entry("x", &big));
        }
        assert_eq!(log.len(), MAX_BYTES / MAX_ENTRY);
    }

    #[test]
    fn long_text_is_cut_on_a_character_boundary() {
        let text = "é".repeat(MAX_ENTRY);
        let cut = cut(text);
        assert!(cut.len() < MAX_ENTRY + 64);
        assert!(cut.ends_with(&format!("of {} bytes", MAX_ENTRY * 2)));
    }

    #[test]
    fn detail_gives_the_whole_entry() {
        let mut log = DebugLog::default();
        log.push(entry("map.example", "{}"));
        assert_eq!(
            log.detail(1).unwrap(),
            "method: map.example\ntime: 2026-10-03T10:00:00Z (game server)\nsize: 2048 bytes on the wire\n\n{}"
        );
        assert_eq!(log.detail(2), None);
    }
}
