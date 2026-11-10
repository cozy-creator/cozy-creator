//! The writer's chunk rule (spec "Chunks", informative): a pure planner from parts in header
//! (construction) order to chunk lengths and locations. It never reorders: block boundaries come
//! from the given order. Readers never need it; they follow offsets.

use super::Location;

/// A part this long or longer is its own object(s).
pub(crate) const STANDALONE: u64 = 16 << 20;
/// Standalone parts split every `GRID` bytes from their first byte.
pub(crate) const GRID: u64 = 64 << 20;
/// A pack never grows past this.
pub(crate) const PACK: u64 = 64 << 20;
/// A pack holding this much closes at the next block boundary.
pub(crate) const BLOCK_CLOSE: u64 = 16 << 20;
/// Every packed part starts at a multiple of this.
pub const ALIGN: u64 = 4096;

/// One stored part: a plain tensor's data or one part of an encoded tensor.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Item<'a> {
    pub component: &'a str,
    pub key: &'a str,
    pub length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    /// Chunk lengths, in `Header::chunks` order.
    pub chunks: Vec<u64>,
    /// Where each input item lives, in input order. Empty items get `{0, 0, 0}`.
    pub places: Vec<Location>,
}

/// Lays out `items`, which are in header order: components, their tensors, each tensor's parts.
pub(crate) fn plan(items: &[Item]) -> Plan {
    let mut chunks: Vec<u64> = Vec::new();
    let mut places = vec![Location::default(); items.len()];
    // The open pack: its chunk, component, and the block of its last part.
    let mut pack: Option<(usize, &str, &str)> = None;
    for (i, it) in items.iter().enumerate() {
        if it.length == 0 {
            continue;
        }
        if it.length >= STANDALONE {
            places[i] = Location {
                chunk: chunks.len() as u64,
                offset: 0,
                length: it.length,
            };
            let mut left = it.length;
            while left > 0 {
                chunks.push(left.min(GRID));
                left -= left.min(GRID);
            }
            continue;
        }
        let block = block_of(it.key);
        let fits = pack.and_then(|(c, component, last)| {
            let start = chunks[c].next_multiple_of(ALIGN);
            let close = component != it.component
                || (last != block && chunks[c] >= BLOCK_CLOSE)
                || start + it.length > PACK;
            (!close).then_some((c, start))
        });
        let (c, offset) = fits.unwrap_or_else(|| {
            chunks.push(0);
            (chunks.len() - 1, 0)
        });
        chunks[c] = offset + it.length;
        places[i] = Location {
            chunk: c as u64,
            offset,
            length: it.length,
        };
        pack = Some((c, it.component, block));
    }
    Plan { chunks, places }
}

impl Plan {
}

/// The deepest numbered prefix of a dotted key: `a.1.b.2.c` → `a.1.b.2`; `""` without one.
pub(crate) fn block_of(key: &str) -> &str {
    let mut end = 0;
    let mut pos = 0;
    for seg in key.split('.') {
        pos += seg.len();
        if !seg.is_empty() && seg.bytes().all(|c| c.is_ascii_digit()) {
            end = pos;
        }
        pos += 1;
    }
    &key[..end]
}
