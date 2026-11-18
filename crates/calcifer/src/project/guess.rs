//! A project's guesses (cl-1104, Paul 11-16: "when a user attaches a tool, like the qwen image 2.1 tool, we could figure out what
//! machine, if we got a request, that request would land on. On that machine, we can pre-emptively install packages + models"): what
//! a render of each of its boxes would run, on the machine a render of it would go to now, for `prefetch::Lists` to have that machine
//! get ready in the background. Guessed while a page shows the project: as it opens, as its boxes change (a tool put on a box, a
//! box's request edited, a box deleted), each settled a moment, and as a machine comes up or the one guessed on goes. Closed, its
//! guesses end.
//!
//! A box's machine is the one placement would send its render to (`placement::soonest`, by what each machine has of it), of the
//! machines that serve now: nothing is started, rented or waited for to hold a guess. It stays where it is guessed unless another
//! would finish it clearly sooner (`plan::clearly_sooner`), and its render stays there by the same rule (`Walk::plan`), so the
//! machine made ready is the one Render picks.
//!
//! What a guess fetches (`item`) is decided here, with nothing installed and no package code run (`Host::foreseen`, cl-1103; Paul,
//! 11-17: "calcifer says 'okay, I'm going to send machine-A this request, so I should tell it to download the required files'"): the
//! function's needs rules, from its published interface, evaluated on the box's draft request as its render would send it
//! (`Asking::request`), walked as its machine walks a call (`needs::wants`). The item carries that walk by slot; its machine resolves
//! each slot for its lane and store and fetches the needed ones, then the extras, never a slot the rules exclude. An edit that changes
//! what the rules say changes the item: its machine drops what only the old one wanted and fetches the new.
use std::{
    borrow::Cow,
    collections::HashSet,
    sync::{Arc, LazyLock, Mutex, atomic::Ordering},
    time::Duration,
};

use api::{calcifer::On, config::Origin, error::Error, machine as v1};
use serde_json::{Map, Value};
use tokio::sync::watch;

use super::{Asking, Project, host::{Host, Package, Renderer}, job_and_package};
use crate::{api::Api, placement, plan, prefetch::Guess, store::now_ms};

/// What's typed settles this long before it's guessed again (cl-1123: a prediction is only as good as how early it starts). A project
/// just shown is guessed at once.
const SETTLE: Duration = Duration::from_millis(300);
/// What machines said settles this long: a load that moved needs no guess sooner.
const LOADS: Duration = Duration::from_millis(800);
/// The most boxes a project guesses for: its first, in its order.
const BOXES: usize = 64;
/// A box's guess waits this long for its package and its models' cards, as its estimate does (`fit`).
const READ: Duration = Duration::from_secs(20);
/// The most guess rounds of a project that go at once: an edit made while one reads starts its own, and the newest stands.
const ROUNDS: usize = 2;
/// A box's guess that takes this long says what it waited on.
const SLOW: Duration = Duration::from_millis(250);

/// Moves whenever a project closes: each one's guesses end with it.
static CLOSED: LazyLock<watch::Sender<u64>> = LazyLock::new(|| watch::channel(0).0);
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Each box's last guess and what it was made of (`item`), by project folder and box: kept while its request holds and no machine's
/// load moved that could place it elsewhere (`kept`), so a round re-guesses only the boxes an edit changed (cl-1123).
static MADE: LazyLock<Mutex<std::collections::HashMap<(String, String), Made>>> = LazyLock::new(Mutex::default);

#[derive(Clone)]
struct Made {
    /// Its request as `request_key` hashed it.
    key: u64,
    /// `load_moved` and the machines' arrivals when it was made.
    load: u64,
    arrived: u64,
    package: super::Package,
    function: String,
    guess: Option<Guess>,
}

/// A box's request, hashed: its tool, kind and fields, its draft's seed, and what its render would send (`Asking::request`).
fn request_key(with: &str, kind: &str, made: &Map<String, Value>, seed: u64, request: Option<&Value>) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (with, kind, Value::Object(made.clone()).to_string(), seed, request.map(Value::to_string)).hash(&mut hasher);
    hasher.finish()
}

/// Box `name`'s last guess, if what it was made of holds: its request the same, its machine serving, no machine come since, and no
/// load moved that could place it elsewhere (`Lists::could_move`).
fn kept(api: &Api, project: &Project, folder: &str, (name, with, kind, made): (&str, &str, &str, &Map<String, Value>), asking: &Asking) -> Option<Guess> {
    let memo = lock(&MADE).get(&(folder.to_owned(), name.to_owned())).cloned()?;
    let guess = memo.guess?;
    let lists = &api.machines.lists;
    if memo.arrived != *api.machines.arrivals.borrow() || api.machines.live_frame(&guess.machine).is_none() || lists.could_move(folder, memo.load, now_ms()).is_some() {
        return None;
    }
    let request = asking.request(name, &memo.package, &memo.function);
    (request_key(with, kind, made, project.draft_seed(name), request.as_ref()) == memo.key).then_some(guess)
}

/// Why each box isn't guessed, by project folder and box ("" for the project), said once until it changes (cl-1123: Paul's picture box
/// with Qwen on it was never guessed, and nothing said why).
static UNGUESSED: LazyLock<Mutex<std::collections::HashMap<(String, String), String>>> = LazyLock::new(Mutex::default);

/// Box `name` of `folder` (the project, when empty) isn't guessed, for `why`: said when that's new.
fn unguessed(folder: &str, name: &str, why: &str) {
    let mut said = lock(&UNGUESSED);
    if said.get(&(folder.to_owned(), name.to_owned())).is_some_and(|was| was == why) {
        return;
    }
    match name.is_empty() {
        true => tracing::info!(target: crate::prefetch::TARGET, "{folder}: no box is guessed: {why}"),
        false => tracing::info!(target: crate::prefetch::TARGET, "{folder}: {name} isn't guessed: {why}"),
    }
    said.insert((folder.to_owned(), name.to_owned()), why.to_owned());
}

/// Box `name` of `folder` is guessed: a reason it isn't is said again once it holds.
fn guessed(folder: &str, name: &str) {
    let mut said = lock(&UNGUESSED);
    said.remove(&(folder.to_owned(), String::new()));
    said.remove(&(folder.to_owned(), name.to_owned()));
}

/// Why box `i` isn't a draft its Render would make, if it isn't: it has no tool, Cozy makes it itself, or it approves a take of what
/// it asks now. One with a tool approving anything else is one (cl-1123; Paul, 11-17: "I just attached qwen image 2.1, to an image,
/// and typed up a prompt, but it didn't pre-fetch the iTi prompt-enhancer until I pushed render"): the picture put on it, or a take
/// of what it asked before, is what its Render edits or makes again.
fn not_a_draft(asking: &Asking, i: usize) -> Option<&'static str> {
    let (doc, stand) = (&asking.doc, &asking.stand[i]);
    if !doc.makes(i) {
        return Some("it has no tool");
    }
    if doc.tool(i).is_some() {
        return Some("Cozy makes it itself, with no model");
    }
    let take = |(number, now): (u32, &String)| asking.lock.asset(&doc.assets[i].name).and_then(|held| held.version(number)).is_some_and(|version| version.inputs == *now);
    (doc.assets[i].pin.is_some() && stand.version().zip(stand.inputs.as_ref()).is_some_and(take)).then_some("it approves a take of what it asks now")
}

/// Local code carried to a machine by a guess (`item`), by machine and its package's manifest: written there once, its source as staged.
static CARRIED: LazyLock<Mutex<std::collections::HashMap<(String, String), v1::LocalSource>>> = LazyLock::new(Mutex::default);
/// One carry of local code to a machine at a time, by machine and manifest: a round going beside it (`ROUNDS`) waits and takes it.
static CARRYING: LazyLock<Mutex<std::collections::HashMap<(String, String), Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Mutex::default);
/// The functions whose Hub reads are read ahead (`fit::read_ahead`): by Hub, package, release and function.
static READ_AHEAD: LazyLock<Mutex<HashSet<(String, String, String, String)>>> = LazyLock::new(Mutex::default);

/// A project closed.
pub(super) fn closed() {
    CLOSED.send_modify(|n| *n += 1);
}

/// A box as it's guessed for: its name, tool, kind, fields, and how many pictures and videos they read.
type Boxed = (String, String, String, Map<String, Value>, u32);

/// A page shows `project`: its guesses are kept as it stands, until it closes.
pub(super) fn keep(api: &Arc<Api>, project: &Arc<Project>) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    if project.guessing.swap(true, Ordering::AcqRel) {
        return;
    }
    let (api, open, folder) = (api.clone(), Arc::downgrade(project), project.folder.dir.to_string_lossy().into_owned());
    runtime.spawn(async move {
        let (mut edits, mut closed) = (crate::events::edits(), CLOSED.subscribe());
        let (mut arrivals, mut machines) = (api.machines.arrivals.subscribe(), api.calcifer.machines_changed());
        // Its revision and the machines' loads as last guessed at; `stale`: something it's guessed from changed since.
        // `why`: what made it stale, said with the guess it makes.
        let (mut read, mut load, mut stale, mut why) = (String::new(), 0, true, Cow::Borrowed("shown"));
        let loads = || api.machines.lists.load_moved.load(Ordering::Acquire);
        // Rounds run beside this loop, so an edit made while one reads (a package's first guess in a fresh project: 2.75 s, which a
        // typed edit waited behind in 44199938129) starts its own as it settles. The newest round to finish stands (`applied`); an
        // older one finishing after it changes nothing, and none does once the project closes.
        let applied = Arc::new(Mutex::new(0_u64));
        let (ended, mut ending) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (mut started, mut going) = (0_u64, 0_usize);
        loop {
            if stale && going < ROUNDS {
                // An edit that came while it guessed settles as an edit, whatever else moved (44068453992: one waited 800 ms).
                if edits.has_changed().unwrap_or(false) && open.upgrade().is_some_and(|project| super::revision(&project.folder.dir) != read) {
                    why = Cow::Borrowed("edited");
                }
                match why.as_ref() {
                    "shown" => {}
                    "edited" => tokio::time::sleep(SETTLE).await,
                    _ => tokio::time::sleep(LOADS).await,
                }
                let Some(project) = open.upgrade() else { break };
                // What changed while it settled is in this guess.
                edits.borrow_and_update();
                arrivals.borrow_and_update();
                machines.borrow_and_update();
                read = super::revision(&project.folder.dir);
                load = loads();
                (started, going) = (started + 1, going + 1);
                let (api, folder, why, applied, ended, round) = (api.clone(), folder.clone(), why.clone(), applied.clone(), ended.clone(), started);
                tokio::spawn(async move {
                    let began = std::time::Instant::now();
                    let guesses = guesses(&api, &project).await;
                    drop(project);
                    let mut newest = lock(&*applied);
                    match round > *newest {
                        true => {
                            *newest = round;
                            tracing::info!(target: crate::prefetch::TARGET, "{folder}: {} guesses in {} ms ({why})", guesses.len(), began.elapsed().as_millis());
                            api.machines.lists.guess(&api.machines, &folder, guesses);
                        }
                        false => tracing::info!(target: crate::prefetch::TARGET, "{folder}: a newer round's guesses stand over these, {} ms ({why})", began.elapsed().as_millis()),
                    }
                    drop(newest);
                    let _ = ended.send(());
                });
                stale = false;
            }
            tokio::select! {
                Some(()) = ending.recv() => going -= 1,
                changed = closed.changed() => {
                    if changed.is_err() || open.strong_count() == 0 {
                        break;
                    }
                }
                changed = edits.changed() => match (changed, open.upgrade()) {
                    (Ok(()), Some(project)) => {
                        if super::revision(&project.folder.dir) != read {
                            (stale, why) = (true, Cow::Borrowed("edited"));
                        }
                    }
                    _ => break,
                },
                changed = arrivals.changed() => match changed {
                    Ok(()) => (stale, why) = (true, Cow::Borrowed("a machine came")),
                    Err(_) => break,
                },
                // A machine said its Status: guessed again when one it's guessed on serves no more, or a machine's load moved so a
                // guess could go elsewhere (cl-1104, cl-1123: `Lists::could_move`), so the one made ready is the one a render would go
                // to now. Any other move changes nothing here.
                changed = machines.changed() => match changed {
                    Ok(()) if api.machines.lists.guess_left(&api.machines, &folder) => (stale, why) = (true, Cow::Borrowed("a machine it guessed on left")),
                    Ok(()) if loads() != load => {
                        if let Some(moved) = api.machines.lists.could_move(&folder, load, now_ms()) {
                            (stale, why) = (true, Cow::Owned(format!("load moved: {moved}")));
                        }
                    }
                    Ok(()) => {}
                    Err(_) => break,
                },
            }
        }
        // A round still going changes nothing now.
        *lock(&*applied) = u64::MAX;
        api.machines.lists.guess_closed(&api.machines, &folder);
        lock(&MADE).retain(|(at, _), _| *at != folder);
        lock(&UNGUESSED).retain(|(at, _), _| *at != folder);
    });
}

/// `project`'s guesses as it stands: one for each box a render would make, where a machine that serves could run it. Why a box has
/// none is said (`unguessed`).
async fn guesses(api: &Arc<Api>, project: &Arc<Project>) -> Vec<Guess> {
    // Its runs, which its times on each machine stand on, are its own: read in its storage, as its estimates are (`fit`).
    let scope = match crate::private_io::of_path(&project.folder.dir) {
        Ok(scope) => scope,
        Err(e) => {
            unguessed(&project.folder.dir.to_string_lossy(), "", &format!("its storage can't be read: {}", e.message));
            return vec![];
        }
    };
    crate::private_io::context(scope, guesses_in(api, project)).await
}

async fn guesses_in(api: &Arc<Api>, project: &Arc<Project>) -> Vec<Guess> {
    let folder = project.folder.dir.to_string_lossy().into_owned();
    if let Err(e) = project.require_view() {
        unguessed(&folder, "", &format!("its view isn't open: {}", e.message));
        return vec![];
    }
    // What each box's render would send, which its function's needs rules read (cl-1103).
    let Some(asking) = Asking::of(&project.folder) else {
        unguessed(&folder, "", "its boxes can't be read");
        return vec![];
    };
    // What a render of each box would make: each draft with a tool of a package (`not_a_draft`), the first `BOXES`.
    let mut drafts = 0;
    let mut boxes: Vec<Boxed> = vec![];
    for &i in &asking.doc.order {
        let (doc, name) = (&asking.doc, &asking.doc.assets[i].name);
        match not_a_draft(&asking, i) {
            Some(why) => unguessed(&folder, name, why),
            None if drafts == BOXES => unguessed(&folder, name, &format!("a project guesses its first {BOXES} drafts")),
            None => {
                drafts += 1;
                boxes.push((name.clone(), doc.target(i), doc.assets[i].kind.clone(), doc.assets[i].made.clone(), doc.media_read(i)));
            }
        }
    }
    if boxes.is_empty() {
        return vec![];
    }
    let on = match crate::runs::unnamed_set(api, Some(&project.id)).await {
        Ok(on) => on,
        Err(e) => {
            unguessed(&folder, "", &format!("its machines can't be read: {}", e.message));
            return vec![];
        }
    };
    let (hub, host) = (api.machines.hubs.default_origin(), Host::new(api));
    // A box an edit didn't change keeps its guess: only the others are guessed again. The guesses stay in the boxes' order.
    lock(&MADE).retain(|(at, name), _| *at != folder || boxes.iter().any(|(held, ..)| held == name));
    let order: Vec<String> = boxes.iter().map(|(name, ..)| name.clone()).collect();
    let (mut kept_now, mut fresh) = (vec![], vec![]);
    for boxed in boxes {
        match kept(api, project, &folder, (boxed.0.as_str(), boxed.1.as_str(), boxed.2.as_str(), &boxed.3), &asking) {
            Some(guess) => {
                guessed(&folder, &guess.asset);
                kept_now.push(guess);
            }
            None => fresh.push(boxed),
        }
    }
    if !kept_now.is_empty() {
        let names: Vec<&str> = kept_now.iter().map(|guess| guess.asset.as_str()).collect();
        tracing::info!(target: crate::prefetch::TARGET, "{folder}: {names:?} keep their guesses: their requests and loads hold");
    }
    let names: Vec<String> = fresh.iter().map(|(name, ..)| name.clone()).collect();
    let each = fresh.into_iter().map(|boxed| Box::pin(guess(api, project, &host, &hub, &folder, on.clone(), boxed, &asking)));
    let made = futures_util::future::join_all(each).await;
    // A box whose guess couldn't be made this time (a read that didn't answer, its machine between frames) keeps the one it had, so
    // its item keeps its key and its machine goes on making it ready: what dropped and listed again 13 s later in 44057515041.
    let made: Vec<Guess> = (names.into_iter().zip(made))
        .filter_map(|(name, made)| match made {
            Guessed::Made(guess, key) => {
                guessed(&folder, &name);
                // Only for the request it was made of: a round that finished after a newer one leaves that one's memo as it is.
                if let Some(memo) = lock(&MADE).get_mut(&(folder.clone(), name)).filter(|memo| memo.key == key) {
                    memo.guess = Some((*guess).clone());
                }
                Some(*guess)
            }
            Guessed::Nothing(why) => {
                unguessed(&folder, &name, &why);
                None
            }
            Guessed::Unknown(why) => {
                let kept = api.machines.lists.last_guess(&folder, &name);
                tracing::info!(target: crate::prefetch::TARGET, "{folder}: {name}'s guess wasn't made ({why}); {}", if kept.is_some() { "it keeps the one it had" } else { "it has none" });
                kept
            }
        })
        .collect();
    let mut all: Vec<Guess> = kept_now.into_iter().chain(made).collect();
    all.sort_by_key(|guess| order.iter().position(|name| *name == guess.asset));
    all
}

/// What a box's guess came to: made; nothing, and why (its package makes nothing of its kind, it is an encrypted project's local
/// code); or not known this time, and why.
enum Guessed {
    /// With the key of the request it was made of (`request_key`).
    Made(Box<Guess>, u64),
    Nothing(Cow<'static, str>),
    Unknown(Cow<'static, str>),
}

/// The guess for one box: none when no machine that serves now could run it, or its package says nothing to run. One that takes `SLOW`
/// says what it waited on, stage by stage (`crate::stages`).
#[allow(clippy::too_many_arguments)]
async fn guess(api: &Arc<Api>, project: &Arc<Project>, host: &Host, hub: &str, folder: &str, on: Vec<On>, boxed: Boxed, asking: &Asking) -> Guessed {
    let stages = Arc::new(Mutex::new(crate::stages::Stages::default()));
    let (name, began) = (boxed.0.clone(), std::time::Instant::now());
    let guessed = crate::stages::recorded(stages.clone(), staged(api, project, host, hub, folder, on, boxed, asking, &stages)).await;
    // One made says its stages as it goes; one not made, here.
    if !matches!(guessed, Guessed::Made(..)) && began.elapsed() >= SLOW {
        tracing::info!(target: crate::prefetch::TARGET, "{folder}: {name}'s guess took {} ms: {}", began.elapsed().as_millis(), lock(&*stages).said());
    }
    guessed
}

#[allow(clippy::too_many_arguments)]
async fn staged(api: &Arc<Api>, project: &Arc<Project>, host: &Host, hub: &str, folder: &str, on: Vec<On>, boxed: Boxed, asking: &Asking, stages: &Mutex<crate::stages::Stages>) -> Guessed {
    let began = std::time::Instant::now();
    let (name, with, kind, made, media) = boxed;
    let first = on.first().cloned();
    let read = job_and_package(host, &project.folder.dir, name.clone(), on, hub, &with, made.clone(), Some((asking, name.as_str())));
    let (mut job, package) = match tokio::time::timeout(READ, read).await {
        Ok(Ok(read)) => read,
        Ok(Err(why)) => return Guessed::Unknown(Cow::Owned(format!("its quote couldn't be read: {why}"))),
        Err(_) => return Guessed::Unknown(Cow::Borrowed("its quote didn't answer in time")),
    };
    job.size.refs = Some(media);
    let (offers, _) = crate::stages::stage("offers", placement::offers_now(api, std::slice::from_ref(&job))).await;
    let now = now_ms();
    let serves = |offer: &placement::Offer| offer.ready_at_ms.is_none() && matches!(&offer.machine, On::Machine(name) if api.machines.live_frame(name).is_some());
    let Some(offers) = offers.into_iter().next() else { return Guessed::Unknown(Cow::Borrowed("no machine quoted it")) };
    let up: Vec<placement::Offer> = offers.into_iter().filter(serves).collect();
    let Some(soonest) = placement::soonest(&up, now) else { return Guessed::Unknown(Cow::Borrowed("no machine that serves now runs it")) };
    let held = api.machines.lists.guessed_on(folder, &name).map(On::Machine);
    let held = held.and_then(|held| up.iter().find(|offer| offer.fits && offer.machine == held));
    let chosen = match held {
        Some(held) if !plan::clearly_sooner(soonest, held, now) => held,
        _ => soonest,
    };
    let On::Machine(machine) = &chosen.machine else { return Guessed::Nothing(Cow::Owned(format!("it would go to {}, no one machine", chosen.machine))) };
    // Where else it would finish, and how long each one's line holds it from GPUs: what a load moving there could change.
    let others = (up.iter().filter(|offer| offer.fits && offer.machine != chosen.machine))
        .filter_map(|offer| match &offer.machine {
            On::Machine(other) => Some((other.clone(), (offer.finish_at_ms, offer.waits_ms(now)))),
            _ => None,
        })
        .collect();
    let placed = crate::prefetch::Placed { finish_ms: chosen.finish_at_ms, others };
    // The package its quote read, where that was read for this machine; else read for it.
    let read = (first.as_ref() == Some(&chosen.machine)).then_some(package);
    let (item, function, key) = match item(api, host, project, hub, (name.as_str(), with.as_str(), kind.as_str(), &made), &chosen.machine, asking, read).await {
        Ok(Ok(item)) => item,
        Ok(Err(why)) => return Guessed::Nothing(why),
        Err(why) => return Guessed::Unknown(Cow::Owned(format!("its request couldn't be read: {}", why.message))),
    };
    let took = began.elapsed();
    match took < SLOW {
        true => tracing::info!(target: crate::prefetch::TARGET, "{folder}: {name} guessed on {machine} in {} ms", took.as_millis()),
        false => tracing::info!(target: crate::prefetch::TARGET, "{folder}: {name} guessed on {machine} in {} ms: {}", took.as_millis(), lock(stages).said()),
    }
    Guessed::Made(Box::new(Guess { project: Arc::downgrade(project), asset: name, machine: machine.clone(), hub: hub.to_owned(), item, function, placed, draft: draft(&with, &made) }), key)
}

/// A box's draft request, hashed: its tool and its fields. One edited is a new guess, which a failed one's doesn't stand in for.
fn draft(with: &str, made: &Map<String, Value>) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (with, Value::Object(made.clone()).to_string()).hash(&mut hasher);
    hasher.finish()
}

/// What `machine` has ready for box `name` of `kind` whose tool is `with` (`PUT /v1/prepare`): the function its text names, else the
/// first of its package that makes this kind, as the request a render of the box would send it (`super::predicted`, cl-1123: its
/// draft bound, its draft's seed); and the function, as its row says it. Its package is read statically, nothing installed to read
/// it (`Host::foreseen`). Err(why) inside: none, as its package makes nothing of the kind, or it is an encrypted project's local
/// code, which goes with the run that calls it.
#[allow(clippy::too_many_arguments)]
async fn item(api: &Arc<Api>, host: &Host, project: &Project, hub: &str, (name, with, kind, made): (&str, &str, &str, &Map<String, Value>), machine: &On, asking: &Asking, read: Option<Package>) -> Result<Result<(v1::Predicted, String, u64), Cow<'static, str>>, Error> {
    let dir = &project.folder.dir;
    let package = match read {
        Some(package) => package,
        None => crate::stages::stage("package", host.foreseen(hub, with, dir, std::slice::from_ref(machine))).await?,
    };
    let Some(renderer) = package.first_making(kind) else { return Ok(Err(Cow::Owned(format!("{} makes no {kind}", package.name)))) };
    let source = match &renderer.local {
        None => v1::Source::Release(v1::Release { package: renderer.package.clone(), release: renderer.release.clone(), digest: recorded(api, project, hub, &renderer)?, ..Default::default() }),
        // Carried to its machine as a run of it carries it (#3646), its files and manifest written there before the item, so a fresh
        // rental installs it too (frozen-RC 44000183889: H3 from a checkout never got ready on one); once a machine has them, by its
        // manifest alone. An encrypted project's own code goes only with the run that calls it; code beside the project (a dev
        // home's checkout, `~/…`) isn't the project's.
        Some(path) => {
            let private = crate::private_io::of_path(dir)?.is_some_and(|scope| !scope.storage.ordinary);
            if private && path.starts_with(dir) {
                return Ok(Err(Cow::Borrowed("its code is this encrypted project's own, which goes only with the run that calls it")));
            }
            let On::Machine(on) = machine else { return Ok(Err(Cow::Owned(format!("its local code would go to {machine}, no one machine")))) };
            let carried = (on.clone(), package.manifest.clone());
            let turn = lock(&CARRYING).entry(carried.clone()).or_default().clone();
            let _carrying = turn.lock().await;
            let mut held = lock(&CARRIED).get(&carried).cloned();
            // There already, a run of it having carried it: carried by its manifest alone.
            if held.is_none() && crate::stages::stage("held there", is_there(api, on, &package.manifest)).await {
                let source = v1::LocalSource { manifest: package.manifest.clone() };
                lock(&CARRIED).insert(carried.clone(), source.clone());
                held = Some(source);
            }
            let source = match held {
                Some(source) => source,
                None => {
                    let hubs = &api.machines.hubs;
                    let origin = Origin::parse(&if hub.is_empty() { hubs.default_origin() } else { hub.to_owned() })?;
                    let staged = crate::stages::stage("source", crate::local_code::source(hubs.home(), &crate::local_code::own(api, path)?, hubs, &origin)).await?;
                    if let Err(why) = crate::stages::stage("carried", api.calcifer.ship(on, &staged.inputs)).await {
                        return Ok(Err(Cow::Owned(format!("its local code wasn't carried to {on}: {why}"))));
                    }
                    lock(&CARRIED).insert(carried, staged.source.clone());
                    staged.source
                }
            };
            v1::Source::Local(source)
        }
    };
    let request = crate::stages::stage("bind", async { asking.request(name, &package, &renderer.function) }).await;
    // What its request is, as its machine will plan it: the fields it sends, and what its rules make of them.
    let answers = (request.as_ref().zip(api::interface::entrypoint(package.document(), &renderer.function))).map(|(request, entry)| ::needs::answers(entry, request));
    let fields: Option<Vec<&String>> = request.as_ref().and_then(Value::as_object).map(|fields| fields.keys().collect());
    // What its prompts reference, by field and kind: what an edit's rules read (a picture box's own picture is one).
    let references: Vec<String> = (request.as_ref().and_then(Value::as_object).into_iter().flatten())
        .flat_map(|(field, value)| value["references"].as_array().into_iter().flatten().map(move |reference| format!("{field}: {}", reference["type"].as_str().unwrap_or("?"))))
        .collect();
    let (needed, unneeded, unanswered) = (answers.as_ref().map(|answers| &answers.needed), answers.as_ref().map(|answers| &answers.unneeded), answers.as_ref().map(|answers| &answers.unanswered));
    tracing::info!(
        target: crate::prefetch::TARGET,
        "guess {name} on {machine}: {}/{} with {fields:?}, referencing {references:?}; its rules hold for {needed:?}, not for {unneeded:?}, give no answer for {unanswered:?}",
        package.name, renderer.function
    );
    // Once a box's function is guessed, what any edit of it reads of the Hub is read behind it.
    if READ_AHEAD.lock().expect("read-ahead lock").insert((hub.to_owned(), package.name.clone(), package.release.clone(), renderer.function.clone())) {
        let (api, hub, package, function, made) = (api.clone(), hub.to_owned(), package.clone(), renderer.function.clone(), made.clone());
        tokio::spawn(async move { super::fit::read_ahead(&api, &hub, &package, &function, &made).await });
    }
    let seed = project.draft_seed(name);
    // What it was made of, kept with it: a round in which that holds keeps it (`kept`).
    let key = request_key(with, kind, made, seed, request.as_ref());
    let made_of = Made {
        key,
        load: api.machines.lists.load_moved.load(Ordering::Acquire),
        arrived: *api.machines.arrivals.borrow(),
        package: package.clone(),
        function: renderer.function.clone(),
        guess: None,
    };
    lock(&MADE).insert((dir.to_string_lossy().into_owned(), name.to_owned()), made_of);
    let mut spec = super::predicted(&renderer, made, request, seed, source)?;
    // Its files, written to its machine behind the item (cl-1123): the item is its render's request to the byte, and its machine has
    // them before the render is sent; its plan reads none of them, so the item goes at once (44065133454: a first write held the
    // edit's guess 1.4 s). An encrypted project's go only with the run that reads them.
    if crate::private_io::of_path(dir)?.is_none_or(|scope| scope.storage.ordinary) {
        let (inputs, staged) = super::predicted_inputs(&project.folder, &asking.lock, &spec);
        if let (On::Machine(on), false) = (machine, staged.is_empty()) {
            let (calcifer, on) = (api.calcifer.clone(), on.clone());
            tokio::spawn(async move {
                if let Err(why) = calcifer.ship(&on, &staged).await {
                    tracing::warn!(machine = on, "a guess's files weren't written there: {why}");
                }
            });
        }
        spec.inputs = inputs;
    }
    // Its key is its machine's list's to give (`Lists::key`).
    // Local code's interface as read here: its machine resolves its slots while it installs the code (cl-1123).
    let interface = renderer.local.as_ref().and_then(|_| v1::Json::parse(package.document().to_string()).ok());
    Ok(Ok((v1::Predicted { key: String::new(), spec, interface }, package.name.clone() + "/" + &renderer.function, key)))
}

/// `machine` holds local source `manifest` whole, as Calcifer kept it.
async fn is_there(api: &Api, machine: &str, manifest: &str) -> bool {
    match crate::local_code::kept_manifest(api.machines.hubs.home(), manifest) {
        Some((digest, length)) => api.calcifer.holds(machine, &digest, length).await,
        None => false,
    }
}

/// The digest `renderer`'s release must install (xs-196): the one this project's lock recorded at its first run of it, else its card's.
/// A guess records nothing: its render does (`Walk::pinned`).
fn recorded(api: &Arc<Api>, project: &Project, hub: &str, renderer: &Renderer) -> Result<String, Error> {
    let hub = crate::runs::hub(api, hub);
    let key = format!("{}@{} {hub}", renderer.package, renderer.release);
    Ok(project.folder.reading_lock(|lock| lock.releases.get(&key).cloned())?.unwrap_or_else(|| renderer.digest.clone()))
}
