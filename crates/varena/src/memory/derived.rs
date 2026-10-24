//! Derived weights in Memory ([`crate::derived`], design varena-local "Derived weights"): a declared
//! set's transform that landed is offered to the write-behind writer when deriving it again costs
//! more than writing it and reading it back (M4), into this executor's temp file, within its cap
//! (its models' own size). A fill of a page whose identity was written reads it back, no transform.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, OnceLock, Weak};

use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use super::{Inner, Memory, State};
use crate::backend::{Event, Va};
use crate::derived::{pays, Hold, Job, Key as Derived, Refill, Store, Writer};
use crate::table::{Fill, Key, SetSpec};
use crate::Result;

/// Memory's side of the derived weights.
#[derive(Default)]
pub(super) struct Cache {
    writer: OnceLock<Arc<Writer>>,
    /// The executor's temp file, opened at the first write (`Err`: unopenable, never retried).
    store: Mutex<Option<std::result::Result<Store, ()>>>,
    /// Entries written: where each is in the file.
    entries: Mutex<HashMap<Derived, (Arc<std::fs::File>, u64)>>,
    /// The page each offered entry is written from.
    pages: Mutex<HashMap<Derived, Key>>,
    /// Transforms that landed, measured once both events complete: (page, the bind's event,
    /// the transform's).
    landed: Mutex<Vec<(Key, Event, Event)>>,
    /// The store's measured read bandwidth, bytes per second (M4's read side).
    read_bps: AtomicU64,
}

impl Memory {
    /// The writer: its hold is this Memory's pages.
    pub(super) fn start_derived(&self) -> Result<()> {
        let writer = Writer::new(self.inner.backend.clone(), Arc::new(Holder(Arc::downgrade(&*self.inner))))?;
        let _ = self.inner.derived.writer.set(Arc::new(writer));
        Ok(())
    }
}

/// A declared set's page `region`: the key of its derived bytes.
pub(super) fn derived_of(spec: &SetSpec, region: u32) -> Option<Derived> {
    let Fill::Declared(identity) = &spec.fingerprint.fill else { return None };
    let digest = Sha256::digest(spec.fingerprint.key().as_bytes());
    let fingerprint = digest.get(..8).and_then(|b| <[u8; 8]>::try_from(b).ok()).map(u64::from_le_bytes)?;
    Some(Derived::new(fingerprint, region, identity))
}

impl Inner {
    fn derived_key(&self, st: &State, key: Key) -> Option<Derived> {
        derived_of(&st.mech(key.0).ok()?.spec, key.1)
    }

    /// The fill threads' refill for a page of `spec`, when its identity was written.
    pub(super) fn derived_refill(&self, spec: &SetSpec, key: Key, span: u64) -> Option<Refill> {
        let dk = derived_of(spec, key.1)?;
        let (file, at) = self.derived.entries.lock().get(&dk).cloned()?;
        Refill::new(&dk, &file, at, span).inspect_err(|e| tracing::debug!("varena: a derived refill of {key:?}: {e}")).ok()
    }

    /// A batch read from disk: M4's read bandwidth.
    pub(super) fn read_measured(&self, bytes: u64, ns: u64) {
        if ns == 0 {
            return;
        }
        let now = (u128::from(bytes) * 1_000_000_000 / u128::from(ns)) as u64;
        let was = self.derived.read_bps.load(Relaxed);
        self.derived.read_bps.store(if was == 0 { now } else { (was * 3 + now) / 4 }, Relaxed);
    }

    /// Whether a transform on this page is measured for the writer.
    pub(super) fn measures(&self) -> bool {
        self.derived.writer.get().is_some_and(|w| w.usable())
    }

    /// A transform landed (`Pin::filled`): `from` (the bind's event, now the cache's to retire)
    /// to `to` (the page's ready event) is its time on the device.
    pub(super) fn transformed(&self, key: Key, from: Event, to: Event) {
        self.derived.landed.lock().push((key, from, to));
    }

    /// Transforms whose events completed: each is offered to the writer when deriving it again
    /// costs more than writing it and reading it back (M4; before the writer measured its own
    /// bandwidth, the store's read stands in) and the executor's file has room under its cap, its
    /// models' own bytes. Under the state lock; nothing waits.
    pub(super) fn offer_derived(&self, st: &State) {
        let Some(writer) = self.derived.writer.get().filter(|w| w.usable()) else { return };
        let mut landed = self.derived.landed.lock();
        if landed.is_empty() {
            return;
        }
        let cap: u64 = st.sets.iter().flatten().filter(|m| !m.closing).map(|m| m.spec.layout.nbytes).sum();
        landed.retain(|&(key, from, to)| {
            let ns = match self.backend.elapsed(from, to) {
                Ok(None) => return true,
                Ok(Some(ns)) => Some(ns),
                Err(_) => None,
            };
            self.backend.retire(from);
            let (Some(ns), Some(dk), Ok((va, off, span))) = (ns, self.derived_key(st, key), st.place(key)) else { return false };
            let read = self.derived.read_bps.load(Relaxed) as f64;
            if !pays(ns, span, writer.write_bps().unwrap_or(read), read) || self.derived.entries.lock().contains_key(&dk) || self.derived.pages.lock().contains_key(&dk) {
                return false;
            }
            let mut store = self.derived.store.lock();
            let opened = store.get_or_insert_with(|| Store::open().map_err(|e| tracing::warn!("varena: no temp file for derived weights: {e}")));
            let Some((file, at)) = opened.as_mut().ok().and_then(|s| s.reserve(span, cap)) else { return false };
            self.derived.pages.lock().insert(dk, key);
            writer.offer(Job { key: dk, file, at, va: Va { base: va.base + off, len: span }, ready: Event::DONE });
            false
        });
    }

    /// No request runs: the writer may write.
    pub(super) fn derived_idle(&self) {
        if let Some(w) = self.derived.writer.get() {
            w.wake();
        }
    }
}

/// The writer's hold on this Memory's pages.
struct Holder(Weak<Inner>);

impl Hold for Holder {
    /// The page, filled with these derived bytes and quiet, pinned for its copy: a reclaim
    /// declines it meanwhile, and never waits on the disk write after (M1).
    fn pin(&self, dk: &Derived) -> Option<Box<dyn Send>> {
        let inner = self.0.upgrade()?;
        let key = *inner.derived.pages.lock().get(dk)?;
        let mut st = inner.state.lock();
        let fit = st.homes.get(&key).is_some_and(|h| h.filled && h.phys.is_some() && !h.filling && !h.transforming) && inner.derived_key(&st, key) == Some(*dk);
        if !fit {
            inner.derived.pages.lock().remove(dk);
            return None;
        }
        let id = st.tick();
        st.pins.insert(id, (key, None, std::thread::current().id()));
        st.sync(key);
        Some(Box::new(Held { inner: Arc::downgrade(&inner), id, key }))
    }

    fn idle(&self) -> bool {
        self.0.upgrade().is_some_and(|i| i.state.lock().table.requests.is_empty())
    }

    fn written(&self, dk: Derived, file: Arc<std::fs::File>, at: u64) {
        let Some(inner) = self.0.upgrade() else { return };
        inner.derived.pages.lock().remove(&dk);
        inner.derived.entries.lock().insert(dk, (file, at));
    }
}

/// The writer's pin on a page for its copy.
struct Held {
    inner: Weak<Inner>,
    id: u64,
    key: Key,
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            let mut st = inner.state.lock();
            st.pins.remove(&self.id);
            st.sync(self.key);
            drop(st);
            inner.landed.notify_all();
        }
    }
}
