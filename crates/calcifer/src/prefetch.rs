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
//! Each list goes when it changes, and again once its machine starts again; the machine fetches, evicts and reports. A prepare item
//! is no job there (`tensord::ahead`): behind every run's needs and guesses in its lines, never a rental's use, the first to go on a
//! short disk. A guess that changes goes as the new list: its machine cancels what the list drops but what the next item reads too
//! (Paul: "I switch to another checkpoint, then cancel the parts of that download which are not needed"). Each item's row moves with
//! the downloads its machine's Status lists under it: an upcoming job's on its walk's board, a guess's on its boxes ("Ready on
//! kamitsure"), and only while it is the guess.
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

/// The prefixes the machine's runs of an upcoming and of a prepare list take (`tensord::ahead`).
pub(crate) const UPCOMING: &str = "cozy:upcoming:";
pub(crate) const PREPARE: &str = "cozy:prepare:";

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
    pub item: v1::Ahead,
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
    pub item: v1::Ahead,
    pub function: String,
}

#[derive(Clone, Default, PartialEq)]
struct Sent {
    boot: String,
    upcoming: Vec<v1::Ahead>,
    prepare: Vec<v1::Ahead>,
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
    /// By machine: what of its Status a quote of its load reads (`load`), and a count that moves whenever one's changes.
    loads: Mutex<HashMap<String, (u64, u64)>>,
    pub(crate) load_moved: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A prepare item's identity: what it makes ready, the same however many boxes or jobs ask it.
fn identity(item: &v1::Ahead) -> String {
    let shape = serde_json::json!([item.source, item.entrypoint, item.models, item.lane]);
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
                self.observe(&machine, &frame);
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

    /// The machine `project`'s guess for box `asset` is on, where it has one.
    pub(crate) fn guessed_on(&self, project: &str, asset: &str) -> Option<String> {
        lock(&self.guessed).get(project)?.iter().find(|guess| guess.asset == asset).map(|guess| guess.machine.clone())
    }

    /// The key `machine`'s prepare list has for what `item` makes ready, given now if it isn't on that list.
    fn key(&self, machine: &str, item: &v1::Ahead) -> String {
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
        if sent.as_deref() == Some(frame.boot_id.as_str()) || (sent.is_none() && !concerned) {
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
            // Its guesses, each once, then what demand elsewhere asks of it that no guess does.
            let mut prepare: Vec<(String, String, v1::Ahead)> = vec![];
            let asked = (guessed.iter().filter(|guess| guess.machine == name).map(|guess| (guess.hub.clone(), guess.item.clone())))
                .chain(demand(&planned, &name, &frame, upcoming.is_empty()));
            for (hub, item) in asked {
                let made = identity(&item);
                if !prepare.iter().any(|(held, ..)| *held == made) {
                    prepare.push((made, hub, item));
                }
            }
            let listed: HashSet<&str> = prepare.iter().map(|(made, ..)| made.as_str()).collect();
            lock(&self.keys).retain(|(machine, made), _| *machine != name || listed.contains(made.as_str()));
            let prepare: Vec<(String, v1::Ahead)> = prepare.into_iter().map(|(_, hub, item)| (hub, v1::Ahead { key: self.key(&name, &item), ..item })).collect();
            let now = Sent {
                boot: frame.boot_id.clone(),
                upcoming: upcoming.iter().map(|listed| listed.item.clone()).collect(),
                prepare: prepare.iter().map(|(_, item)| item.clone()).collect(),
            };
            if lock(&self.sent).get(&name) == Some(&now) {
                continue;
            }
            // A machine that isn't up is sent nothing, started or kept for nothing: its lists go once it says its Status (`seen`).
            if machines.live_frame(&name).is_none() {
                lock(&self.sent).remove(&name);
                continue;
            }
            lock(&self.sent).insert(name.clone(), now);
            let hubs: Vec<String> = upcoming.iter().map(|listed| listed.hub.clone()).chain(prepare.iter().map(|(hub, _)| hub.clone())).collect();
            let upcoming: Vec<(String, v1::Ahead)> = upcoming.iter().map(|listed| (listed.hub.clone(), listed.item.clone())).collect();
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
    /// it, and is ready once that run succeeded.
    pub(crate) fn observe(&self, machine: &str, frame: &v1::StatusFrame) {
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
                    None => "preparing it".to_owned(),
                }),
                done: downloads.iter().map(|op| op.bytes_done).sum(),
                total: downloads.iter().map(|op| op.bytes_total).sum(),
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
                if let Some(row) = row_of(&id, &function, names) {
                    project.jobs.guessed(&id, Row { guess: true, ..row });
                }
                // Its machine lost what it made ready, and may fetch it again now: listed under a new key, it is started anew.
                if frame.ahead.iter().any(|run| run.id == id && run.state == "evicted" && run.waiting.is_empty()) {
                    lost |= self.rekey(machine, &id);
                }
            }
            if lost {
                let now: HashSet<String> = self.rows(&folder).into_iter().map(|(id, ..)| id).collect();
                project.jobs.guesses_kept(&now);
                self.rekeyed.store(true, Ordering::Release);
            }
        }
    }

    /// The prepare item `machine`'s list names `id` takes a new key at its next listing. Whether it had that one.
    fn rekey(&self, machine: &str, id: &str) -> bool {
        let key = id.trim_start_matches(PREPARE);
        let mut keys = lock(&self.keys);
        let before = keys.len();
        keys.retain(|(on, _), held| on != machine || held != key);
        keys.len() != before
    }

    /// What of `machine`'s Status a quote of its load reads (cl-1104): the roots in its queue, each where it stands but for its bytes
    /// still to come, the update it may wait on, its phase and its GPUs out of service. Where that changed, `load_moved` moves: each
    /// open project's guesses are made again, so the machine made ready is still the one a render would go to.
    fn weigh(&self, machine: &str, frame: &v1::StatusFrame) {
        use std::hash::{Hash as _, Hasher as _};
        let mut loads = lock(&self.loads);
        let was = loads.get(machine).copied();
        // A stream sends its queue only when it changed: absent, as its last frame said.
        let queue = match &frame.queue {
            Some(queue) => {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                for root in queue.iter().filter(|root| !root.run.starts_with(PREPARE) && !root.run.starts_with(UPCOMING)) {
                    (&root.run, &root.function, root.running, root.width, root.gpu_waits, root.started_at_unix_ms).hash(&mut hasher);
                }
                hasher.finish()
            }
            None => was.map_or(0, |(queue, _)| queue),
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let update = frame.update.as_ref().map(|update| update.state.as_str());
        let lost: Vec<_> = frame.gpus.iter().map(|gpu| &gpu.out_of_service).collect();
        (queue, &frame.phase, update, lost).hash(&mut hasher);
        let load = (queue, hasher.finish());
        if was != Some(load) {
            loads.insert(machine.to_owned(), load);
            if was.is_some() {
                self.load_moved.fetch_add(1, Ordering::AcqRel);
            }
        }
    }
}

/// What `machine` prepares for demand elsewhere: nothing while it has upcoming jobs or a queue; else the published functions planned
/// on other machines that it can run, the most-asked first, each once. Local code isn't: it is sent to a machine with the job that
/// runs it there.
fn demand(planned: &HashMap<String, HashMap<String, Vec<Listed>>>, machine: &str, frame: &v1::StatusFrame, idle: bool) -> Vec<(String, v1::Ahead)> {
    let queued = frame.queue.as_ref().is_some_and(|queue| !queue.is_empty());
    if !idle || queued {
        return vec![];
    }
    let mut asked: Vec<(String, usize, String, v1::Ahead)> = vec![];
    for listed in (planned.iter().filter(|(name, _)| name.as_str() != machine))
        .flat_map(|(_, lists)| lists.values().flatten())
        .filter(|listed| listed.fits.iter().any(|fits| fits == machine))
        .filter(|listed| matches!(listed.item.source, Some(v1::Source::Release(_))))
    {
        let key = identity(&listed.item);
        match asked.iter_mut().find(|(held, ..)| *held == key) {
            Some((_, count, ..)) => *count += 1,
            None => asked.push((key, 1, listed.hub.clone(), listed.item.clone())),
        }
    }
    asked.sort_by_key(|(_, count, ..)| std::cmp::Reverse(*count));
    asked.into_iter().map(|(_, _, hub, item)| (hub, item)).collect()
}

/// Sends `machine` its lists, each item under the access its Hub gives that machine.
async fn put(machines: &Arc<Machines>, machine: &str, hubs: Vec<String>, upcoming: Vec<(String, v1::Ahead)>, prepare: Vec<(String, v1::Ahead)>) -> Result<(), String> {
    let (_, link) = machines.linked(machine).map_err(|e| e.message)?;
    let mut access: HashMap<String, Option<v1::HubAccess>> = HashMap::new();
    for hub in hubs {
        if access.contains_key(&hub) {
            continue;
        }
        let specs: Vec<v1::RunSpec> = (upcoming.iter().chain(&prepare))
            .filter(|(held, _)| *held == hub)
            .map(|(_, item)| v1::RunSpec { kind: v1::RunKind::Warm, source: item.source.clone(), entrypoint: item.entrypoint.clone(), models: item.models.clone(), ..Default::default() })
            .collect();
        access.insert(hub.clone(), machines.list_access(&hub, machine, &specs).await?);
    }
    let with = |(hub, item): (String, v1::Ahead)| v1::Ahead { hub: access.get(&hub).cloned().flatten(), ..item };
    let answer = link.upcoming(&v1::Upcoming { jobs: upcoming.into_iter().map(with).collect() }).await.map_err(|e| e.to_string())?;
    for refused in answer.items.iter().filter_map(|item| item.refusal.as_ref()) {
        warn!(machine, "an upcoming job won't be ready ahead: {}", refused.message);
    }
    let answer = link.prepare(&v1::Prepare { items: prepare.into_iter().map(with).collect() }).await.map_err(|e| e.to_string())?;
    for refused in answer.items.iter().filter_map(|item| item.refusal.as_ref()) {
        warn!(machine, "a guess won't be ready ahead: {}", refused.message);
    }
    Ok(())
}
