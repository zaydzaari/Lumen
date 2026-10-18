//! Hand-written encoding for Spotify's tiny `collection2v2` protobuf messages
//! (librespot-protocol doesn't compile that .proto). Field numbers follow
//! `collection2v2.proto` shipped with librespot-protocol.

fn varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn key(out: &mut Vec<u8>, field: u32, wire: u8) {
    varint(out, ((field as u64) << 3) | wire as u64);
}

fn string(out: &mut Vec<u8>, field: u32, s: &str) {
    if s.is_empty() {
        return;
    }
    key(out, field, 2);
    varint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn int(out: &mut Vec<u8>, field: u32, v: i64) {
    if v == 0 {
        return;
    }
    key(out, field, 0);
    varint(out, v as u64);
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Item {
    pub uri: String,
    pub added_at: i32,
    pub is_removed: bool,
}

fn item(i: &Item) -> Vec<u8> {
    let mut b = Vec::new();
    string(&mut b, 1, &i.uri);
    int(&mut b, 2, i.added_at as i64);
    int(&mut b, 3, i.is_removed as i64);
    b
}

/// `PageRequest { username, set, pagination_token, limit }`.
pub fn page_request(username: &str, set: &str, token: &str, limit: i32) -> Vec<u8> {
    let mut b = Vec::new();
    string(&mut b, 1, username);
    string(&mut b, 2, set);
    string(&mut b, 3, token);
    int(&mut b, 4, limit as i64);
    b
}

/// `WriteRequest { username, set, items, client_update_id }`.
pub fn write_request(username: &str, set: &str, items: &[Item]) -> Vec<u8> {
    let mut b = Vec::new();
    string(&mut b, 1, username);
    string(&mut b, 2, set);
    for i in items {
        let m = item(i);
        key(&mut b, 3, 2);
        varint(&mut b, m.len() as u64);
        b.extend_from_slice(&m);
    }
    b
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.b.get(self.i)?;
            self.i += 1;
            v |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    fn bytes(&mut self) -> Option<&[u8]> {
        let n = self.varint()? as usize;
        let s = self.b.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(s)
    }
    /// (field, wire type) or None at the end.
    fn next(&mut self) -> Option<(u32, u8)> {
        if self.i >= self.b.len() {
            return None;
        }
        let k = self.varint()?;
        Some(((k >> 3) as u32, (k & 7) as u8))
    }
    fn skip(&mut self, wire: u8) -> Option<()> {
        match wire {
            0 => self.varint().map(|_| ()),
            1 => (self.i + 8 <= self.b.len()).then(|| self.i += 8),
            2 => self.bytes().map(|_| ()),
            5 => (self.i + 4 <= self.b.len()).then(|| self.i += 4),
            _ => None,
        }
    }
}

fn parse_item(b: &[u8]) -> Option<Item> {
    let mut r = Reader { b, i: 0 };
    let mut it = Item::default();
    while let Some((f, w)) = r.next() {
        match (f, w) {
            (1, 2) => it.uri = String::from_utf8_lossy(r.bytes()?).into_owned(),
            (2, 0) => it.added_at = r.varint()? as i32,
            (3, 0) => it.is_removed = r.varint()? != 0,
            (_, w) => r.skip(w)?,
        }
    }
    Some(it)
}

/// `PageResponse { items, next_page_token, sync_token }` → (items, next token).
pub fn parse_page(b: &[u8]) -> Option<(Vec<Item>, String)> {
    let mut r = Reader { b, i: 0 };
    let mut items = Vec::new();
    let mut next = String::new();
    while let Some((f, w)) = r.next() {
        match (f, w) {
            (1, 2) => items.push(parse_item(r.bytes()?)?),
            (2, 2) => next = String::from_utf8_lossy(r.bytes()?).into_owned(),
            (_, w) => r.skip(w)?,
        }
    }
    Some((items, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_roundtrip_through_write_encoding() {
        // A PageResponse is field 1 = items, field 2 = token: build one by hand
        // from the same item encoding the write request uses.
        let items = vec![
            Item {
                uri: "spotify:album:a".into(),
                added_at: 1_700_000_000,
                is_removed: false,
            },
            Item {
                uri: "spotify:track:b".into(),
                added_at: 5,
                is_removed: true,
            },
        ];
        let mut resp = Vec::new();
        for i in &items {
            let m = item(i);
            key(&mut resp, 1, 2);
            varint(&mut resp, m.len() as u64);
            resp.extend_from_slice(&m);
        }
        string(&mut resp, 2, "next-token");
        string(&mut resp, 3, "sync"); // unknown field is skipped
        let (back, next) = parse_page(&resp).unwrap();
        assert_eq!(back, items);
        assert_eq!(next, "next-token");
        assert!(
            parse_page(&[0x0a, 0xff]).is_none(),
            "truncated input is rejected"
        );
    }

    #[test]
    fn requests_encode_fields() {
        let b = page_request("u", "artist", "", 300);
        assert_eq!(
            b,
            vec![0x0a, 1, b'u', 0x12, 6, b'a', b'r', b't', b'i', b's', b't', 0x20, 0xac, 0x02]
        );
        let w = write_request(
            "u",
            "collection",
            &[Item {
                uri: "x".into(),
                added_at: 0,
                is_removed: true,
            }],
        );
        assert!(w.ends_with(&[0x1a, 5, 0x0a, 1, b'x', 0x18, 1]));
    }
}
