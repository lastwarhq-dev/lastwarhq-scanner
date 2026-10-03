//! SmartFox 2X SFSObject decoding.
//!
//! Each value is a type byte followed by its data, big-endian. Objects are a u16 entry count,
//! then entries of u16 key length · UTF-8 key · value.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Str(String),
    BoolArray(Vec<bool>),
    ByteArray(Vec<u8>),
    ShortArray(Vec<i16>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
    FloatArray(Vec<f32>),
    DoubleArray(Vec<f64>),
    StrArray(Vec<String>),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
    Text(String),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) | Value::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Any integer type, widened.
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::Byte(v) => Some(v.into()),
            Value::Short(v) => Some(v.into()),
            Value::Int(v) => Some(v.into()),
            Value::Long(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodeError {
    pub offset: usize,
    pub reason: String,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.reason, self.offset)
    }
}

impl std::error::Error for DecodeError {}

/// Memory a decoded message may take, as a multiple of its encoded size. A value takes 32
/// bytes once decoded (56 as an object entry, with its key) but can be as little as one byte
/// encoded. Ordinary fields (ids, names, ranks, scores) take about 5–8 times their encoded size.
const MAX_EXPANSION: usize = 16;
/// The decoding budget of a small message.
const MIN_BUDGET: usize = 64 * 1024;
/// The decoding budget of any message.
const MAX_BUDGET: usize = 64 * 1024 * 1024;

/// Decodes a whole buffer as one SFSObject. Trailing bytes are an error, and so is a message
/// that would take more memory decoded than its budget (see [`MAX_EXPANSION`]).
pub fn decode_object(bytes: &[u8]) -> Result<Value, DecodeError> {
    let budget = bytes
        .len()
        .saturating_mul(MAX_EXPANSION)
        .clamp(MIN_BUDGET, MAX_BUDGET);
    let mut r = Reader {
        bytes,
        pos: 0,
        budget,
    };
    r.charge(size_of::<Value>())?;
    let value = r.value(0)?;
    if !matches!(value, Value::Object(_)) {
        return Err(r.error(0, "top level is not an SFSObject"));
    }
    if r.pos != bytes.len() {
        return Err(r.error(r.pos, format!("{} trailing bytes", bytes.len() - r.pos)));
    }
    Ok(value)
}

const MAX_DEPTH: usize = 64;

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    /// Bytes of memory the rest of the decode may still allocate.
    budget: usize,
}

impl<'a> Reader<'a> {
    /// Takes `bytes` from the budget before they are allocated.
    fn charge(&mut self, bytes: usize) -> Result<(), DecodeError> {
        match self.budget.checked_sub(bytes) {
            Some(left) => {
                self.budget = left;
                Ok(())
            }
            None => Err(self.error(self.pos, "message takes too much memory to decode")),
        }
    }

    fn error(&self, offset: usize, reason: impl Into<String>) -> DecodeError {
        DecodeError {
            offset,
            reason: reason.into(),
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.bytes.len());
        let Some(end) = end else {
            return Err(self.error(self.pos, format!("need {n} bytes, input ends")));
        };
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        Ok(self.take(N)?.try_into().unwrap())
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.array()?))
    }
    fn i16(&mut self) -> Result<i16, DecodeError> {
        Ok(i16::from_be_bytes(self.array()?))
    }
    fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_be_bytes(self.array()?))
    }
    fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_be_bytes(self.array()?))
    }
    fn f32(&mut self) -> Result<f32, DecodeError> {
        Ok(f32::from_be_bytes(self.array()?))
    }
    fn f64(&mut self) -> Result<f64, DecodeError> {
        Ok(f64::from_be_bytes(self.array()?))
    }

    fn string_of_len(&mut self, len: usize) -> Result<String, DecodeError> {
        let bytes = self.take(len)?;
        self.charge(len)?;
        let text = String::from_utf8_lossy(bytes).into_owned();
        // Invalid UTF-8 is replaced by a 3-byte character per bad byte.
        self.charge(text.len().saturating_sub(len))?;
        Ok(text)
    }

    fn short_string(&mut self) -> Result<String, DecodeError> {
        let len = self.u16()? as usize;
        self.string_of_len(len)
    }

    fn int_len(&mut self) -> Result<usize, DecodeError> {
        let at = self.pos;
        let len = self.i32()?;
        usize::try_from(len).map_err(|_| self.error(at, format!("negative length {len}")))
    }

    /// A u16 count, then that many items. The list is charged in full and allocated once.
    fn repeat<T>(
        &mut self,
        mut item: impl FnMut(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<Vec<T>, DecodeError> {
        let count = self.u16()? as usize;
        self.charge(count * size_of::<T>())?;
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            items.push(item(self)?);
        }
        Ok(items)
    }

    fn value(&mut self, depth: usize) -> Result<Value, DecodeError> {
        if depth > MAX_DEPTH {
            return Err(self.error(self.pos, "nesting too deep"));
        }
        let at = self.pos;
        let value = match self.u8()? {
            0 => Value::Null,
            1 => Value::Bool(self.u8()? != 0),
            2 => Value::Byte(self.u8()? as i8),
            3 => Value::Short(self.i16()?),
            4 => Value::Int(self.i32()?),
            5 => Value::Long(self.i64()?),
            6 => Value::Float(self.f32()?),
            7 => Value::Double(self.f64()?),
            8 => Value::Str(self.short_string()?),
            9 => Value::BoolArray(self.repeat(|r| Ok(r.u8()? != 0))?),
            10 => {
                let len = self.int_len()?;
                let bytes = self.take(len)?;
                self.charge(len)?;
                Value::ByteArray(bytes.to_vec())
            }
            11 => Value::ShortArray(self.repeat(Self::i16)?),
            12 => Value::IntArray(self.repeat(Self::i32)?),
            13 => Value::LongArray(self.repeat(Self::i64)?),
            14 => Value::FloatArray(self.repeat(Self::f32)?),
            15 => Value::DoubleArray(self.repeat(Self::f64)?),
            16 => Value::StrArray(self.repeat(Self::short_string)?),
            17 => Value::Array(self.repeat(|r| r.value(depth + 1))?),
            18 => Value::Object(self.repeat(|r| {
                let key = r.short_string()?;
                Ok((key, r.value(depth + 1)?))
            })?),
            20 => {
                let len = self.int_len()?;
                Value::Text(self.string_of_len(len)?)
            }
            other => return Err(self.error(at, format!("unknown type byte {other:#04x}"))),
        };
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(out: &mut Vec<u8>, k: &str) {
        out.extend_from_slice(&(k.len() as u16).to_be_bytes());
        out.extend_from_slice(k.as_bytes());
    }

    #[test]
    fn decodes_nested_envelope() {
        // { c: 1 (byte), a: 13 (short), p: { c: "cmd", p: { n: 7 (int), l: [5] (longs) } } }
        let mut b = vec![18, 0, 3];
        key(&mut b, "c");
        b.extend_from_slice(&[2, 1]);
        key(&mut b, "a");
        b.extend_from_slice(&[3, 0, 13]);
        key(&mut b, "p");
        b.extend_from_slice(&[18, 0, 2]);
        key(&mut b, "c");
        b.push(8);
        key(&mut b, "cmd");
        key(&mut b, "p");
        b.extend_from_slice(&[18, 0, 2]);
        key(&mut b, "n");
        b.extend_from_slice(&[4, 0, 0, 0, 7]);
        key(&mut b, "l");
        b.extend_from_slice(&[13, 0, 1, 0, 0, 0, 0, 0, 0, 0, 5]);

        let v = decode_object(&b).unwrap();
        assert_eq!(v.get("a"), Some(&Value::Short(13)));
        let inner = v.get("p").unwrap();
        assert_eq!(inner.get("c").and_then(Value::as_str), Some("cmd"));
        let data = inner.get("p").unwrap();
        assert_eq!(data.get("n"), Some(&Value::Int(7)));
        assert_eq!(data.get("l"), Some(&Value::LongArray(vec![5])));
    }

    #[test]
    fn truncated_input_is_an_error() {
        let err = decode_object(&[18, 0, 1, 0, 1, b'k', 4, 0, 0]).unwrap_err();
        assert!(err.reason.contains("input ends"), "{err}");
    }

    #[test]
    fn unknown_type_is_an_error() {
        let err = decode_object(&[18, 0, 1, 0, 1, b'k', 0x7f]).unwrap_err();
        assert_eq!(err.offset, 6);
    }

    #[test]
    fn trailing_bytes_are_an_error() {
        assert!(decode_object(&[18, 0, 0, 0xff]).is_err());
    }

    /// `{list: [n × {uid, name, rank, power}]}`, like a member list.
    fn member_list(n: u16) -> Vec<u8> {
        let mut b = vec![18, 0, 1];
        key(&mut b, "list");
        b.push(17);
        b.extend_from_slice(&n.to_be_bytes());
        for i in 0..n {
            b.extend_from_slice(&[18, 0, 4]);
            key(&mut b, "uid");
            b.push(8);
            key(&mut b, &format!("{}", 100_000_000 + u32::from(i)));
            key(&mut b, "name");
            b.push(8);
            key(&mut b, "Player");
            key(&mut b, "rank");
            b.extend_from_slice(&[4, 0, 0, 0, 4]);
            key(&mut b, "power");
            b.extend_from_slice(&[5, 0, 0, 0, 0, 0, 0, 0, 1]);
        }
        b
    }

    #[test]
    fn ordinary_messages_fit_the_budget() {
        let b = member_list(5000);
        assert!(b.len() > MIN_BUDGET, "large enough to use the 16× budget");
        let v = decode_object(&b).unwrap();
        assert_eq!(
            v.get("list").and_then(Value::as_array).map(<[_]>::len),
            Some(5000)
        );
    }

    #[test]
    fn values_that_expand_too_much_are_an_error() {
        // {a: [20,000 nulls]}: 20 KB that would take 640 KB decoded.
        let mut b = vec![18, 0, 1];
        key(&mut b, "a");
        b.push(17);
        b.extend_from_slice(&20_000u16.to_be_bytes());
        b.resize(b.len() + 20_000, 0);
        let err = decode_object(&b).unwrap_err();
        assert!(err.reason.contains("too much memory"), "{err}");
    }
}
