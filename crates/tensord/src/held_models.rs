//! The models this machine's store holds, for Status: each local model (`local/<name>`) and
//! each Hub checkpoint it pulled (cached Hub refs), with what it weighs. A listing is read
//! again only when the set of models changed, not when one was merely used.
use crate::api::v1;
use crate::storage::{self, Store};
use std::{
    collections::{BTreeMap, HashMap},
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tensorfs::format::{ObjectRef, Repo};

#[derive(Clone, Default, PartialEq, Debug)]
pub struct Listing {
    pub models: Vec<v1::Model>,
    /// Every held model's bytes together, shared content counted once.
    pub bytes: u64,
}

/// A Hub commit this machine pulled: `tree`, `repository`'s `release` in `lane` (an empty
/// release: a commit by hash). The store's ref of the repository records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pulled {
    pub repository: String,
    pub release: String,
    pub lane: String,
    pub tree: ObjectRef,
}

impl Pulled {
    /// Names the pull in the store: `repository`'s ref, a cache TensorFS may expire. The tree
    /// must be here. One the ref already names writes nothing.
    pub fn record(&self, store: &Store) -> io::Result<()> {
        let held = store.get_ref(&self.repository).map_err(storage::io_error)?;
        if held.as_ref().is_some_and(|repo| entries(repo).any(|(release, _, tree)| release == self.release && *tree == self.tree)) {
            return Ok(());
        }
        store
            .update_ref(&self.repository, |old| {
                let mut repo = old.unwrap_or_else(|| Repo::new(&self.repository));
                match self.release.is_empty() {
                    true => repo.add_commit(self.tree.clone()),
                    false => repo.set_lane(&self.release, &self.lane, self.tree.clone()),
                }
                Some(repo)
            })
            .map(drop)
            .map_err(storage::io_error)
    }

    /// The Hub commit whose header is `header`, as this machine pulled it; None for one never
    /// pulled.
    pub fn of(store: &Store, header: &ObjectRef) -> io::Result<Option<Pulled>> {
        for repo in store.refs().map_err(storage::io_error)? {
            if repo.name.starts_with("local/") {
                continue;
            }
            for (release, lane, tree) in entries(&repo) {
                let Ok(parsed) = store.tree(tree) else {
                    continue;
                };
                if parsed.header().is_ok_and(|(_, h)| h == header) {
                    let lane = lane.map_or(parsed.lane, str::to_owned);
                    let (repository, release) = (repo.name.clone(), release.to_string());
                    return Ok(Some(Pulled { repository, release, lane, tree: tree.clone() }));
                }
            }
        }
        Ok(None)
    }
}

/// Each commit `repo` records: (its release, empty for none; its lane there; the commit).
fn entries(repo: &Repo) -> impl Iterator<Item = (&str, Option<&str>, &ObjectRef)> {
    let released = repo.releases.iter().flat_map(|r| r.lanes.iter().map(|(lane, tree)| (r.version.as_str(), Some(lane.as_str()), tree)));
    released.chain(repo.commits.iter().map(|tree| ("", None, tree)))
}

/// Each model's rows: repository, release, lane, header.
type Seen = Vec<(String, String, String, ObjectRef)>;

static LAST: Mutex<Option<(PathBuf, Seen, Listing)>> = Mutex::new(None);

/// The store at `root`'s held models; none while it cannot be read.
pub fn listing(root: &Path) -> Listing {
    if !root.join("format").is_file() {
        return Listing::default();
    }
    let read = Store::open(root).map_err(storage::io_error).and_then(|store| Ok((seen(&store)?, store)));
    let (seen, store) = match read {
        Ok(read) => read,
        Err(error) => {
            tracing::warn!("held models unreadable: {error}");
            return Listing::default();
        }
    };
    let mut last = LAST.lock().unwrap();
    if let Some((at, was, listing)) = last.as_ref() {
        if at == root && *was == seen {
            return listing.clone();
        }
    }
    let listing = list(&store, &seen).unwrap_or_else(|error| {
        tracing::warn!("held models unreadable: {error}");
        Listing::default()
    });
    *last = Some((root.into(), seen, listing.clone()));
    listing
}

fn seen(store: &Store) -> std::io::Result<Seen> {
    let mut seen = Seen::new();
    for repo in store.refs().map_err(storage::io_error)? {
        for (release, lane, tree) in entries(&repo) {
            let Ok(tree) = store.tree(tree) else {
                continue;
            };
            let Ok((_, header)) = tree.header() else { continue };
            if storage::present(store, header) {
                let lane = lane.unwrap_or(&tree.lane);
                seen.push((repo.name.clone(), release.into(), lane.into(), header.clone()));
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
    for (repository, release, lane, header) in seen {
        let closure = store.closure(header).map_err(storage::io_error)?;
        let held: Vec<ObjectRef> = closure.into_iter().filter(|o| storage::present(store, o)).collect();
        let entry = closures.entry(repository).or_default();
        for object in held {
            if !entry.contains(&object) {
                entry.push(object);
            }
        }
        if models.last().is_none_or(|m| m.repository != *repository) {
            models.push(v1::Model { repository: repository.clone(), ..Default::default() });
        }
        let precision = store.checkpoint(header).map(|c| crate::precision::Profile::of(c.header()).family).unwrap_or_default();
        models.last_mut().expect("pushed").checkpoints.push(v1::Checkpoint {
            release: release.clone(),
            lane: lane.clone(),
            precision,
            manifest: header.id(),
        });
    }
    for objects in closures.values() {
        for object in objects {
            *holders.entry(object.clone()).or_default() += 1;
        }
    }
    for model in &mut models {
        let objects = &closures[model.repository.as_str()];
        model.total_bytes = objects.iter().map(|o| o.length).sum();
        model.unique_bytes = objects.iter().filter(|o| holders[*o] == 1).map(|o| o.length).sum();
    }
    Ok(Listing { models, bytes: holders.keys().map(|o| o.length).sum() })
}
