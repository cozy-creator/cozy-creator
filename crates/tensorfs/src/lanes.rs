//! The machine's lane choice (design `lane-choice.md`): a release's lanes as its endpoint lists
//! them, or, with the Hub unreachable, as this store recorded them; each marked held where the
//! store has it whole; then ranked by [`precision::choose`].
use crate::err::{Code, Refusal, Result};
use crate::failure::{Failure, Kind};
use crate::fetch::Released;
use crate::format::{Header, ObjectRef, Repo};
use crate::precision::{self, Ask, Chosen, Gpus, Lane, Profile};
use crate::store::{wanted, Store};

/// `model`'s lane for `ask` on `gpus`, from its endpoint's `answer` for `release` (none: the
/// newest), or `None` to read local files only. Every lane of an answer the store holds whole is
/// recorded in its repo record, so an unreachable Hub (a network failure) falls back to them: the
/// held lanes of `release`, else of the newest release it holds one of, with a warning.
pub fn resolve(store: &Store, model: &str, release: Option<&str>, answer: Option<Result<Released>>, gpus: &Gpus, ask: &Ask) -> Result<Chosen> {
    let unreachable = match answer {
        Some(Ok(mut released)) => {
            for lane in &mut released.lanes {
                lane.held = false;
                read(store, lane);
            }
            if let Err(error) = record(store, model, &released) {
                tracing::warn!("{model}: its held lanes are not recorded for offline use: {error}");
            }
            return precision::choose(model, &released.release, &released.lanes, gpus, ask);
        }
        Some(Err(error)) if Failure::from(error.clone()).kind != Kind::Network => return Err(error),
        Some(Err(error)) => Some(error),
        None => None,
    };
    let released = recorded(store, model, release).ok().flatten()
        .unwrap_or_else(|| Released { release: release.unwrap_or_default().to_owned(), lanes: vec![] });
    let chosen = precision::choose(model, &released.release, &released.lanes, gpus, &Ask { offline: true, ..ask.clone() });
    match (chosen, unreachable) {
        (Ok(mut chosen), Some(_)) => {
            chosen.reasons.insert(0, format!("Hub unreachable: served cached {}; its release may have a better one", chosen.lane.label));
            Ok(chosen)
        }
        (Err(refused), Some(error)) => Err(Refusal { code: error.code, detail: format!("{} ({error})", refused.detail) }),
        (chosen, None) => chosen,
    }
}

/// `model`'s `release` (none: the newest it holds a lane of, else its newest) as the store
/// recorded it, each lane whose commit is here; a model with no release, its newest commit. None
/// when the store has no record of it.
pub(crate) fn recorded(store: &Store, model: &str, release: Option<&str>) -> Result<Option<Released>> {
    let Some(repo) = store.get_ref(model).map_err(crate::store::Error::refusal)? else { return Ok(None) };
    let lanes = |release: &crate::format::Release| -> Vec<Lane> {
        release.lanes.iter().filter_map(|(label, commit)| stored(store, label, commit)).collect()
    };
    if repo.releases.is_empty() && release.is_none() {
        let newest = repo.commits.last().and_then(|commit| stored(store, "", commit));
        return Ok(Some(Released { release: String::new(), lanes: newest.into_iter().collect() }));
    }
    let found = match release {
        Some(version) => repo.release(version).map(|r| (r, lanes(r))),
        None => repo.releases.iter().rev().map(|r| (r, lanes(r))).find(|(_, lanes)| lanes.iter().any(|l| l.held))
            .or_else(|| repo.releases.last().map(|r| (r, lanes(r)))),
    };
    let Some((found, lanes)) = found else {
        let has: Vec<&str> = repo.releases.iter().map(|r| r.version.as_str()).collect();
        return Err(Refusal { code: Code::REF_NOT_FOUND, detail: format!("{model} has no release {} (has {has:?})", release.unwrap_or_default()) });
    };
    Ok(Some(Released { release: found.version.clone(), lanes }))
}

/// Lane `label` (empty: its commit's own) of `commit` as the store has it; none without its tree.
fn stored(store: &Store, label: &str, commit: &ObjectRef) -> Option<Lane> {
    let tree = store.holds(commit).then(|| store.tree(commit).ok()).flatten()?;
    let header = tree.header().ok()?.1.clone();
    let label = if label.is_empty() { tree.lane.clone() } else { label.to_owned() };
    let mut lane = Lane { label, commit: commit.clone(), header, profile: Profile::default(), held: false };
    read(store, &mut lane);
    Some(lane)
}

/// With its header here: the lane's profile as this machine's table reads it, and held when its
/// commit, every chunk and every asset are here too. Nothing is marked used.
fn read(store: &Store, lane: &mut Lane) {
    let header = store.holds(&lane.header).then(|| store.read(&lane.header).ok()).flatten();
    let Some(header) = header.and_then(|bytes| Header::from_cbor(&bytes).ok()) else { return };
    lane.held = store.holds(&lane.commit) && wanted(&header, None).is_ok_and(|objects| objects.iter().all(|o| store.holds(o)));
    lane.profile = Profile::of(&header);
}

/// Records each held lane of `released` the repo record does not name yet.
fn record(store: &Store, model: &str, released: &Released) -> Result<()> {
    if released.release.is_empty() || model.starts_with("local/") {
        return Ok(());
    }
    let had = store.get_ref(model).map_err(crate::store::Error::refusal)?;
    let known = |lane: &Lane| had.as_ref().and_then(|repo| repo.release(&released.release)).is_some_and(|r| r.lanes.get(&lane.label) == Some(&lane.commit));
    let new: Vec<&Lane> = released.lanes.iter().filter(|lane| lane.held && !known(lane)).collect();
    if new.is_empty() {
        return Ok(());
    }
    store.update_ref(model, |old| {
        let mut repo = old.unwrap_or_else(|| Repo::new(model));
        for lane in &new {
            repo.set_lane(&released.release, &lane.label, lane.commit.clone());
        }
        Some(repo)
    }).map_err(crate::store::Error::refusal)?;
    Ok(())
}
