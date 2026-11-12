//! The models this machine's store holds, for Status: each local model (`local/<name>`) and
//! each Hub checkpoint it pulled (cached Hub refs), with what it weighs. A listing is read
//! again after model work, eviction, a changed model set or a substantial disk change, not
//! every status poll. A header alone is listed as partial until its whole closure is cached.
use crate::api::v1;
use crate::storage::{self, Store};
use std::{
    collections::{BTreeMap, HashMap},
    io,
    path::{Path, PathBuf},
    sync::{Mutex, atomic::{AtomicU64, Ordering}},
    time::SystemTime,
};
use tensorfs::format::{ObjectRef, Model};

#[derive(Clone, Default, PartialEq, Debug)]
pub(crate) struct Listing {
    pub models: Vec<v1::Model>,
    /// Every held model's bytes together, shared content counted once.
    pub bytes: u64,
}

/// A Hub checkpoint this machine pulled: `tree`, `model`'s `release` in `lane` (an empty
/// release: a checkpoint by hash). The store's ref of the model records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pulled {
    pub model: String,
    pub release: String,
    pub lane: String,
    pub tree: ObjectRef,
}

impl Pulled {
    /// Names the pull in the store: `model`'s ref, a cache TensorFS may expire. The tree
    /// must be here. One the ref already names writes nothing.
    pub(crate) fn record(&self, store: &Store) -> io::Result<()> {
        let held = store.get_ref(&self.model).map_err(storage::io_error)?;
        if held.as_ref().is_some_and(|model| entries(model).any(|(release, _, tree)| release == self.release && *tree == self.tree)) {
            return Ok(());
        }
        store
            .update_ref(&self.model, |old| {
                let mut model = old.unwrap_or_else(|| Model::new(&self.model));
                match self.release.is_empty() {
                    true => model.add_checkpoint(self.tree.clone()),
                    false => model.set_lane(&self.release, &self.lane, self.tree.clone()),
                }
                Some(model)
            })
            .map(drop)
            .map_err(storage::io_error)
    }

    /// The Hub checkpoint whose header is `header`, as this machine pulled it; None for one never
    /// pulled. A header made here from one with what it lacks of its base (`inherit`, cl-935) is
    /// that one's: its chunks come from the same Hub.
    pub(crate) fn of(store: &Store, header: &ObjectRef) -> io::Result<Option<Pulled>> {
        if let Some(pulled) = Self::named(store, header)? {
            return Ok(Some(pulled));
        }
        let Ok(bytes) = store.read(header) else {
            return Ok(None);
        };
        let made = tensorfs::format::Header::from_cbor(&bytes).ok().as_ref().and_then(tensorfs::import::base::inherited_from);
        match made.filter(|checkpoint| checkpoint != header) {
            Some(checkpoint) => Self::named(store, &checkpoint),
            None => Ok(None),
        }
    }

    /// The Hub checkpoint a model's ref records with header `header`.
    fn named(store: &Store, header: &ObjectRef) -> io::Result<Option<Pulled>> {
        for model in store.refs().map_err(storage::io_error)? {
            if model.name.starts_with("local/") {
                continue;
            }
            for (release, lane, tree) in entries(&model) {
                let Ok(parsed) = store.tree(tree) else {
                    continue;
                };
                if parsed.header().is_ok_and(|(_, h)| h == header) {
                    let lane = lane.map_or(parsed.lane, str::to_owned);
                    let (model, release) = (model.name.clone(), release.to_string());
                    return Ok(Some(Pulled { model, release, lane, tree: tree.clone() }));
                }
            }
        }
        Ok(None)
    }
}

/// Each checkpoint `model` records: (its release, empty for none; its lane there; the checkpoint).
fn entries(model: &Model) -> impl Iterator<Item = (&str, Option<&str>, &ObjectRef)> {
    let released = model.releases.iter().flat_map(|r| r.lanes.iter().map(|(lane, tree)| (r.version.as_str(), Some(lane.as_str()), tree)));
    released.chain(model.checkpoints.iter().map(|tree| ("", None, tree)))
}

/// Each model's rows: model, release, lane, header.
type Seen = Vec<(String, String, String, ObjectRef)>;

#[derive(PartialEq, Eq)]
struct Stamp {
    free: u64,
    changed: u64,
    evicted: Option<SystemTime>,
}

static CHANGED: AtomicU64 = AtomicU64::new(0);
static LAST: Mutex<Option<(PathBuf, Seen, Stamp, Listing)>> = Mutex::new(None);

/// Refresh model metadata after completed, failed or canceled model work, not every progress tick.
pub(crate) fn changed() {
    CHANGED.fetch_add(1, Ordering::AcqRel);
}

/// A pull or import refreshes the final inventory even when it failed or was canceled.
pub(crate) struct ModelChange;

impl Drop for ModelChange {
    fn drop(&mut self) { changed(); }
}

/// Where the disk under `root` stands, in hundredths of it free.
fn moved(root: &Path) -> u64 {
    platform::durable::space(root).map_or(0, |space| space.free / (space.total / 100).max(1))
}

/// The store at `root`'s held models; none while it cannot be read.
pub(crate) fn listing(root: &Path) -> Listing {
    if !root.join("format").is_file() {
        return Listing::default();
    }
    let changed = CHANGED.load(Ordering::Acquire);
    let read = Store::open(root).map_err(storage::io_error).and_then(|store| Ok((seen(&store)?, store)));
    let (seen, store) = match read {
        Ok(read) => read,
        Err(error) => {
            tracing::warn!("held models unreadable: {error}");
            return Listing::default();
        }
    };
    let stamp = Stamp { free: moved(root), changed, evicted: store.evicted_at() };
    let mut last = LAST.lock().unwrap();
    if let Some((at, was, then, listing)) = last.as_ref() {
        if at == root && *was == seen && *then == stamp {
            return listing.clone();
        }
    }
    let listing = list(&store, &seen).unwrap_or_else(|error| {
        tracing::warn!("held models unreadable: {error}");
        Listing::default()
    });
    *last = Some((root.into(), seen, stamp, listing.clone()));
    listing
}

fn seen(store: &Store) -> std::io::Result<Seen> {
    let mut seen = Seen::new();
    for model in store.refs().map_err(storage::io_error)? {
        for (release, lane, tree) in entries(&model) {
            let Ok(tree) = store.tree(tree) else {
                continue;
            };
            let Ok((_, header)) = tree.header() else { continue };
            if store.holds(header) {
                let lane = lane.unwrap_or(&tree.lane);
                seen.push((model.name.clone(), release.into(), lane.into(), header.clone()));
            }
        }
    }
    seen.sort();
    Ok(seen)
}

fn list(store: &Store, seen: &Seen) -> std::io::Result<Listing> {
    let mut closures: BTreeMap<&str, Vec<ObjectRef>> = BTreeMap::new();
    let mut holders: HashMap<ObjectRef, usize> = HashMap::new();
    let mut models: Vec<v1::Model> = vec![];
    for (model, release, lane, header) in seen {
        let closure = store.closure(header).map_err(storage::io_error)?;
        let complete = closure.iter().all(|object| store.holds(object));
        let held: Vec<ObjectRef> = closure.into_iter().filter(|o| store.holds(o)).collect();
        let entry = closures.entry(model).or_default();
        for object in held {
            if !entry.contains(&object) {
                entry.push(object);
            }
        }
        if models.last().is_none_or(|m| m.model != *model) {
            models.push(v1::Model { model: model.clone(), ..Default::default() });
        }
        let (precision, role) = store.checkpoint(header).map(|c| (
            crate::precision::Profile::of(c.header()).family,
            c.header().model_role().to_owned(),
        )).unwrap_or_else(|_| (String::new(), "unknown".into()));
        models.last_mut().expect("pushed").checkpoints.push(v1::Checkpoint {
            release: release.clone(),
            lane: lane.clone(),
            precision,
            manifest: header.id(),
            role,
            complete: Some(complete),
        });
    }
    for objects in closures.values() {
        for object in objects {
            *holders.entry(object.clone()).or_default() += 1;
        }
    }
    for model in &mut models {
        let objects = &closures[model.model.as_str()];
        model.total_bytes = objects.iter().map(|o| o.length).sum();
        model.unique_bytes = objects.iter().filter(|o| holders[*o] == 1).map(|o| o.length).sum();
    }
    Ok(Listing { models, bytes: holders.keys().map(|o| o.length).sum() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tensorfs::format::{Header, Tree};

    #[test]
    fn header_only_partial_and_complete_downloads_are_distinct() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let first = ObjectRef::of(b"first weights");
        let second = ObjectRef::of(b"second weights");
        let header = Header::new(vec![first.clone(), second.clone()], None, vec![], Default::default()).unwrap();
        let header = store.put_bytes(&header.to_cbor().unwrap()).unwrap();
        let tree = store.put_tree(&Tree::of("bf16", header.clone(), &Default::default()).unwrap()).unwrap();
        store.update_ref("test/model", |_| {
            let mut model = Model::new("test/model");
            model.set_lane("1", "bf16", tree.clone());
            Some(model)
        }).unwrap();
        let before = listing(root.path());
        assert_eq!(before.models[0].checkpoints[0].complete, Some(false));
        assert_eq!(before.bytes, header.length);

        // A failed/canceled pull may have left useful bytes. Its exit invalidates the cache
        // even when the object is much smaller than the disk-percentage threshold.
        let interrupted = || -> Result<(), ()> {
            let _inventory = ModelChange;
            store.put_bytes(b"first weights").unwrap();
            Err(())
        };
        assert!(interrupted().is_err());
        let partial = listing(root.path());
        assert_eq!(partial.models[0].checkpoints[0].complete, Some(false));
        assert_eq!(partial.bytes, header.length + first.length);

        {
            let _inventory = ModelChange;
            store.put_bytes(b"second weights").unwrap();
        }
        let complete = listing(root.path());
        assert_eq!(complete.models[0].checkpoints[0].complete, Some(true));
        assert_eq!(complete.bytes, header.length + first.length + second.length);
    }
}
