//! Random UUIDs (RFC 9562 version 4), for names nothing else may pick.

/// 32 lowercase hex digits.
pub(crate) fn simple() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS's random bytes");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 8-4-4-4-12 lowercase hex digits.
pub(crate) fn hyphenated() -> String {
    let s = simple();
    format!("{}-{}-{}-{}-{}", &s[..8], &s[8..12], &s[12..16], &s[16..20], &s[20..])
}
