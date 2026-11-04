//! Measure empty capture-pool packing from an eager allocation lifetime sequence.

use super::{Block, Error, Life, Pool, Result, Segment, State, Working, rounded};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
enum Reuse {
    Ordered(u64),
    Complete,
    Blocked,
}

enum Event {
    Allocate {
        address: u64,
        bytes: usize,
        stream: u64,
    },
    Free {
        address: u64,
        reuse: Reuse,
    },
}

#[derive(Default)]
pub(super) struct Trace(Vec<Event>);

pub(super) fn allocated(state: &mut State, address: u64) {
    let Some(trace) = state
        .phases
        .active
        .as_mut()
        .and_then(|phase| phase.packing.as_mut())
    else {
        return;
    };
    let block = &state.blocks[&address];
    trace.0.push(Event::Allocate {
        address,
        bytes: block.capacity,
        stream: block.stream,
    });
}

pub(super) fn freed(state: &mut State, address: u64, completed: bool) {
    let Some(phase) = &mut state.phases.active else {
        return;
    };
    let Some(trace) = &mut phase.packing else {
        return;
    };
    let block = &state.blocks[&address];
    if block.phase != Some(phase.token) {
        return;
    }
    // Eager fence polling is not a capture-time completion proof. Only the actual
    // single-stream order or explicit completed native retirement permits reuse.
    let reuse = if completed {
        Reuse::Complete
    } else if block.streams.len() == 1 {
        Reuse::Ordered(*block.streams.first().expect("one consumer stream"))
    } else {
        Reuse::Blocked
    };
    trace.0.push(Event::Free { address, reuse });
}

impl Trace {
    pub(super) fn capacity(&self, peak: usize, page: usize) -> Result<usize> {
        let mut capacity = rounded(peak, page)?;
        while let Some(request) = self.fit(capacity)? {
            capacity = rounded(
                capacity
                    .checked_add(request)
                    .ok_or(Error::Invalid("capture packing capacity overflow"))?,
                page,
            )?;
        }
        Ok(capacity)
    }

    // No Working/Driver, mapped memory, capture guard or GPU owner is constructed.
    // These offsets exercise exactly the live allocator's take/fit metadata paths.
    fn fit(&self, capacity: usize) -> Result<Option<usize>> {
        let pool = (0, 0);
        let mut state = State::default();
        state.pools.insert(pool, Pool::default());
        state.segments.insert(
            0,
            Segment {
                bytes: capacity,
                size_class: 0,
                handle: 0,
                pool: Some(pool),
                phase_blocks: 0,
            },
        );
        state.blocks.insert(0, Block::free(0, capacity));
        state
            .free
            .entry((Some(pool), capacity, None))
            .or_default()
            .push(0);
        let mut live = BTreeMap::new();
        for event in &self.0 {
            match *event {
                Event::Allocate {
                    address,
                    bytes,
                    stream,
                } => {
                    if bytes == 0 || live.contains_key(&address) {
                        return Err(Error::Invalid(
                            "invalid capture packing allocation lifetime",
                        ));
                    }
                    let pointer = Working::take(&mut state, bytes, bytes, stream, Some(pool))
                        .or_else(|| Working::fit_pool(&mut state, pool, bytes, bytes, stream));
                    let Some(pointer) = pointer else {
                        return Ok(Some(bytes));
                    };
                    live.insert(address, pointer);
                }
                Event::Free { address, reuse } => {
                    let pointer = live
                        .remove(&address)
                        .ok_or(Error::Invalid("invalid capture packing free lifetime"))?;
                    let block = state
                        .blocks
                        .get_mut(&pointer)
                        .expect("allocated packing block");
                    block.requested = 0;
                    let (life, origin) = match reuse {
                        Reuse::Ordered(stream) => (Life::Captured, Some(stream)),
                        Reuse::Complete => (Life::Ready, None),
                        Reuse::Blocked => (Life::Captured, None),
                    };
                    block.life = life;
                    block.reusable_on = origin;
                    if !matches!(reuse, Reuse::Blocked) {
                        state
                            .free
                            .entry((Some(pool), block.capacity, origin))
                            .or_default()
                            .push(pointer);
                    }
                }
            }
        }
        Ok(None)
    }
}
