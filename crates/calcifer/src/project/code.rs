//! Public project codes use ContentKit's nine-character Crockford alphabet.

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const LENGTH: usize = 9;

pub(crate) fn generate() -> String {
    loop {
        let mut bytes = [0u8; LENGTH];
        getrandom::fill(&mut bytes).expect("the OS gives random bytes");
        for byte in &mut bytes {
            *byte = ALPHABET[usize::from(*byte & 31)];
        }
        if bytes.iter().any(u8::is_ascii_uppercase) {
            return String::from_utf8(bytes.to_vec()).expect("the code alphabet is ASCII");
        }
    }
}

/// Accept the spellings ContentKit accepts, returning the canonical public code.
pub(crate) fn parse(text: &str) -> Option<String> {
    let mut code = String::with_capacity(LENGTH);
    for byte in text.bytes().filter(|byte| *byte != b'-') {
        let byte = match byte.to_ascii_uppercase() {
            b'O' => b'0',
            b'I' | b'L' => b'1',
            other => other,
        };
        if code.len() == LENGTH || !ALPHABET.contains(&byte) {
            return None;
        }
        code.push(char::from(byte));
    }
    (code.len() == LENGTH && code.bytes().any(|byte| byte.is_ascii_uppercase())).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_codes_have_the_contentkit_format() {
        for _ in 0..1000 {
            let code = generate();
            assert_eq!(code.len(), 9);
            assert_eq!(parse(&code), Some(code));
        }
    }

    #[test]
    fn code_spelling_normalizes_like_contentkit() {
        assert_eq!(parse("ab-cdo-il23"), Some("ABCD01123".into()));
        for text in ["123456789", "ABCDEFGHIJ", "ABCDU1234", "ABC123", "ABCD123456", "ABC 12345", "éBCD12345"] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
