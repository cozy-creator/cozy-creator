//! What a caller expects to send this machine (cl-967, Paul 11-12): its upcoming jobs, nearest first (`PUT /v1/upcoming`), and what
//! to have ready with no job yet (`PUT /v1/prepare`): its guesses, as a browser fetches the page a person is still typing the
//! address of (cl-1104, Paul 11-16). Each item is a function's warm run of the machine's own, as a project's fetch ahead was: its code
//! installed, then what its call reads up front and its guesses while they fit, behind every run in the machine's lines, a prepare
//! behind every upcoming job and every run's guesses (`origin::priority`). A list is the caller's whole one: what it drops is
//! canceled. Their ids are kept out of every listing; their downloads show in Status as any, under these ids.
//!
//! A prepare item is no job (cl-1104): it never holds a rental or starts its idle clock again (`journal::Invocation::background`),
//! its build gives the install line to a run's (`line`), its download ends for a run's need that has to make room (`make_way`), and
//! what only it brought is the first the store drops (`brought`). A guess that changes is a set difference over the store's objects
//! (Paul: "I switch to another checkpoint, then cancel the parts of that download which are not needed, and start downloading the
//! new checkpoint instead"): a dropped item's run goes on until what replaced it is in the download line (`lined`), where an object
//! two want is one download, so what both read never stops; then it ends, and what only it wanted with it. What landed stays.
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, LazyLock, Mutex, MutexGuard, PoisonError,
    },
    time::Duration,
};

use api::{error::Error, machine as v1};

use crate::{api::auth::VerifiedActor, runs::Runs, stop::Stop};

/// The prefixes of the ids an upcoming job's and a prepare item's runs take.
pub(crate) const UPCOMING: &str = "cozy:upcoming:";
pub(crate) const PREPARE: &str = "cozy:prepare:";

/// A caller and a list's prefix.
type List = (String, &'static str);

/// Each caller's latest lists' run ids: what its Status says of them.
static LISTED: LazyLock<Mutex<HashMap<List, Vec<String>>>> = LazyLock::new(Mutex::default);

/// Items a list dropped whose runs go on until those that replaced them are in the download line or ended (`lined`).
static HANDED: LazyLock<Mutex<Vec<Handed>>> = LazyLock::new(Mutex::default);

struct Handed {
    /// Which `put` it is.
    put: u64,
    actor: VerifiedActor,
    dropped: Vec<String>,
    /// The items new with the list that dropped them, not yet in line.
    waiting: Vec<String>,
}

/// The guesses downloading now, each with its run's stop (`downloading`).
static DOWNLOADING: LazyLock<(Mutex<Vec<(u64, Stop)>>, Condvar)> = LazyLock::new(Default::default);

/// How long a run's need waits for the guesses it ended to let go of what they held.
const LET_GO: Duration = Duration::from_secs(5);

/// What each prepare item's run needs here, by its caller's id (`needs`): what its Status weighs it by once it ended (`standing`).
static NEEDS: LazyLock<Mutex<HashMap<String, Vec<crate::journal::FunctionRead>>>> = LazyLock::new(Mutex::default);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The run ids of `actor`'s latest lists.
pub(crate) fn listed(actor: &str) -> Vec<String> {
    let listed = lock(&LISTED);
    listed.iter().filter(|((owner, _), _)| owner == actor).flat_map(|(_, ids)| ids.clone()).collect()
}

/// Where `actor`'s item `id` stands among its lists: upcoming jobs before guesses, each list in its order. None: no list names it.
pub(crate) fn rank(actor: &str, id: &str) -> Option<(usize, usize)> {
    let listed = lock(&LISTED);
    [UPCOMING, PREPARE].into_iter().enumerate()
        .find_map(|(class, prefix)| Some((class, listed.get(&(actor.to_owned(), prefix))?.iter().position(|listed| listed == id)?)))
}

/// A run of a list's: never listed.
pub(crate) fn hidden(id: &str) -> bool {
    id.starts_with(UPCOMING) || id.starts_with(PREPARE)
}

/// A run of a prepare list's, by its caller's id: a guess, no job.
pub(crate) fn guess(id: &str) -> bool {
    id.starts_with(PREPARE)
}

/// Replaces `actor`'s list under `prefix` with `items`, in their order: each new one admitted as a warm run, each it drops canceled,
/// once what replaced it is in the download line when anything did.
pub(crate) fn put(runs: &Arc<Runs>, actor: VerifiedActor, prefix: &'static str, items: Vec<v1::Ahead>) -> Result<v1::AheadAnswer, Error> {
    let requests: Vec<v1::RunRequest> = (items.into_iter())
        .map(|item| {
            // Its Hub's transport for TensorFS too, as a run's spec carries it.
            let tensorfs = (item.hub.as_ref().filter(|hub| !hub.origin.is_empty()))
                .map(|hub| v1::TensorFSOptions { endpoint: hub.origin.clone(), ca_der: hub.ca_der.clone(), ..Default::default() });
            let spec = v1::RunSpec {
                kind: v1::RunKind::Warm,
                source: item.source,
                entrypoint: item.entrypoint,
                models: item.models,
                lane: item.lane,
                hub: item.hub,
                tensorfs,
                ..Default::default()
            };
            v1::RunRequest { id: format!("{prefix}{}", item.key), spec }
        })
        .collect();
    let ids: Vec<String> = requests.iter().map(|request| request.id.clone()).collect();
    let specs: Vec<v1::RunSpec> = requests.iter().map(|request| request.spec.clone()).collect();
    lock(&LISTED).insert((crate::runs::actor_id(actor), prefix), ids.clone());
    // What no list names now loads no more (cl-1123).
    if let Some(pool) = runs.service.pool() {
        pool.unlist(&crate::runs::actor_id(actor));
    }
    // What no list names any more is weighed no more.
    let named: Vec<String> = lock(&LISTED).values().flatten().cloned().collect();
    lock(&NEEDS).retain(|id, _| named.contains(id));
    let dropped: Vec<String> = runs.live_ids(actor)?.into_iter().filter(|id| id.starts_with(prefix) && !ids.contains(id)).collect();
    // Noted before any is admitted: one in line at once is heard.
    static PUTS: AtomicU64 = AtomicU64::new(0);
    let put = PUTS.fetch_add(1, Ordering::Relaxed);
    lock(&HANDED).push(Handed { put, actor, dropped, waiting: ids.clone() });
    let admitted = runs.accept(actor, requests);
    // Only an item new now takes over what a dropped one fetched: any other is in line already, or never will be.
    let new: Vec<&String> = match &admitted {
        Ok(admitted) => ids.iter().zip(admitted).filter(|(_, admitted)| matches!(admitted, Ok((true, _)))).map(|(id, _)| id).collect(),
        Err(_) => vec![],
    };
    settle(runs, |handed| {
        if let Some(handed) = handed.iter_mut().find(|handed| handed.put == put) {
            handed.waiting.retain(|id| new.contains(&id));
        }
    });
    // An item whose run made it ready before (before a restart, or before its list last named it) is planned again, so the
    // machine loads it as one sent now (cl-1123).
    if let (Ok(admitted), Some(pool)) = (&admitted, runs.service.pool()) {
        let signer = crate::runs::actor_id(actor);
        for ((id, spec), admitted) in ids.iter().zip(specs).zip(admitted) {
            if let Ok((false, state)) = admitted {
                if state.state == "succeeded" && pool.plans_again(&signer, id) {
                    runs.replan(actor, state.number.to_string(), spec);
                }
            }
        }
    }
    let items = (admitted?.into_iter())
        .map(|admitted| match admitted {
            Ok((created, state)) => v1::Admitted { created, state: Some(state), refusal: None },
            Err(error) => v1::Admitted { refusal: Some(error), ..Default::default() },
        })
        .collect();
    Ok(v1::AheadAnswer { items })
}

/// The run its caller names `id` has what it reads in the download line, or ended: what a list dropped for it and its like ends now.
pub(crate) fn lined(runs: &Runs, id: &str) {
    if hidden(id) {
        settle(runs, |handed| handed.iter_mut().for_each(|handed| handed.waiting.retain(|waiting| waiting != id)));
    }
}

/// `change` made to what is handed over, then each handover nothing new is still to line for ends what it dropped.
fn settle(runs: &Runs, change: impl FnOnce(&mut Vec<Handed>)) {
    let done: Vec<Handed> = {
        let mut handed = lock(&HANDED);
        change(&mut handed);
        let (done, waiting): (Vec<Handed>, Vec<Handed>) = std::mem::take(&mut *handed).into_iter().partition(|handed| handed.waiting.is_empty());
        *handed = waiting;
        done
    };
    for Handed { actor, dropped, .. } in done.into_iter().filter(|handed| !handed.dropped.is_empty()) {
        // One that ended meanwhile is left as it ended.
        let live = runs.live_ids(actor).unwrap_or_default();
        for id in dropped.iter().filter(|id| live.contains(id)) {
            if let Err(error) = runs.cancel(actor, id) {
                tracing::warn!("a list's dropped item {id} wasn't canceled: {}", error.message);
            }
        }
    }
}

/// A guess's download, for as long as this is held: a run's need that has to make room ends it (`make_way`).
pub(crate) struct Downloading(u64);

/// The guess whose run `stop` ends downloads now.
pub(crate) fn downloading(stop: &Stop) -> Downloading {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    lock(&DOWNLOADING.0).push((id, stop.clone()));
    Downloading(id)
}

impl Drop for Downloading {
    fn drop(&mut self) {
        lock(&DOWNLOADING.0).retain(|(id, _)| *id != self.0);
        DOWNLOADING.1.notify_all();
    }
}

/// A run's need has to make room: every guess downloading ends, and this returns once each has let go of the checkpoints it held
/// (or `LET_GO` passed), so the store may drop what they brought. A guess never keeps a run's bytes off the disk.
pub(crate) fn make_way() {
    let downloading = lock(&DOWNLOADING.0);
    let ended: Vec<u64> = downloading.iter().map(|(id, _)| *id).collect();
    if ended.is_empty() {
        return;
    }
    let stops: Vec<Stop> = downloading.iter().map(|(_, stop)| stop.clone()).collect();
    drop(downloading);
    stops.iter().for_each(Stop::cancel);
    tracing::info!("{} guesses' downloads ended for a run that needs their room", stops.len());
    let held = |downloading: &mut Vec<(u64, Stop)>| downloading.iter().any(|(id, _)| ended.contains(id));
    drop(DOWNLOADING.1.wait_timeout_while(lock(&DOWNLOADING.0), LET_GO, held).unwrap_or_else(PoisonError::into_inner));
}

/// Run `caller` lines what it needs (`needed`): for a prepare item, what its Status weighs it by (`standing`).
pub(crate) fn needs(store: &crate::storage::Store, caller: &str, needed: &crate::access::Selected) {
    if guess(caller) {
        lock(&NEEDS).insert(caller.to_owned(), crate::access::reads(store, needed));
    }
}

/// The list items' runs `states` as their caller's Status says them. A prepare item whose run succeeded or was ended for a run's room
/// (`make_way`) but lacks some of what it needs here now is `evicted` (cl-1104): no longer ready, and sent again under a new key its
/// machine fetches it anew. `waiting` says why not yet: a run here goes first (`busy`), or what it lacks doesn't fit on the disk past
/// its reserve, as a guess never makes room. Empty: it may come again now.
pub(crate) fn standing(store: &crate::storage::Store, busy: bool, mut states: Vec<v1::RunState>) -> Vec<v1::RunState> {
    let needs = lock(&NEEDS);
    let free = || crate::reclaim::Disk::measure(store.root()).map_or(u64::MAX, |disk| disk.free());
    for state in states.iter_mut().filter(|state| guess(&state.id) && matches!(state.state.as_str(), "succeeded" | "canceled")) {
        let Some(reads) = needs.get(&state.id) else { continue };
        let missing: u64 = reads.iter().map(|read| crate::access::missing(store, read)).sum();
        if missing == 0 {
            continue;
        }
        state.waiting = match (busy, free()) {
            (true, _) => "the runs here go first".to_owned(),
            (false, free) if missing > free => format!("room on the disk for {}", crate::reclaim::size(missing)),
            _ => String::new(),
        };
        state.state = "evicted".to_owned();
    }
    states
}

/// The items `states` as their caller `actor` reads them: one whose run made it ready (its weights here) says how far the load
/// step took it (`gpu_service::Pool::readiness`, cl-1123).
pub(crate) fn readiness(pool: Option<&crate::gpu_service::Pool>, actor: &str, mut states: Vec<v1::RunState>) -> Vec<v1::RunState> {
    for state in states.iter_mut().filter(|state| state.state == "succeeded") {
        state.readiness = pool.and_then(|pool| pool.readiness(actor, &state.id));
    }
    states
}

/// `selected`'s checkpoints (header, components) came for a guess when nothing of them was here (`guessed`), or a call reads them
/// now: what only a guess brought is the first the store drops on a short disk (`tensorfs::store::Store::guess`).
pub(crate) fn brought(store: &crate::storage::Store, selected: &[(String, Vec<String>)], guessed: bool) {
    for (header, _) in selected {
        let digest = header.trim_start_matches("sha256:");
        if guessed {
            let held = crate::storage::checkpoint(store, header).is_ok_and(|checkpoint| crate::storage::chunks(&checkpoint, &[]).iter().any(|object| store.holds(object)));
            if held {
                continue;
            }
        }
        // One named by no digest (a folder's) is no guess's.
        if let (Err(error), true) = (store.guess(digest, guessed), guessed) {
            tracing::warn!("{header}: that a guess brought it wasn't noted: {error}");
        }
    }
}
