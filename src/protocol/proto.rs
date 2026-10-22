//! Protocol Buffers wire format, read only. The world map sends its objects as protobuf
//! messages as byte arrays inside the SFSObject. Damaged input gives an error, never
//! a panic, and a message's fields are never more than its bytes.

/// One field's value, as the wire format gives it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Field<'a> {
    Varint(u64),
    Fixed64(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
}

impl<'a> Field<'a> {
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Field::Varint(v) | Field::Fixed64(v) => Some(v),
            Field::Fixed32(v) => Some(v.into()),
            Field::Bytes(_) => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&'a [u8]> {
        match *self {
            Field::Bytes(b) => Some(b),
            _ => None,
        }
    }
}

/// A message's fields in order, as (field number, value). Repeated fields come once per value.
pub fn fields(data: &[u8]) -> Result<Vec<(u64, Field<'_>)>, String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let key = varint(data, &mut pos)?;
        let number = key >> 3;
        let value = match key & 7 {
            0 => Field::Varint(varint(data, &mut pos)?),
            1 => Field::Fixed64(u64::from_le_bytes(
                take(data, &mut pos, 8)?.try_into().unwrap(),
            )),
            2 => {
                let len = varint(data, &mut pos)?;
                let len = usize::try_from(len).map_err(|_| "length too large")?;
                Field::Bytes(take(data, &mut pos, len)?)
            }
            5 => Field::Fixed32(u32::from_le_bytes(
                take(data, &mut pos, 4)?.try_into().unwrap(),
            )),
            wire => return Err(format!("unknown wire type {wire} at byte {pos}")),
        };
        out.push((number, value));
    }
    Ok(out)
}

/// The first value of field `number`.
pub fn first<'a>(fields: &[(u64, Field<'a>)], number: u64) -> Option<Field<'a>> {
    fields.iter().find(|(n, _)| *n == number).map(|(_, f)| *f)
}

/// How many values field `number` has.
pub fn count(fields: &[(u64, Field<'_>)], number: u64) -> usize {
    fields.iter().filter(|(n, _)| *n == number).count()
}

/// Bytes from a hex string, or `None` if it isn't one.
pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    digits
        .chunks(2)
        .map(|pair| {
            let digit = |c: u8| (c as char).to_digit(16);
            Some((digit(pair[0])? * 16 + digit(pair[1])?) as u8)
        })
        .collect()
}

fn varint(data: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *data
            .get(*pos)
            .ok_or_else(|| format!("varint runs past the end at byte {pos}"))?;
        *pos += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(format!("varint longer than 10 bytes at byte {pos}"))
}

fn take<'a>(data: &'a [u8], pos: &mut usize, len: usize) -> Result<&'a [u8], String> {
    let end = pos
        .checked_add(len)
        .filter(|end| *end <= data.len())
        .ok_or_else(|| format!("{len} bytes run past the end at byte {pos}"))?;
    let bytes = &data[*pos..end];
    *pos = end;
    Ok(bytes)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Test messages, written field by field.
    pub(crate) fn varint_bytes(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    pub(crate) fn int_field(number: u64, v: u64) -> Vec<u8> {
        let mut out = varint_bytes(number << 3);
        out.extend(varint_bytes(v));
        out
    }

    pub(crate) fn bytes_field(number: u64, bytes: &[u8]) -> Vec<u8> {
        let mut out = varint_bytes((number << 3) | 2);
        out.extend(varint_bytes(bytes.len() as u64));
        out.extend_from_slice(bytes);
        out
    }

    pub(crate) fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn fields_are_read_in_order() {
        let mut data = int_field(1, 800_701);
        data.extend(int_field(2, 17));
        data.extend(bytes_field(10, b"abc"));
        data.extend([0x6d, 0, 0, 0x80, 0x3f]); // field 13, fixed32
        data.extend(int_field(100, u64::MAX));
        let f = fields(&data).unwrap();
        assert_eq!(first(&f, 1), Some(Field::Varint(800_701)));
        assert_eq!(first(&f, 10).and_then(|v| v.as_bytes()), Some(&b"abc"[..]));
        assert_eq!(first(&f, 13), Some(Field::Fixed32(0x3f80_0000)));
        assert_eq!(first(&f, 100).and_then(|v| v.as_u64()), Some(u64::MAX));
        assert_eq!(count(&f, 2), 1);
        assert_eq!(first(&f, 3), None);
    }

    #[test]
    fn damaged_input_is_an_error() {
        // Cut off in a varint, in a length, past a length, an unknown wire type, a varint
        // that never ends.
        for data in [
            &[0x08, 0x80][..],
            &[0x52][..],
            &[0x52, 0x05, 1, 2][..],
            &[0x0b][..],
            &[
                0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
            ][..],
        ] {
            assert!(fields(data).is_err(), "{data:02x?}");
        }
        assert_eq!(fields(&[]).unwrap(), []);
    }

    #[test]
    fn hex_strings_become_bytes() {
        assert_eq!(from_hex("08Ff"), Some(vec![0x08, 0xff]));
        assert_eq!(from_hex("0"), None);
        assert_eq!(from_hex("zz"), None);
        assert_eq!(hex(&varint_bytes(800_701)), "bdef30");
    }
}
