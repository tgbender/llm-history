//! Stable UTF-8 JSON string encoding for reconstructing retained native lines.

/// Append a JSON string token without depending on a serializer's escape policy.
/// Archive readers must keep these byte spellings stable.
pub fn append_token(out: &mut Vec<u8>, text: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut start = 0;
    out.push(b'"');
    for (i, b) in text.bytes().enumerate() {
        let escape: &[u8] = match b {
            b'"' => br#"\""#,
            b'\\' => br"\\",
            b'\n' => br"\n",
            b'\r' => br"\r",
            b'\t' => br"\t",
            8 => br"\b",
            12 => br"\f",
            0..32 => {
                out.extend_from_slice(&text.as_bytes()[start..i]);
                out.extend_from_slice(&[
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[(b >> 4) as usize],
                    HEX[(b & 15) as usize],
                ]);
                start = i + 1;
                continue;
            }
            _ => continue,
        };
        out.extend_from_slice(&text.as_bytes()[start..i]);
        out.extend_from_slice(escape);
        start = i + 1;
    }
    out.extend_from_slice(&text.as_bytes()[start..]);
    out.push(b'"');
}
