//! Minimal reader for the binary source-file encoding returned by
//! `getSourceFile` (the base64 `data` field). We only need the fixed-size
//! node records to construct `index.kind.pathKey` node handles for
//! `getTypeAtLocation`; string/extended/structured payloads are ignored.
//!
//! Format (microsoft/TypeScript `tsc/internal/api/encoder/encoder.go`):
//! a 64-byte header whose bytes 40..44 hold the little-endian offset of the
//! nodes section, then 28-byte records in pre-order — index 0 is a nil
//! sentinel, index 1 is the SourceFile root — each with kind, UTF-16 pos/end,
//! next, parent, data, and flags as u32 fields. NodeList records
//! (kind == u32::MAX) occupy an index but are not real nodes.

/// One real node's address and extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeRecord {
    pub index: u32,
    pub kind: u32,
    pub pos: u32,
    pub end: u32,
}

/// The fixed record size in the nodes section (7 u32 fields).
const NODE_SIZE: usize = 28;
/// Header offset of the nodes-section byte offset (HeaderOffsetNodes).
const HEADER_NODES_OFFSET: usize = 40;

/// Decodes standard base64 (with padding) as used by JSON-encoded binary
/// fields. Returns `None` on malformed input.
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &byte in text.trim().as_bytes() {
        if byte == b'=' {
            break;
        }
        acc = (acc << 6) | u32::from(value(byte)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Reads all real node records from an encoded source file.
pub fn read_nodes(data: &[u8]) -> Option<Vec<NodeRecord>> {
    if data.len() < 64 {
        return None;
    }
    let nodes_offset = u32::from_le_bytes(data[HEADER_NODES_OFFSET..HEADER_NODES_OFFSET + 4].try_into().ok()?) as usize;
    // The nodes section follows variable-length string data and is not
    // guaranteed to be 4-byte aligned.
    if nodes_offset >= data.len() {
        return None;
    }
    let mut records = Vec::new();
    let mut index = 0u32;
    let mut cursor = nodes_offset;
    while cursor + NODE_SIZE <= data.len() {
        let kind = u32::from_le_bytes(data[cursor..cursor + 4].try_into().ok()?);
        let pos = u32::from_le_bytes(data[cursor + 4..cursor + 8].try_into().ok()?);
        let end = u32::from_le_bytes(data[cursor + 8..cursor + 12].try_into().ok()?);
        if index > 0 && kind != u32::MAX {
            records.push(NodeRecord { index, kind, pos, end });
        }
        index += 1;
        cursor += NODE_SIZE;
    }
    Some(records)
}

/// Skips leading trivia (whitespace and comments) from a node's stored
/// full-start position, reproducing TypeScript's `getStart` semantics:
/// encoded records store `pos` (trivia included), while spans coming from
/// our parser start at the first real token. Everything stays in UTF-16
/// code units, so non-ASCII content cannot shift the arithmetic.
fn skip_trivia(units: &[u16], pos: u32) -> u32 {
    const SPACE: u16 = 0x20;
    const TAB: u16 = 0x09;
    const CR: u16 = 0x0D;
    const LF: u16 = 0x0A;
    const SLASH: u16 = 0x2F;
    const STAR: u16 = 0x2A;
    let mut i = pos as usize;
    while let Some(&u) = units.get(i) {
        match u {
            SPACE | TAB | CR | LF => i += 1,
            SLASH if units.get(i + 1) == Some(&SLASH) => {
                i += 2;
                while units.get(i).is_some_and(|&u| u != LF) {
                    i += 1;
                }
            }
            SLASH if units.get(i + 1) == Some(&STAR) => {
                i += 2;
                while units.get(i).is_some() && !(units[i] == STAR && units.get(i + 1) == Some(&SLASH)) {
                    i += 1;
                }
                i = (i + 2).min(units.len());
            }
            _ => break,
        }
    }
    i as u32
}

/// Finds nodes with exactly this UTF-16 token-start/end span. Several nodes
/// can share a span (a parenthesized expression and its inner expression);
/// the outermost match (lowest pre-order index) wins, which matches how the
/// checker resolves an expression's type.
pub fn find_by_span<'a>(records: &'a [NodeRecord], units: &[u16], start: u32, end: u32) -> Option<&'a NodeRecord> {
    records
        .iter()
        .find(move |record| skip_trivia(units, record.pos) == start && record.end == end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip_shapes() {
        assert_eq!(decode_base64("aGVsbG8=").as_deref(), Some(b"hello".as_slice()));
        assert_eq!(decode_base64("aGVsbG8h").as_deref(), Some(b"hello!".as_slice()));
        assert_eq!(decode_base64("AAECAwQ=").as_deref(), Some([0u8, 1, 2, 3, 4].as_slice()));
        assert!(decode_base64("not base64!!").is_none());
    }

    #[test]
    fn reads_fixed_size_node_records() {
        let mut data = vec![0u8; 64];
        data[40..44].copy_from_slice(&64u32.to_le_bytes());
        // Sentinel at index 0, then a SourceFile-ish root at index 1.
        data.extend_from_slice(&[0u8; NODE_SIZE]);
        let mut root = [0u8; NODE_SIZE];
        root[0..4].copy_from_slice(&300u32.to_le_bytes());
        root[4..8].copy_from_slice(&0u32.to_le_bytes());
        root[8..12].copy_from_slice(&42u32.to_le_bytes());
        data.extend_from_slice(&root);
        let records = read_nodes(&data).unwrap();
        assert_eq!(
            records,
            [NodeRecord {
                index: 1,
                kind: 300,
                pos: 0,
                end: 42
            }]
        );
    }
}
