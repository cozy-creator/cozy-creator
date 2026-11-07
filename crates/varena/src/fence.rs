//! One completion mechanism. A free, unpin or replay stamps (stream, sequence) with no driver
//! call. Outside capture, `poll` records one event per dirty stream and queries them in order.
use crate::{Result, cuda::Driver};
use std::collections::{BTreeMap, VecDeque};

/// Complete once each stream finishes its sequence. Internal IDs survive raw-handle reuse.
pub(crate) type Waits = Vec<(u64, u64)>;

// A stream that outruns its queries stays dirty; its stamps seal as older events finish.
const SEALED_MAX: usize = 4;

struct Stream {
    raw: u64,
    next: u64,
    done: u64,
    dirty: bool,
    sealed: VecDeque<(usize, u64)>,
}

/// Events are pooled and never destroyed, so a waiter outside every lock holds a valid handle.
#[derive(Default)]
pub(crate) struct Fences {
    ids: BTreeMap<u64, u64>,
    streams: BTreeMap<u64, Stream>,
    last_id: u64,
    idle: Vec<usize>,
}

impl Fences {
    /// All work submitted to `raw` so far finishes by this stamp.
    pub(crate) fn stamp(&mut self, raw: u64, waits: &mut Waits) {
        let id = *self.ids.entry(raw).or_insert_with(|| {
            self.last_id += 1;
            self.last_id
        });
        let stream = self.streams.entry(id).or_insert_with(|| Stream {
            raw,
            next: 1,
            done: 0,
            dirty: false,
            sealed: VecDeque::new(),
        });
        stream.dirty = true;
        let next = stream.next;
        // Metadata may outlive many streams: drop what is already settled.
        waits.retain(|(held, sequence)| *held != id
            && self.streams.get(held).is_some_and(|stream| stream.done < *sequence));
        waits.push((id, next));
    }

    /// As known at the last poll. A retired stream has drained, so it completes everything.
    pub(crate) fn complete(&self, waits: &[(u64, u64)]) -> bool {
        waits
            .iter()
            .all(|(id, sequence)| self.streams.get(id).is_none_or(|stream| stream.done >= *sequence))
    }

    /// Caller holds the device gate with no capture active, inside this device's context.
    pub(crate) fn poll(&mut self, driver: &Driver) -> Result<()> {
        for stream in self.streams.values_mut() {
            if stream.dirty && stream.sealed.len() < SEALED_MAX {
                let event = match self.idle.pop() {
                    Some(event) => event,
                    None => driver.create_event()?,
                };
                if let Err(error) = driver.record(event, stream.raw as usize) {
                    self.idle.push(event);
                    return Err(error);
                }
                stream.sealed.push_back((event, stream.next));
                stream.next += 1;
                stream.dirty = false;
            }
            while let Some(&(event, sequence)) = stream.sealed.front() {
                if !driver.query(event)? {
                    break;
                }
                stream.done = sequence;
                stream.sealed.pop_front();
                self.idle.push(event);
            }
        }
        Ok(())
    }

    /// Events to wait on before these waits can complete. If a stamp is not sealed yet the
    /// newest event stands in: the caller polls and comes back, having made progress.
    pub(crate) fn events(&self, waits: &[(u64, u64)], events: &mut Vec<usize>) {
        for (id, sequence) in waits {
            let Some(stream) = self.streams.get(id).filter(|stream| stream.done < *sequence) else {
                continue;
            };
            let covering = stream.sealed.iter().find(|(_, sealed)| sealed >= sequence);
            if let Some((event, _)) = covering.or(stream.sealed.back())
                && !events.contains(event)
            {
                events.push(*event);
            }
        }
    }

    /// The stream's owner proved it drained and will destroy it next.
    pub(crate) fn retire(&mut self, raw: u64) {
        if let Some(id) = self.ids.remove(&raw)
            && let Some(stream) = self.streams.remove(&id)
        {
            self.idle.extend(stream.sealed.into_iter().map(|(event, _)| event));
        }
    }

    pub(crate) fn handles(&self) -> usize {
        self.idle.len() + self.streams.values().map(|stream| stream.sealed.len()).sum::<usize>()
    }
}
