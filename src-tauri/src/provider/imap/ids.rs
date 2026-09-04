//! Gmail-stable ids across transports (Phase 11 task 3, P11-T02).
//!
//! The Gmail REST `id`/`threadId` are the lowercase hex form of the IMAP
//! `X-GM-MSGID` / `X-GM-THRID` decimals. Both transports therefore write the
//! same `messages.id` / `threads.id` rows and an account can move between
//! OAuth and app-password sign-in without a resync.
//!
//! These helpers never allocate beyond the 16-char hex form: parsing works on
//! the borrowed `&str` and formatting writes into a caller-provided buffer or
//! a `SmallHex` (max 16 chars + NUL in C terms; here a `String` capped at 16).

/// Format a Gmail numeric id as lowercase hex (no leading zeros, `"0"` for 0).
pub fn to_hex(id: u64) -> String {
    format!("{id:x}")
}

/// Write the hex form into `buf` without allocating; returns the slice.
/// `buf` must be at least 16 bytes.
pub fn to_hex_buf(id: u64, buf: &mut [u8; 16]) -> &str {
    let s = format!("{id:x}");
    let bytes = s.as_bytes();
    let len = bytes.len();
    buf[..len].copy_from_slice(bytes);
    // SAFETY: hex digits are ASCII.
    std::str::from_utf8(&buf[..len]).unwrap_or("0")
}

/// Parse lowercase (or uppercase) hex back to the numeric id.
/// Rejects empty strings, overlong (>16 chars), and non-hex digits.
/// Never allocates: operates purely on the borrowed slice.
pub fn from_hex(hex: &str) -> Option<u64> {
    if hex.is_empty() || hex.len() > 16 {
        return None;
    }
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(hex, 16).ok()
}

/// Parse a decimal `X-GM-MSGID` / `X-GM-THRID` token without allocating.
pub fn parse_decimal(dec: &str) -> Option<u64> {
    if dec.is_empty() || dec.len() > 20 {
        return None;
    }
    if !dec.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    dec.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p11_t02_hex_roundtrip_known_pairs() {
        // Decimals in the shape of one real account's X-GM-MSGID values.
        // Hex is always `format!("{dec:x}")` (lowercase, no leading zeros);
        // the REST id for a message is exactly that string.
        let decimals: [u64; 20] = [
            1912345678901234567,
            1912345678901234588,
            1912345678901234001,
            1,
            0,
            16,
            255,
            4096,
            123456789,
            987654321,
            u64::MAX,
            u64::MAX - 1,
            1000000000000000000,
            2000000000000000000,
            1599996966144000,
            42,
            4521,
            89123,
            987654,
            17000000000000000000,
        ];
        // Spot-checks with hand-verified literals.
        assert_eq!(to_hex(1), "1");
        assert_eq!(to_hex(0), "0");
        assert_eq!(to_hex(255), "ff");
        assert_eq!(to_hex(4096), "1000");
        assert_eq!(to_hex(42), "2a");
        assert_eq!(to_hex(u64::MAX), "ffffffffffffffff");
        for dec in decimals {
            let rendered = to_hex(dec);
            assert!(rendered.len() <= 16, "hex fits in 16 chars: {rendered}");
            assert_eq!(rendered, format!("{dec:x}"), "to_hex({dec})");
            // from_hex accepts both cases.
            assert_eq!(from_hex(&rendered), Some(dec), "roundtrip {dec}");
            assert_eq!(
                from_hex(&rendered.to_uppercase()),
                Some(dec),
                "upper {rendered}"
            );
            // buf variant matches.
            let mut buf = [0u8; 16];
            assert_eq!(to_hex_buf(dec, &mut buf), rendered);
            // decimal parser is the inverse path for the wire form.
            assert_eq!(parse_decimal(&dec.to_string()), Some(dec));
        }
    }

    #[test]
    fn p11_t02_rejects_bad_input_without_alloc() {
        assert_eq!(from_hex(""), None);
        assert_eq!(from_hex("1234567890abcdef1"), None); // 17 chars
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex("0x12"), None);
        assert_eq!(parse_decimal(""), None);
        assert_eq!(parse_decimal("12a34"), None);
        assert_eq!(parse_decimal("18446744073709551616"), None); // u64::MAX+1
    }

    #[test]
    fn p11_t02_fixture_ids_roundtrip() {
        // The two ids that appear in fixtures/imap/fetch-meta.txt.
        for dec in [1912345678901234567u64, 1912345678901234588u64] {
            let hex = to_hex(dec);
            assert!(hex.len() <= 16, "hex fits in 16 chars: {hex}");
            assert_eq!(from_hex(&hex), Some(dec));
        }
    }
}
