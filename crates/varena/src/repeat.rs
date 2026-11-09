//! A loop found from the order regions are used, with nothing declared. A pass is a run of uses
//! with no region twice; where the last whole pass first used each region is the loop's plan.
//! Plain arithmetic: no device, lock or allocation, and a constant cost per use.

/// A region's place in the passes that last used it.
#[derive(Clone, Copy, Default)]
pub(crate) struct Stamp {
    // The pass of its latest first use (0: never), its place there, and its place the time before.
    pass: u64,
    place: u64,
    prior: Option<u64>,
}

#[derive(Default)]
pub(crate) struct Passes {
    // Whole passes so far: the running one is the next.
    done: u64,
    // First uses this pass, and how many came in the last pass's order.
    places: u64,
    agreed: u64,
    // The last first use's place in the pass before.
    before: Option<u64>,
    // Whether the last whole pass kept the order of the one before it.
    repeated: bool,
    // The last whole pass's first uses.
    length: u64,
}

impl Passes {
    /// A use of a region other than the one used just before it. One already used this pass
    /// begins the next.
    pub(crate) fn used(&mut self, stamp: &mut Stamp) {
        if stamp.pass == self.done + 1 {
            self.repeated = self.follows();
            self.length = self.places;
            self.done += 1;
            (self.places, self.agreed, self.before) = (0, 0, None);
        }
        let prior = (stamp.pass != 0).then_some(stamp.place);
        if let Some(prior) = prior {
            if self.before.is_none_or(|before| prior > before) {
                self.agreed += 1;
            }
            self.before = Some(prior);
        }
        *stamp = Stamp { pass: self.done + 1, place: self.places, prior };
        self.places += 1;
    }

    /// The order repeats: most first uses this pass followed the last pass's order, so a region
    /// only some passes use, or one out of turn, leaves the rest a loop. Before a pass's first
    /// use, as the last pass found.
    fn follows(&self) -> bool {
        match self.places {
            0 => self.repeated,
            places => self.agreed * 2 > places,
        }
    }

    /// A region's place in the loop's plan, where the last whole pass first used it. None in a
    /// loop's first pass, for a region new this pass, or while the order does not repeat.
    pub(crate) fn place(&self, stamp: &Stamp) -> Option<u64> {
        if !self.follows() {
            return None;
        }
        match stamp.pass == self.done + 1 {
            true => stamp.prior,
            false => (stamp.pass != 0).then_some(stamp.place),
        }
    }
}

impl Passes {
    /// A region's place in what the loop keeps, lowest kept first (`keep_order`). None as `place`.
    pub(crate) fn keep(&self, stamp: &Stamp) -> Option<u64> {
        Some(keep_order(self.place(stamp)?, self.length))
    }
}

/// Place `place` of a pass of `n` in the order its loop keeps them, lowest kept first. It holds the
/// 32-bit points of [place/n, (place+1)/n) and goes as late as the least radical inverse among
/// them (its point with the most trailing zeros): whatever count gives way is spread through the
/// pass (a half, the quarters, then the eighths), so a streamed region's copy runs under the
/// compute of the kept ones before it, and one region's size never draws it first. A kept prefix
/// streams its tail back to back and leaves the copy engine idle through the prefix: Flux-2 on a
/// 4090 stepped 1.65 s streaming its tail (v1, pasan 42221) and 1.33 s with its streamed regions
/// scattered (42309, more bytes), against ComfyUI's 1.64 (pingu, 11-04).
fn keep_order(place: u64, n: u64) -> u64 {
    let at = |i: u64| (i << 32).div_ceil(n.max(1));
    let (start, end) = (at(place), at(place + 1));
    let point = (0..=32).rev().map(|k| start.div_ceil(1 << k) << k).find(|point| *point < end).unwrap_or(start);
    let inverse = (point.min(u64::from(u32::MAX)) as u32).reverse_bits();
    u64::from(u32::MAX - inverse) << 32 | place.min(u64::from(u32::MAX))
}

/// What a loop keeps: where it ends in key order (`key`), and the size past which a region is big.
#[derive(Clone, Copy)]
pub(crate) struct Kept {
    end: u64,
    big: usize,
}

/// A region's place among what its loop keeps, lowest kept first: the big ones, then keep order.
fn key(keep: u64, bytes: usize, big: usize) -> u64 {
    u64::from(bytes <= big) << 63 | keep >> 1
}

/// What a loop keeps, from each region's (keep, bytes, on the card): the most of its key order
/// that fits what it holds on the card now beside a window of two of the largest regions left to
/// stream (one in use, one filling). None with no region placed. By bytes over
/// every region of the loop, held or not, so a layout far from the keep order moves to it and
/// grows into the room it has: a test of kept against streamed bytes in keep order held for one
/// region of Qwen 4 MP at 8 GB, and its kept set shrank call on call (guel 43054: 8 of 32 kept,
/// 5.2 GB a step against a plan prefix's 3.2).
///
/// A region over twice the loop's median is big and kept first: streamed, it sets the window. In
/// replay a 1.9 GB embedding among 250 MB blocks under 4.2 GB, first to go in keep order, left
/// 320 MB to keep, 9.4 GB a pass against 7.5 with it kept.
pub(crate) fn prefix(regions: Vec<(u64, usize, bool)>) -> Option<Kept> {
    let mut sizes: Vec<usize> = regions.iter().map(|(_, bytes, _)| *bytes).collect();
    sizes.sort_unstable();
    let big = 2 * sizes.get(sizes.len() / 2)?;
    let held: usize = regions.iter().filter(|(_, _, held)| *held).map(|(_, bytes, _)| bytes).sum();
    let mut regions: Vec<(u64, usize)> = regions.into_iter().map(|(keep, bytes, _)| (key(keep, bytes, big), bytes)).collect();
    regions.sort_unstable();
    let mut kept: usize = regions.iter().map(|(_, bytes)| bytes).sum();
    let (mut largest, mut end) = (0usize, u64::MAX);
    // From everything kept, the last of the key order streams until the rest and its window fit.
    for (key, bytes) in regions.into_iter().rev() {
        if kept + 2 * largest <= held {
            break;
        }
        kept -= bytes;
        largest = largest.max(bytes);
        end = key;
    }
    Some(Kept { end, big })
}

/// A loop's region for eviction: whether it ranks with the kept, and its key, lowest first. One
/// that stayed, or was refilled inside the kept part, is kept and goes from the kept part's end in
/// key order; the rest go most recently used first.
pub(crate) fn rank(stayed: bool, keep: Option<u64>, bytes: usize, kept: Option<Kept>, last_use: u64) -> (bool, u64) {
    match (keep, kept) {
        (Some(keep), Some(kept)) if stayed || key(keep, bytes, kept.big) < kept.end => {
            (true, u64::MAX - key(keep, bytes, kept.big))
        }
        _ => (stayed, u64::MAX - last_use),
    }
}
