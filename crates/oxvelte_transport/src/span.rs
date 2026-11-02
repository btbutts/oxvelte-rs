//! Span-unit conversion for wire protocols. oxc spans are UTF-8 byte
//! offsets; everything across a process boundary (the Svelte compiler's
//! warnings, the tsrs wire protocol, ESLint's column conventions, LSP
//! `Position.character`) counts UTF-16 code units.

/// Convert a UTF-8 byte offset into a UTF-16 code-unit index.
pub fn byte_utf16(source: &str, byte: u32) -> usize {
    source.get(..byte as usize).unwrap_or(source).encode_utf16().count()
}

/// Convert a UTF-16 code-unit index into a UTF-8 byte offset.
pub fn utf16_byte(source: &str, index: usize) -> u32 {
    let mut units = 0;
    for (byte, ch) in source.char_indices() {
        if units >= index {
            return byte as u32;
        }
        units += ch.len_utf16();
    }
    source.len() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_positions_map_to_utf8_boundaries() {
        // 1-byte/1-unit ASCII, a 4-byte/2-unit (surrogate pair) non-BMP
        // character, and a 2-byte/1-unit BMP character — each encoding
        // asymmetry must land on the right byte boundary.
        assert_eq!(utf16_byte("a\u{1F980}\u{e9}", 1), 1);
        assert_eq!(utf16_byte("a\u{1F980}\u{e9}", 3), 5);
        assert_eq!(utf16_byte("a\u{1F980}\u{e9}", 4), 7);
    }
}
