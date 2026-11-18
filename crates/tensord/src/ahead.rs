//! What a caller expects to send this machine (cl-967, Paul 11-12): its upcoming jobs, nearest first (`PUT /v1/upcoming`), and what
//! to have ready with no job yet (`PUT /v1/prepare`): its guesses, as a browser fetches the page a person is still typing the
//! address of (cl-1104, Paul 11-16). Each item is a request the caller predicts (cl-1123; Paul, 11-17: "the remote machine only needs
//! to worry about what it's told, what a request MAY look like, to ensure it's ready for it"), made ready by the machine's own warm
//! run of it: planned as that request's call would be (its function's needs rules on its payload, its lane for these GPUs, this
//! store), its code installed, what it needs downloaded, behind every run in the machine's lines, a prepare behind every upcoming job
//! and every run's guesses (`origin::priority`). A list is the caller's whole one: what it drops is canceled. Their ids are kept out
//! of every listing; their readiness shows in Status (`readiness`), their downloads as any, under these ids. A call later sent as
//! the same request (`RunSpec::predicted`, its identity the item's) is that request (`adopts`).
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
    time::{Duration, Instant},
};

use api::{error::Error, machine as v1};

use crate::{api::auth::VerifiedActor, runs::Runs, stop::Stop};

/// The prefixes of the ids an upcoming job's and a prepare item's runs take.
pub(crate) const UPCOMING: &str = "cozy:upcoming:";
pub(crate) const PREPARE: &str = "cozy:prepare:";
/// The prefix of a speculative run's id: a guess run as if it were sent (cl-1123 D, `speculate`), `<prefix><key>.<attempt>`.
pub(crate) const SPECULATIVE: &str = "cozy:speculative:";

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

/// The listed items whose runs have what they read in the download line (`lined`): a dropped one that isn't holds nothing a
/// replacement could share there yet, so it ends at once (cl-1281: a swapped LoRA's item, still fetching its LoRA while it planned,
/// went on 16.5 s until its replacement's own LoRA had landed, and its 1.83 GB landed for nothing, 44394166400).
static LINED: LazyLock<Mutex<std::collections::HashSet<String>>> = LazyLock::new(Mutex::default);

/// The guesses downloading now, each with its run's stop (`downloading`).
static DOWNLOADING: LazyLock<(Mutex<Vec<(u64, Stop)>>, Condvar)> = LazyLock::new(Default::default);

/// How long a run's need waits for the guesses it ended to let go of what they held.
const LET_GO: Duration = Duration::from_secs(5);

/// What each prepare item's run needs here, by its caller's id (`needs`): what its Status weighs it by once it ended (`standing`).
static NEEDS: LazyLock<Mutex<HashMap<String, Vec<crate::journal::FunctionRead>>>> = LazyLock::new(Mutex::default);

/// Each listed item's local code's interface as its caller read it, by its run's id (`v1::Predicted::interface`): what its slots are
/// resolved by while its code installs (cl-1123).
static INTERFACES: LazyLock<Mutex<HashMap<String, Vec<u8>>>> = LazyLock::new(Mutex::default);

/// List item `id`'s local code's interface as its caller read it, if it said one.
pub(crate) fn interface(id: &str) -> Option<Vec<u8>> {
    lock(&INTERFACES).get(id).cloned()
}

/// Each listed item's request, by its run's id: its identity as a call of it would have it (`runs::identity`).
static PREDICTED: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Mutex::default);

/// What each listed item's identity is of (`runs::identity_value`), by its run's id: to say what a call naming it differs in.
static IDENTIFIED: LazyLock<Mutex<HashMap<String, serde_json::Value>>> = LazyLock::new(Mutex::default);

/// Items the lists dropped, and when: a render of one is sent as its list drops it (the render takes its box's draft seed, so the box
/// is guessed anew), and may reach this machine after the list does. Its request is kept `ADOPTABLE` for that call to adopt.
static DROPPED: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Mutex::default);

/// How long a dropped item's request may still be adopted.
const ADOPTABLE: Duration = Duration::from_secs(120);

/// Each listed item's extras still coming once its needs landed (`extras`), by its run's id: a list that drops it ends them.
static EXTRAS: LazyLock<Mutex<HashMap<String, Stop>>> = LazyLock::new(Mutex::default);

/// Each list's newest put, by caller and list (`put`): an older one its admission hasn't reached is never admitted.
static PUTS: LazyLock<Mutex<HashMap<List, u64>>> = LazyLock::new(Mutex::default);

/// Listed items not yet admitted, by their run's id, and why its admission refused one (`admit`): their Status until their runs say.
static PLANNING: LazyLock<Mutex<HashMap<String, Option<String>>>> = LazyLock::new(Mutex::default);

/// Each listed guess's request as it was sent (inputs and all), by its run's id, its caller, and since when the lists have named it
/// (`drafts`): what a speculative run runs (cl-1123 D).
static DRAFTS: LazyLock<Mutex<HashMap<String, Draft>>> = LazyLock::new(Mutex::default);

#[derive(Clone)]
pub(crate) struct Draft {
    pub actor: VerifiedActor,
    pub key: String,
    pub spec: v1::RunSpec,
    /// Since when it has stood unchanged: a draft edited is another item, under another key.
    pub since: std::time::Instant,
}

/// Each guess the lists name now, its request as sent, in the lists' order: what a speculative run may run (`speculate`).
pub(crate) fn drafts() -> Vec<(String, Draft)> {
    let drafts = lock(&DRAFTS);
    let mut named: Vec<(String, Draft)> = drafts.iter().map(|(id, draft)| (id.clone(), draft.clone())).collect();
    named.sort_by_key(|(id, draft)| rank(&crate::runs::actor_id(draft.actor), id));
    named
}

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

/// Run `id`, sent naming list item `key` (`RunSpec::predicted`, cl-1123), is the request that item predicted: its identity
/// (`digest`) is the item's. Whether it is; said in the log either way.
pub(crate) fn adopts(key: &str, id: &str, digest: &str) -> bool {
    let predicted = lock(&PREDICTED);
    let item = [UPCOMING, PREPARE].iter().map(|prefix| format!("{prefix}{key}")).find(|item| predicted.contains_key(item));
    let Some(item) = item else {
        tracing::info!("run {id} names the predicted request {key}, which no list holds now");
        return false;
    };
    let same = predicted.get(&item).is_some_and(|held| !held.is_empty() && held == digest);
    match same {
        true => tracing::info!("run {id} is the request {item} predicted, and takes what it made ready"),
        false => tracing::info!("run {id} names {item}, but isn't the request it predicted"),
    }
    same
}

/// Run `id` names list item `key` but isn't its request (`adopts`): the fields of what it is that differ, by name, the payload's by
/// its own (no value is said), so the two can be made one.
pub(crate) fn differs(key: &str, id: &str, asked: Option<serde_json::Value>) {
    let item = [UPCOMING, PREPARE].iter().find_map(|prefix| lock(&IDENTIFIED).get(&format!("{prefix}{key}")).cloned());
    let (Some(item), Some(asked)) = (item, asked) else { return };
    let (Some(held), Some(sent)) = (item.as_object(), asked.as_object()) else { return };
    let keys = |a: &serde_json::Map<String, serde_json::Value>, b: &serde_json::Map<String, serde_json::Value>| -> std::collections::BTreeSet<String> {
        a.keys().chain(b.keys()).cloned().collect()
    };
    let mut fields = vec![];
    for name in keys(held, sent).into_iter().filter(|name| held.get(name) != sent.get(name)) {
        match (held.get(&name).and_then(serde_json::Value::as_object), sent.get(&name).and_then(serde_json::Value::as_object)) {
            (Some(a), Some(b)) if name == "payload" => {
                fields.extend(keys(a, b).into_iter().filter(|field| a.get(field) != b.get(field)).map(|field| format!("payload.{field}")));
            }
            _ => fields.push(name),
        }
    }
    tracing::info!("run {id} differs from item {key}'s request in {fields:?}");
}

/// Run `caller`'s needs (Y) landed (`Runs::download`): a list item is `ready` from here, its extras (Z) still coming behind
/// (cl-1123). Its run returns now, and its load is registered as it does (`gpu_service::Pool::predict`, from `Runs::prepare`).
pub(crate) fn landed(caller: &str) {
    if hidden(caller) {
        tracing::info!("{caller}: what it needs is here");
    }
}

/// What ends run `caller`'s extras once its needs landed: for a list item, its list dropping it; for any other run, nothing but their
/// landing or a need that has to make room.
pub(crate) fn extras(caller: &str) -> Stop {
    let stop = Stop::default();
    if hidden(caller) {
        lock(&EXTRAS).insert(caller.to_owned(), stop.clone());
    }
    stop
}

/// A run of a list's: never listed.
pub(crate) fn hidden(id: &str) -> bool {
    id.starts_with(UPCOMING) || id.starts_with(PREPARE) || speculative(id)
}

/// A run of a list item's (an upcoming job's, a guess's): one the machine makes ready, and loads (cl-1123).
pub(crate) fn item(id: &str) -> bool {
    id.starts_with(UPCOMING) || id.starts_with(PREPARE)
}

/// A speculative run's (`speculate`): no job, never listed, until a call adopts it.
pub(crate) fn speculative(id: &str) -> bool {
    id.starts_with(SPECULATIVE)
}

/// A run of a prepare list's, by its caller's id: a guess, no job.
pub(crate) fn guess(id: &str) -> bool {
    id.starts_with(PREPARE)
}

/// A list put, answered at once (cl-1123: 44082666492's took 1.27 s, most of an edit's way to its first byte): what it names and drops
/// is noted now, and its items are admitted behind the answer (`admit`), `planning` in Status until they are.
pub(crate) fn put(
    runs: &Arc<Runs>,
    actor: VerifiedActor,
    prefix: &'static str,
    items: Vec<v1::Predicted>,
    admission: crate::machine::lifecycle::Admission,
) -> Result<(), Error> {
    let mut identities = vec![];
    let mut sent = vec![];
    let requests: Vec<v1::RunRequest> = (items.into_iter())
        .map(|v1::Predicted { key, spec, interface }| {
            let id = format!("{prefix}{key}");
            if let Some(interface) = interface {
                lock(&INTERFACES).insert(id.clone(), interface.get().as_bytes().to_vec());
            }
            identities.push((id.clone(), crate::runs::identity(spec.clone()).unwrap_or_default()));
            if let Some(value) = crate::runs::identity_value(spec.clone()) {
                lock(&IDENTIFIED).insert(id.clone(), value);
            }
            // A guess of the plain namespace may be run as sent, once loaded and still (`speculate`).
            if prefix == PREPARE && crate::private_io::current().is_none() {
                sent.push((id.clone(), Draft { actor, key: key.clone(), spec: spec.clone(), since: std::time::Instant::now() }));
            }
            // Its Hub's transport for TensorFS too, as a run's spec carries it.
            let tensorfs = spec.tensorfs.clone().or_else(|| {
                (spec.hub.as_ref().filter(|hub| !hub.origin.is_empty()))
                    .map(|hub| v1::TensorFSOptions { endpoint: hub.origin.clone(), ca_der: hub.ca_der.clone(), ..Default::default() })
            });
            // The request, made ready and never run: its payload kept, which its function's rules read as its call's; its files are
            // its call's to bring, as nothing here reads them.
            let spec = v1::RunSpec { kind: v1::RunKind::Warm, inputs: vec![], predicted: None, tensorfs, ..spec };
            v1::RunRequest { id, spec }
        })
        .collect();
    let ids: Vec<String> = requests.iter().map(|request| request.id.clone()).collect();
    let list = (crate::runs::actor_id(actor), prefix);
    let before = lock(&LISTED).insert(list.clone(), ids.clone()).unwrap_or_default();
    // A draft that stays keeps the time it was first named; one edited is another item, and its speculative run ends (cl-1123 D).
    {
        let mut drafts = lock(&DRAFTS);
        let gone: Vec<Draft> = before.iter().filter(|id| !ids.contains(id)).filter_map(|id| drafts.remove(id)).collect();
        for (id, draft) in sent {
            if let std::collections::hash_map::Entry::Vacant(vacant) = drafts.entry(id) {
                crate::speculate::listed(draft.actor, &draft.key);
                vacant.insert(draft);
            }
        }
        drop(drafts);
        for draft in gone {
            crate::speculate::dropped(runs, draft.actor, &draft.key);
        }
    }
    crate::speculate::start(runs);
    // What no list names now loads no more (cl-1123).
    if let Some(pool) = runs.service.pool() {
        pool.unlist(&crate::runs::actor_id(actor));
    }
    // What no list names any more is weighed no more.
    let named: Vec<String> = lock(&LISTED).values().flatten().cloned().collect();
    lock(&NEEDS).retain(|id, _| named.contains(id));
    // What no list names now is adoptable a while yet (`DROPPED`).
    let kept = {
        let mut dropped = lock(&DROPPED);
        let now = Instant::now();
        for id in lock(&PREDICTED).keys().filter(|id| !named.contains(id)) {
            dropped.entry(id.clone()).or_insert(now);
        }
        dropped.retain(|id, at| !named.contains(id) && at.elapsed() < ADOPTABLE);
        dropped.keys().cloned().collect::<Vec<String>>()
    };
    {
        let mut predicted = lock(&PREDICTED);
        predicted.extend(identities);
        predicted.retain(|id, _| named.contains(id) || kept.contains(id));
    }
    lock(&IDENTIFIED).retain(|id, _| named.contains(id) || kept.contains(id));
    lock(&INTERFACES).retain(|id, _| named.contains(id));
    lock(&EXTRAS).retain(|id, stop| {
        let named = named.contains(id);
        if !named {
            stop.cancel();
            tracing::info!("list item {id} dropped: its extras stop downloading");
        }
        named
    });
    {
        let mut planning = lock(&PLANNING);
        planning.retain(|id, _| named.contains(id));
        for id in &ids {
            planning.entry(id.clone()).or_insert(None);
        }
    }
    let put = {
        let mut puts = lock(&PUTS);
        let put = puts.entry(list.clone()).or_default();
        *put += 1;
        *put
    };
    let runs = runs.clone();
    std::thread::Builder::new()
        .name(format!("list-{}", prefix.trim_end_matches(':').rsplit(':').next().unwrap_or(prefix)))
        .spawn(move || admit(&runs, actor, list, put, requests, ids, admission))
        .map_err(|error| Error::internal("list_unadmitted", error.to_string()))?;
    Ok(())
}

/// List put `put`'s items admitted, one put at a time, unless a newer put of its list came since (which admits its own, and drops
/// what this one would have). Its admission holds the machine's release meanwhile.
fn admit(runs: &Arc<Runs>, actor: VerifiedActor, list: List, put: u64, requests: Vec<v1::RunRequest>, ids: Vec<String>, admission: crate::machine::lifecycle::Admission) {
    static ADMITTING: Mutex<()> = Mutex::new(());
    let _one = lock(&ADMITTING);
    if lock(&PUTS).get(&list) != Some(&put) {
        return;
    }
    let began = std::time::Instant::now();
    let prefix = list.1;
    let dropped: Vec<String> = match runs.live_ids(actor) {
        Ok(live) => live.into_iter().filter(|id| id.starts_with(prefix) && !ids.contains(id)).collect(),
        Err(error) => {
            tracing::warn!("{prefix} list: its runs weren't read: {}", error.message);
            return;
        }
    };
    // One not in line yet ends now; the rest wait for what replaced them (`settle`).
    let (dropped, unlined): (Vec<String>, Vec<String>) = {
        let lined = lock(&LINED);
        dropped.into_iter().partition(|id| lined.contains(id))
    };
    for id in &unlined {
        match runs.cancel(actor, id) {
            Ok(_) => tracing::info!("list item {id} dropped before it was in line: its run ends, and what it was fetching stops"),
            Err(error) => tracing::warn!("a list's dropped item {id} wasn't canceled: {}", error.message),
        }
    }
    // Noted before any is admitted: one in line at once is heard.
    static HANDOVERS: AtomicU64 = AtomicU64::new(0);
    let handover = HANDOVERS.fetch_add(1, Ordering::Relaxed);
    lock(&HANDED).push(Handed { put: handover, actor, dropped, waiting: ids.clone() });
    let specs: Vec<v1::RunSpec> = requests.iter().map(|request| request.spec.clone()).collect();
    let admitted = runs.accept(actor, requests);
    drop(admission);
    // Only an item new now takes over what a dropped one fetched: any other is in line already, or never will be.
    let new: Vec<&String> = match &admitted {
        Ok(admitted) => ids.iter().zip(admitted).filter(|(_, admitted)| matches!(admitted, Ok((true, _)))).map(|(id, _)| id).collect(),
        Err(_) => vec![],
    };
    settle(runs, |handed| {
        if let Some(handed) = handed.iter_mut().find(|handed| handed.put == handover) {
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
    let mut planning = lock(&PLANNING);
    let refused = |planning: &mut HashMap<String, Option<String>>, id: &String, why: &str| {
        tracing::warn!("list item {id} won't be ready ahead: {why}");
        if let Some(held) = planning.get_mut(id) {
            *held = Some(why.to_owned());
        }
    };
    match admitted {
        Ok(admitted) => {
            for (id, admitted) in ids.iter().zip(admitted) {
                match admitted {
                    Ok(_) => drop(planning.remove(id)),
                    Err(error) => refused(&mut *planning, id, &error.message),
                }
            }
        }
        Err(error) => ids.iter().for_each(|id| refused(&mut *planning, id, &error.message)),
    }
    tracing::info!("{prefix} list: {} items admitted in {} ms", ids.len(), began.elapsed().as_millis());
}

/// To `states` (the runs of `listed`), each listed item not yet admitted, `planning`; one its admission refused, failed and why.
pub(crate) fn planning(listed: &[String], states: &mut Vec<v1::RunState>) {
    let planning = lock(&PLANNING);
    let unrun: Vec<&String> = listed.iter().filter(|id| !states.iter().any(|state| &state.id == *id)).collect();
    for id in unrun {
        let Some(refused) = planning.get(id) else { continue };
        let (state, stage) = match refused {
            Some(_) => ("failed", "refused"),
            None => ("queued", "planning"),
        };
        let readiness = v1::Readiness { stage: stage.to_owned(), reason: refused.clone().unwrap_or_default(), ..Default::default() };
        states.push(v1::RunState { id: id.clone(), state: state.to_owned(), readiness: Some(readiness), ..Default::default() });
    }
}

/// The run its caller names `id` has what it reads in the download line, or ended: what a list dropped for it and its like ends now.
pub(crate) fn lined(runs: &Runs, id: &str) {
    if hidden(id) {
        lock(&LINED).insert(id.to_owned());
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
        for id in &dropped {
            lock(&LINED).remove(id);
            if !live.contains(id) {
                tracing::info!("list item {id} dropped: its run had ended");
                continue;
            }
            match runs.cancel(actor, id) {
                Ok(_) => tracing::info!("list item {id} dropped: its run ends, and what only it wanted stops downloading"),
                Err(error) => tracing::warn!("a list's dropped item {id} wasn't canceled: {}", error.message),
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
        let reads = crate::access::reads(store, needed);
        lock(&NEEDS).insert(caller.to_owned(), reads);
    }
}

/// The list items' runs `states` as their caller's Status says them. A prepare item whose run succeeded, was ended for a run's room
/// (`make_way`) or failed for want of room on the disk, but lacks some of what it needs here now, is `evicted` (cl-1104): no longer
/// ready, and sent again under a new key its machine fetches it anew. `waiting` says why not yet: a run here goes first (`busy`), or
/// what it lacks doesn't fit on the disk past its reserve, as a guess never makes room. Empty: it may come again now. One that failed
/// keeps its `reason`, and its caller counts each try.
pub(crate) fn standing(store: &crate::storage::Store, busy: bool, mut states: Vec<v1::RunState>) -> Vec<v1::RunState> {
    // Read off the lock: what a list put or a guess's planning takes waits on no disk (cl-1202).
    let needs = lock(&NEEDS).clone();
    let free = || crate::reclaim::Disk::measure(store.root()).map_or(u64::MAX, |disk| disk.free());
    let lost = |state: &v1::RunState| match state.state.as_str() {
        "succeeded" | "canceled" => true,
        "failed" => state.reason.as_ref().is_some_and(|reason| reason.code == "machine_disk_full"),
        _ => false,
    };
    for state in states.iter_mut().filter(|state| guess(&state.id) && lost(state)) {
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

/// The items `states` as their caller `reader` reads them: one whose run made it ready (its weights here) says how far it ran ahead
/// (`speculate::readiness`), else how far the load step took it (`gpu_service::Pool::readiness`, cl-1123).
pub(crate) fn readiness(runs: &Runs, reader: VerifiedActor, mut states: Vec<v1::RunState>) -> Vec<v1::RunState> {
    let (pool, actor) = (runs.service.pool(), crate::runs::actor_id(reader));
    for state in states.iter_mut().filter(|state| state.state == "succeeded") {
        // Run ahead (`running`, `done`, cl-1123 D), else as far as the load step took it.
        state.readiness = crate::speculate::readiness(runs, reader, &state.id).or_else(|| pool.as_deref().and_then(|pool| pool.readiness(&actor, &state.id)));
    }
    states
}

/// Each list item's readiness up to `ready` (cl-1123), from its run's state (`standing`) and what `activity` lists under its id:
/// `planning` until its request is planned (`planning` before it is admitted), `installing` while its code installs, `downloading`
/// while what it needs comes (bytes), `ready` once its run made it ready, `evicted` once that went. One the load step says more of
/// keeps that; one that failed or was canceled says so in its state.
pub(crate) fn progress(states: &mut [v1::RunState], activity: &[v1::Operation]) {
    let planned = lock(&NEEDS);
    for state in states.iter_mut().filter(|state| state.readiness.is_none()) {
        let id = state.id.clone();
        let under = |kind: &str| activity.iter().filter(|op| op.kind == kind && op.runs.iter().any(|run| *run == id)).collect::<Vec<&v1::Operation>>();
        let downloads = under("download");
        let (stage, done, total) = match state.state.as_str() {
            "succeeded" => ("ready", 0, 0),
            "evicted" => ("evicted", 0, 0),
            "failed" | "canceled" => continue,
            _ if !downloads.is_empty() => ("downloading", downloads.iter().map(|op| op.bytes_done).sum(), downloads.iter().map(|op| op.bytes_total).sum()),
            _ if !under("install").is_empty() => ("installing", 0, 0),
            _ if !planned.contains_key(&id) => ("planning", 0, 0),
            _ => ("installing", 0, 0),
        };
        state.readiness = Some(v1::Readiness { stage: stage.to_owned(), done, total, ..Default::default() });
    }
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
