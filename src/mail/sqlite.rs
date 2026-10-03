//! A minimal read-only SQLite reader: walks table b-trees in a database file held in memory.
//! Enough for the game's `config.db`; it never writes. Every value read from the file is
//! bounds-checked, so a damaged or half-written copy gives an error, never a panic.

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl SqlValue {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            SqlValue::Text(s) => Some(s),
            _ => None,
        }
    }
}

pub struct Database<'a> {
    bytes: &'a [u8],
    page_size: usize,
    usable: usize,
}

#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    pub root_page: u32,
    pub sql: String,
}

/// Big-endian u16 at `at`, or an error if it runs past the end.
fn be16(bytes: &[u8], at: usize) -> Result<u16, String> {
    let b = bytes.get(at..at + 2).ok_or("read past end of page")?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

/// Big-endian u32 at `at`, or an error if it runs past the end.
fn be32(bytes: &[u8], at: usize) -> Result<u32, String> {
    let b = bytes.get(at..at + 4).ok_or("read past end of page")?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

impl<'a> Database<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.len() < 100 || &bytes[..16] != b"SQLite format 3\0" {
            return Err("not a SQLite 3 database".into());
        }
        let page_size = match be16(bytes, 16)? {
            1 => 65_536,
            n => usize::from(n),
        };
        if !(512..=65_536).contains(&page_size) || !page_size.is_power_of_two() {
            return Err(format!("invalid page size {page_size}"));
        }
        // SQLite requires at least 480 usable bytes per page.
        let usable = page_size
            .checked_sub(usize::from(bytes[20]))
            .filter(|&u| u >= 480);
        let usable = usable.ok_or("invalid reserved space per page")?;
        Ok(Self {
            bytes,
            page_size,
            usable,
        })
    }

    /// Tables listed in `sqlite_master`.
    pub fn tables(&self) -> Result<Vec<Table>, String> {
        let mut tables = Vec::new();
        self.for_each_row(1, |row| {
            if let [
                SqlValue::Text(kind),
                SqlValue::Text(name),
                _,
                SqlValue::Int(root),
                sql,
            ] = &row[..]
                && kind == "table"
            {
                let sql = sql.as_text().unwrap_or_default().to_string();
                tables.push(Table {
                    name: name.clone(),
                    root_page: *root as u32,
                    sql,
                });
            }
            Ok(())
        })?;
        Ok(tables)
    }

    /// Hands each row of the table b-tree rooted at `root_page` to `f`, in rowid order. Rows
    /// are decoded one at a time, so only the rows `f` keeps stay in memory.
    ///
    /// A damaged file can't make this run long or use much memory:
    /// - The tree is walked with an explicit list of pages still to visit rather than by
    ///   recursion, so a long chain of pages can't exhaust the stack.
    /// - Each page, b-tree or overflow, may be used once, and each cell once per page, so
    ///   pointers that loop or repeat give an error.
    /// - The payloads decoded can't add up to more than the file's size, as in any valid file.
    pub fn for_each_row(
        &self,
        root_page: u32,
        mut f: impl FnMut(Vec<SqlValue>) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut walk = Walk {
            seen: HashSet::new(),
            budget: self.bytes.len(),
        };
        let mut to_visit = vec![root_page];
        while let Some(number) = to_visit.pop() {
            walk.use_page(number)?;
            self.visit(number, &mut walk, &mut to_visit, &mut f)?;
        }
        Ok(())
    }

    /// Every row of a table, kept in memory.
    #[cfg(test)]
    pub fn rows(&self, root_page: u32) -> Result<Vec<Vec<SqlValue>>, String> {
        let mut rows = Vec::new();
        self.for_each_row(root_page, |row| {
            rows.push(row);
            Ok(())
        })?;
        Ok(rows)
    }

    fn page(&self, number: u32) -> Result<&'a [u8], String> {
        let start = (number as usize)
            .checked_sub(1)
            .ok_or("page 0")?
            .checked_mul(self.page_size)
            .ok_or("page number too large")?;
        self.bytes
            .get(start..start + self.page_size)
            .ok_or_else(|| format!("page {number} past end of file"))
    }

    /// Reads one b-tree page: a leaf hands over its rows; an interior page queues its children
    /// so that the leftmost is visited next, keeping rowid order.
    fn visit(
        &self,
        number: u32,
        walk: &mut Walk,
        to_visit: &mut Vec<u32>,
        f: &mut impl FnMut(Vec<SqlValue>) -> Result<(), String>,
    ) -> Result<(), String> {
        let page = self.page(number)?;
        let header = if number == 1 { 100 } else { 0 };
        let kind = *page.get(header).ok_or("page header past end")?;
        let cells = usize::from(be16(page, header + 3)?);
        let pointers = header + if kind == 0x05 { 12 } else { 8 };
        let mut children = Vec::new();
        let mut offsets = HashSet::new();
        for i in 0..cells {
            let at = usize::from(be16(page, pointers + 2 * i)?);
            if !offsets.insert(at) {
                return Err(format!("page {number}: cell at {at} is listed twice"));
            }
            match kind {
                0x05 => children.push(be32(page, at)?),
                0x0d => {
                    let mut pos = at;
                    let size = varint(page, &mut pos)?;
                    let _rowid = varint(page, &mut pos)?;
                    let size = usize::try_from(size).map_err(|_| "cell too large")?;
                    walk.spend(size)?;
                    f(record(&self.payload(page, pos, size, walk)?)?)?;
                }
                other => {
                    return Err(format!(
                        "page {number}: not a table b-tree page (type {other:#04x})"
                    ));
                }
            }
        }
        if kind == 0x05 {
            // The right-most child comes last; the list is a stack, so push in reverse.
            children.push(be32(page, header + 8)?);
            to_visit.extend(children.into_iter().rev());
        }
        Ok(())
    }

    /// A cell's payload, following overflow pages when it does not fit on the leaf. Overflow
    /// pages count as used pages, so a chain can't loop or share pages with another.
    fn payload(
        &self,
        page: &[u8],
        at: usize,
        size: usize,
        walk: &mut Walk,
    ) -> Result<Vec<u8>, String> {
        let u = self.usable;
        let max_local = u - 35;
        let local = if size <= max_local {
            size
        } else {
            let min_local = (u - 12) * 32 / 255 - 23;
            let k = min_local + (size - min_local) % (u - 4);
            if k <= max_local { k } else { min_local }
        };
        let mut out = page
            .get(at..at + local)
            .ok_or("cell past end of page")?
            .to_vec();
        let mut next = if local < size {
            be32(page, at + local)?
        } else {
            0
        };
        while out.len() < size {
            if next == 0 {
                return Err("overflow chain ends early".into());
            }
            walk.use_page(next)?;
            let overflow = self.page(next)?;
            next = be32(overflow, 0)?;
            let take = (size - out.len()).min(u - 4);
            out.extend_from_slice(overflow.get(4..4 + take).ok_or("overflow page too short")?);
        }
        Ok(out)
    }
}

/// What one table walk has used: the pages, and how much payload it may still decode.
struct Walk {
    seen: HashSet<u32>,
    budget: usize,
}

impl Walk {
    fn use_page(&mut self, number: u32) -> Result<(), String> {
        if self.seen.insert(number) {
            Ok(())
        } else {
            Err(format!("page {number} is referenced twice"))
        }
    }

    /// In a valid file every payload byte is stored once, so all payloads together fit in it.
    fn spend(&mut self, size: usize) -> Result<(), String> {
        self.budget = self
            .budget
            .checked_sub(size)
            .ok_or("the table holds more data than the file")?;
        Ok(())
    }
}

/// SQLite's limit on columns in a table, and so on values in a record.
const MAX_VALUES: usize = 32_767;

/// SQLite varint: big-endian 7-bit groups, up to 9 bytes, the 9th using all 8 bits.
fn varint(bytes: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    for i in 0..9 {
        let byte = *bytes.get(*pos).ok_or("varint past end")?;
        *pos += 1;
        if i == 8 {
            return Ok((value << 8) | u64::from(byte));
        }
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            break;
        }
    }
    Ok(value)
}

fn record(payload: &[u8]) -> Result<Vec<SqlValue>, String> {
    let mut pos = 0;
    let header_len = varint(payload, &mut pos)? as usize;
    let mut types = Vec::new();
    while pos < header_len {
        if types.len() == MAX_VALUES {
            return Err(format!("record has more than {MAX_VALUES} values"));
        }
        types.push(varint(payload, &mut pos)?);
    }
    let mut data = header_len;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let end = data.checked_add(n).ok_or("record value too large")?;
        let slice = payload.get(data..end).ok_or("record value past end")?;
        data = end;
        Ok(slice)
    };
    let int = |b: &[u8]| -> i64 {
        let mut v = if b.first().is_some_and(|x| x & 0x80 != 0) {
            -1i64
        } else {
            0
        };
        for &x in b {
            v = (v << 8) | i64::from(x);
        }
        v
    };
    types
        .into_iter()
        .map(|t| {
            Ok(match t {
                0 => SqlValue::Null,
                1..=4 => SqlValue::Int(int(take(t as usize)?)),
                5 => SqlValue::Int(int(take(6)?)),
                6 => SqlValue::Int(int(take(8)?)),
                7 => SqlValue::Float(f64::from_be_bytes(take(8)?.try_into().unwrap())),
                8 => SqlValue::Int(0),
                9 => SqlValue::Int(1),
                n if n >= 12 && n % 2 == 0 => {
                    SqlValue::Blob(take(((n - 12) / 2) as usize)?.to_vec())
                }
                n if n >= 13 => SqlValue::Text(
                    String::from_utf8_lossy(take(((n - 13) / 2) as usize)?).into_owned(),
                ),
                n => return Err(format!("reserved serial type {n}")),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints() {
        let mut pos = 0;
        assert_eq!(varint(&[0x81, 0x00], &mut pos).unwrap(), 128);
        let mut pos = 0;
        assert_eq!(varint(&[0xff; 9], &mut pos).unwrap(), u64::MAX);
    }

    /// A 512-byte-page database whose first page is a b-tree page of `kind`, given cells and
    /// right-most pointer.
    fn db(kind: u8, cells: u16, right: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 512];
        bytes[..16].copy_from_slice(b"SQLite format 3\0");
        bytes[16..18].copy_from_slice(&512u16.to_be_bytes());
        bytes[100] = kind;
        bytes[103..105].copy_from_slice(&cells.to_be_bytes());
        bytes[108..112].copy_from_slice(&right.to_be_bytes());
        bytes
    }

    #[test]
    fn malformed_headers_are_errors() {
        let mut bad_size = db(0x0d, 0, 0);
        bad_size[16..18].copy_from_slice(&0u16.to_be_bytes());
        assert!(Database::open(&bad_size).is_err(), "page size 0");
        bad_size[16..18].copy_from_slice(&1000u16.to_be_bytes());
        assert!(Database::open(&bad_size).is_err(), "not a power of two");
        let mut bad_reserved = db(0x0d, 0, 0);
        bad_reserved[20] = 255;
        assert!(
            Database::open(&bad_reserved).is_err(),
            "too few usable bytes"
        );
        assert!(Database::open(b"SQLite format 3\0").is_err(), "truncated");
    }

    #[test]
    fn malformed_pages_are_errors_not_panics() {
        // Leaf page claiming many cells whose pointers run off the page.
        let many = db(0x0d, 60_000, 0);
        assert!(Database::open(&many).unwrap().rows(1).is_err());
        // Interior page pointing back to itself.
        let looped = db(0x05, 0, 1);
        assert!(
            Database::open(&looped)
                .unwrap()
                .rows(1)
                .unwrap_err()
                .contains("twice")
        );
        // Pointer to a page beyond the file.
        let missing = db(0x05, 0, 99);
        assert!(Database::open(&missing).unwrap().rows(1).is_err());
        // An empty leaf is fine.
        assert_eq!(
            Database::open(&db(0x0d, 0, 0)).unwrap().rows(1).unwrap(),
            Vec::<Vec<SqlValue>>::new()
        );
    }

    #[test]
    fn long_chains_of_pages_do_not_exhaust_the_stack() {
        // Pages 1..PAGES are interior pages with no cells whose right-most pointer is the next
        // page; the last page is an empty leaf.
        const PAGES: u32 = 4_096;
        let mut bytes = db(0x05, 0, 2);
        for number in 2..=PAGES {
            let mut page = vec![0u8; 512];
            if number < PAGES {
                page[0] = 0x05;
                page[8..12].copy_from_slice(&(number + 1).to_be_bytes());
            } else {
                page[0] = 0x0d;
            }
            bytes.extend_from_slice(&page);
        }
        let rows = Database::open(&bytes).unwrap().rows(1).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn interior_pages_keep_rowid_order() {
        // Page 1: interior with one cell (child page 2) and right-most pointer page 3.
        // Pages 2 and 3: leaves holding one row each, an integer 1 and 2.
        let mut bytes = db(0x05, 1, 3);
        bytes[112..114].copy_from_slice(&200u16.to_be_bytes());
        bytes[200..204].copy_from_slice(&2u32.to_be_bytes());
        for value in [1u8, 2] {
            let mut page = vec![0u8; 512];
            page[0] = 0x0d;
            page[3..5].copy_from_slice(&1u16.to_be_bytes());
            page[8..10].copy_from_slice(&100u16.to_be_bytes());
            // Cell: payload size 3, rowid, record (header length 2, type 1 = i8, value).
            page[100..105].copy_from_slice(&[3, value, 2, 1, value]);
            bytes.extend_from_slice(&page);
        }
        let rows = Database::open(&bytes).unwrap().rows(1).unwrap();
        assert_eq!(rows, vec![vec![SqlValue::Int(1)], vec![SqlValue::Int(2)]]);
    }

    #[test]
    fn a_cell_listed_twice_is_an_error() {
        // A leaf whose two cell pointers both point at one row.
        let mut bytes = db(0x0d, 2, 0);
        bytes[108..110].copy_from_slice(&200u16.to_be_bytes());
        bytes[110..112].copy_from_slice(&200u16.to_be_bytes());
        bytes[200..205].copy_from_slice(&[3, 1, 2, 1, 7]);
        let err = Database::open(&bytes).unwrap().rows(1).unwrap_err();
        assert!(err.contains("listed twice"), "{err}");
    }

    #[test]
    fn an_overflow_chain_that_loops_is_an_error() {
        // A 1,000-byte cell keeps 39 bytes on the leaf; the rest is on overflow page 2, whose
        // next page is itself.
        let mut bytes = db(0x0d, 1, 0);
        bytes[108..110].copy_from_slice(&200u16.to_be_bytes());
        bytes[200..203].copy_from_slice(&[0x87, 0x68, 1]);
        bytes[203 + 39..203 + 43].copy_from_slice(&2u32.to_be_bytes());
        let mut overflow = vec![0u8; 512];
        overflow[..4].copy_from_slice(&2u32.to_be_bytes());
        bytes.extend_from_slice(&overflow);
        let err = Database::open(&bytes).unwrap().rows(1).unwrap_err();
        assert!(err.contains("referenced twice"), "{err}");
    }

    #[test]
    fn a_cell_larger_than_the_file_is_an_error() {
        let mut bytes = db(0x0d, 1, 0);
        bytes[108..110].copy_from_slice(&200u16.to_be_bytes());
        // Payload size 16,384 in a 512-byte file.
        bytes[200..204].copy_from_slice(&[0x81, 0x80, 0x00, 1]);
        let err = Database::open(&bytes).unwrap().rows(1).unwrap_err();
        assert!(err.contains("more data than the file"), "{err}");
    }

    #[test]
    fn a_record_with_too_many_values_is_an_error() {
        // Header length 32,772: three bytes of length, then 32,769 null values.
        let mut payload = vec![0x82, 0x80, 0x04];
        payload.resize(32_772, 0);
        let err = record(&payload).unwrap_err();
        assert!(err.contains("more than"), "{err}");
    }

    #[test]
    fn records_with_huge_lengths_are_errors() {
        // Serial type for a blob of about 2^62 bytes.
        let payload = [10, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe];
        assert!(record(&payload).is_err());
    }

    #[test]
    fn records() {
        // header: len 5, types: 1 (i8), 17 (2-char text), 0 (null), 9 (one); values: -2, "hi"
        let payload = [5, 1, 17, 0, 9, 0xfe, b'h', b'i'];
        assert_eq!(
            record(&payload).unwrap(),
            vec![
                SqlValue::Int(-2),
                SqlValue::Text("hi".into()),
                SqlValue::Null,
                SqlValue::Int(1)
            ]
        );
    }
}
