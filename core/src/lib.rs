//! Shared, hardware-free building blocks for the CLI, the bridge and the watchdog.
//!
//! This crate is the single source of truth for the RAPL register model, the
//! PL1/PL2 policy, the bridge info protocol and the JSONL audit record format.
pub mod audit;
pub mod policy;
pub mod protocol;
pub mod rapl;

/// Parses a strict `0x`-prefixed hexadecimal u64 (1-16 digits, no sign or spaces).
pub fn parse_hex_u64(text: &str) -> Result<u64, String> {
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .ok_or_else(|| format!("expected a 0x-prefixed hexadecimal value: {text:?}"))?;
    if digits.is_empty() || digits.len() > 16 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("invalid hexadecimal value: {text:?}"));
    }
    u64::from_str_radix(digits, 16).map_err(|_| format!("invalid hexadecimal value: {text:?}"))
}

#[cfg(test)]
mod tests {
    use super::parse_hex_u64;

    #[test]
    fn hex_parsing_is_strict() {
        assert_eq!(parse_hex_u64("0x0"), Ok(0));
        assert_eq!(parse_hex_u64("0XFFFFFFFFFFFFFFFF"), Ok(u64::MAX));
        for bad in [
            "0x",
            "0x+1",
            "ff",
            "0x1 ",
            " 0x1",
            "0x00000000000000000",
            "0xg",
        ] {
            assert!(parse_hex_u64(bad).is_err(), "{bad}");
        }
    }
}
