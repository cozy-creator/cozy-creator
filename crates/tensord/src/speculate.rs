//! Speculative runs (cl-1123 D; Paul, 11-17: "or even start running the run, as if the current stuff entered into the text-box were
//! the final request, and cancel it and re-submit it if the input changes. This assumes that we can cancel quickly and that running
//! prematurely has no consequence (if the machine is empty, that's likely true)"). A guess (`ahead::drafts`) loaded on a GPU
//! (`gpu_service::Pool::item_loaded`) whose draft has stood unchanged `STILL`, while no call wants a GPU, runs as the call it predicts
//! under an id of its own (`ahead::SPECULATIVE`): no job, never listed, its outputs held here. A call sent as that request adopts it
//! (`adopt`): one going goes on as the call, one finished is the call's at once. An edit ends it (`dropped`), and so does a call of
//! any other request (`gpu_service::Pool::yield_to`): its executor told to stop at its next step, its GPU the call's at once; once the
//! GPU is idle again it runs anew. One at a time.
//!
//! Off by default (Paul 11-18: "I would stop short at actually doing the full run"; it "kind of violates a user's expectations"):
//! only a dedicated machine whose config has `dev = true` and `[machine] speculate = true` runs one (`Pool::speculates`). Elsewhere
//! `start` starts nothing, so no attempt is ever made and `adopt`, `dropped`, `listed`, `stopped` and `readiness` find none.
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock, PoisonError, Weak},
    time::{Duration, Instant},
};

use api::machine as v1;

use crate::{ahead::SPECULATIVE, api::auth::VerifiedActor, journal::Execution, runs::Runs};

/// How long a draft stands unchanged before it runs: past Calcifer's settling of an edit (300 ms), a person still typing.
const STILL: Duration = Duration::from_secs(3);

/// How long a run of a draft the lists dropped goes on: its render (which takes the draft's seed, so the box is guessed anew) may
/// reach this machine after the list that dropped it, and adopts it. An edit's run ends then.
const GRACE: Duration = Duration::from_secs(2);

/// How long a finished run of a dropped draft is kept for its render to adopt (`ahead::ADOPTABLE`'s).
const KEPT: Duration = Duration::from_secs(120);

/// Each draft's latest speculative run, by its actor and key.
static ATTEMPTS: LazyLock<Mutex<HashMap<(String, String), Attempt>>> = LazyLock::new(Mutex::default);

#[derive(Clone, Copy)]
struct Attempt {
    actor: VerifiedActor,
    /// Its run's id's last part (`id`): when it was sent, Unix ms, so no two are one.
    n: u64,
    /// Told to stop (for a call, or its draft dropped): no call adopts it, and while its draft is listed it runs anew once it ended.
    told: bool,
    /// A call adopted it: it never runs again.
    adopted: bool,
    /// When the lists dropped its draft.
    dropped: Option<Instant>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Draft `key`'s attempt `n`'s run id.
fn id(key: &str, n: u64) -> String {
    format!("{SPECULATIVE}{key}.{n}")
}

/// Its thread, once: a look at each engine event, and when a draft's `STILL` passes. None unless the machine speculates.
pub(crate) fn start(runs: &Arc<Runs>) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if runs.service.pool().is_none_or(|pool| !pool.speculates()) {
        return;
    }
    let weak = Arc::downgrade(runs);
    STARTED.get_or_init(|| match std::thread::Builder::new().name("speculate".into()).spawn(move || serve(&weak)) {
        Ok(_) => tracing::info!("predicted requests run ahead here (dev: machine.speculate)"),
        Err(error) => tracing::warn!("predicted requests won't run ahead: {error}"),
    });
}

fn serve(runs: &Weak<Runs>) {
    loop {
        let Some(this) = runs.upgrade() else { return };
        let engine = this.service.engine.clone();
        let epoch = engine.activity_epoch();
        let next = look(&this);
        drop(this);
        // The engine stopped.
        if engine.wait_activity(epoch, next.map(|at| at.saturating_duration_since(Instant::now()))) == u64::MAX {
            return;
        }
    }
}

/// A call wants a GPU or holds one: no speculative run starts. A guess's run, or a speculative one's call, is none.
fn gpu_wanted(runs: &Runs) -> bool {
    let engine = &runs.service.engine;
    let gpu = |record: &Execution| record.invocation.degree > 0 && !record.invocation.background;
    engine.ready().map_or(true, |ready| ready.iter().any(gpu)) || engine.active(usize::MAX).map_or(true, |active| active.iter().any(gpu))
}

/// Run `id` of `actor`'s, as it stands: its state, or none (it was adopted, or never was).
fn state(runs: &Runs, actor: VerifiedActor, id: &str) -> Option<v1::RunState> {
    runs.state(actor, id).ok()
}

fn going(state: &str) -> bool {
    matches!(state, "queued" | "preparing" | "starting" | "running")
}

fn earliest(next: &mut Option<Instant>, at: Instant) {
    *next = Some(next.map_or(at, |next| next.min(at)));
}

/// One look: a dropped draft's run past its `GRACE` ends; then, if none runs and no call wants a GPU, the first draft loaded and
/// still runs. When to look again for a draft not still yet, or a grace not past.
fn look(runs: &Arc<Runs>) -> Option<Instant> {
    let pool = runs.service.pool()?;
    let now = Instant::now();
    let mut next: Option<Instant> = None;
    let mut running = false;
    let attempts = lock(&ATTEMPTS).clone();
    let mut done = vec![];
    for ((owner, key), attempt) in &attempts {
        let run = id(key, attempt.n);
        let state = state(runs, attempt.actor, &run);
        let live = state.as_ref().is_some_and(|state| going(&state.state));
        running |= live;
        let Some(dropped) = attempt.dropped else { continue };
        if live && !attempt.told {
            match now < dropped + GRACE {
                true => earliest(&mut next, dropped + GRACE),
                false => end(runs, &pool, (owner, key), attempt.actor, &run, state.map(|state| state.number), "its draft changed"),
            }
        } else if !live && (attempt.adopted || attempt.told || now >= dropped + KEPT) {
            done.push((owner.clone(), key.clone()));
        } else if !live {
            earliest(&mut next, dropped + KEPT);
        }
    }
    {
        let mut attempts = lock(&ATTEMPTS);
        done.iter().for_each(|key| drop(attempts.remove(key)));
    }
    // One at a time.
    if running || gpu_wanted(runs) {
        return next;
    }
    for (caller, draft) in crate::ahead::drafts() {
        let actor = crate::runs::actor_id(draft.actor);
        // A call, writing nowhere but here: its files come with its call, so a request reading any is never run ahead, nor one
        // whose weights go elsewhere.
        let spec = &draft.spec;
        let call = spec.kind == v1::RunKind::Call && spec.inputs.is_empty() && spec.weights_destination.is_empty() && spec.set.is_none();
        if !call || !pool.item_loaded(&actor, &caller) {
            continue;
        }
        if let Some(attempt) = attempts.get(&(actor.clone(), draft.key.clone())) {
            let ended = state(runs, draft.actor, &id(&draft.key, attempt.n)).map(|state| state.state);
            // Ended for a call: anew. Else done, failing on its own, or adopted: not again.
            if attempt.adopted || !attempt.told || !matches!(ended.as_deref(), Some("failed" | "canceled")) {
                continue;
            }
        }
        let still = draft.since + STILL;
        if now < still {
            earliest(&mut next, still);
            continue;
        }
        submit(runs, draft);
        return None;
    }
    next
}

/// `draft` run as sent.
fn submit(runs: &Arc<Runs>, draft: crate::ahead::Draft) {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |at| at.as_millis() as u64);
    let run = id(&draft.key, n);
    let spec = v1::RunSpec { predicted: None, ..draft.spec };
    let key = (crate::runs::actor_id(draft.actor), draft.key.clone());
    lock(&ATTEMPTS).insert(key.clone(), Attempt { actor: draft.actor, n, told: false, adopted: false, dropped: None });
    match runs.accept(draft.actor, vec![v1::RunRequest { id: run.clone(), spec }]).map(|mut answers| answers.pop()) {
        Ok(Some(Ok((_, state)))) => {
            tracing::info!(run = %run, "a guess runs as if it were sent, its draft unchanged {} s (machine run {})", STILL.as_secs(), state.number);
        }
        Ok(Some(Err(refused))) => tracing::warn!(run = %run, "a guess doesn't run ahead: {}", refused.message),
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(run = %run, "a guess doesn't run ahead: {}", error.message);
            lock(&ATTEMPTS).remove(&key);
        }
    }
}

/// Speculative run `run` (machine run `number`) ends, `by` what: on its executor it is told to stop at its next step and its GPU is
/// given back now (`gpu_service::Pool::stop_speculation`); else, still preparing or queued, it is canceled.
fn end(runs: &Runs, pool: &crate::gpu_service::Pool, (owner, key): (&String, &String), actor: VerifiedActor, run: &str, number: Option<u64>,
    by: &'static str) {
    {
        let mut attempts = lock(&ATTEMPTS);
        let Some(attempt) = attempts.get_mut(&(owner.clone(), key.clone())) else { return };
        // A call took it meanwhile.
        if attempt.adopted {
            return;
        }
        attempt.told = true;
    }
    if number.is_some_and(|number| pool.stop_speculation(Some(&number.to_string()), by, &[])) {
        return;
    }
    match runs.cancel(actor, run) {
        Ok(_) => tracing::info!(run = %run, "speculative run ended for {by}: canceled before its executor ran it"),
        Err(error) => tracing::warn!(run = %run, "speculative run not ended for {by}: {}", error.message),
    }
}

/// `actor`'s draft `key` was edited or dropped: its speculative run, going, ends `GRACE` from now unless a call adopts it first; one
/// finished is kept `KEPT` for it.
pub(crate) fn dropped(runs: &Arc<Runs>, actor: VerifiedActor, key: &str) {
    if let Some(attempt) = lock(&ATTEMPTS).get_mut(&(crate::runs::actor_id(actor), key.to_owned())) {
        attempt.dropped.get_or_insert_with(Instant::now);
    }
    runs.service.engine.notify_activity();
}

/// `actor`'s draft `key` is listed again (an edit undone): its run is its own again, not one to end or let go.
pub(crate) fn listed(actor: VerifiedActor, key: &str) {
    if let Some(attempt) = lock(&ATTEMPTS).get_mut(&(crate::runs::actor_id(actor), key.to_owned())) {
        attempt.dropped = None;
    }
}

/// Speculative run `execution` was told to stop (`gpu_service::Pool::stop_speculation`): no call adopts it, and its draft, still
/// listed, runs anew once the GPU is idle.
pub(crate) fn stopped(execution: &str, engine: &crate::execution::Engine) {
    let caller = engine.caller_id(execution);
    let mut attempts = lock(&ATTEMPTS);
    for ((_, key), attempt) in attempts.iter_mut() {
        if id(key, attempt.n) == caller {
            attempt.told = true;
        }
    }
}

/// Run `to`, sent by `actor` as the request its draft `key` predicted (`ahead::adopts`), adopts that draft's speculative run, going or
/// succeeded: the run is `to`'s from now on, and a job. Whether it did.
pub(crate) fn adopt(runs: &Runs, actor: &str, key: &str, to: &str) -> bool {
    let attempt = {
        let mut attempts = lock(&ATTEMPTS);
        let Some(attempt) = attempts.get_mut(&(actor.to_owned(), key.to_owned())) else { return false };
        // Told to stop: it ends canceled, and the call runs as any.
        if attempt.told || attempt.adopted {
            tracing::info!(run = %to, "doesn't adopt the speculative run {}: it was told to stop", id(key, attempt.n));
            return false;
        }
        // Claimed now: nothing ends it from here (`end`, and the pool's own first).
        attempt.adopted = true;
        *attempt
    };
    let from = id(key, attempt.n);
    let release = || {
        if let Some(attempt) = lock(&ATTEMPTS).get_mut(&(actor.to_owned(), key.to_owned())) {
            attempt.adopted = false;
        }
    };
    let number = state(runs, attempt.actor, &from).map(|state| state.number.to_string());
    if let (Some(pool), Some(number)) = (runs.service.pool(), &number) {
        if !pool.adopted(number) {
            release();
            tracing::info!(run = %to, "doesn't adopt the speculative run {from}: a run took its GPU");
            return false;
        }
    }
    let engine = &runs.service.engine;
    match engine.with_journal(|journal| journal.adopt(actor, &from, to)) {
        Ok(Some(execution)) => {
            let state = engine.get(&execution).map(|record| format!("{:?}", record.state).to_lowercase()).unwrap_or_default();
            tracing::info!(run = %to, "adopts the speculative run {from} (machine run {execution}, {state})");
            engine.notify_activity();
            true
        }
        Ok(None) => {
            release();
            false
        }
        Err(error) => {
            release();
            tracing::warn!(run = %to, "didn't adopt the speculative run {from}: {error}");
            false
        }
    }
}

/// How far `actor`'s draft `caller` (`cozy:prepare:<key>`) has run ahead: `running` (its stage's `done`/`total` steps, the stage its
/// `reason`), then `done`. None while it hasn't, or once a call adopted it.
pub(crate) fn readiness(runs: &Runs, actor: VerifiedActor, caller: &str) -> Option<v1::Readiness> {
    let key = caller.strip_prefix(crate::ahead::PREPARE)?;
    let attempt = lock(&ATTEMPTS).get(&(crate::runs::actor_id(actor), key.to_owned())).copied()?;
    let state = state(runs, actor, &id(key, attempt.n))?;
    match state.state.as_str() {
        "succeeded" => Some(v1::Readiness { stage: "done".into(), ..Default::default() }),
        live if going(live) => {
            let progress = runs.service.engine.get(&state.number.to_string()).ok().and_then(|record| record.progress);
            let progress: serde_json::Value = progress.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
            let count = |field: &str| progress[field].as_u64().unwrap_or(0);
            Some(v1::Readiness {
                stage: "running".into(),
                done: count("position"),
                total: count("total"),
                reason: progress["stage"].as_str().unwrap_or_default().to_owned(),
            })
        }
        _ => None,
    }
}
