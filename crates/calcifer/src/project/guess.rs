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

use super::{Asking, PackageRead, Project, host::{Host, Renderer}, job_of};
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

/// Moves whenever a project closes: each one's guesses end with it.
static CLOSED: LazyLock<watch::Sender<u64>> = LazyLock::new(|| watch::channel(0).0);
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Local code carried to a machine by a guess (`item`), by machine and its package's manifest: written there once, its source as staged.
static CARRIED: LazyLock<Mutex<std::collections::HashMap<(String, String), v1::LocalSource>>> = LazyLock::new(Mutex::default);
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
        loop {
            if stale {
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
                let began = std::time::Instant::now();
                let guesses = guesses(&api, &project).await;
                tracing::info!(target: crate::prefetch::TARGET, "{folder}: {} guesses in {} ms ({why})", guesses.len(), began.elapsed().as_millis());
                api.machines.lists.guess(&api.machines, &folder, guesses);
                stale = false;
            }
            tokio::select! {
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
        api.machines.lists.guess_closed(&api.machines, &folder);
    });
}

/// `project`'s guesses as it stands: one for each box a render would make, where a machine that serves could run it.
async fn guesses(api: &Arc<Api>, project: &Arc<Project>) -> Vec<Guess> {
    // Its runs, which its times on each machine stand on, are its own: read in its storage, as its estimates are (`fit`).
    let Ok(scope) = crate::private_io::of_path(&project.folder.dir) else { return vec![] };
    crate::private_io::context(scope, guesses_in(api, project)).await
}

async fn guesses_in(api: &Arc<Api>, project: &Arc<Project>) -> Vec<Guess> {
    if project.require_view().is_err() {
        return vec![];
    }
    // What each box's render would send, which its function's needs rules read (cl-1103).
    let Some(asking) = Asking::of(&project.folder) else { return vec![] };
    let boxes: Vec<Boxed> = {
        let doc = &asking.doc;
        // What a render of everything would make: each box with a tool of a package, not approved.
        (doc.order.iter().copied())
            .filter(|&i| doc.makes(i) && doc.tool(i).is_none() && doc.assets[i].pin.is_none())
            .take(BOXES)
            .map(|i| (doc.assets[i].name.clone(), doc.target(i), doc.assets[i].kind.clone(), doc.assets[i].made.clone(), doc.media_read(i)))
            .collect()
    };
    if boxes.is_empty() {
        return vec![];
    }
    let Ok(on) = crate::runs::unnamed_set(api, Some(&project.id)).await else { return vec![] };
    let (hub, host, folder) = (api.machines.hubs.default_origin(), Host::new(api), project.folder.dir.to_string_lossy().into_owned());
    let names: Vec<String> = boxes.iter().map(|(name, ..)| name.clone()).collect();
    let each = boxes.into_iter().map(|boxed| Box::pin(guess(api, project, &host, &hub, &folder, on.clone(), boxed, &asking)));
    let made = futures_util::future::join_all(each).await;
    // A box whose guess couldn't be made this time (a read that didn't answer, its machine between frames) keeps the one it had, so
    // its item keeps its key and its machine goes on making it ready: what dropped and listed again 13 s later in 44057515041.
    (names.into_iter().zip(made))
        .filter_map(|(name, made)| match made {
            Guessed::Made(guess) => Some(*guess),
            Guessed::Nothing => None,
            Guessed::Unknown(why) => {
                let kept = api.machines.lists.last_guess(&folder, &name);
                tracing::info!(target: crate::prefetch::TARGET, "{folder}: {name}'s guess wasn't made ({why}); {}", if kept.is_some() { "it keeps the one it had" } else { "it has none" });
                kept
            }
        })
        .collect()
}

/// What a box's guess came to: made; nothing (its package makes nothing of its kind, or it is an encrypted project's local code); or
/// not known this time, and why.
enum Guessed {
    Made(Box<Guess>),
    Nothing,
    Unknown(&'static str),
}

/// The guess for one box: none when no machine that serves now could run it, or its package says nothing to run.
#[allow(clippy::too_many_arguments)]
async fn guess(api: &Arc<Api>, project: &Arc<Project>, host: &Host, hub: &str, folder: &str, on: Vec<On>, boxed: Boxed, asking: &Asking) -> Guessed {
    let began = std::time::Instant::now();
    let (name, with, kind, made, media) = boxed;
    let read = job_of(host, &project.folder.dir, name.clone(), on, hub, &with, made.clone(), PackageRead::Foreseen, Some((asking, name.as_str())));
    let mut job = match tokio::time::timeout(READ, read).await {
        Ok(Ok(Some(job))) => job,
        Ok(Ok(None)) => return Guessed::Unknown("its package is installing"),
        Ok(Err(_)) => return Guessed::Unknown("its quote couldn't be read"),
        Err(_) => return Guessed::Unknown("its quote didn't answer in time"),
    };
    job.size.refs = Some(media);
    let (offers, _) = placement::offers_now(api, std::slice::from_ref(&job)).await;
    let now = now_ms();
    let serves = |offer: &placement::Offer| offer.ready_at_ms.is_none() && matches!(&offer.machine, On::Machine(name) if api.machines.live_frame(name).is_some());
    let Some(offers) = offers.into_iter().next() else { return Guessed::Unknown("no machine quoted it") };
    let up: Vec<placement::Offer> = offers.into_iter().filter(serves).collect();
    let Some(soonest) = placement::soonest(&up, now) else { return Guessed::Unknown("no machine that serves now runs it") };
    let held = api.machines.lists.guessed_on(folder, &name).map(On::Machine);
    let held = held.and_then(|held| up.iter().find(|offer| offer.fits && offer.machine == held));
    let chosen = match held {
        Some(held) if !plan::clearly_sooner(soonest, held, now) => held,
        _ => soonest,
    };
    let On::Machine(machine) = &chosen.machine else { return Guessed::Nothing };
    // Where else it would finish, and how long each one's line holds it from GPUs: what a load moving there could change.
    let others = (up.iter().filter(|offer| offer.fits && offer.machine != chosen.machine))
        .filter_map(|offer| match &offer.machine {
            On::Machine(other) => Some((other.clone(), (offer.finish_at_ms, offer.waits_ms(now)))),
            _ => None,
        })
        .collect();
    let placed = crate::prefetch::Placed { finish_ms: chosen.finish_at_ms, others };
    let (item, function) = match item(api, host, project, hub, (name.as_str(), with.as_str(), kind.as_str(), &made), &chosen.machine, asking).await {
        Ok(Some(item)) => item,
        Ok(None) => return Guessed::Nothing,
        Err(_) => return Guessed::Unknown("its request couldn't be read"),
    };
    tracing::info!(target: crate::prefetch::TARGET, "{folder}: {name} guessed on {machine} in {} ms", began.elapsed().as_millis());
    Guessed::Made(Box::new(Guess { project: Arc::downgrade(project), asset: name, machine: machine.clone(), hub: hub.to_owned(), item, function, placed, draft: draft(&with, &made) }))
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
/// it (`Host::foreseen`). None: its package makes nothing of the kind, or it is an encrypted project's local code, which goes with
/// the run that calls it.
async fn item(api: &Arc<Api>, host: &Host, project: &Project, hub: &str, (name, with, kind, made): (&str, &str, &str, &Map<String, Value>), machine: &On, asking: &Asking) -> Result<Option<(v1::Predicted, String)>, Error> {
    let dir = &project.folder.dir;
    let package = host.foreseen(hub, with, dir, std::slice::from_ref(machine)).await?;
    let Some(renderer) = package.first_making(kind) else { return Ok(None) };
    let source = match &renderer.local {
        None => v1::Source::Release(v1::Release { package: renderer.package.clone(), release: renderer.release.clone(), digest: recorded(api, project, hub, &renderer)?, ..Default::default() }),
        // Carried to its machine as a run of it carries it (#3646), its files and manifest written there before the item, so a fresh
        // rental installs it too (frozen-RC 44000183889: H3 from a checkout never got ready on one); once a machine has them, by its
        // manifest alone. An encrypted project's own code goes only with the run that calls it; code beside the project (a dev
        // home's checkout, `~/…`) isn't the project's.
        Some(path) => {
            let private = crate::private_io::of_path(dir)?.is_some_and(|scope| !scope.storage.ordinary);
            if private && path.starts_with(dir) {
                return Ok(None);
            }
            let On::Machine(on) = machine else { return Ok(None) };
            let carried = (on.clone(), package.manifest.clone());
            let held = lock(&CARRIED).get(&carried).cloned();
            let source = match held {
                Some(source) => source,
                None => {
                    let hubs = &api.machines.hubs;
                    let origin = Origin::parse(&if hub.is_empty() { hubs.default_origin() } else { hub.to_owned() })?;
                    let staged = crate::local_code::source(hubs.home(), &crate::local_code::own(api, path)?, hubs, &origin).await?;
                    if let Err(why) = api.calcifer.ship(on, &staged.inputs).await {
                        tracing::warn!(machine = on, "a guess's local code wasn't carried there: {why}");
                        return Ok(None);
                    }
                    lock(&CARRIED).insert(carried, staged.source.clone());
                    staged.source
                }
            };
            v1::Source::Local(source)
        }
    };
    let request = asking.request(name, &package, &renderer.function);
    // What its request is, as its machine will plan it: the fields it sends, and what its rules make of them.
    let answers = (request.as_ref().zip(api::interface::entrypoint(package.document(), &renderer.function))).map(|(request, entry)| ::needs::answers(entry, request));
    let fields: Option<Vec<&String>> = request.as_ref().and_then(Value::as_object).map(|fields| fields.keys().collect());
    let (needed, unneeded, unanswered) = (answers.as_ref().map(|answers| &answers.needed), answers.as_ref().map(|answers| &answers.unneeded), answers.as_ref().map(|answers| &answers.unanswered));
    tracing::info!(
        target: crate::prefetch::TARGET,
        "guess {name} on {machine}: {}/{} with {fields:?}; its rules hold for {needed:?}, not for {unneeded:?}, give no answer for {unanswered:?}",
        package.name, renderer.function
    );
    // Once a box's function is guessed, what any edit of it reads of the Hub is read behind it.
    if READ_AHEAD.lock().expect("read-ahead lock").insert((hub.to_owned(), package.name.clone(), package.release.clone(), renderer.function.clone())) {
        let (api, hub, package, function, made) = (api.clone(), hub.to_owned(), package.clone(), renderer.function.clone(), made.clone());
        tokio::spawn(async move { super::fit::read_ahead(&api, &hub, &package, &function, &made).await });
    }
    let mut spec = super::predicted(&renderer, made, request, project.draft_seed(name), source)?;
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
    Ok(Some((v1::Predicted { key: String::new(), spec }, package.name.clone() + "/" + &renderer.function)))
}

/// The digest `renderer`'s release must install (xs-196): the one this project's lock recorded at its first run of it, else its card's.
/// A guess records nothing: its render does (`Walk::pinned`).
fn recorded(api: &Arc<Api>, project: &Project, hub: &str, renderer: &Renderer) -> Result<String, Error> {
    let hub = crate::runs::hub(api, hub);
    let key = format!("{}@{} {hub}", renderer.package, renderer.release);
    Ok(project.folder.reading_lock(|lock| lock.releases.get(&key).cloned())?.unwrap_or_else(|| renderer.digest.clone()))
}
