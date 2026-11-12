//! A deleted box is gone (cl-919; Paul, 11-12: "when you delete a picture it should be _gone_ ... deleted permanently within a few hours").
//! Nothing resolves its name, and Undo brings it back for `UNDO_WINDOW`. Then its files leave the project for good: from `files/`, and
//! Calcifer's thumbnails and clips of them, unless a box still there holds the same file. Calcifer does it at the window's end whether or not
//! anything has the project open, and a Calcifer that was stopped then does it as it starts. An Undo after that says it can't, and brings
//! back nothing (`history`).
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use api::error::Error;
use tokio::sync::Notify;
use tracing::warn;

use super::{
    Doc, cut_name,
    folder::{Folder, Lock, TOMBSTONE},
    history, now_ms, vault,
};
use crate::api::Api;

/// How long Undo can bring a deleted box back.
pub(crate) const UNDO_WINDOW: Duration = Duration::from_secs(3 * 60 * 60);
/// How long a project that can't be read now (its vault locked, its drive away) waits for its next try.
const AGAIN: Duration = Duration::from_secs(10 * 60);

static WINDOW: OnceLock<Duration> = OnceLock::new();
static WAKE: Notify = Notify::const_new();
/// When each project's deleted boxes are next due, by id. In memory only: the global index keeps just the project reference, and a
/// start looks at every project it knows once.
static DUE: Mutex<BTreeMap<String, i64>> = Mutex::new(BTreeMap::new());

/// The window this Calcifer keeps: `calcifer.undo_seconds` (tests only), else `UNDO_WINDOW`.
pub(crate) fn set_window(seconds: Option<u32>) {
    let _ = WINDOW.set(seconds.filter(|seconds| *seconds > 0).map_or(UNDO_WINDOW, |seconds| Duration::from_secs(seconds.into())));
}

pub(crate) fn window_ms() -> i64 {
    WINDOW.get().copied().unwrap_or(UNDO_WINDOW).as_millis() as i64
}

/// The window as a person says it: `3 hours`, `40 seconds`.
pub(crate) fn window_said() -> String {
    let seconds = window_ms() / 1000;
    let (n, unit) = match seconds {
        s if s % 3600 == 0 => (s / 3600, "hour"),
        s if s % 60 == 0 => (s / 60, "minute"),
        s => (s, "second"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// Project `id` has deleted boxes whose files may go at `at_ms`: Calcifer looks then.
pub(crate) fn due(id: &str, at_ms: i64) {
    let mut due = DUE.lock().expect("purge due lock");
    let at = due.entry(id.to_owned()).or_insert(at_ms);
    *at = (*at).min(at_ms);
    drop(due);
    WAKE.notify_one();
}

/// `id` purged as it was due at `was_ms`, next due at `next` if any: a delete since, due sooner, stands.
fn settled(id: &str, was_ms: i64, next: Option<i64>) {
    let mut due = DUE.lock().expect("purge due lock");
    let kept = match due.get(id).copied() {
        Some(at) if at != was_ms => Some(next.map_or(at, |next| at.min(next))),
        _ => next,
    };
    match kept {
        Some(at) => due.insert(id.to_owned(), at),
        None => due.remove(id),
    };
}

/// Calcifer's purger: each project whose deleted boxes' windows have ended, purged, in the background, then asleep until the next is due.
/// A Calcifer that was stopped catches up as it starts.
pub(crate) fn start(api: Arc<Api>) {
    let now = now_ms();
    for project in api.calcifer.store().known_projects().unwrap_or_default() {
        due(&project.id, now);
    }
    tokio::spawn(async move {
        loop {
            let now = now_ms();
            let ready: Vec<(String, i64)> = DUE.lock().expect("purge due lock").iter().filter(|(_, at)| **at <= now).map(|(id, at)| (id.clone(), *at)).collect();
            for (id, was) in ready {
                let next = match api.calcifer.store().known_project(&id) {
                    Ok(Some(project)) => {
                        let (held, path) = (api.clone(), project.path);
                        tokio::task::spawn_blocking(move || purged(&held, &path)).await.unwrap_or(Some(now + AGAIN.as_millis() as i64))
                    }
                    Ok(None) => None,
                    Err(_) => Some(now + AGAIN.as_millis() as i64),
                };
                settled(&id, was, next);
            }
            let next = DUE.lock().expect("purge due lock").values().min().copied();
            let wait = next.map_or(Duration::from_secs(24 * 60 * 60), |at| Duration::from_millis((at - now_ms()).max(0) as u64));
            let _ = tokio::time::timeout(wait, WAKE.notified()).await;
        }
    });
}

/// The project at `path` purged: when its next deleted box is due, if any is. Its folder gone, none; unreadable now, a try later.
fn purged(api: &Api, path: &Path) -> Option<i64> {
    if !vault::logical(path).exists() {
        return None;
    }
    let again = || Some(now_ms() + AGAIN.as_millis() as i64);
    // Held as any reader's: a lock or a move of the project waits for it, and one going now means later.
    let Ok(_access) = vault::try_admit_root(path) else { return again() };
    let Ok(dir) = vault::resolve_path(path) else { return again() };
    let home = api.machines.hubs.home();
    let caches = [home.thumbs(), home.root().join("cache").join("clips")];
    match purge(&Folder::at(&dir), now_ms(), &caches) {
        Ok(next) => next,
        Err(error) => {
            warn!(code = %error.code, "a project's deleted boxes' files stay for now");
            again()
        }
    }
}

/// `folder`'s deleted boxes whose undo window ended by `now`, gone for good: each file of theirs that no box still there, nothing on its
/// way, no run's output and no box Undo can still bring back holds, from `files/`, its record and the `caches` named by its hash, and a
/// cut's film at the root. A box keeps its name, its id and when, so an Undo of it says why it can't, until the history no longer reaches
/// it. One deleted before cl-919, which says no time, goes now. When the next is due, if any is.
pub(super) fn purge(folder: &Folder, now: i64, caches: &[PathBuf]) -> Result<Option<i64>, Error> {
    let window = window_ms();
    // Held throughout: a file kept meanwhile isn't deleted.
    let _collecting = folder.collecting();
    let lock = folder.lock()?;
    let set_aside = |key: &str| key.contains(TOMBSTONE);
    let ended = |held: &super::folder::Asset| held.files().next().is_some() && (held.deleted_ms.is_none() || held.lapsed(now));
    let gone: Vec<String> = lock.assets.iter().filter(|(key, held)| set_aside(key) && ended(held)).map(|(key, _)| key.clone()).collect();
    // Its history read only for a box already emptied: a start looks at every project.
    let emptied = |held: &super::folder::Asset| held.files().next().is_none() && held.deleted_ms.is_some();
    let oldest = match lock.assets.iter().any(|(key, held)| set_aside(key) && emptied(held)) {
        true => history::oldest_ms(folder).unwrap_or(i64::MAX),
        false => i64::MAX,
    };
    let forgotten = |held: &super::folder::Asset| emptied(held) && held.deleted_ms.is_some_and(|at| at < oldest);
    let stubs = lock.assets.iter().any(|(key, held)| set_aside(key) && forgotten(held));
    // A box deleted before cl-919, said by a run's row of its file rather than its own.
    let legacy = |row: &super::folder::RunFile| row.run == 0 && row.output == "deleted";
    if gone.is_empty() && !stubs && !lock.runs.iter().any(legacy) {
        return Ok(next_due(&lock, window));
    }
    let doc = Doc::load(folder)?;
    let kept = kept(&doc, &lock, &gone, now, legacy);
    let freed: BTreeSet<String> = gone.iter().flat_map(|key| lock.assets[key].files().cloned())
        .chain(lock.runs.iter().filter(|row| legacy(row)).map(|row| row.sha256.clone()))
        .filter(|sha256| !kept.contains(sha256))
        .collect();
    let paths: Vec<PathBuf> = freed.iter().filter_map(|sha256| lock.files.get(sha256)).map(|file| folder.path(&file.path)).collect();
    let films: Vec<String> = gone.iter().map(|key| &lock.assets[key]).filter(|held| held.versions.iter().any(|version| !version.clips.is_empty())).map(|held| cut_name(&held.name)).collect();
    folder.update_lock(|lock| {
        for (key, held) in lock.assets.iter_mut().filter(|(key, _)| set_aside(key)) {
            if gone.contains(key) {
                held.versions.clear();
                held.discarded.clear();
                held.held = None;
                held.deleted_ms.get_or_insert(now);
            }
        }
        lock.assets.retain(|key, held| !(set_aside(key) && forgotten(held)));
        lock.runs.retain(|row| !legacy(row));
        lock.files.retain(|sha256, _| !freed.contains(sha256));
    })?;
    // What's made of them first, so once a file is gone, all of it is.
    forget_derived(&folder.dir, &freed, caches);
    for path in &paths {
        if let Err(error) = std::fs::remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            warn!(kind = %error.kind(), "a deleted box's file stays");
        }
    }
    for name in &films {
        for film in [folder.dir.join(name), folder.dir.join(Path::new(name).with_extension("otio"))] {
            let _ = std::fs::remove_file(film);
        }
    }
    Ok(next_due(&folder.lock()?, window))
}

/// Every file something still in the project holds: its boxes' versions, discarded too, their pins and a cut's clips, what's on its way,
/// a run's outputs, and what a box Undo can still bring back holds.
fn kept(doc: &Doc, lock: &Lock, gone: &[String], now: i64, legacy: impl Fn(&super::folder::RunFile) -> bool) -> BTreeSet<String> {
    let live = |key: &str| doc.find(key).is_some();
    let mut kept: BTreeSet<String> = lock.named(live).into_iter().map(str::to_owned).collect();
    kept.extend(doc.assets.iter().filter_map(|asset| asset.pin.clone()));
    for (key, held) in &lock.assets {
        let undoable = key.contains(TOMBSTONE) && !gone.contains(key) && !held.lapsed(now);
        if live(key) || undoable {
            kept.extend(held.files().cloned());
            kept.extend(held.on_record().flat_map(|version| &version.clips).filter_map(|clip| clip.sha256.clone()));
        }
    }
    kept.extend(lock.runs.iter().filter(|row| !legacy(row)).map(|row| row.sha256.clone()));
    kept
}

/// When the next box set aside in `lock` is due to go.
fn next_due(lock: &Lock, window: i64) -> Option<i64> {
    (lock.assets.iter()).filter(|(key, held)| key.contains(TOMBSTONE) && held.files().next().is_some()).filter_map(|(_, held)| held.deleted_ms).map(|at| at + window).min()
}

/// What Calcifer made of each of `freed` and keeps by its hash, gone: thumbnails, posters and previews (`<hex>-…`), and clips (`<hex>.…`),
/// in its own caches and a private project's.
fn forget_derived(dir: &Path, freed: &BTreeSet<String>, caches: &[PathBuf]) {
    if freed.is_empty() {
        return;
    }
    let scope = crate::private_io::of_path(dir).ok().flatten();
    let private = scope.iter().flat_map(|scope| {
        [("thumbs", 0), ("clips", 1)].into_iter().filter_map(|(name, at)| crate::private_io::cache(Some(scope), caches.get(at)?.clone(), name).ok())
    });
    let dirs: Vec<PathBuf> = caches.iter().cloned().chain(private).collect();
    let hexes: Vec<&str> = freed.iter().map(|sha256| sha256.trim_start_matches("sha256:")).collect();
    for cache in dirs {
        for entry in std::fs::read_dir(&cache).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if hexes.iter().any(|hex| name.strip_prefix(hex).is_some_and(|rest| rest.starts_with(['-', '.']))) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}
