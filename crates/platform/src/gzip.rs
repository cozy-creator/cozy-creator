//! gzip as HTTP's content coding, for an answer held whole (a log's text): whether its caller takes it, and the body
//! packed or unpacked.
use std::io::{self, Read as _, Write as _};

/// The most an unpacked body may be: one that claims more is refused, not held (a machine's answer isn't trusted
/// with this computer's memory).
const MOST: u64 = 1 << 30;

/// Whether an `Accept-Encoding` value takes gzip: named, or covered by `*`, with a weight above 0. gzip's own weight
/// goes before `*`'s.
pub fn accepted(accept_encoding: &str) -> bool {
    let weight = |coding: &str| {
        accept_encoding.split(',').find_map(|part| {
            let mut fields = part.split(';').map(str::trim);
            let named = fields.next()?.eq_ignore_ascii_case(coding);
            let weight = fields
                .find_map(|field| field.split_once('=').filter(|(name, _)| name.trim().eq_ignore_ascii_case("q")))
                .map_or(Some(1.0), |(_, q)| q.trim().parse::<f32>().ok());
            named.then_some(weight.unwrap_or(0.0))
        })
    };
    weight("gzip").or_else(|| weight("*")).is_some_and(|q| q > 0.0)
}

/// `bytes` as one gzip member.
pub fn pack(bytes: &[u8]) -> Vec<u8> {
    let mut packed = flate2::write::GzEncoder::new(Vec::with_capacity(bytes.len() / 8), flate2::Compression::default());
    packed.write_all(bytes).expect("a Vec takes every write");
    packed.finish().expect("a Vec takes every write")
}

/// What `packed` holds, at most 1 GiB of it.
pub fn unpack(packed: &[u8]) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let read = flate2::read::GzDecoder::new(packed).take(MOST + 1).read_to_end(&mut bytes)? as u64;
    if read > MOST {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "it unpacks to more than 1 GiB"));
    }
    Ok(bytes)
}
