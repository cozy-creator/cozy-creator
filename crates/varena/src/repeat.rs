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
}

impl Passes {
    /// A use of a region other than the one used just before it. One already used this pass
    /// begins the next.
    pub(crate) fn used(&mut self, stamp: &mut Stamp) {
        if stamp.pass == self.done + 1 {
            self.repeated = self.follows();
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

/// Where a loop's kept prefix ends, from each region's (place, bytes, stayed through the pass):
/// the latest place of a region that stayed with more bytes kept than streamed up to it. A
/// region kept far past the streamed ones does not stretch it.
pub(crate) fn prefix(mut regions: Vec<(u64, usize, bool)>) -> Option<u64> {
    regions.sort_unstable();
    let (mut kept, mut streamed, mut end) = (0usize, 0usize, None);
    for (place, bytes, stayed) in regions {
        if stayed {
            kept += bytes;
            if kept > streamed {
                end = Some(place);
            }
        } else {
            streamed += bytes;
        }
    }
    end
}

/// A loop's region for eviction: whether it ranks with the kept, and its key, lowest first. One
/// that stayed, or was refilled inside the prefix, is kept and goes from the prefix's end, the
/// region the next pass needs last; the rest go most recently used first.
pub(crate) fn rank(stayed: bool, place: Option<u64>, prefix: Option<u64>, last_use: u64) -> (bool, u64) {
    match (place, prefix) {
        (Some(place), Some(end)) if stayed || place < end => (true, u64::MAX - place),
        _ => (stayed, u64::MAX - last_use),
    }
}
