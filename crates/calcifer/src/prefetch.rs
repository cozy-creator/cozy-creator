//! What Calcifer has each machine get ready before it is asked: one system (cl-1104, Paul 11-16: "you know how google chrome, when
//! you're typing into a browser, will pre-fetch the url you're likely to complete typing? … when a user attaches a tool … we could
//! figure out what machine, if we got a request, that request would land on. On that machine, we can pre-emptively install packages +
//! models, so that when we do get a request, we'll have those already ready"). Three things feed it, by how sure each is:
//! - a walk's planned jobs for a machine, in the order they would start (`PUT /v1/upcoming`, cl-967, Paul 11-12): each will be sent;
//! - each open project's guesses (`PUT /v1/prepare`): what a render of each of its boxes would run, on the machine a render of it
//!   would go to now (`project::guess`), from the moment a tool is on a box, again as its request is edited;
//! - for a machine with neither and nothing in its queue while other machines hold unstarted work it can run, that work's
//!   functions, the most first (cl-967's prepare: "if calcifer expects a large number of minimax h3 jobs, and a new machine just
//!   started, telling it to download minimax h3 … so that it's ready").
//!
//! Each item is a request Calcifer predicts (cl-1123; Paul, 11-17: "calcifer only has to worry about what it knows, i.e., it predicts
//! what the user MAY want in the future, and the remote machine only needs to worry about what it's told, what a request MAY look
//! like, to ensure it's ready for it"): the spec a render of the box, or the walk's job, would send. The machine plans it as it would
//! plan that run and owns everything about being ready for it: its code, its downloads, its lane.
//!
//! Each list goes when it changes, and again once its machine starts again; the machine fetches, evicts and reports. A prepare item
//! is no job there (`tensord::ahead`): behind every run's needs and guesses in its lines, never a rental's use, the first to go on a
//! short disk. A prepare list's order is the order its machine makes its items ready in (cl-1278): what a render named, then the
//! guesses, the box touched last first (the likeliest rendered next), then demand elsewhere. A guess that changes goes as the new
//! list: its machine cancels what the list drops but what the next item reads too (Paul: "I switch to another checkpoint, then
//! cancel the parts of that download which are not needed"). Each item's row moves with the downloads its machine's Status lists
//! under it: an upcoming job's on its walk's board, a guess's on its boxes ("Ready on kamitsure"), and only while it is the guess.
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use api::{calcifer::project::Ahead as Row, machine as v1};
use sha2::{Digest as _, Sha256};
use tracing::warn;

use crate::{machines::Machines, project::Project};

/// What Calcifer's log says of its predictions (cl-1123): each guess round, each box's guess, each list sent and how long each took.
pub(crate) const TARGET: &str = "cozy.prefetch";

/// A prediction's line about the project in `folder` (its readable folder): Calcifer's log names an unencrypted project's folder and
/// boxes, and nothing of an encrypted one, whose line goes to its own log in its vault, kept only for a test's diagnostics
/// (`private_io::diagnose`): a locked project leaves nothing of itself in Calcifer's (cl-1123).
pub(crate) fn say(folder: &str, line: impl FnOnce() -> String) {
    let dir = std::path::Path::new(folder);
    match !crate::project::vault::ordinary_storage(dir) || crate::project::vault::encrypted_within(dir) {
        true => crate::private_io::diagnose(dir, line),
        false => tracing::info!(target: TARGET, "{}", line()),
    }
}

/// How long a prepare item a render named stays listed for it at most, should its machine never answer the render.
const PINNED: std::time::Duration = std::time::Duration::from_secs(600);

/// The prefixes the machine's runs of an upcoming and of a prepare list take (`tensord::ahead`).
pub(crate) const UPCOMING: &str = "cozy:upcoming:";
pub(crate) const PREPARE: &str = "cozy:prepare:";

/// A list item's run (`tensord::ahead`): a box's guess, or a job fetched ahead. No run of Calcifer's.
pub(crate) fn listed(id: &str) -> bool {
    id.starts_with(UPCOMING) || id.starts_with(PREPARE)
}

/// This Calcifer, in a prepare item's key: one a Calcifer before this listed is started anew, never answered as it ended then.
static BORN: LazyLock<String> = LazyLock::new(|| format!("{:x}", crate::store::now_ms()));

/// One job a walk planned on a machine, as it is to be ready there.
#[derive(Clone)]
pub(crate) struct Listed {
    pub project: Arc<Project>,
    /// Its box, which its row names.
    pub asset: String,
    pub start_at_ms: i64,
    pub hub: String,
    pub item: v1::Predicted,
    /// `org/package/function`, as its row says it.
    pub function: String,
    /// The machines its plan found can run it: where it is prepared with no job.
    pub fits: Vec<String>,
}

/// What a render of a project's box would run, on the machine it would go to now: a guess, with no job.
#[derive(Clone)]
pub(crate) struct Guess {
    /// Its project while it's open: a guess keeps none open.
    pub project: std::sync::Weak<Project>,
    pub asset: String,
    pub machine: String,
    pub hub: String,
    /// Its key is its machine's list's to give (`Lists::key`).
    pub item: v1::Predicted,
    pub function: String,
    pub placed: Placed,
    /// Its box's draft request, hashed (`guess::draft`): edited, it is a new guess, and a failed one starts over.
    pub draft: u64,
    /// When that draft was last new (`guess::touched`): the newest goes first on its machine's prepare list (cl-1278).
    pub touched: std::time::Instant,
    /// The files its item's request reads (`spec.inputs`), carried to its machine ahead of its render (`Calcifer::carry_ahead`).
    pub files: Vec<crate::AheadFile>,
}

/// A guess's tries in a row that failed, on one machine (cl-1104): as any request is asked again (retry-standard-api-practice).
struct Tries {
    backoff: tensorfs::fetch::Backoff,
    /// The run the last word is of: each Status of it says it again.
    run: String,
    /// Ok: tried again soon, said with its attempt (`<why>; attempt 2/5 in 6 s`). Err: it stays failed, in a line.
    said: Result<String, String>,
}

/// Where a guess's render would finish when it was placed (`placement::Offer`): on its machine, and on each other machine that serves
/// and fits it, with how long that one's line held it from GPUs. What a machine's load moving could change of it (`Lists::could_move`).
#[derive(Clone, Default)]
pub(crate) struct Placed {
    pub finish_ms: i64,
    pub others: HashMap<String, (i64, i64)>,
}

#[derive(Clone, Default, PartialEq)]
struct Sent {
    /// The machine's run it went to (`started`).
    boot: String,
    upcoming: Vec<v1::Predicted>,
    prepare: Vec<v1::Predicted>,
}

/// A machine's run: its boot, and its service's start in it. A service that started again (an update, a restart) holds no list.
fn started(frame: &v1::StatusFrame) -> String {
    format!("{}@{}", frame.boot_id, frame.started_at_unix_ms)
}

/// Every machine's lists.
#[derive(Default)]
pub(crate) struct Lists {
    /// By machine, by project folder: what its walk planned there.
    planned: Mutex<HashMap<String, HashMap<String, Vec<Listed>>>>,
    /// By project folder: its guesses.
    guessed: Mutex<HashMap<String, Vec<Guess>>>,
    /// By machine: what it was last sent, on which boot of it.
    sent: Mutex<HashMap<String, Sent>>,
    /// By machine and what an item makes ready (`identity`): the key it has while it stays on that machine's prepare list. Listed
    /// there again later, it takes another, and its machine starts it anew.
    keys: Mutex<HashMap<(String, String), String>>,
    listings: AtomicU64,
    /// A prepare item its machine says it lost was given a new key: the lists go again at its next Status (`seen`).
    rekeyed: std::sync::atomic::AtomicBool,
    /// By machine: what of its Status a quote of its load reads (`weigh`), and a count that moves whenever one's changes.
    loads: Mutex<HashMap<String, Load>>,
    pub(crate) load_moved: AtomicU64,
    /// Each move of `load_moved`, the newest `MOVES`: its count, its machine and what moved.
    moves: Mutex<std::collections::VecDeque<(u64, String, Moved)>>,
    /// By machine and key: a prepare item a render names (`RunSpec::predicted`, cl-1123), kept on its list until the machine answers
    /// that render (`answered`), whatever its box's guess is meanwhile: the request it made ready is the one the render adopts. With
    /// its Hub, the project and box it was guessed for, and since when.
    pinned: Mutex<HashMap<(String, String), Pinned>>,
    /// By machine and what an item makes ready (`identity`): its guess's failed tries, until it is ready or a new guess.
    tries: Mutex<HashMap<(String, String), Tries>>,
}

#[derive(Clone)]
struct Pinned {
    hub: String,
    item: v1::Predicted,
    /// Its project's folder, and its box.
    project: String,
    asset: String,
    since: std::time::Instant,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A prepare item's identity: the request it predicts, the same however many boxes or jobs ask it, its access aside. A draft edited is
/// another item (cl-1123), so its machine ends what only the old one wanted.
fn identity(item: &v1::Predicted) -> String {
    let spec = v1::RunSpec { hub: None, tensorfs: None, providers: None, ..item.spec.clone() };
    let shape = serde_json::to_value(&spec).unwrap_or_default();
    let digest = Sha256::digest(shape.to_string().as_bytes());
    digest.iter().take(12).map(|byte| format!("{byte:02x}")).collect()
}

impl Lists {
    /// What `project`'s walk planned now, by machine: each machine's lists go again where that changed them.
    pub(crate) fn set(&self, machines: &Arc<Machines>, project: &str, by_machine: HashMap<String, Vec<Listed>>) {
        {
            let mut planned = lock(&self.planned);
            for lists in planned.values_mut() {
                lists.remove(project);
            }
            for (machine, listed) in by_machine {
                planned.entry(machine).or_default().insert(project.to_owned(), listed);
            }
            planned.retain(|_, lists| !lists.is_empty());
        }
        self.send(machines);
    }

    /// `project`'s guesses now (none: it closed, or guesses nothing): each machine's prepare list goes again where that changed it, and
    /// a row of a guess that is one no more names no box.
    pub(crate) fn guess(&self, machines: &Arc<Machines>, project: &str, guesses: Vec<Guess>) {
        // A box new or its draft edited is a new guess: what failed of its item on its machine starts over, under a new key.
        let renewed: Vec<(String, String)> = {
            let guessed = lock(&self.guessed);
            let before = guessed.get(project);
            let same = |guess: &Guess| before.is_some_and(|before| before.iter().any(|was| was.asset == guess.asset && was.draft == guess.draft));
            guesses.iter().filter(|guess| !same(guess)).map(|guess| (guess.machine.clone(), identity(&guess.item))).collect()
        };
        {
            let mut tries = lock(&self.tries);
            let mut keys = lock(&self.keys);
            for at in renewed {
                if tries.remove(&at).is_some() {
                    keys.remove(&at);
                }
            }
        }
        let was = match guesses.is_empty() {
            true => lock(&self.guessed).remove(project),
            false => lock(&self.guessed).insert(project.to_owned(), guesses),
        };
        self.send(machines);
        let now: HashSet<String> = self.rows(project).into_iter().map(|(id, ..)| id).collect();
        if let Some(project) = was.as_ref().and_then(|was| was.first()).and_then(|guess| guess.project.upgrade()) {
            project.jobs.guesses_kept(&now);
        }
        // What its machines last said of them, now: a guess its machine has ready already says so at once.
        let on: HashSet<String> = lock(&self.guessed).get(project).into_iter().flatten().map(|guess| guess.machine.clone()).collect();
        for machine in on {
            if let Some(frame) = machines.frame(&machine) {
                self.observe(machines, &machine, &frame);
            }
        }
    }

    /// Whether a machine `project` is guessed on serves no more: its guesses are to be made again.
    pub(crate) fn guess_left(&self, machines: &Arc<Machines>, project: &str) -> bool {
        let on: Vec<String> = lock(&self.guessed).get(project).into_iter().flatten().map(|guess| guess.machine.clone()).collect();
        on.iter().any(|machine| machines.live_frame(machine).is_none())
    }

    /// `project` closed: its guesses end, unless it opened again since and guesses anew.
    pub(crate) fn guess_closed(&self, machines: &Arc<Machines>, project: &str) {
        let open = lock(&self.guessed).get(project).is_some_and(|guesses| guesses.iter().any(|guess| guess.project.strong_count() > 0));
        if !open {
            self.guess(machines, project, vec![]);
        }
    }

    /// Whose list item `id` is, while it is listed: its project's id and the boxes it is for. What its machine does for it is that
    /// project's, as its runs' is (`Machines::operation`, cl-1104).
    pub(crate) fn owner(&self, id: &str) -> Option<(String, Vec<String>)> {
        if let Some(key) = id.strip_prefix(UPCOMING) {
            let planned = lock(&self.planned);
            let listed = planned.values().flat_map(|lists| lists.values().flatten()).find(|listed| listed.item.key == key)?;
            return Some((listed.project.id().to_owned(), vec![listed.asset.clone()]));
        }
        let folders: Vec<String> = lock(&self.guessed).keys().cloned().collect();
        for folder in folders {
            let Some((.., names)) = self.rows(&folder).into_iter().find(|(held, ..)| held == id) else { continue };
            let project = lock(&self.guessed).get(&folder)?.first()?.project.upgrade()?;
            return Some((project.id().to_owned(), names));
        }
        None
    }

    /// The machine `project`'s guess for box `asset` is on, where it has one.
    pub(crate) fn guessed_on(&self, project: &str, asset: &str) -> Option<String> {
        lock(&self.guessed).get(project)?.iter().find(|guess| guess.asset == asset).map(|guess| guess.machine.clone())
    }

    /// A render of box `asset` of `project` takes the seed its draft is guessed with (`Project::take_draft_seed`): the item its guess
    /// holds now is the request that render sends, so it stays listed until its machine answers (`answered`), whatever the box is
    /// guessed as meanwhile. 44188035440: the round the render's own writes started guessed the box afresh, with a fresh seed, before
    /// the render named its item, and the render named the new one, which it wasn't.
    pub(crate) fn taking(&self, project: &str, asset: &str) {
        let Some(guess) = lock(&self.guessed).get(project).and_then(|guesses| guesses.iter().find(|guess| guess.asset == asset).cloned()) else { return };
        let Some(key) = lock(&self.keys).get(&(guess.machine.clone(), identity(&guess.item))).cloned() else { return };
        let pinned = Pinned { hub: guess.hub, item: v1::Predicted { key: key.clone(), ..guess.item }, project: project.to_owned(), asset: asset.to_owned(), since: std::time::Instant::now() };
        lock(&self.pinned).insert((guess.machine.clone(), key.clone()), pinned);
        say(project, || format!("{asset}'s render takes its draft's seed: {key} on {} stays listed until the machine answers", guess.machine));
    }

    /// The key of the item `machine`'s lists predict job `job` (box `asset`) of `project` by (cl-1123): its upcoming job's, else the item
    /// whose request `spec` is to the byte (one a render pinned as it took its seed, `taking`), else its box's guess's, which stays
    /// listed until the machine answers the render (`answered`). What its render names (`RunSpec::predicted`), so the machine knows
    /// the request it made ready is this one, or says it isn't.
    pub(crate) fn predicted(&self, machine: &str, project: &str, asset: &str, job: &str, spec: &v1::RunSpec) -> Option<String> {
        let planned = lock(&self.planned).get(machine).and_then(|lists| lists.get(project)).and_then(|listed| listed.iter().find(|listed| listed.item.key == job).map(|listed| listed.item.key.clone()));
        if planned.is_some() {
            return planned;
        }
        let sent = identity(&v1::Predicted { key: String::new(), spec: v1::RunSpec { predicted: None, ..spec.clone() }, interface: None });
        let held = lock(&self.pinned).iter().find(|((on, _), pinned)| on == machine && pinned.asset == asset && identity(&pinned.item) == sent).map(|((_, key), _)| key.clone());
        if let Some(key) = held {
            say(project, || format!("{asset}'s render names {key} on {machine}, the request it made ready"));
            return Some(key);
        }
        let guess = lock(&self.guessed).get(project)?.iter().find(|guess| guess.asset == asset && guess.machine == machine)?.clone();
        let key = lock(&self.keys).get(&(machine.to_owned(), identity(&guess.item))).cloned()?;
        let same = identity(&guess.item) == sent;
        let pinned = Pinned { hub: guess.hub, item: v1::Predicted { key: key.clone(), ..guess.item }, project: project.to_owned(), asset: asset.to_owned(), since: std::time::Instant::now() };
        lock(&self.pinned).insert((machine.to_owned(), key.clone()), pinned);
        say(project, || match same {
            true => format!("{asset}'s render names {key} on {machine}: it stays listed until the machine answers"),
            false => format!("{asset}'s render names {key} on {machine}, its guess, though it isn't the request that guess made ready"),
        });
        Some(key)
    }

    /// `machine` answered a render that named item `key`: the item goes as its box's guess says now. The render took its draft's seed
    /// (`Project::take_draft_seed`), so that box's next draft is already predicted with a fresh one.
    pub(crate) fn answered(&self, machines: &Arc<Machines>, machine: &str, key: &str) {
        let Some(pinned) = lock(&self.pinned).remove(&(machine.to_owned(), key.to_owned())) else { return };
        say(&pinned.project, || format!("{machine} answered {}'s render, which named {key}", pinned.asset));
        self.send(machines);
    }

    /// `project`'s guesses as last made, for a box whose guess couldn't be made this time (a read that didn't answer, a machine
    /// between frames): it keeps the one it had, so its item keeps its key.
    pub(crate) fn last_guess(&self, project: &str, asset: &str) -> Option<Guess> {
        lock(&self.guessed).get(project)?.iter().find(|guess| guess.asset == asset).cloned()
    }

    /// The key `machine`'s prepare list has for what `item` makes ready, given now if it isn't on that list.
    fn key(&self, machine: &str, item: &v1::Predicted) -> String {
        let at = (machine.to_owned(), identity(item));
        let mut keys = lock(&self.keys);
        if let Some(key) = keys.get(&at) {
            return key.clone();
        }
        let key = format!("{}.{}.{}", at.1, *BORN, self.listings.fetch_add(1, Ordering::Relaxed));
        keys.insert(at, key.clone());
        key
    }

    /// `project`'s guesses as their machines' lists name them: each item's run id, machine, function and the boxes it is for.
    fn rows(&self, project: &str) -> Vec<(String, String, String, Vec<String>)> {
        let guessed = lock(&self.guessed).get(project).cloned().unwrap_or_default();
        let mut rows: Vec<(String, String, String, Vec<String>)> = vec![];
        for guess in guessed {
            let id = format!("{PREPARE}{}", self.key(&guess.machine, &guess.item));
            match rows.iter_mut().find(|(held, ..)| *held == id) {
                Some((.., names)) => names.push(guess.asset),
                None => rows.push((id, guess.machine, guess.function, vec![guess.asset])),
            }
        }
        rows
    }

    /// `machine` said its Status: what its lists concern it with goes now unless this boot of it was sent it (it came up since the
    /// lists were set, or started again and holds none).
    pub(crate) fn seen(&self, machines: &Arc<Machines>, machine: &str) {
        if self.rekeyed.swap(false, Ordering::AcqRel) {
            self.send(machines);
        }
        let Some(frame) = machines.frame(machine) else { return };
        let concerned = {
            let planned = lock(&self.planned);
            planned.contains_key(machine)
                || planned.values().flat_map(|lists| lists.values().flatten()).any(|listed| listed.fits.iter().any(|fits| fits == machine))
                || lock(&self.guessed).values().flatten().any(|guess| guess.machine == machine)
        };
        let sent = lock(&self.sent).get(machine).map(|sent| sent.boot.clone());
        if sent == Some(started(&frame)) || (sent.is_none() && !concerned) {
            return;
        }
        lock(&self.sent).remove(machine);
        self.send(machines);
    }

    /// Each machine's lists as they stand, each sent where it differs from what that machine was last sent.
    /// Each machine's lists again, as they are now (cl-1064: a clean-up a lock owed local was paid).
    pub(crate) fn again(&self, machines: &Arc<Machines>) {
        self.send(machines);
    }

    fn send(&self, machines: &Arc<Machines>) {
        let planned = lock(&self.planned).clone();
        let mut guessed: Vec<Guess> = lock(&self.guessed).values().flatten().cloned().collect();
        // Nothing of a project a lock owes local a clean-up for is warmed on local until it's paid (cl-1064): its clean-up would
        // end that warm-up again, and the project's own runs there wait for it as well. Other machines' lists are as planned.
        let owed = machines.owed_folders();
        let mut planned_local = planned.clone();
        if !owed.is_empty() {
            for lists in planned_local.values_mut() {
                for listed in lists.values_mut() {
                    listed.retain(|item| !owed.iter().any(|folder| *folder == item.project.folder_root()));
                }
            }
            guessed.retain(|guess| guess.machine != "local" || guess.project.upgrade().is_none_or(|project| !owed.iter().any(|folder| *folder == project.folder_root())));
        }
        // The box touched last is the likeliest rendered next (cl-1278): each machine's guesses go newest first, the rest as they stood.
        guessed.sort_by_key(|guess| std::cmp::Reverse(guess.touched));
        // What each machine's guesses read, carried there behind every run's carry (`Calcifer::carry_ahead`, cl-1123): each file of a
        // project once, with the boxes that read it.
        let mut files: HashMap<String, Vec<crate::AheadFile>> = HashMap::new();
        for guess in guessed.iter().filter(|guess| guess.project.strong_count() > 0) {
            let at = files.entry(guess.machine.clone()).or_default();
            for file in &guess.files {
                match at.iter_mut().find(|held| held.project == file.project && held.input.digest == file.input.digest) {
                    Some(held) => {
                        for name in &file.boxes {
                            if !held.boxes.contains(name) {
                                held.boxes.push(name.clone());
                            }
                        }
                    }
                    None => at.push(file.clone()),
                }
            }
        }
        machines.calcifer.carry_ahead(files);
        // Each machine planned or guessed on, sent before, or able to run what's planned: an idle one may prepare it.
        let mut names: Vec<String> = planned.keys().cloned().collect();
        names.extend(lock(&self.sent).keys().cloned());
        names.extend(planned.values().flat_map(|lists| lists.values().flatten()).flat_map(|listed| listed.fits.iter().cloned()));
        names.extend(guessed.iter().map(|guess| guess.machine.clone()));
        names.sort();
        names.dedup();
        for name in names {
            let Some(frame) = machines.frame(&name) else { continue };
            let planned = if name == "local" { &planned_local } else { &planned };
            let mut upcoming: Vec<&Listed> = planned.get(&name).into_iter().flat_map(|lists| lists.values().flatten()).collect();
            upcoming.sort_by_key(|listed| listed.start_at_ms);
            // Its list, each item once, in the order its machine makes them ready (cl-1278): what a render named, until the machine
            // answers it (`answered`), the oldest first; its guesses, the box touched last first; then what demand elsewhere asks of
            // it that no guess does: nothing of an encrypted project where its request may not go ahead (`Calcifer::sealed_from`,
            // cl-1123).
            let mut prepare: Vec<(String, String, v1::Predicted)> = vec![];
            lock(&self.pinned).retain(|_, pinned| pinned.since.elapsed() < PINNED);
            let mut pinned: Vec<Pinned> = lock(&self.pinned).iter().filter(|((machine, _), _)| *machine == name).map(|(_, pinned)| pinned.clone()).collect();
            pinned.sort_by_key(|pinned| pinned.since);
            let pinned = pinned.into_iter().map(|pinned| (pinned.hub, v1::Predicted { key: String::new(), ..pinned.item }));
            let sealed = upcoming.is_empty().then(|| machines.calcifer.sealed_from(&name)).flatten();
            let asked = (guessed.iter().filter(|guess| guess.machine == name).map(|guess| (guess.hub.clone(), guess.item.clone())))
                .chain(demand(&planned, &name, &frame, upcoming.is_empty(), sealed.as_deref()));
            for (hub, item) in pinned.chain(asked) {
                let made = identity(&item);
                if !prepare.iter().any(|(held, ..)| *held == made) {
                    prepare.push((made, hub, item));
                }
            }
            let listed: HashSet<&str> = prepare.iter().map(|(made, ..)| made.as_str()).collect();
            lock(&self.keys).retain(|(machine, made), _| *machine != name || listed.contains(made.as_str()));
            lock(&self.tries).retain(|(machine, made), _| *machine != name || listed.contains(made.as_str()));
            let prepare: Vec<(String, v1::Predicted)> = prepare.into_iter().map(|(_, hub, item)| (hub, v1::Predicted { key: self.key(&name, &item), ..item })).collect();
            let now = Sent {
                boot: started(&frame),
                upcoming: upcoming.iter().map(|listed| listed.item.clone()).collect(),
                prepare: prepare.iter().map(|(_, item)| item.clone()).collect(),
            };
            let was = lock(&self.sent).get(&name).cloned();
            if was.as_ref() == Some(&now) {
                continue;
            }
            // A machine that isn't up is sent nothing, started or kept for nothing: its lists go once it says its Status (`seen`).
            if machines.live_frame(&name).is_none() {
                lock(&self.sent).remove(&name);
                continue;
            }
            // Only a list that changed goes, each a round trip on an edit's way to its machine (cl-1123); both to one started since.
            let started_since = |was: &Sent| was.boot != now.boot;
            let upcoming_goes = was.as_ref().is_none_or(|was| started_since(was) || was.upcoming != now.upcoming);
            let prepare_goes = was.as_ref().is_none_or(|was| started_since(was) || was.prepare != now.prepare);
            lock(&self.sent).insert(name.clone(), now);
            let keys: Vec<&str> = prepare.iter().map(|(_, item)| item.key.as_str()).collect();
            let said = |goes: bool| if goes { "" } else { " (unchanged)" };
            tracing::info!(target: TARGET, "lists to {name}: {} upcoming{}, prepare {keys:?}{}", upcoming.len(), said(upcoming_goes), said(prepare_goes));
            let hubs: Vec<String> = upcoming.iter().map(|listed| listed.hub.clone()).chain(prepare.iter().map(|(hub, _)| hub.clone())).collect();
            let upcoming: Vec<(String, v1::Predicted)> = upcoming.iter().map(|listed| (listed.hub.clone(), listed.item.clone())).collect();
            let (upcoming, prepare) = (upcoming_goes.then_some(upcoming), prepare_goes.then_some(prepare));
            let machines = machines.clone();
            machines.clone().spawn(async move {
                if let Err(why) = put(&machines, &name, hubs, upcoming, prepare).await {
                    warn!(machine = name, "its upcoming jobs: {why}");
                    // Sent again at the next change.
                    lock(&machines.lists.sent).remove(&name);
                }
            });
        }
    }

    /// `machine`'s Status: each upcoming job's and each guess's row there moves with its item's run and the downloads it lists under
    /// it, and is ready once that run succeeded. A guess that failed is tried again or says why it stays failed (`tried`).
    pub(crate) fn observe(&self, machines: &Arc<Machines>, machine: &str, frame: &v1::StatusFrame) {
        self.weigh(machine, frame);
        let row_of = |id: &str, function: &str, names: Vec<String>| {
            let run = frame.ahead.iter().find(|run| run.id == id)?;
            let ops: Vec<&v1::Operation> = frame.activity.iter().filter(|op| op.runs.iter().any(|run| run == id)).collect();
            let downloads: Vec<&&v1::Operation> = ops.iter().filter(|op| op.kind == "download").collect();
            let ended = matches!(run.state.as_str(), "succeeded" | "failed" | "canceled");
            // What it made ready went since (`evicted`, cl-1104): no longer ready, and fetched again once its machine says it may.
            if run.state == "evicted" {
                return Some(Row {
                    ready: false,
                    held_back: None,
                    waiting: (!run.waiting.is_empty()).then(|| run.waiting.clone()),
                    function: function.to_owned(),
                    machine: machine.to_owned(),
                    run: 0,
                    names,
                    stage: Some("fetching it again".to_owned()),
                    done: 0,
                    total: 0,
                    guess: false,
                });
            }
            Some(Row {
                ready: run.state == "succeeded",
                // A fetch ahead that failed is no failure of the job: its own run makes it ready, and says so if that fails.
                held_back: None,
                waiting: (!run.waiting.is_empty() && !ended).then(|| run.waiting.clone()),
                function: function.to_owned(),
                machine: machine.to_owned(),
                run: 0,
                names,
                stage: (!ended).then(|| match ops.first() {
                    Some(op) if op.kind == "install" => "installing it".to_owned(),
                    Some(op) => format!("downloading {}", op.subject),
                    None if run.readiness.as_ref().is_some_and(|ready| ready.stage == "planning") => "planning it".to_owned(),
                    None => "preparing it".to_owned(),
                }),
                // How far its machine says it is (cl-1123), else what its downloads there come to.
                done: run.readiness.as_ref().filter(|ready| ready.stage == "downloading").map_or_else(|| downloads.iter().map(|op| op.bytes_done).sum(), |ready| ready.done),
                total: run.readiness.as_ref().filter(|ready| ready.stage == "downloading").map_or_else(|| downloads.iter().map(|op| op.bytes_total).sum(), |ready| ready.total),
                guess: false,
            })
        };
        {
            let planned = lock(&self.planned);
            for listed in planned.get(machine).into_iter().flat_map(|lists| lists.values().flatten()) {
                let id = format!("{UPCOMING}{}", listed.item.key);
                if let Some(row) = row_of(&id, &listed.function, vec![listed.asset.clone()]) {
                    listed.project.jobs.upcoming(&id, &listed.item.key, row);
                }
            }
        }
        let guessing: Vec<(String, Arc<Project>)> = {
            let guessed = lock(&self.guessed);
            let on = |guesses: &&Vec<Guess>| guesses.iter().any(|guess| guess.machine == machine);
            guessed.iter().filter(|(_, guesses)| on(guesses)).filter_map(|(folder, guesses)| Some((folder.clone(), guesses.first()?.project.upgrade()?))).collect()
        };
        for (folder, project) in guessing {
            let mut lost = false;
            for (id, _, function, names) in self.rows(&folder).into_iter().filter(|(_, on, ..)| on == machine) {
                let mut row = row_of(&id, &function, names);
                match frame.ahead.iter().find(|run| run.id == id) {
                    // Its machine lost what it made ready, and may fetch it again now: listed under a new key, it is started anew.
                    Some(run) if run.state == "evicted" && run.waiting.is_empty() && run.reason.is_none() => lost |= self.rekey(machine, &id),
                    // It failed (or failed for room, which there is now): tried again as any request is, else it says why it stays failed.
                    Some(run) if run.reason.is_some() && (run.state == "failed" || (run.state == "evicted" && run.waiting.is_empty())) => {
                        let reason = run.reason.as_ref().expect("a failed try says why");
                        let tried = self.tried(machines, machine, &id, reason, &folder, &project);
                        if let Some(row) = &mut row {
                            row.stage = None;
                            (row.waiting, row.held_back) = match tried {
                                Ok(again) => (Some(again), None),
                                Err(stays) => (None, Some(stays)),
                            };
                        }
                    }
                    Some(run) if run.state == "succeeded" => drop(lock(&self.tries).remove(&(machine.to_owned(), made_of(&id)))),
                    _ => {}
                }
                if let Some(row) = row {
                    project.jobs.guessed(&id, Row { guess: true, ..row });
                }
            }
            if lost {
                let now: HashSet<String> = self.rows(&folder).into_iter().map(|(id, ..)| id).collect();
                project.jobs.guesses_kept(&now);
                self.rekeyed.store(true, Ordering::Release);
            }
        }
    }

    /// Guess `id` on `machine` failed, as `reason` says (cl-1104: a failed guess stayed failed). One the next try may not repeat
    /// (`transient`) is tried again under a new key once its pause passes, said with its attempt (`<why>; attempt 2/5 in 6 s`); one
    /// that may, or whose tries are spent, stays failed until a new guess, said in a line. The pause is the one clock: nothing polls.
    fn tried(&self, machines: &Arc<Machines>, machine: &str, id: &str, reason: &v1::Reason, folder: &str, project: &Arc<Project>) -> Result<String, String> {
        let mut tries = lock(&self.tries);
        let tried = tries.entry((machine.to_owned(), made_of(id))).or_insert_with(|| Tries { backoff: tensorfs::fetch::Backoff::background(), run: String::new(), said: Err(String::new()) });
        // Each Status of the same failed run says it again.
        if tried.run == id {
            return tried.said.clone();
        }
        tried.run = id.to_owned();
        let why = line(&reason.message);
        tried.said = match transient(&reason.code) {
            false => Err(why),
            true => match tried.backoff.again(&why, false, None) {
                None => Err(tried.backoff.spent(&why)),
                Some((pause, said)) => {
                    let (held, machine, id, folder, project) = (machines.clone(), machine.to_owned(), id.to_owned(), folder.to_owned(), project.clone());
                    machines.clone().spawn(async move {
                        tokio::time::sleep(pause).await;
                        let lists = &held.lists;
                        if lists.rekey(&machine, &id) {
                            let now: HashSet<String> = lists.rows(&folder).into_iter().map(|(id, ..)| id).collect();
                            project.jobs.guesses_kept(&now);
                            lists.send(&held);
                        }
                    });
                    Ok(said)
                }
            },
        };
        tried.said.clone()
    }

    /// The prepare item `machine`'s list names `id` takes a new key at its next listing. Whether it had that one.
    fn rekey(&self, machine: &str, id: &str) -> bool {
        let key = id.trim_start_matches(PREPARE);
        let mut keys = lock(&self.keys);
        let before = keys.len();
        keys.retain(|(on, _), held| on != machine || held != key);
        keys.len() != before
    }

    /// What of `machine`'s Status could change where a render goes (cl-1104, cl-1123): the runs in its line (each where it stands),
    /// the update it may wait on, its phase and its GPUs out of service; and how long the weights its line waits for take at its
    /// link. `load_moved` moves when one of the first changed, or that time by the move threshold (`plan::STAY_MS`), and each open
    /// project's guesses are placed again. An install's or a model's download comes and goes in the line with each download, and
    /// its bytes tick down: neither moves it (44068453992: 26 of 30 rounds came of them, every 1–2 s).
    fn weigh(&self, machine: &str, frame: &v1::StatusFrame) {
        use std::hash::{Hash as _, Hasher as _};
        let mut loads = lock(&self.loads);
        let was = loads.get(machine).copied();
        let theirs = |root: &&v1::Queued| !root.run.starts_with(PREPARE) && !root.run.starts_with(UPCOMING);
        // A stream sends its queue only when it changed: absent, as its last frame said.
        let (line, runs, waits_ms) = match &frame.queue {
            Some(queue) => {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                // A run of a function, or one on GPUs; not an install or a model's download, which names no function and holds none.
                let line: Vec<&v1::Queued> =
                    queue.iter().filter(theirs).filter(|root| !root.function.is_empty() || !root.widths.is_empty() || root.width > 0).collect();
                for root in &line {
                    (&root.run, &root.function, root.running, root.width, root.gpu_waits, root.started_at_unix_ms).hash(&mut hasher);
                }
                let bytes: u64 = queue.iter().filter(theirs).map(|root| root.bytes_left).sum();
                let waits_ms = match frame.link_bytes_per_s > 0.0 {
                    true => (bytes as f64 / frame.link_bytes_per_s * 1000.0) as i64,
                    false => 0,
                };
                (hasher.finish(), line.len(), waits_ms)
            }
            None => was.map_or((0, 0, 0), |load| (load.line, load.runs, load.waits_ms)),
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let update = frame.update.as_ref().map(|update| update.state.as_str());
        let lost: Vec<_> = frame.gpus.iter().map(|gpu| &gpu.out_of_service).collect();
        (&frame.phase, update, lost).hash(&mut hasher);
        let state = hasher.finish();
        let moved = match was {
            None => None,
            Some(was) if was.line != line => Some(Moved::Line(runs.cmp(&was.runs))),
            Some(was) if was.state != state => Some(Moved::State),
            Some(was) if (waits_ms - was.waits_ms).abs() >= crate::plan::STAY_MS => Some(Moved::Waits(waits_ms - was.waits_ms)),
            Some(_) => return,
        };
        // The time its line waits stands as last weighed until it moves by the threshold, so a slow drift moves it once.
        loads.insert(machine.to_owned(), Load { line, runs, state, waits_ms });
        drop(loads);
        if let Some(moved) = moved {
            let count = self.load_moved.fetch_add(1, Ordering::AcqRel) + 1;
            let mut moves = lock(&self.moves);
            moves.push_back((count, machine.to_owned(), moved));
            while moves.len() > MOVES {
                moves.pop_front();
            }
            tracing::info!(target: TARGET, "{machine}'s load moved: {}", moved.said());
        }
    }

    /// Of the machines' loads that moved since `load_moved` was `since`, one that could change where a guess of `project` goes, said:
    /// its machine got later while another could finish it clearly sooner; or another got sooner by enough to (its GPU wait gone,
    /// for one with fewer runs in its line; its weights' wait, by how much it fell). Its phase, update or GPUs moving always could.
    pub(crate) fn could_move(&self, project: &str, since: u64, now: i64) -> Option<String> {
        #[derive(Default)]
        struct Since {
            fewer: bool,
            later: bool,
            state: bool,
            waits_ms: i64,
        }
        let mut by_machine: HashMap<String, Since> = HashMap::new();
        {
            let moves = lock(&self.moves);
            if moves.front().is_some_and(|(count, ..)| *count > since + 1) {
                return Some("more machines' loads moved than are kept".to_owned());
            }
            for (_, machine, moved) in moves.iter().filter(|(count, ..)| *count > since) {
                let at = by_machine.entry(machine.clone()).or_default();
                match moved {
                    Moved::Line(std::cmp::Ordering::Less) => at.fewer = true,
                    Moved::Line(_) => at.later = true,
                    Moved::State => at.state = true,
                    Moved::Waits(by) => at.waits_ms += by,
                }
            }
        }
        let sooner = crate::plan::finishes_clearly_sooner;
        let guessed = lock(&self.guessed);
        let guesses = guessed.get(project).map(Vec::as_slice).unwrap_or_default();
        for (machine, moved) in by_machine {
            let could = moved.state
                || guesses.iter().any(|guess| {
                    let placed = &guess.placed;
                    match guess.machine == machine {
                        true => {
                            (moved.later && !placed.others.is_empty())
                                || (moved.waits_ms > 0 && placed.others.values().any(|(other, _)| sooner(*other, placed.finish_ms + moved.waits_ms, now)))
                        }
                        false => placed.others.get(&machine).is_some_and(|(finish, gpu_wait)| {
                            (moved.fewer && sooner(finish - gpu_wait, placed.finish_ms, now))
                                || (moved.waits_ms < 0 && sooner(finish + moved.waits_ms, placed.finish_ms, now))
                        }),
                    }
                });
            if could {
                let what = [(moved.state, "its phase, update or GPUs"), (moved.fewer, "fewer runs in its line"), (moved.later, "more runs in its line")];
                let mut said: Vec<String> = what.iter().filter(|(held, _)| *held).map(|(_, what)| (*what).to_owned()).collect();
                if moved.waits_ms != 0 {
                    said.push(format!("its weights' wait by {} s", moved.waits_ms / 1000));
                }
                return Some(format!("{machine}: {}", said.join(", ")));
            }
        }
        None
    }
}

/// What of a machine's Status could change where a render goes (`Lists::weigh`): its line's runs (and how many), its phase, update
/// and GPUs, and its weights' wait as last moved.
#[derive(Clone, Copy)]
struct Load {
    line: u64,
    runs: usize,
    state: u64,
    waits_ms: i64,
}

/// The most moves of machines' loads kept (`Lists::could_move`).
const MOVES: usize = 4096;

/// What of a machine's load moved (`Lists::weigh`).
#[derive(Clone, Copy)]
enum Moved {
    /// The runs in its line changed: fewer of them, more, or as many.
    Line(std::cmp::Ordering),
    /// Its phase, update or GPUs out of service.
    State,
    /// The time the weights its line waits for take, by this much (ms).
    Waits(i64),
}

impl Moved {
    fn said(self) -> String {
        match self {
            Self::Line(std::cmp::Ordering::Less) => "fewer runs in its line".to_owned(),
            Self::Line(std::cmp::Ordering::Greater) => "more runs in its line".to_owned(),
            Self::Line(std::cmp::Ordering::Equal) => "the runs in its line moved".to_owned(),
            Self::State => "its phase, update or GPUs changed".to_owned(),
            Self::Waits(by) => format!("its weights' wait moved {} s", by / 1000),
        }
    }
}

/// What a prepare item's run `id` makes ready (`identity`), as its key begins.
fn made_of(id: &str) -> String {
    id.trim_start_matches(PREPARE).split('.').next().unwrap_or_default().to_owned()
}

/// A guess's failure the next try may not repeat (retry-standard-api-practice): the network, a Hub or an index not answering, a
/// download cut short, no room on the disk yet (its machine says when there is: `tensord::ahead::standing`), its preparation cut by
/// a restart. Any other is the request's or the package's, and stays.
fn transient(code: &str) -> bool {
    matches!(code, "hub_unreachable" | "model_fetch_failed" | "model_download_failed" | "package_file_download_failed" | "machine_disk_full" | "preparation_interrupted")
}

/// A failure's words in one line, at most a short sentence's length (concise-errors).
fn line(message: &str) -> String {
    let said = message.split_whitespace().collect::<Vec<_>>().join(" ");
    match said.char_indices().nth(240) {
        Some((at, _)) => format!("{}…", &said[..at]),
        None => said,
    }
}

/// What `machine` prepares for demand elsewhere: nothing while it has upcoming jobs or a queue; else the published functions planned
/// on other machines that it can run, the most-asked first, each once. Local code isn't: it is sent to a machine with the job that
/// runs it there. Nor is an encrypted project's where `sealed` says why nothing of it goes ahead (`Calcifer::sealed_from`; cl-1123,
/// the release coordinator: "for an encrypted project the request should be on the machine that runs the run and nowhere else").
fn demand(planned: &HashMap<String, HashMap<String, Vec<Listed>>>, machine: &str, frame: &v1::StatusFrame, idle: bool, sealed: Option<&str>) -> Vec<(String, v1::Predicted)> {
    let queued = frame.queue.as_ref().is_some_and(|queue| !queue.is_empty());
    if !idle || queued {
        return vec![];
    }
    let mut asked: Vec<(String, usize, String, v1::Predicted)> = vec![];
    for (folder, listed) in (planned.iter().filter(|(name, _)| name.as_str() != machine))
        .flat_map(|(_, lists)| lists.iter().flat_map(|(folder, listed)| listed.iter().map(move |listed| (folder, listed))))
        .filter(|(_, listed)| listed.fits.iter().any(|fits| fits == machine))
        .filter(|(_, listed)| matches!(listed.item.spec.source, Some(v1::Source::Release(_))))
    {
        if let Some(why) = sealed.filter(|_| crate::project::vault::encrypted_within(listed.project.folder_root())) {
            withheld(folder, &listed.asset, machine, why);
            continue;
        }
        let key = identity(&listed.item);
        match asked.iter_mut().find(|(held, ..)| *held == key) {
            Some((_, count, ..)) => *count += 1,
            None => asked.push((key, 1, listed.hub.clone(), listed.item.clone())),
        }
    }
    asked.sort_by_key(|(_, count, ..)| std::cmp::Reverse(*count));
    asked.into_iter().map(|(_, _, hub, item)| (hub, item)).collect()
}

/// Why each encrypted project's waiting job isn't prepared on a machine, by its folder, box and machine: said once until it changes.
static WITHHELD: LazyLock<Mutex<HashMap<(String, String, String), String>>> = LazyLock::new(Mutex::default);

/// Box `asset` of `folder` isn't prepared on `machine` for `why`: said in its project's own log (`say`) when that's new.
fn withheld(folder: &str, asset: &str, machine: &str, why: &str) {
    let at = (folder.to_owned(), asset.to_owned(), machine.to_owned());
    if lock(&WITHHELD).get(&at).is_some_and(|said| said == why) {
        return;
    }
    say(folder, || format!("{folder}: {asset} isn't prepared on {machine} while it waits elsewhere: nothing of this encrypted project goes ahead: {why}"));
    lock(&WITHHELD).insert(at, why.to_owned());
}

/// Sends `machine` its lists, each item under the access its Hub gives that machine.
/// None: that list is as it was last sent.
async fn put(
    machines: &Arc<Machines>,
    machine: &str,
    hubs: Vec<String>,
    upcoming: Option<Vec<(String, v1::Predicted)>>,
    prepare: Option<Vec<(String, v1::Predicted)>>,
) -> Result<(), String> {
    let began = std::time::Instant::now();
    let (_, link) = machines.linked(machine).map_err(|e| e.message)?;
    let mut access: HashMap<String, (Option<v1::HubAccess>, Option<String>)> = HashMap::new();
    for hub in hubs {
        if access.contains_key(&hub) {
            continue;
        }
        let specs: Vec<v1::RunSpec> = (upcoming.iter().flatten().chain(prepare.iter().flatten()))
            .filter(|(held, _)| *held == hub)
            .map(|(_, item)| item.spec.clone())
            .collect();
        access.insert(hub.clone(), (machines.list_access(&hub, machine, &specs).await?, machines.list_owner(&hub).await));
    }
    // Each item with what its run's send gives a run (`finish_spec`): its Hub access, and for local code the account it names.
    let with = |(hub, item): (String, v1::Predicted)| {
        let (hub, owner) = access.get(&hub).cloned().unwrap_or_default();
        let local = matches!(item.spec.source, Some(v1::Source::Local(_))) && item.spec.owner.is_empty();
        let owner = match (local, owner) {
            (true, Some(owner)) => owner,
            _ => item.spec.owner.clone(),
        };
        v1::Predicted { spec: v1::RunSpec { hub, owner, ..item.spec }, ..item }
    };
    let accessed = began.elapsed().as_millis();
    // Each is taken at once and admitted behind the answer: one refused says so in its readiness (cl-1123).
    if let Some(upcoming) = upcoming {
        link.upcoming(&v1::Upcoming { jobs: upcoming.into_iter().map(with).collect() }).await.map_err(|e| e.to_string())?;
    }
    if let Some(prepare) = prepare {
        link.prepare(&v1::Prepare { items: prepare.into_iter().map(with).collect() }).await.map_err(|e| e.to_string())?;
    }
    tracing::info!(target: TARGET, "lists put on {machine} in {} ms ({accessed} ms for their access)", began.elapsed().as_millis());
    Ok(())
}
