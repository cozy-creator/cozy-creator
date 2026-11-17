//! Machines: this computer's (`local`, which starts when used), rentals, and other computers'
//! added by address.
//!
//! Each machine with an endpoint has one `Status` stream. Its phase, idle deadline, GPUs and
//! live runs are held here and answer `GET /v1/machines`; nothing is polled. A machine that
//! cannot be reached waits and is shown so: only its Hub's word, its own release, or its owner
//! ends it. Unpinned runs are placed here: `any` on a rental of this controller that fits,
//! `new:<sku>` on one rented for the run (`rentals`).
use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path as UrlPath, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{self, KeepAlive, Sse},
    },
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use platform::{
    durable,
    paths::Home,
    tail::{Log, Start},
};
use api::{
    calcifer::{
        self as wire, Ended, Event, Keepalive, LogEvent, Machine, MachineKind, MachineList,
        MachineLog, MachineState, MachineStatus, NewMachine, On, SubmitRun,
    },
    config,
    error::{Error, Kind},
    ids::{Digest, RunId},
    machine as v1,
};
use ed25519_dalek::{
    SigningKey,
    pkcs8::{DecodePrivateKey as _, EncodePrivateKey as _},
};
use futures_util::StreamExt as _;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tokio::{
    sync::{Notify, broadcast::error::RecvError},
    task::{AbortHandle, JoinSet},
};
use tracing::warn;

use crate::{
    Calcifer,
    api::{Api, Refusal, parse},
    link::{self, Link, LinkError},
    rentals::{Hubs, Shape, Word, hub_refusal},
    store::{AskKind, Endpoint, MachineRow, Store},
    took::Took,
};

pub(crate) struct Machines {
    pub(crate) calcifer: Arc<Calcifer>,
    pub(crate) home: Home,
    /// This user's home: what `~` in a request's path means.
    pub(crate) user_home: PathBuf,
    pub(crate) hubs: Hubs,
    /// Model-source tokens every run carries (`ProviderAccess`), from the config.
    pub(crate) providers: Option<v1::ProviderAccess>,
    pub(crate) seen: Mutex<HashMap<String, Seen>>,
    watching: Mutex<HashMap<String, (Endpoint, AbortHandle)>>,
    /// What this computer's machine does for each open project's own runs, read in that project's scope (`follow_project`): the
    /// machine's own status never lists it.
    project_activity: Mutex<HashMap<String, Vec<v1::Operation>>>,
    /// Rentals whose boot is followed, by rental id.
    pub(crate) booting: Mutex<HashSet<String>>,
    /// Runs a rental is being asked for.
    pub(crate) renting: Mutex<HashSet<RunId>>,
    /// The rent asks being asked of a Hub now: one loop per key.
    pub(crate) asking: Mutex<HashSet<String>>,
    /// The rent asks a Hub has not answered, by key, and why.
    pub(crate) unanswered: Mutex<HashMap<String, String>>,
    /// Told when a rental appears or is ready: `any` runs may fit it.
    pub(crate) placing: Notify,
    /// Told when an end stops waiting for its machine's outputs (`--force`).
    pub(crate) end_waits: Notify,
    /// This computer's machine, started when something uses it.
    pub(crate) local: crate::local::Local,
    tasks: Mutex<JoinSet<()>>,
    /// The config's `dev = true`: its rentals follow the open RC (`rc`, cl-922).
    pub(crate) dev: bool,
    /// The config's `download_url` (`COZY_DOWNLOAD_URL`): where cozy's own update reads its release in place of GitHub (cl-978).
    pub(crate) download_url: Option<String>,
    pub(crate) rc: crate::rc::Rc,
    /// Told when a rental becomes ready: a dev home's moves to the RC before its first run.
    pub(crate) rc_ready: Notify,
    /// What each machine is told to have ready (cl-967).
    pub(crate) lists: crate::prefetch::Lists,
    /// Moves when a machine becomes ready or ends: what plans re-plan on (cl-967).
    pub(crate) arrivals: tokio::sync::watch::Sender<u64>,
    /// Anonymous performance data: whether it's sent, and the reports waiting (cl-1006).
    pub(crate) reports: crate::reports::Reports,
}

/// How long a rental may say nothing after an update restarted it before this computer calls it broken (cl-977). Its own
/// restart past a TensorD before it that can't exit takes about 6 minutes at most (`tensord::machine::stuck`: 10 s to exit, up
/// to 5 × 60 s killed while it still moves, then 60 s for a fresh process to open the GPUs).
const UPDATE_RETURN_MS: i64 = 10 * 60_000;

/// What Calcifer last heard of a machine, never stored.
#[derive(Default)]
pub(crate) struct Seen {
    frame: Option<v1::StatusFrame>,
    /// Its Status stream is open: `frame` is what it is now, not what it was when the stream broke.
    streaming: bool,
    /// When its last frame came, else (none came since this Calcifer started) its first failed attempt (unix ms).
    heard: Option<i64>,
    /// This computer's word that it is broken: silent since an update restarted it (cl-977). Cleared when it answers.
    silent: Option<v1::Broken>,
    unreachable: Option<String>,
    /// Since when it has not answered (unix ms).
    since: Option<i64>,
    /// A booting rental's phase and attempt, as its Hub says them.
    pub(crate) boot: Option<String>,
    pub(crate) attempt: Option<wire::Attempt>,
    /// `local` is not running: it starts when something uses it.
    stopped: bool,
    /// The boot this computer last vouched for to the rental's Hub, and when (cl-977).
    vouched: Option<(String, tokio::time::Instant)>,
    /// Connected, it didn't answer its frame (xs-199): asked again in the background, and meanwhile no job is placed on it.
    unanswered: Option<Unanswered>,
}

/// A machine connected that didn't answer its frame: the attempts so far, and whether every one went unanswered.
#[derive(Clone, Copy)]
struct Unanswered {
    attempt: u32,
    dropped: bool,
}

/// How many times a machine that doesn't answer its frame is asked, backing off between (1, 2, 4, 8 s), before no job waits for it.
const STATUS_ATTEMPTS: u32 = 5;
/// How often one that answered none of them is asked again: it's a candidate once it answers.
const STATUS_AGAIN: Duration = Duration::from_secs(60);

/// How often this computer vouches for a rental to its Hub while a run of its is live there (cl-977): a third of the Hub's 15-minute
/// window, so two can be lost.
const VOUCH_EVERY: Duration = Duration::from_secs(5 * 60);

/// A ready machine's frame shows this computer's runs (or an update) queued or running there.
fn working(frame: &v1::StatusFrame) -> bool {
    let live = |state: &str| matches!(state, "queued" | "preparing" | "needs_access" | "starting" | "running");
    frame.phase == "ready" && (frame.runs.iter().any(|run| live(&run.state)) || frame.update.as_ref().is_some_and(|u| live(&u.state)))
}

impl Machines {
    /// Watches every machine Calcifer holds, follows booting rentals, asks again what it asked
    /// a Hub and was not answered, and places unplaced runs. The config names the Hub listed (a
    /// rental's own Hub is always read), this computer's machine root and the model-source
    /// tokens runs carry.
    pub(crate) fn start(
        calcifer: Arc<Calcifer>,
        home: Home,
        user_home: &Path,
        config: &config::Config,
    ) -> Arc<Self> {
        let token = |secret: &Option<config::Secret>| {
            secret
                .as_ref()
                .map(|s| s.expose().to_owned())
                .unwrap_or_default()
        };
        let providers = v1::ProviderAccess {
            huggingface: token(&config.huggingface_token),
            civitai: token(&config.civitai_token),
        };
        let local = crate::local::Local::new(&home, user_home, config);
        let reports = crate::reports::Reports::new(config, &home);
        let me = Arc::new(Self {
            hubs: Hubs::new(&home, config.hub_url.clone()),
            providers: (providers != v1::ProviderAccess::default()).then_some(providers),
            home,
            user_home: user_home.to_owned(),
            seen: Mutex::default(),
            watching: Mutex::default(),
            project_activity: Mutex::default(),
            booting: Mutex::default(),
            renting: Mutex::default(),
            asking: Mutex::default(),
            unanswered: Mutex::default(),
            placing: Notify::new(),
            end_waits: Notify::new(),
            local,
            tasks: Mutex::default(),
            dev: config.dev,
            download_url: config.download_url.as_ref().map(|url| url.as_str().to_owned()),
            rc: crate::rc::Rc::default(),
            rc_ready: Notify::new(),
            lists: crate::prefetch::Lists::default(),
            arrivals: tokio::sync::watch::channel(0).0,
            reports,
            calcifer,
        });
        me.calcifer.serve_with(&me);
        // The Hubs' last word from before this start, answered until the first listing (cl-1010). It lives in state.db, as
        // all of Calcifer's state does (cl-818): the file a 0.26.0 dev build kept it in goes.
        let _ = std::fs::remove_file(me.home.calcifer().join("kept-hub-listings.json"));
        match (me.store().hub_listings(), me.store().machines()) {
            (Ok(kept), Ok(rows)) => me.hubs.load_listings(kept, &rows),
            (Err(e), _) | (_, Err(e)) => warn!("reading the Hubs' last listing: {e}"),
        }
        match me.store().live_machines() {
            Ok(rows) => {
                // `local` is followed once recorded, while its TensorD runs.
                let others = rows.iter().filter(|r| r.kind != MachineKind::Local);
                for row in others.filter(|r| r.state != MachineState::Ended) {
                    match row.endpoint {
                        Some(_) => me.watch(row),
                        None if row.kind == MachineKind::Rental => me.follow_boot(row.clone()),
                        None => {}
                    }
                }
            }
            Err(e) => warn!("reading the machines: {e}"),
        }
        me.resume_asks();
        me.spawn(me.clone().local_tasks());
        me.spawn(me.clone().place_runs());
        me.spawn(me.clone().notice_stalled());
        if me.dev {
            me.spawn(me.clone().follow_rcs());
        }
        me.refresh_hubs();
        me
    }

    /// Each minute while a run is unsettled, says once of each one that stopped moving that it has. With none, nothing
    /// wakes until one is created.
    async fn notice_stalled(self: Arc<Self>) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(60);
        let mut last = crate::store::now_ms();
        loop {
            let mut events = self.calcifer.subscribe();
            let noticed = {
                let _took = Took::new("notice_stalled");
                self.calcifer.notice_stalled()
            };
            if !noticed {
                loop {
                    match events.recv().await {
                        Ok((_, Event::Created { .. })) | Err(RecvError::Lagged(_)) => break,
                        Ok(_) => {}
                        Err(RecvError::Closed) => return,
                    }
                }
                last = crate::store::now_ms();
            }
            drop(events);
            tokio::time::sleep(EVERY).await;
            let now = crate::store::now_ms();
            // The clock moved on by more than this timer did: this computer slept.
            if now - last > 2 * EVERY.as_millis() as i64 {
                self.calcifer.woke();
            }
            last = now;
        }
    }

    /// What `name` costs an hour, as its Hub last said.
    pub(crate) fn hourly(&self, name: &str) -> Option<i64> {
        self.hubs.hourly(&self.store().machine(name).ok()??)
    }

    /// The machine's latest Status, once one has arrived.
    pub(crate) fn frame(&self, name: &str) -> Option<v1::StatusFrame> {
        lock(&self.seen)
            .get(name)
            .and_then(|seen| seen.frame.clone())
    }

    /// The machine's Status while its stream is open: none once the stream broke, until it says it again (cl-967: a machine that stopped
    /// answering is never quoted from what it said before).
    pub(crate) fn live_frame(&self, name: &str) -> Option<v1::StatusFrame> {
        lock(&self.seen).get(name).filter(|seen| seen.streaming && seen.unreachable.is_none()).and_then(|seen| seen.frame.clone())
    }

    /// Its Status stream ended or broke.
    fn stream_closed(&self, name: &str) {
        if let Some(seen) = lock(&self.seen).get_mut(name) {
            seen.streaming = false;
        }
        self.calcifer.machine_changed();
    }

    /// Its first GPU's name, as it last said: a run's record reads it, without its whole frame.
    pub(crate) fn gpu(&self, name: &str) -> Option<String> {
        lock(&self.seen).get(name).and_then(|seen| seen.frame.as_ref()?.gpus.first().map(|gpu| gpu.name.clone()))
    }

    /// What rental `name`'s weights would arrive at, bytes a second, where it hasn't measured its link (cl-1107, the user-flow: lillie
    /// was quoted 712 s for 59 GB at the estimator's flat 100 MB/s, while chocola, rented beside it, landed its own at 348 MB/s): the
    /// median of what this account's other rentals from its Hub and provider measured, else from its Hub. None: none did, or it isn't
    /// a rental (the Hub's own guess for a host is th-377's).
    pub(crate) fn link_guess(&self, name: &str) -> Option<f64> {
        let rows = self.store().live_machines().ok()?;
        let row = rows.iter().find(|row| row.name == name && row.kind == MachineKind::Rental)?;
        let provider = self.hubs.provider(row);
        let others: Vec<(bool, &MachineRow)> = (rows.iter())
            .filter(|other| other.name != row.name && other.kind == MachineKind::Rental && other.hub == row.hub)
            .map(|other| (self.hubs.provider(other) == provider, other))
            .collect();
        let measured: Vec<(bool, f64)> = {
            let seen = lock(&self.seen);
            (others.iter())
                .filter_map(|(same, other)| Some((*same, seen.get(&other.name)?.frame.as_ref()?.link_bytes_per_s)))
                .filter(|(_, link)| *link > 0.0)
                .collect()
        };
        let mut links: Vec<f64> = measured.iter().filter(|(same, _)| *same).map(|(_, link)| *link).collect();
        if links.is_empty() {
            links = measured.iter().map(|(_, link)| *link).collect();
        }
        links.sort_by(f64::total_cmp);
        links.get(links.len() / 2).copied()
    }

    /// Stops every task where it stands, as if the process ended.
    pub(crate) fn close(&self) {
        lock(&self.tasks).abort_all();
    }

    pub(crate) fn store(&self) -> &Store {
        self.calcifer.store()
    }

    pub(crate) fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) -> AbortHandle {
        let mut tasks = lock(&self.tasks);
        while tasks.try_join_next().is_some() {}
        tasks.spawn(task)
    }

    /// Every machine: this computer's, those added, and every live rental of this account, ordered
    /// here or by another of its clients, by the Hubs' last word. `refresh` lists every Hub again beside the answer (cl-1010: it waited
    /// on every Hub, 1 to 15 s on Paul's laptop): the new word reaches a page as a machine change, and the next list.
    pub(crate) async fn list(self: &Arc<Self>, refresh: bool) -> Result<MachineList, Error> {
        self.list_for(refresh, &[]).await
    }

    /// The list as a page showing `projects` sees it: what each machine does for their runs too (`picture_for`).
    pub(crate) async fn list_for(self: &Arc<Self>, refresh: bool, projects: &[String]) -> Result<MachineList, Error> {
        if refresh {
            self.refresh_hubs();
        }
        let here = platform::Os::CURRENT.local_inference();
        if here {
            self.local_row()?;
        }
        let rows = self.store().live_machines().map_err(internal)?;
        let mut machines = Vec::new();
        let listed = |r: &&MachineRow| r.state != MachineState::Ended && (here || r.kind != MachineKind::Local);
        for row in rows.iter().filter(listed) {
            machines.push(self.picture_for(row, projects)?);
        }
        let (hubs, unanswered, signed_out) = self.hubs.listing();
        machines.extend(self.orders()?);
        let live = machines.iter().filter(|m| m.state != MachineState::Ended);
        let rentals = wire::LiveRentals::of(live.filter_map(|m| m.rental.as_ref()));
        Ok(MachineList {
            machines,
            hubs,
            unanswered,
            signed_out,
            rentals,
        })
    }

    /// One machine by its name, former name or rental id (`named`), with its Status read now:
    /// `local` starts for it.
    pub(crate) async fn show(self: &Arc<Self>, text: &str) -> Result<Machine, Error> {
        if let Some(order) = self.order_named(text)? {
            return Ok(self.order(&order));
        }
        let row = match self.named_now(text).await {
            Ok(row) => row,
            // To read, a name only ended machines had is the newest of them.
            Err(e) if e.code == "machine_name_ended" => match self.store().machines_named(text).map_err(internal)?.into_iter().next() {
                Some(row) => row,
                None => return Err(e),
            },
            Err(e) => return Err(e),
        };
        if row.state != MachineState::Ended
            && self.offline(&row.name).is_none()
            && self.not_answering(&row.name).is_none()
            && let Ok(Some(link)) = self.calcifer.link(&row.name)
        {
            match link.status().await {
                Ok(frame) => {
                    // A Hub its installations name that no Hub's facts read so far do.
                    let unnamed = (frame.environments.iter())
                        .filter_map(|env| config::Origin::parse(&env.hub).ok())
                        .any(|hub| self.hubs.known(&hub).is_none());
                    if unnamed {
                        self.hubs.learn().await;
                    }
                    self.observe(&row.name, frame);
                }
                Err(LinkError::Unreachable(e)) => self.unreachable(&row.name, e),
                // Connected but not answering: said so and asked again; its last picture stands for what's shown, never to place a job.
                Err(e) => self.not_answered(&row.name, &e.to_string()),
            }
        }
        self.picture(&row)
    }

    /// The name of the machine a run `on` goes to and the environments it holds now, its Status
    /// read now since an install that just settled may not have reached the stream: `local` starts
    /// for it (unless `wake` is false: then, not running, as it last said), and `any` is the rental it would take now.
    /// None for a machine still to come.
    pub(crate) async fn holds(self: &Arc<Self>, on: &On, wake: bool) -> Option<(String, Vec<v1::Environment>)> {
        let text = match on {
            On::Machine(text) => text.clone(),
            On::Any => self.fitting(0).ok()??,
            On::New(_) => return None,
        };
        if !wake && text.eq_ignore_ascii_case("local") && !self.local_runs() {
            return self.frame("local").map(|frame| ("local".to_owned(), frame.environments));
        }
        let name = self.show(&text).await.ok()?.name;
        let frame = self.frame(&name)?;
        Some((name, frame.environments))
    }

    /// What local holds for one project: its runs install into the project's own worker area, which the machine's frame
    /// never lists. Nothing while local is down.
    pub(crate) async fn project_holds(&self, scope: crate::private_io::Scope) -> Vec<v1::Environment> {
        let Some(link) = self.local_runs().then(|| self.calcifer.link("local").ok().flatten()).flatten() else { return vec![] };
        link.with_private(scope).status().await.map(|frame| frame.environments).unwrap_or_default()
    }

    /// Adds another computer's machine: it must present the leaf `fingerprint` names and admit
    /// this controller's key before anything is recorded.
    pub(crate) async fn add(
        self: &Arc<Self>,
        name: &str,
        address: &str,
        fingerprint: &Digest,
    ) -> Result<Machine, Error> {
        let name = &name.to_ascii_lowercase();
        valid_name(name)?;
        let rows = self.store().machines_named(name).map_err(internal)?;
        if (rows.iter()).any(|row| row.state != MachineState::Ended || row.kind != MachineKind::Added) {
            return Err(Error::new(
                Kind::Conflict,
                "machine_name_taken",
                format!("There's already a machine named {name}."),
            )
            .next(format!("cozy machine show {name}")));
        }
        let unreachable = |e: LinkError| {
            Error::new(
                Kind::Unavailable,
                "machine_unreachable",
                format!("Cozy couldn't reach {address}: {e}."),
            )
            .remedy("Check the address that computer shows for itself, then try again.")
            .next("cozy machine show local")
        };
        let leaf: [u8; 32] =
            Sha256::digest(link::presented_leaf(address).await.map_err(unreachable)?).into();
        let local = self.store().machine("local").map_err(internal)?.and_then(|row| row.endpoint);
        if local.is_some_and(|endpoint| endpoint.pin == leaf) {
            return Err(Error::new(
                Kind::Conflict,
                "machine_is_local",
                format!("{address} is this computer, which Cozy already uses."),
            )
            .next("cozy machine show local"));
        }
        if &leaf != fingerprint.bytes() {
            return Err(Error::new(
                Kind::Credential,
                "fingerprint_mismatch",
                format!(
                    "{address} has a different fingerprint than the one you gave: it presents {}, not {fingerprint}.",
                    Digest::from_bytes(leaf)
                ),
            )
            .remedy("Ask the machine's owner for its fingerprint again."));
        }
        let identity = Link::identity(address, leaf).await.map_err(unreachable)?;
        let key = self.owner_key()?;
        let row = MachineRow {
            name: name.into(),
            kind: MachineKind::Added,
            hub: String::new(),
            rental_id: None,
            endpoint: Some(Endpoint {
                worker_id: identity.worker_id,
                address: address.into(),
                pin: leaf,
            }),
            key: key.clone(),
            state: MachineState::Ready,
            served: true,
            reason: None,
        };
        let link =
            Link::new(row.endpoint.as_ref().expect("set"), signer(&key)?).map_err(unreachable)?;
        let frame = link.status().await.map_err(|e| match e {
            LinkError::Unauthorized(_) => match self.controller_key() {
                Ok(public) => Error::new(
                    Kind::Credential,
                    "machine_unauthorized",
                    format!("{address} doesn't accept this computer's key."),
                )
                .remedy("Authorize this computer's key on that computer, then try again.")
                .next(format!("cozy machine authorize {public}")),
                Err(e) => e,
            },
            e => unreachable(e),
        })?;
        self.calcifer.put_machine(&row).map_err(internal)?;
        self.observe(name, frame);
        self.watch(&row);
        self.picture(&row)
    }

    /// Records this computer's machine where its TensorD serves (none before its first start),
    /// signed with this controller's owner key (minted on first use: a pod's TensorD the image
    /// started reads it from the home when it first signs), and follows it while it serves.
    fn put_local(self: &Arc<Self>, endpoint: Option<Endpoint>, serving: bool) -> Result<(), Error> {
        let row = MachineRow {
            name: "local".into(),
            kind: MachineKind::Local,
            hub: String::new(),
            rental_id: None,
            endpoint,
            key: self.owner_key()?,
            state: MachineState::Ready,
            served: true,
            reason: None,
        };
        lock(&self.seen).entry(row.name.clone()).or_default().stopped = !serving;
        self.calcifer.put_machine(&row).map_err(internal)?;
        if serving {
            self.watch(&row);
        }
        Ok(())
    }

    /// `local` always exists: recorded as Calcifer starts (after its first answer, which writes
    /// nothing before it), or as something names it first.
    fn local_row(self: &Arc<Self>) -> Result<MachineRow, Error> {
        api::calcifer::local_usable("local")?;
        if let Some(row) = self.store().machine("local").map_err(internal)? {
            return Ok(row);
        }
        self.record_local()?;
        self.row("local")
    }

    /// Records `local` where its TensorD serves when one runs, else where it last served.
    fn record_local(self: &Arc<Self>) -> Result<(), Error> {
        let kept = self.store().machine("local").map_err(internal)?.and_then(|row| row.endpoint);
        let serving = self.local.running().then(|| self.local.endpoint().ok()).flatten();
        self.put_local(serving.clone().or(kept), serving.is_some())
    }

    /// With Calcifer: `local` starts at once when another computer's controller is authorized to
    /// use it (Calcifer starts at login).
    async fn local_tasks(self: Arc<Self>) {
        // Off the runtime's workers: a write may wait out a commit stuck on a busy disk.
        let me = self.clone();
        let here = platform::Os::CURRENT.local_inference();
        if here && let Ok(Err(e)) = tokio::task::spawn_blocking(move || me.record_local()).await {
            warn!("recording this computer's machine: {}", e.message);
        }
        if here
            && self.local.authorized()
            && let Err(e) = self.local_up().await
        {
            warn!("starting this computer's machine for its authorized controllers: {}", e.message);
        }
    }

    /// Brings `local` up: starts its TensorD when none runs, records where it serves and
    /// follows it. Callers share one start.
    /// Whether this computer's machine runs and is followed: `local_up` returns at once.
    pub(crate) fn local_runs(&self) -> bool {
        !lock(&self.seen).get("local").is_some_and(|seen| seen.stopped) && self.local.running() && self.local.silent().is_none()
    }

    pub(crate) async fn local_up(self: &Arc<Self>) -> Result<(), Error> {
        api::calcifer::local_usable("local")?;
        if self.local_runs() {
            return Ok(());
        }
        let _start = self.local.starting.lock().await;
        if self.local_runs() {
            return Ok(());
        }
        // A start that isn't answering is refused at once until it serves or exits (cl-977).
        if let Some(refusal) = self.local.silent() {
            self.local_silent(&refusal);
            return Err(refusal);
        }
        // TensorD admits this controller's key from the home as it starts.
        let key = self.owner_key()?;
        let me = self.clone();
        let answered = move |endpoint| match me.put_local(Some(endpoint), true) {
            Ok(()) => me.calcifer.machine_changed(),
            Err(e) => warn!("this computer's machine serves, late: {}", e.message),
        };
        // Said as it's judged, whether or not this caller still waits (cl-1066).
        let me = self.clone();
        let silent = move |refusal: Error| me.local_silent(&refusal);
        let started = match self.local.serving().await {
            Ok(Some(endpoint)) => Ok(endpoint),
            Ok(None) => self.local.start(key, answered, silent).await,
            Err(refusal) => Err(refusal),
        };
        let endpoint = started.inspect_err(|refusal| {
            if refusal.code == "machine_not_answering" {
                self.local_silent(refusal);
            }
        })?;
        self.put_local(Some(endpoint), true)
    }

    /// `local` isn't answering (cl-977): `machine ls`/`show` and the page say why, and placement takes it as unreachable until
    /// it serves (its first frame clears it) or exits.
    fn local_silent(&self, refusal: &Error) {
        {
            let mut seen = lock(&self.seen);
            let seen = seen.entry("local".into()).or_default();
            seen.stopped = false;
            seen.since.get_or_insert(crate::store::now_ms());
            seen.unreachable = Some(refusal.message.clone());
        }
        self.calcifer.touch("local");
        self.calcifer.machine_changed();
    }

    /// What `text` names. A rental id names its machine whatever its state. A name or former
    /// name names the live machine that has it; several such are refused by rental id, and an
    /// ended machine is named only by its rental id: a Hub gives a name again once its rental ends.
    pub(crate) fn named(&self, text: &str) -> Result<Option<MachineRow>, Error> {
        let rows = self.store().machines_named(text).map_err(internal)?;
        if let Some(row) = rows.iter().find(|row| row.rental_id.as_deref().is_some_and(|id| id.eq_ignore_ascii_case(text))) {
            return Ok(Some(row.clone()));
        }
        let (ended, mut live): (Vec<MachineRow>, Vec<MachineRow>) =
            rows.into_iter().partition(|row| row.state == MachineState::Ended);
        match live.len() {
            0 if ended.is_empty() => Ok(None),
            0 => Err(ended_by_name(text, &ended)),
            1 => Ok(live.pop()),
            _ => Err(ambiguous(text, &live)),
        }
    }

    /// `named`, after the Hubs' word now when nothing here has the name: a rental another client of
    /// this account ordered since is recorded by that listing. `local` starts for it.
    pub(crate) async fn named_now(self: &Arc<Self>, text: &str) -> Result<MachineRow, Error> {
        if text.eq_ignore_ascii_case("local") {
            self.local_row()?;
            self.local_up().await?;
        }
        if let Ok(Some(row)) = self.named(text) {
            return Ok(row);
        }
        self.list_hubs().await;
        self.named(text)?.ok_or_else(|| not_found(text))
    }

    /// `local`'s TensorD exited (after its idle time, or ended): it is no longer followed, and
    /// the next use starts it.
    fn local_stopped(&self) {
        if let Some((_, task)) = lock(&self.watching).remove("local") {
            task.abort();
        }
        {
            let mut seen = lock(&self.seen);
            let seen = seen.entry("local".into()).or_default();
            (seen.stopped, seen.unreachable, seen.since) = (true, None, None);
        }
        self.calcifer.touch("local");
        self.calcifer.machine_changed();
    }

    /// `local`'s TensorD exited: while another computer's controller is authorized to use it, it
    /// starts again at once (after `cozy update`, on the new binary).
    fn local_exited(self: &Arc<Self>) {
        // Stopped broken (cl-977), it starts again at its next use, not at once: each start checks its GPUs again.
        let broken = lock(&self.seen).get("local").and_then(|seen| seen.frame.as_ref()).is_some_and(|frame| frame.broken.is_some());
        self.local_stopped();
        if broken {
            return;
        }
        // The old joined status/log task was just aborted. Machines owns the wake through readiness,
        // so canceling that watcher cannot drop its startup mutex while the detached child boots.
        let me = self.clone();
        self.spawn(async move {
            if me.local.authorized()
                && let Err(e) = me.local_up().await
            {
                warn!("starting this computer's machine again for its authorized controllers: {}", e.message);
            }
        });
    }

    /// Ends a rental at its Hub (`ending` until the Hub affirms it), or removes an added machine
    /// from this controller, once the outputs of its runs have landed here: the end is recorded
    /// at once and carried out then (`end_here`). `force` ends it now and names the outputs given
    /// up. `local` is never ended.
    pub(crate) async fn end(self: &Arc<Self>, text: &str, force: bool) -> Result<Ended, Error> {
        if text.eq_ignore_ascii_case("local") {
            return Err(local_refusal());
        }
        if let Some(order) = self.order_named(text)? {
            return self.stop_order(order);
        }
        let row = self.named_now(text).await?;
        if row.kind == MachineKind::Local {
            return Err(local_refusal());
        }
        if row.state == MachineState::Ended {
            return Ok(Ended {
                machine: self.picture(&row)?,
                changed: false,
                abandoned: vec![],
            });
        }
        self.end_here(row, force)
    }

    /// Starts a rental's 15-minute idle clock over, once the machine acknowledges it.
    pub(crate) async fn keepalive(&self, text: &str) -> Result<Keepalive, Error> {
        let (row, link) = self.linked(text)?;
        let frame = link
            .keepalive()
            .await
            .map_err(|e| refusal(&row.name, e))?;
        let idle_deadline_ms = deadline(&frame);
        self.observe(&row.name, frame);
        Ok(Keepalive {
            acknowledged_at_ms: crate::store::now_ms(),
            idle_deadline_ms,
        })
    }

    /// The newest `lines` whole lines (0: all) of a log the machine keeps.
    /// A machine that cannot answer (ended, lost, unreachable) answers its own lines Calcifer kept of its TensorD log.
    pub(crate) async fn logs(&self, text: &str, log: MachineLog, lines: u64) -> Result<Vec<u8>, Error> {
        let read = match self.linked(text) {
            Ok((row, link)) => link.read_log(log_name(log), lines).await.map_err(|e| log_refusal(&row.name, log, e)),
            Err(error) => Err(error),
        };
        match read {
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "machine_ended" | "machine_unreachable" | "machine_unanswered" | "machine_name_ended"
                ) =>
            {
                let Some(row) = self.kept_named(text)? else { return Err(error) };
                if log != MachineLog::Tensord {
                    return Err(Error::new(
                        Kind::NotFound,
                        "machine_log_not_kept",
                        format!("{} isn't answering, and Cozy keeps a copy of its TensorD log only.", row.name),
                    )
                    .next(format!("cozy machine logs {}", row.name)));
                }
                self.kept_log(&row.name, lines)
                    .map(String::into_bytes)
                    .ok_or_else(|| error.remedy("Cozy didn't keep any of its log."))
            }
            read => read,
        }
    }

    /// As `logs`, or the lines after `after` (a batch's place), then each batch as the machine
    /// appends it.
    pub(crate) async fn follow_logs(
        &self,
        text: &str,
        log: MachineLog,
        lines: u64,
        after: Option<&str>,
    ) -> Result<crate::link::Live<v1::LogLines>, Error> {
        let (row, link) = self.linked(text).map_err(|error| match self.kept_named(text) {
            Ok(Some(row)) if log == MachineLog::Tensord && self.kept_log(&row.name, 1).is_some() => error
                .remedy("Without --follow, it prints the copy of its log Cozy kept."),
            _ => error,
        })?;
        let lines = link.follow_log(log_name(log), lines, after).await;
        lines.map_err(|e| log_refusal(&row.name, log, e))
    }

    /// Its GPUs' use, once a second while followed (cl-1100). A machine whose Cozy predates it says so.
    pub(crate) async fn follow_gpus(&self, text: &str) -> Result<crate::link::Live<v1::GpuUse>, Error> {
        let (row, link) = self.linked(text)?;
        link.follow_gpus().await.map_err(|e| match e {
            LinkError::NotFound => Error::new(
                Kind::NotFound,
                "machine_gpu_use_unknown",
                format!("{} runs an older Cozy that doesn't report GPU use. It will once it updates.", row.name),
            ),
            e => refusal(&row.name, e),
        })
    }

    /// The machine whose kept lines `text` reads: as `named`, or by its name the one ended machine that had it when no live
    /// one has (a rental id names it too; an added machine has none).
    fn kept_named(&self, text: &str) -> Result<Option<MachineRow>, Error> {
        match self.named(text) {
            Err(error) if error.code == "machine_name_ended" => {
                let mut ended = self.store().machines_named(text).map_err(internal)?;
                ended.retain(|row| row.state == MachineState::Ended);
                match ended.len() {
                    1 => Ok(ended.pop()),
                    _ => Err(error),
                }
            }
            named => named,
        }
    }

    pub(crate) fn row(&self, name: &str) -> Result<MachineRow, Error> {
        self.store()
            .machine(name)
            .map_err(internal)?
            .ok_or_else(|| not_found(name))
    }

    /// A machine and its connection, which it has only once it has an endpoint.
    pub(crate) fn linked(&self, text: &str) -> Result<(MachineRow, Link), Error> {
        let row = self.named(text)?.ok_or_else(|| not_found(text))?;
        if row.state == MachineState::Ended {
            return Err(Error::new(
                Kind::Conflict,
                "machine_ended",
                format!("{} has ended.", row.name),
            )
            .next("cozy machine ls"));
        }
        match self.calcifer.link(&row.name) {
            Ok(Some(link)) => Ok((row, link)),
            Ok(None) => Err(Error::new(
                Kind::Unavailable,
                "machine_booting",
                format!("{} is still starting up.", row.name),
            )
            .next(format!("cozy machine show {}", row.name))),
            Err(e) => Err(Error::new(Kind::Unavailable, "machine_unreachable", e)),
        }
    }

    /// The machine as `GET /v1/machines` answers it.
    pub(crate) fn picture(&self, row: &MachineRow) -> Result<Machine, Error> {
        self.picture_for(row, &[])
    }

    /// The machine, its installs and downloads with those of `projects`' own runs: never another project's, nor a locked one's.
    pub(crate) fn picture_for(&self, row: &MachineRow, projects: &[String]) -> Result<Machine, Error> {
        let store = self.store();
        let ending = self.pending_end(&row.name)?;
        // `row` may have been read before its end settled (a list reads every row, then each one's asks): an end that is no
        // longer pending ended it first (`finish_end`), so the row is read again after its asks, never an old state beside them.
        let again = match ending.is_none() && row.state != MachineState::Ended && row.rental_id.is_some() {
            true => store.machine(&row.name).map_err(internal)?,
            false => None,
        };
        let row = again.as_ref().unwrap_or(row);
        let mut machine = Machine::from(row);
        if row.kind == MachineKind::Local {
            machine.controller_key = self.controller_key().ok();
            machine.reach = self.local.reach();
        }
        machine.alias = store.alias(&row.name).map_err(internal)?;
        machine.ordered = self.ordered(row);
        let load = self.calcifer.machine_load().map_err(internal)?;
        (machine.running, machine.queued) = load.get(&row.name).copied().unwrap_or_default();
        machine.jobs = (store.jobs_on(&row.name, row.kind == MachineKind::Local).map_err(internal)?.iter())
            .filter_map(|job| self.calcifer.record(job).ok())
            .collect();
        if let Some(ask) = ending.filter(|_| row.state != MachineState::Ended) {
            machine.state = MachineState::Ending;
            let owed = self.end_owes(&ask.key, &row.name);
            machine.ending = (!owed.is_empty()).then(|| exporting(&owed));
            machine.end_unconfirmed = ask.reason.clone();
        } else if let Some(failed) = self.failed_end(row)? {
            machine.end_failed = Some(failed);
        }
        if machine.state == MachineState::Ready
            && let Some(reason) = self.offline(&row.name)
        {
            machine.offline = Some(reason);
        }
        let (mut held, mut activity) = (vec![], vec![]);
        // Copied under the lock and worked out after it: a picture never holds up what a machine's Status says.
        let seen = lock(&self.seen).get(&row.name).map(|seen| Seen { frame: seen.frame.clone(), boot: seen.boot.clone(), attempt: seen.attempt.clone(), unreachable: seen.unreachable.clone(), silent: seen.silent.clone(), vouched: seen.vouched.clone(), unanswered: seen.unanswered, ..*seen });
        if let Some(seen) = seen {
            if seen.stopped {
                machine.state = MachineState::Stopped;
            }
            machine.status = seen.frame.as_ref().map(|frame| self.visible(&row.name, frame));
            held = seen.frame.as_ref().map(|f| f.runs.clone()).unwrap_or_default();
            activity = seen.frame.as_ref().map(|f| f.activity.clone()).unwrap_or_default();
            if row.kind == MachineKind::Local {
                let followed = lock(&self.project_activity);
                activity.extend(projects.iter().filter_map(|project| followed.get(project)).flatten().cloned());
            }
            machine.broken = broken_in(&row.name, &seen).filter(|_| row.state != MachineState::Ended);
            machine.unreachable = seen.unreachable;
            machine.unreachable_since_ms = seen.since;
            machine.boot = seen.boot;
            machine.attempt = seen.attempt;
        }
        if let Some(status) = &mut machine.status {
            status.runs = held.into_iter().filter_map(|run| self.held(run)).collect();
            // A project's runs execute in its own engine: listed from its journal, as the machine holds them (cl-811).
            let listed: HashSet<String> = status.runs.iter().map(|run| crate::store::calcifer_id(&run.id).to_owned()).collect();
            for project_run in self.calcifer.project_runs_on(&row.name).into_iter().filter(|run| !listed.contains(run.id.as_str())) {
                let Ok(run) = self.calcifer.record(&project_run) else { continue };
                status.runs.push(wire::LiveRun {
                    id: project_run.id.to_string(),
                    state: run.machine_state.clone().unwrap_or_else(|| "running".into()),
                    waiting: run.waiting.clone().unwrap_or_default(),
                    run: Some(run),
                });
            }
            status.activity = activity.iter().filter_map(|operation| self.operation(operation, projects)).collect();
        }
        machine.rc = self.rc_follow(&row.name, self.frame(&row.name).as_ref());
        machine.held = self.held_by_hand(&row.name);
        let asked = match row.kind {
            MachineKind::Rental => store.rent_ask(&row.name).map_err(internal)?,
            _ => None,
        };
        self.hubs.describe(&mut machine, asked.as_ref());
        machine.ssh = self.ssh(&machine);
        Ok(machine)
    }

    /// `frame` as this controller's own views show it. Its models and installed packages are the machine's, whoever's run put them
    /// there (Paul, 11-13: "models / packages installed are machine-specific (not project specific)"); of its warm set, only what
    /// Calcifer's own runs set, as a project's warm items carry its inputs.
    fn visible(&self, name: &str, frame: &v1::StatusFrame) -> MachineStatus {
        let mut visible = frame.clone();
        for environment in &mut visible.environments {
            environment.warning.clear();
        }
        visible.warm = self.calcifer.maintenance_warm(name, std::mem::take(&mut visible.warm));
        visible.kernels.clear();
        for process in visible.stuck.iter_mut().filter(|process| process.command != v1::STUCK_TENSORD) {
            process.command.clear();
        }
        visible.toolchain.detail.clear();
        let mut status = status(&visible, &self.hubs);
        // When its GPUs are free of what it holds, as placement counts it (cl-1107): to 10 s, so a countdown isn't a change.
        status.busy_until_ms = crate::placement::busy_ms(self, frame).map(|ms| (crate::store::now_ms() + ms as i64 + 9_999) / 10_000 * 10_000);
        // An update says what it waits for by this controller's run numbers.
        status.update = (frame.update.as_ref())
            .map(|u| self.calcifer.machine_waiting_for(&u.waiting_for).unwrap_or_else(|| Some(&u.waiting).filter(|w| !w.is_empty()).unwrap_or(&u.state).clone()))
            .unwrap_or_default();
        status
    }

    /// The `ssh` that logs in to a rental rented with a key: its known-hosts file is its own, under this computer's home.
    fn ssh(&self, machine: &Machine) -> Option<wire::Ssh> {
        let (rental, address) = machine.rental_id.as_ref().zip(machine.rental.as_ref()?.ssh.as_deref())?;
        let (host, port) = address.rsplit_once(':')?;
        let known = self.hubs.home().root().join("ssh");
        let _ = std::fs::create_dir_all(&known);
        let args = vec![
            "ssh".to_owned(),
            "-p".to_owned(),
            port.to_owned(),
            "-o".to_owned(),
            "StrictHostKeyChecking=accept-new".to_owned(),
            "-o".to_owned(),
            format!("UserKnownHostsFile={}", known.join(format!("{rental}.known_hosts")).display()),
            format!("root@{}", host.trim_start_matches('[').trim_end_matches(']')),
        ];
        let word = |arg: &String| match arg.chars().any(|c| c.is_whitespace() || "'\"\\$`".contains(c)) {
            true => format!("'{}'", arg.replace('\'', r"'\''")),
            false => arg.clone(),
        };
        let line = args.iter().map(word).collect::<Vec<_>>().join(" ");
        Some(wire::Ssh { args, line })
    }

    /// A run the machine holds for this controller, with this controller's record of it: Calcifer's own or an open project's.
    fn held(&self, run:v1::RunState)->Option<wire::LiveRun> {
        // A run sent again (cl-935) is `<id>~<attempt>` there.
        let row=self.calcifer.active_run(crate::store::calcifer_id(&run.id))?;
        Some(wire::LiveRun {run:self.calcifer.record(&row).ok(),id:run.id,state:run.state,waiting:self.calcifer.runs_named(&run.waiting)})
    }

    /// `text` with each run it names as people know it (`Calcifer::runs_named`).
    fn runs_named(&self, text: &str) -> String {
        self.calcifer.runs_named(text)
    }

    /// What `operation` is waited on by, by run number: maintenance runs, and runs of `projects` that are open. An operation of a
    /// project's own is in its private scope (`resource_key`), so its runs are all that project's.
    fn operation(&self,operation:&v1::Operation,projects:&[String])->Option<wire::Operation> {
        let mut project=None;
        let mut number=|id:&str| {
            let id=crate::store::calcifer_id(id);
            if let Some(row)=self.calcifer.maintenance_store().run(id).ok().flatten() { return Some((row.number,self.calcifer.named(&row))); }
            match self.calcifer.run_store(id).ok()? {
                crate::store::RunStore::Project(store) if projects.iter().any(|project| project==store.project_id()) => {
                    let row=store.run(id).ok().flatten()?;
                    project=Some(store.project_id().to_owned());
                    Some((row.number,self.calcifer.run_name(&row)))
                }
                _ => None,
            }
        };
        let (runs,names):(Vec<u64>,Vec<String>)=operation.runs.iter().map(|id|number(id)).collect::<Option<Vec<_>>>()?.into_iter().unzip();
        if runs.is_empty() {return None;}
        let rate=Some(operation.bytes_per_s).filter(|rate|*rate>=1.0);
        let left=operation.bytes_total.saturating_sub(operation.bytes_done);
        Some(wire::Operation {id:operation.id.clone(),kind:operation.kind.clone(),subject:operation.subject.clone(),stage:operation.stage.clone(),bytes_done:operation.bytes_done,bytes_total:operation.bytes_total,bytes_here:operation.bytes_here,bytes_per_s:rate,eta_s:rate.filter(|_|left>0).map(|rate|(left as f64/rate) as i64),started_at_ms:operation.started_at_unix_ms,runs,names,project})
    }

    /// What this computer's machine does for `project`'s own runs, followed in the project's scope while it has runs there: its runs
    /// execute in the project's own engine, whose installs and downloads only that scope's status lists. A locked project's ends.
    pub(crate) fn follow_project(self: &Arc<Self>, project: &str) {
        if lock(&self.project_activity).contains_key(project) { return; }
        lock(&self.project_activity).insert(project.to_owned(), vec![]);
        let (me, project) = (self.clone(), project.to_owned());
        self.spawn(async move {
            let listed = me.project_frames(&project).await;
            // Asked again no sooner: one that can't be followed now isn't tried in a loop.
            tokio::time::sleep(Duration::from_secs(1)).await;
            lock(&me.project_activity).remove(&project);
            if listed { me.calcifer.machine_changed(); }
        });
    }

    /// Whether it listed anything.
    async fn project_frames(&self, project: &str) -> bool {
        let mut listed = false;
        let working = || self.calcifer.project_store(project).and_then(|store| store.working("local")).is_ok_and(|n| n > 0);
        while working() {
            let scoped = (|| {
                let store = self.calcifer.project_store(project).ok()?;
                let (id, root) = store.execution_scope();
                let scope = crate::private_io::scope_for_project(id, root).ok()?;
                Some(self.calcifer.link("local").ok()??.with_private(scope))
            })();
            let Some(link) = scoped else { return listed };
            if let Ok(mut frames) = link.watch().await {
                while let Some(Ok(frame)) = frames.next().await {
                    let quiet = frame.activity.is_empty();
                    listed |= !quiet;
                    if let Some(held) = lock(&self.project_activity).get_mut(project) { *held = frame.activity; }
                    self.calcifer.machine_changed();
                    if quiet && !working() { return listed; }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        listed
    }

    /// Keeps one `Status` stream open to the machine while it has an endpoint and has not ended.
    pub(crate) fn watch(self: &Arc<Self>, row: &MachineRow) {
        let mut watching = lock(&self.watching);
        let endpoint = row
            .endpoint
            .clone()
            .filter(|_| row.state != MachineState::Ended);
        let current = watching.get(&row.name);
        if current.is_some_and(|(e, task)| Some(e) == endpoint.as_ref() && !task.is_finished()) {
            return;
        }
        if let Some((_, task)) = watching.remove(&row.name) {
            task.abort();
        }
        if let Some(endpoint) = endpoint {
            let task = self.spawn(self.clone().follow_status(row.name.clone(), row.kind));
            watching.insert(row.name.clone(), (endpoint, task));
        }
    }

    /// Records the machine as gone for good: its runs settle as the core rules, and it is no
    /// longer watched.
    pub(crate) fn ended(&self, name: &str, reason: &str) {
        if let Err(e) = self.calcifer.machine_ended(name, reason) {
            warn!(machine = name, "recording its end: {e}");
        }
        if let Some((_, task)) = lock(&self.watching).remove(name) {
            task.abort();
        }
        if let Some(seen) = lock(&self.seen).get_mut(name) {
            (seen.boot, seen.attempt) = (None, None);
        }
        self.arrivals.send_modify(|n| *n += 1);
        self.calcifer.machine_changed();
    }

    /// Follows the machine's Status for as long as it lives. A broken stream is opened again,
    /// paced; the machine is unreachable only when no connection to it can be made, never for a
    /// stream that broke or an answer that is slow. For a rental that is unreachable its Hub is
    /// asked, paced apart, and its word is the only one that ends it. While its Hub says it is
    /// offline only its Hub is read, never the machine, until the Hub says it is back or gone.
    async fn follow_status(self: Arc<Self>, name: String, kind: MachineKind) {
        let (mut pause, mut hub_pause) = (Duration::ZERO, Duration::ZERO);
        let mut hub_at = tokio::time::Instant::now();
        loop {
            match self.store().machine(&name) {
                Ok(Some(row)) if row.state != MachineState::Ended => {}
                _ => return,
            }
            let offline = self.offline(&name).is_some();
            let mut unreachable = false;
            if !offline {
                let streamed = self.status_frames(&name, &mut pause).await;
                self.stream_closed(&name);
                let error = match streamed {
                    Ok(()) if kind == MachineKind::Local => return self.local_exited(),
                    // A rental's word that it released itself is not its end: its Hub's is, as for
                    // any end. Until the Hub affirms it gone it is `ending`, and if this computer
                    // can't ask, it shows why and that it may still bill (`end_failed`).
                    Ok(()) if kind == MachineKind::Rental => {
                        if let Err(e) = self.released_itself(&name) {
                            warn!(machine = name, "confirming its release with its Hub: {}", e.message);
                        }
                        return;
                    }
                    Ok(()) => {
                        self.ended(&name, "released_by_pod");
                        return;
                    }
                    Err(error) => error,
                };
                if kind == MachineKind::Local && !self.local.running() {
                    return self.local_exited();
                }
                if let LinkError::Unreachable(reason) = error {
                    self.unreachable(&name, reason);
                    unreachable = true;
                    if pause.is_zero() {
                        hub_pause = Duration::ZERO;
                    }
                }
                self.silent_after_update(&name);
            }
            let ask = kind == MachineKind::Rental && (offline || unreachable);
            if ask && tokio::time::Instant::now() >= hub_at {
                hub_pause = (hub_pause * 2).clamp(Duration::from_secs(5), Duration::from_secs(60));
                hub_at = tokio::time::Instant::now() + hub_pause;
                if let Some(Word::Gone(_) | Word::Moved) = self.hub_word(&name).await {
                    return;
                }
            }
            match self.offline(&name).is_some() {
                true => tokio::time::sleep_until(hub_at).await,
                false => {
                    pause = (pause * 2).clamp(Duration::from_millis(500), Duration::from_secs(10));
                    tokio::time::sleep(pause).await;
                }
            }
        }
    }

    /// Reads the machine's Status frames until it says it released itself (`Ok`) or the stream
    /// breaks. Any frame resets the pause. While its frames show this computer's runs live there, a
    /// rental's Hub is told so (`vouch`) at each frame and, with none coming, every `VOUCH_EVERY`.
    async fn status_frames(self: &Arc<Self>, name: &str, pause: &mut Duration) -> Result<(), LinkError> {
        let link = (self.calcifer.link(name).map_err(LinkError::Unreachable)?)
            .ok_or_else(|| LinkError::Unreachable("it has no address yet".into()))?;
        let mut frames = link.watch().await?;
        let mut busy = false;
        loop {
            let next = tokio::select! {
                next = frames.next() => next,
                _ = tokio::time::sleep(VOUCH_EVERY), if busy => {
                    self.vouch(name);
                    continue;
                }
            };
            match next {
                Some(Ok(frame)) => {
                    *pause = Duration::ZERO;
                    let released = frame.phase == "releasing";
                    busy = working(&frame);
                    self.observe(name, frame);
                    self.vouch(name);
                    if released {
                        return Ok(());
                    }
                }
                None => {
                    let closed = "the machine closed its connection";
                    return Err(LinkError::Unanswered(closed.into()));
                }
                Some(Err(e)) => return Err(e),
            }
        }
    }

    fn observe(&self, name: &str, mut frame: v1::StatusFrame) {
        if let Some(text) = frame.machine_log.take() {
            self.keep_log(name, &text);
        }
        self.calcifer.observe_warm(name, frame.warm.clone());
        self.calcifer.observe_downloads(name, &frame.activity);
        self.lists.observe(name, &frame);
        let installed;
        let (back, ready) = {
            let mut seen = lock(&self.seen);
            let seen = seen.entry(name.to_owned()).or_default();
            // A stream sends its queue and times only when they change: absent, they stand as its last frame said.
            if let Some(last) = &seen.frame {
                if frame.queue.is_none() {
                    frame.queue = last.queue.clone();
                }
                if frame.times.is_none() {
                    frame.times = last.times.clone();
                }
            }
            let ready = frame.phase == "ready" && seen.frame.as_ref().is_none_or(|f| f.phase != "ready");
            if ready {
                self.placing.notify_one();
                self.rc_ready.notify_one();
            }
            // Ready on a boot not seen ready before (it came up, or started again on new software): plans count it now.
            if frame.phase == "ready" && seen.frame.as_ref().is_none_or(|f| f.phase != "ready" || f.boot_id != frame.boot_id) {
                self.arrivals.send_modify(|n| *n += 1);
            }
            seen.stopped &= frame.phase == "releasing";
            seen.streaming = true;
            // What this computer has installed is in the tool list: a change there is told (cl-1126).
            let installs = |frame: &v1::StatusFrame| frame.environments.iter().map(|env| (env.package.clone(), env.release.clone(), env.hub.clone())).collect::<std::collections::BTreeSet<_>>();
            installed = name == "local" && seen.frame.as_ref().is_none_or(|last| installs(last) != installs(&frame));
            seen.frame = Some(frame);
            seen.heard = Some(crate::store::now_ms());
            seen.since = None;
            seen.silent = None;
            seen.unanswered = None;
            (seen.unreachable.take().is_some(), ready)
        };
        if installed {
            crate::events::tools_changed();
        }
        // Its runs and exports that wait on it try again now: it came back, or came up (a run held for its boot, cl-1069); and
        // Calcifer's queue may go to it (cl-1132).
        if back || ready {
            self.calcifer.touch(name);
            self.placing.notify_one();
        }
        // Its lists, unless this boot of it was sent them: it came up since they were set, or started again.
        if let Some(machines) = self.calcifer.machines() {
            self.lists.seen(&machines, name);
        }
        self.calcifer.machine_changed();
    }

    /// Tells a rental's Hub that this computer's runs are live on it, on the boot its last frame names, at most every `VOUCH_EVERY`
    /// for a boot: the Hub releases a ready machine that shows no sign of life, and this is one when it neither hears the machine
    /// check in nor reaches it (cl-977). Nothing while no run of this computer's is live there.
    fn vouch(self: &Arc<Self>, name: &str) {
        let Ok(Some(row)) = self.store().machine(name) else { return };
        let Some(rental) = row.rental_id.clone().filter(|_| row.kind == MachineKind::Rental && row.state != MachineState::Ended) else {
            return;
        };
        let boot = {
            let mut seen = lock(&self.seen);
            let Some(seen) = seen.get_mut(name) else { return };
            let Some(boot) = seen.frame.as_ref().filter(|frame| working(frame)).map(|frame| frame.boot_id.clone()) else { return };
            let now = tokio::time::Instant::now();
            if seen.vouched.as_ref().is_some_and(|(at_boot, at)| *at_boot == boot && now.duration_since(*at) < VOUCH_EVERY) {
                return;
            }
            seen.vouched = Some((boot.clone(), now));
            boot
        };
        let me = self.clone();
        let name = name.to_owned();
        self.spawn(async move {
            let vouched = match me.hubs.client(&row.hub) {
                Ok(client) => client.vouch_rental(&rental, &boot).await.map_err(|e| hub_refusal(&row.hub, e)),
                Err(e) => Err(e),
            };
            match vouched {
                Ok(()) => {}
                // A Hub from before vouches (404) keeps rentals without them; one that hasn't reached this boot yet takes the next.
                Err(e) if e.kind == Kind::NotFound || e.cause.as_deref() == Some("rental.boot_not_pinned") => {
                    tracing::debug!(machine = name, "its Hub takes no vouch now: {}", e.message)
                }
                Err(e) => warn!(machine = name, "telling its Hub this computer's runs are live there: {}", e.message),
            }
        });
    }

    /// Its frame as it answers now, each ask at most the machine client's bound (xs-199). One connected that doesn't answer is said not
    /// to (its reason on the board) and asked again in the background (`unanswered`); meanwhile this answers at once, never waiting on it.
    pub(crate) async fn status_read(self: &Arc<Self>, name: &str, link: &Link) -> Result<v1::StatusFrame, String> {
        if let Some((why, _)) = self.not_answering(name) {
            return Err(why);
        }
        match link.status().await {
            Ok(frame) => {
                self.answered(name);
                Ok(frame)
            }
            Err(LinkError::Unreachable(e)) => {
                self.unreachable(name, e.clone());
                Err(e)
            }
            Err(e) => {
                self.not_answered(name, &e.to_string());
                Err(self.not_answering(name).map_or_else(|| e.to_string(), |(why, _)| why))
            }
        }
    }

    /// Why `name` isn't answering its frame, and whether it answered none of its attempts (no job waits for it); none while it answers.
    pub(crate) fn not_answering(&self, name: &str) -> Option<(String, bool)> {
        let seen = lock(&self.seen);
        let seen = seen.get(name)?;
        let unanswered = seen.unanswered?;
        Some((seen.unreachable.clone().unwrap_or_else(|| "not answering".into()), unanswered.dropped))
    }

    /// `name` answered: a candidate again, on what it says.
    fn answered(&self, name: &str) {
        let back = {
            let mut seen = lock(&self.seen);
            let Some(seen) = seen.get_mut(name) else { return };
            match seen.unanswered.take() {
                Some(_) => {
                    seen.unreachable = None;
                    seen.since = None;
                    true
                }
                None => false,
            }
        };
        if back {
            tracing::info!(machine = name, "it answers again");
            self.calcifer.touch(name);
            self.calcifer.machine_changed();
        }
    }

    /// `name`, connected, didn't answer its frame (`error`): said so, and asked again in the background, backing off 1, 2, 4, 8 s, each
    /// attempt said; after `STATUS_ATTEMPTS` no job waits for it, and it is asked every `STATUS_AGAIN` until it answers.
    fn not_answered(self: &Arc<Self>, name: &str, error: &str) {
        let first = {
            let mut seen = lock(&self.seen);
            let seen = seen.entry(name.to_owned()).or_default();
            let first = seen.unanswered.is_none();
            seen.unanswered.get_or_insert(Unanswered { attempt: 1, dropped: false });
            first
        };
        if !first {
            return;
        }
        warn!(machine = name, "it doesn't answer: {error} (attempt 1 of {STATUS_ATTEMPTS})");
        self.unreachable(name, format!("{error}, attempt 1 of {STATUS_ATTEMPTS}"));
        let me = self.clone();
        let name = name.to_owned();
        self.spawn(async move {
            loop {
                let Some(unanswered) = lock(&me.seen).get(&name).and_then(|seen| seen.unanswered) else { return };
                let wait = match unanswered.dropped {
                    true => STATUS_AGAIN,
                    false => Duration::from_secs(1 << (unanswered.attempt - 1)),
                };
                tokio::time::sleep(wait).await;
                if lock(&me.seen).get(&name).is_none_or(|seen| seen.unanswered.is_none()) {
                    return;
                }
                let Ok(Some(link)) = me.calcifer.link(&name) else {
                    me.answered(&name);
                    return;
                };
                let error = match link.status().await {
                    Ok(_) => {
                        me.answered(&name);
                        return;
                    }
                    Err(e) => e.to_string(),
                };
                let reason = {
                    let mut seen = lock(&me.seen);
                    let Some(held) = seen.get_mut(&name).and_then(|seen| seen.unanswered.as_mut()) else { return };
                    match held.dropped {
                        true => format!("{error}, after {STATUS_ATTEMPTS} attempts; trying again every {}s", STATUS_AGAIN.as_secs()),
                        false => {
                            held.attempt += 1;
                            held.dropped = held.attempt >= STATUS_ATTEMPTS;
                            format!("{error}, attempt {} of {STATUS_ATTEMPTS}", held.attempt)
                        }
                    }
                };
                warn!(machine = name, "{reason}");
                me.unreachable(&name, reason);
            }
        });
    }

    fn unreachable(&self, name: &str, reason: String) {
        self.calcifer.observe_downloads(name, &[]);
        {
            let mut seen = lock(&self.seen);
            let seen = seen.entry(name.to_owned()).or_default();
            seen.since.get_or_insert_with(crate::store::now_ms);
            seen.unreachable = Some(reason);
        }
        self.calcifer.machine_changed();
    }

    /// The workload key `name`'s TensorD last said it holds: what its runs' capabilities bind.
    pub(crate) fn seen_workload_key(&self, name: &str) -> Option<String> {
        let seen = lock(&self.seen);
        let frame = seen.get(name)?.frame.as_ref()?;
        (!frame.workload_key.is_empty()).then(|| frame.workload_key.clone())
    }

    /// Forgets the workload key `name` was seen with: its next send reads it again.
    pub(crate) fn forget_workload_key(&self, name: &str) {
        if let Some(frame) = lock(&self.seen).get_mut(name).and_then(|seen| seen.frame.as_mut()) {
            frame.workload_key.clear();
        }
    }

    /// Why the machine is offline: its Hub's last word is that its provider stopped it or that
    /// it is ending, and it has not answered since. None while it answers, whatever its Hub said.
    pub(crate) fn offline(&self, name: &str) -> Option<String> {
        let row = self.store().machine(name).ok()??;
        self.offline_in(&row, &lock(&self.seen))
    }

    /// Why nothing can run on the machine (cl-977): its own word, else this computer's that it went silent after an update.
    pub(crate) fn broken(&self, name: &str) -> Option<v1::Broken> {
        broken_in(name, lock(&self.seen).get(name)?)
    }

    /// A rental silent since an update restarted it, past `UPDATE_RETURN_MS` (cl-977): broken by this computer's word, its runs
    /// unsent here leave it, and one this computer ordered is ended, billing for nothing. Nothing there can be making progress:
    /// it has said nothing since its update drained its runs. Asked at each failed attempt to reach it, never on a timer.
    fn silent_after_update(self: &Arc<Self>, name: &str) {
        let Ok(Some(row)) = self.store().machine(name) else { return };
        if row.kind != MachineKind::Rental || row.state == MachineState::Ended {
            return;
        }
        let updating = self.store().updating(name).unwrap_or_default();
        let now = crate::store::now_ms();
        let said = {
            let mut seen = lock(&self.seen);
            let seen = seen.entry(name.to_owned()).or_default();
            let restarted = updating || seen.frame.as_ref().and_then(|f| f.update.as_ref()).is_some_and(|u| u.state == "running");
            match *seen.heard.get_or_insert(now) {
                heard if seen.silent.is_none() && restarted && now - heard >= UPDATE_RETURN_MS => {
                    let said = format!("it hasn't answered since an update restarted it {} ago", wire::clock(now - heard));
                    seen.silent = Some(v1::Broken { code: "silent_after_update".into(), message: said.clone() });
                    said
                }
                _ => return,
            }
        };
        warn!(machine = name, "broken: {said}");
        self.calcifer.touch(name);
        self.calcifer.machine_changed();
        if !self.ordered(&row) || self.pending_end(name).ok().flatten().is_some() {
            return;
        }
        match self.end_here(row, true) {
            Ok(_) => warn!(machine = name, "ending it: broken, it bills for nothing"),
            Err(e) => warn!(machine = name, "ending it: {}", e.message),
        }
    }

    fn offline_in(&self, row: &MachineRow, seen: &HashMap<String, Seen>) -> Option<String> {
        let answering = seen
            .get(&row.name)
            .is_some_and(|s| s.frame.is_some() && s.unreachable.is_none());
        match row.kind {
            MachineKind::Rental if !answering => self.hubs.offline(row),
            _ => None,
        }
    }

    /// The machine's failure to answer, and for how long, for people.
    pub(crate) fn unanswered(&self, name: &str, error: &str) -> String {
        match lock(&self.seen).get(name).and_then(|s| s.since) {
            Some(at) => format!(
                "{name} hasn't answered for {}: {error}",
                wire::clock(crate::store::now_ms() - at)
            ),
            None => format!("{name} isn't answering: {error}"),
        }
    }

    /// The name of the machine a run's `--on` names (`named`), refused before anything is
    /// recorded when the run could not reach it.
    pub(crate) async fn reach(self: &Arc<Self>, text: &str) -> Result<String, Error> {
        // This computer's machine starts for the run.
        if text.eq_ignore_ascii_case("local") {
            return self.local_row().map(|_| "local".to_owned());
        }
        let row = match self.named(text) {
            Ok(Some(row)) => row,
            Ok(None) => return Ok(text.to_owned()),
            // A name only ended machines had is the newest of them, refused as ended: a render places its job again (`project::GONE`).
            Err(e) if e.code == "machine_name_ended" => match self.store().machines_named(text).map_err(internal)?.into_iter().next() {
                Some(row) => row,
                None => return Err(e),
            },
            Err(e) => return Err(e),
        };
        self.reachable(&row).await.map(|()| row.name)
    }

    /// Refuses a run to a machine it cannot reach: one that ended, one its Hub says is offline,
    /// and one that answers no connect while its Hub says nothing of it coming. A rental on its
    /// way takes it. A machine whose Status stream is live is asked nothing, nor is its Hub; a
    /// Hub is asked only of a rental that did not answer.
    async fn reachable(self: &Arc<Self>, row: &MachineRow) -> Result<(), Error> {
        let name = row.name.clone();
        if row.state == MachineState::Ended {
            let reason = row.reason.as_deref().unwrap_or("no reason given");
            return Err(ended_refusal(&name, reason));
        }
        // Its end waits only for what its runs owe here: it takes no new run.
        if self.pending_end(&name)?.is_some() {
            let message = format!("{name} is ending, so it can't take new runs.");
            return Err(Error::new(Kind::Conflict, "machine_ending", message).next("cozy machine ls"));
        }
        if row.endpoint.is_none() {
            return Ok(());
        }
        if let Some(reason) = self.offline(&name) {
            return Err(offline_refusal(&name, &reason));
        }
        if let Some(broken) = self.broken(&name) {
            return Err(broken_refusal(&name, &broken.message));
        }
        let failed = match lock(&self.seen).get(&name) {
            Some(seen) if seen.unreachable.is_some() => true,
            Some(seen) if seen.frame.is_some() => return Ok(()),
            _ => false,
        };
        // Not heard from yet: one connect first, and the Hub only if it fails.
        if !failed && self.status_now(&name).await {
            return Ok(());
        }
        let word = match row.kind {
            MachineKind::Rental => self.hub_word(&name).await,
            _ => None,
        };
        match word {
            Some(Word::Gone(cause)) => return Err(ended_refusal(&name, &cause)),
            Some(Word::Offline(reason)) => return Err(offline_refusal(&name, &reason)),
            Some(Word::Moved | Word::Booting) => return Ok(()),
            Some(Word::Up) | None => {}
        }
        // Its stream's failure may have been a moment's: one connect of its own.
        if failed && self.status_now(&name).await {
            return Ok(());
        }
        let error = lock(&self.seen)
            .get(&name)
            .and_then(|s| s.unreachable.clone())
            .unwrap_or_default();
        let mut message = self.unanswered(&name, &error);
        if let Some(Word::Up) = word {
            message += ". The Hub says its provider hasn't stopped it";
        }
        Err(Error::new(Kind::Unavailable, "machine_unreachable", format!("{message}."))
            .remedy("Run it on another machine, or rent a new one.")
            .next(format!("cozy machine show {name}")))
    }

    /// Reads the machine's Status now: whether a connection to it was made.
    async fn status_now(&self, name: &str) -> bool {
        let Ok(Some(link)) = self.calcifer.link(name) else {
            return false;
        };
        match link.status().await {
            Ok(frame) => {
                self.observe(name, frame);
                true
            }
            Err(LinkError::Unreachable(e)) => {
                self.unreachable(name, e);
                false
            }
            Err(_) => true,
        }
    }

    /// Places each job of Calcifer's queue, now, whenever one is created or returns to it, and whenever a machine appears, is
    /// ready, or answers again.
    async fn place_runs(self: Arc<Self>) {
        let mut events = self.calcifer.subscribe();
        loop {
            {
                let _took = Took::new("place_runs");
                Box::pin(self.place_unplaced()).await;
            }
            loop {
                let event = tokio::select! {
                    event = events.recv() => event,
                    () = self.placing.notified() => break,
                };
                match event {
                    Ok((_, Event::Created { run, .. } | Event::Queued { run, .. }))
                        if run.machine.is_none() =>
                    {
                        break;
                    }
                    Ok(_) => {}
                    Err(RecvError::Lagged(_)) => break,
                    Err(RecvError::Closed) => return,
                }
            }
        }
    }

    /// Calcifer's queue, placed (cl-1132, Paul 11-17: "calcifer has a queue of jobs, it gives that queue of jobs to a machine"): each
    /// job goes where it finishes soonest of its machines (`project::queued_on`), said in its log, or waits in the queue saying why
    /// while none can take it. `new` rents what the Hub places it on; `new:<sku>` that product.
    async fn place_unplaced(self: &Arc<Self>) {
        let unplaced = match self.calcifer.each_store(|store| store.unplaced()).map(|lists| lists.into_iter().flatten().collect::<Vec<_>>()) {
            Ok(unplaced) => unplaced,
            Err(e) => return warn!("reading unplaced runs: {e}"),
        };
        let Some(api) = self.calcifer.api.get().and_then(std::sync::Weak::upgrade) else { return };
        for run in unplaced {
            if run.wants != "any" {
                self.rent_for(&run, run.wants.strip_prefix("new:"), run.spec.gpus);
                continue;
            }
            let set = Box::pin(crate::project::queue_set(&api, &run)).await;
            let offer = match Box::pin(crate::project::queued_on(&api, &set, &run)).await {
                Ok(offer) => offer,
                Err(why) => {
                    self.calcifer.waiting(&run.id, why);
                    continue;
                }
            };
            let On::Machine(name) = &offer.machine else { continue };
            match self.calcifer.place(&run.id, name) {
                Ok(true) => {
                    self.calcifer.quoted(&run.id, offer.quoted());
                    let said = format!("Cozy placed it: {}", crate::placement::said(&offer, crate::store::now_ms()));
                    match self.calcifer.with_run_store(run.id.as_str(), |store| store.note(run.id.as_str(), "info", &said)) {
                        Ok(events) => self.calcifer.publish(&run.id, events),
                        Err(e) => warn!(run = %run.id, "{e}"),
                    }
                }
                Ok(false) => {}
                Err(e) => warn!(run = %run.id, "placing it: {e}"),
            }
        }
    }

    /// The name of the machine a run names, refused when it could not reach it (`reach`);
    /// refuses `new` before the run is recorded from a computer not signed in to the run's Hub
    /// (it could only fail once it was), and without the quote its renter agreed to.
    pub(crate) async fn admit(self: &Arc<Self>, submit: &SubmitRun) -> Result<Option<String>, Error> {
        match &submit.on {
            On::Machine(name) => self.reach(name).await.map(Some),
            On::New(_) => match self.hubs.client(&submit.hub).map(|_| submit.agreed) {
                Ok(None) => Err(Error::new(
                    Kind::Usage,
                    "quote_unagreed",
                    "A run that rents a machine needs the price you agreed to (POST /v1/run-quotes).",
                )),
                agreed => agreed.map(|_| None),
            },
            On::Any => Ok(None),
        }
    }

    /// The least busy rental of this controller that fits a run of `gpus` GPUs (0: any), a
    /// ready one before a booting one: runs wait for a rental on its way, never buy around it.
    fn fitting(&self, gpus: u32) -> Result<Option<String>, Error> {
        let store = self.store();
        let load = self.calcifer.machine_load().map_err(internal)?;
        let ending: HashSet<String> = store
            .pending_asks()
            .map_err(internal)?
            .into_iter()
            .filter(|ask| ask.kind == AskKind::End)
            .filter_map(|ask| ask.machine)
            .collect();
        let rows = store.machines().map_err(internal)?;
        let seen = lock(&self.seen);
        let count = |row: &MachineRow| match seen.get(&row.name).and_then(|s| s.frame.as_ref()) {
            // GPUs out of service (cl-935) don't count.
            Some(frame) if frame.phase == "ready" => Some(frame.gpus.iter().filter(|g| g.out_of_service.is_empty()).count() as u32),
            _ => self.hubs.gpus(row),
        };
        Ok(rows
            .into_iter()
            .filter(|row| row.kind == MachineKind::Rental && row.state != MachineState::Ended && self.ordered(row))
            .filter(|row| !ending.contains(&row.name))
            .filter(|row| self.offline_in(row, &seen).is_none())
            .filter(|row| seen.get(&row.name).and_then(|seen| broken_in(&row.name, seen)).is_none())
            .filter(|row| gpus == 0 || count(row).is_none_or(|n| n >= gpus))
            .min_by_key(|row| {
                let (running, queued) = load.get(&row.name).copied().unwrap_or_default();
                (row.endpoint.is_none(), running + queued, row.name.clone())
            })
            .map(|row| row.name))
    }

    /// Why `name` takes no work now (cl-920): it ended or is no machine here, its end is asked, or its Hub says it's offline. None: it may.
    pub(crate) fn gone(&self, name: &str) -> Option<String> {
        if name == "local" {
            return None;
        }
        let row = match self.store().machine(name) {
            Ok(Some(row)) => row,
            Ok(None) => return Some(format!("there's no machine named {name}")),
            Err(_) => return None,
        };
        if row.state == MachineState::Ended {
            return Some(format!("{name} has ended"));
        }
        if self.pending_end(name).ok().flatten().is_some() {
            return Some(format!("{name} is ending"));
        }
        if let Some(broken) = self.broken(name) {
            return Some(format!("{name} isn't working ({})", broken.message));
        }
        self.offline(name).map(|why| format!("{name} is offline ({why})"))
    }

    /// Whether `name` ended or is no machine here: it leaves every project's chosen machines (cl-920).
    pub(crate) fn ended_here(&self, name: &str) -> bool {
        match self.store().machine(name) {
            _ if name == "local" => false,
            Ok(Some(row)) => row.state == MachineState::Ended,
            Ok(None) => true,
            Err(_) => false,
        }
    }

    /// Where a job goes instead when the machines it names are gone or can't take it (cl-920, cl-1066): as `automatic`, but this
    /// computer and this account's rentals alone, never an added machine, which may be someone else's computer.
    pub(crate) fn fallback(&self) -> Vec<On> {
        let added = |name: &str| self.store().machine(name).ok().flatten().is_some_and(|row| row.kind == MachineKind::Added);
        let set: Vec<On> = self.automatic().into_iter().filter(|on| !matches!(on, On::Machine(name) if added(name))).collect();
        match set.is_empty() {
            true => vec![On::Machine("local".into())],
            false => set,
        }
    }

    /// Where a render's job goes with no machine chosen (`api::calcifer::render_set`), by what's held here this moment.
    pub(crate) fn automatic(&self) -> Vec<On> {
        let here = platform::Os::CURRENT.local_inference();
        let rows = self.store().live_machines().unwrap_or_default();
        let listed = |row: &&MachineRow| row.state != MachineState::Ended && (here || row.kind != MachineKind::Local);
        let machines: Vec<Machine> = rows.iter().filter(listed).filter_map(|row| self.picture(row).ok()).collect();
        wire::render_set(&machines)
    }

    /// Whether this controller placed `row` (no rental) or ordered it: a rental another client of
    /// the account ordered speaks with this computer's device key at its Hub, and takes no run
    /// unless one names it.
    pub(crate) fn ordered(&self, row: &MachineRow) -> bool {
        row.kind != MachineKind::Rental || config::Origin::parse(&row.hub).map_or(true, |hub| row.key != hub.device_key(&self.home))
    }

    /// This controller's signer for its own and added machines, minted on first use.
    pub(crate) fn owner_key(&self) -> Result<PathBuf, Error> {
        let path = self.home.owner_key();
        if !path.exists() {
            mint_key(&path)?;
        }
        Ok(path)
    }

    /// The owner key's public half, as `cozy machine authorize` takes it.
    pub(crate) fn controller_key(&self) -> Result<String, Error> {
        let key = signer(&self.owner_key()?)?;
        Ok(URL_SAFE_NO_PAD.encode(key.verifying_key().to_bytes()))
    }
}

#[derive(Deserialize)]
pub(crate) struct Refresh {
    #[serde(default)]
    refresh: bool,
}

pub(crate) async fn list(
    State(api): State<Arc<Api>>,
    query: Result<Query<Refresh>, QueryRejection>,
) -> Result<Json<MachineList>, Refusal> {
    let Query(query) = query.map_err(|e| Refusal(usage(e.body_text())))?;
    Ok(Json(
        api.machines.list(query.refresh).await.map_err(Refusal)?,
    ))
}

#[derive(Deserialize)]
pub(crate) struct Fresh {
    #[serde(default)]
    fresh: bool,
    /// The Hub whose client reads it.
    hub: Option<String>,
    /// The project its client is in: what that project's own runs installed on this computer's machine is listed too.
    project: Option<String>,
}

/// The machine; `?fresh=true` answers a Status it read now, or refuses.
pub(crate) async fn show(
    State(api): State<Arc<Api>>,
    UrlPath(name): UrlPath<String>,
    query: Result<Query<Fresh>, QueryRejection>,
) -> Result<Json<Machine>, Refusal> {
    Ok(Json(asked(&api, &name, query).await?.0))
}

/// The machine `GET /v1/machines/{name}` answers.
async fn asked(api: &Arc<Api>, name: &str, query: Result<Query<Fresh>, QueryRejection>) -> Result<(Machine, Option<String>), Refusal> {
    let Query(query) = query.map_err(|e| Refusal(usage(e.body_text())))?;
    let machine = match query.fresh {
        true => fresh(&api.machines, name).await,
        false => api.machines.show(name).await,
    };
    Ok((machine.map_err(Refusal)?, query.hub))
}

/// `GET /v1/machines/{name}/packages`: what it holds installed, each with where it came from for a client of `?hub=`, else this
/// computer's Hub.
pub(crate) async fn packages(
    State(api): State<Arc<Api>>,
    UrlPath(name): UrlPath<String>,
    query: Result<Query<Fresh>, QueryRejection>,
) -> Result<Json<wire::MachinePackages>, Refusal> {
    let Query(query) = query.map_err(|e| Refusal(usage(e.body_text())))?;
    let project = query.project.clone();
    let (mut machine, hub) = asked(&api, &name, Ok(Query(query))).await?;
    let hub = crate::hub::origin(&api, hub.as_deref())?;
    // A project's local code installs into its own worker area, which the machine's frame never lists (`project_holds`).
    if let Some(project) = project.filter(|_| machine.kind == MachineKind::Local) {
        let scope = {
            let store = api.projects.store(&api, &project).await.map_err(Refusal)?;
            let (id, root) = store.execution_scope();
            crate::private_io::scope_for_project(id, root).map_err(Refusal)?
        };
        let held = api.machines.project_holds(scope).await;
        if let Some(status) = &mut machine.status {
            status.environments.extend(held.iter().map(|env| environment(env, &api.machines.hubs)));
        }
    }
    Ok(Json(machine.packages(&hub.to_string())))
}

/// `GET /v1/machines/{name}/models`: the models it holds.
pub(crate) async fn models(
    State(api): State<Arc<Api>>,
    UrlPath(name): UrlPath<String>,
    query: Result<Query<Fresh>, QueryRejection>,
) -> Result<Json<wire::MachineModels>, Refusal> {
    Ok(Json(asked(&api, &name, query).await?.0.models()))
}

/// `POST /v1/machines/{name}/models/remove`: a model's downloaded copies leave the machine (local, added or a rental), never the
/// Hub. The machine refuses one a run there reads; the refusal names that run as a person does.
pub(crate) async fn remove_model(
    State(api): State<Arc<Api>>,
    UrlPath(name): UrlPath<String>,
    body: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<Json<wire::RemovedModel>, Refusal> {
    let asked: wire::RemoveModel = crate::api::parse(&body?)?;
    let row = api.machines.named_now(&name).await.map_err(Refusal)?;
    let (row, link) = api.machines.linked(&row.name).map_err(Refusal)?;
    match link.remove_model(&asked).await {
        Ok(removed) => {
            // `model ls --on` reads it gone at once.
            if let Ok(frame) = link.status().await {
                api.machines.observe(&row.name, frame);
            }
            Ok(Json(removed))
        }
        Err(error) => {
            let mut error = refusal(&row.name, error);
            if error.cause.as_deref() == Some("model_in_use") {
                error.message = api.machines.runs_named(&error.message);
                error.next = error.next.iter().map(|next| api.machines.runs_named(next)).collect();
            }
            Err(Refusal(error))
        }
    }
}

/// One of Calcifer's machines with the Status frame it answered now, recorded as its latest:
/// one that cannot answer is `machine_unavailable`, never its last picture.
pub(crate) async fn fresh(machines: &Arc<Machines>, text: &str) -> Result<Machine, Error> {
    let row = machines.named_now(text).await?;
    let unavailable = |why: String| {
        let message = format!("{} can't answer right now: {why}.", row.name);
        Error::new(Kind::Unavailable, "machine_unavailable", message).next("cozy machine ls")
    };
    if row.state == MachineState::Ended {
        return Err(unavailable("it has ended".into()));
    }
    if let Some(reason) = machines.offline(&row.name) {
        return Err(offline_refusal(&row.name, &reason));
    }
    let link = machines.calcifer.link(&row.name).map_err(unavailable)?;
    let link = link.ok_or_else(|| unavailable("it has no address yet".into()))?;
    match link.status().await {
        Ok(frame) => machines.observe(&row.name, frame),
        Err(LinkError::Unreachable(e)) => {
            machines.unreachable(&row.name, e.clone());
            return Err(match machines.hub_word(&row.name).await {
                Some(Word::Gone(cause)) => ended_refusal(&row.name, &cause),
                Some(Word::Offline(reason)) => offline_refusal(&row.name, &reason),
                _ => unavailable(e),
            });
        }
        Err(e) => return Err(unavailable(e.to_string())),
    }
    machines.picture(&row)
}

/// `POST /v1/rental-quotes`: the Hub's price for a `Rent`, exactly as `POST /v1/machines` would
/// ask it. Nothing is rented or recorded.
pub(crate) async fn quote(
    State(api): State<Arc<Api>>,
    body: Bytes,
) -> Result<Json<wire::Quote>, Refusal> {
    let NewMachine::Rent {
        sku,
        hub,
        gpus,
        disk_gb,
        provider,
        agreed: _,
        ssh_key,
    } = parse::<NewMachine>(&body)?
    else {
        return Err(Refusal(Error::new(
            Kind::Usage,
            "quote_needs_rent",
            "Only a rental has a price.",
        )));
    };
    let hub = crate::hub::origin(&api, Some(&hub))?.to_string();
    // The product asked for is honoured or refused, never widened to another.
    let sku = api.machines.spelled_sku(&hub, &sku).await;
    api.machines.admit_sku(&hub, &sku, gpus.unwrap_or(1)).await.map_err(Refusal)?;
    let shape = Shape {
        gpus,
        disk_gb,
        provider,
        agreed: None,
        ssh_key,
        ..Shape::default()
    };
    Ok(Json(api.machines.quote(&hub, &sku, shape).await.map_err(Refusal)?))
}

#[derive(Deserialize)]
pub(crate) struct HubQuery {
    hub: Option<String>,
    since_ms: Option<i64>,
}

impl HubQuery {
    fn hub(&self, machines: &Machines) -> String {
        self.hub.clone().unwrap_or_else(|| machines.hubs.default_origin())
    }
}

/// `GET /v1/rental-skus[?hub=]`: what the Hub has for rent now, cheapest first.
pub(crate) async fn rental_skus(
    State(api): State<Arc<Api>>,
    query: Result<Query<HubQuery>, QueryRejection>,
) -> Result<Json<wire::RentalSkus>, Refusal> {
    let Query(query) = query.map_err(|e| Refusal(usage(e.body_text())))?;
    let hub = query.hub(&api.machines);
    Ok(Json(api.machines.rental_skus(&hub).await.map_err(Refusal)?))
}

/// `GET /v1/rentals[?since_ms=&hub=]`: every rental of this account at the Hub, with the sums.
pub(crate) async fn rentals(
    State(api): State<Arc<Api>>,
    query: Result<Query<HubQuery>, QueryRejection>,
) -> Result<Json<wire::RentalHistory>, Refusal> {
    let Query(query) = query.map_err(|e| Refusal(usage(e.body_text())))?;
    let hub = query.hub(&api.machines);
    Ok(Json(api.machines.history(&hub, query.since_ms).await.map_err(Refusal)?))
}

/// Rents one under the request's `Idempotency-Key` (202, or 200 when the key had), or adds
/// another computer's.
pub(crate) async fn create(
    State(api): State<Arc<Api>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Refusal> {
    match parse::<NewMachine>(&body)? {
        NewMachine::Rent {
            sku,
            hub,
            gpus,
            disk_gb,
            provider,
            agreed,
            ssh_key,
        } => {
            let key = headers
                .get("idempotency-key")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .ok_or_else(|| {
                    Refusal(
                        Error::new(
                            Kind::Usage,
                            "idempotency_key_required",
                            "Renting costs money, so the request needs an Idempotency-Key header.",
                        )
                        .remedy("One key always names the same rental, so retrying with it is safe."),
                    )
                })?;
            let hub = crate::hub::origin(&api, Some(&hub))?.to_string();
            let shape = Shape {
                gpus,
                disk_gb,
                provider,
                agreed,
                ssh_key,
                ..Shape::default()
            };
            let rented = (api.machines.rent(key, &hub, &sku, shape).await).map_err(Refusal)?;
            let status = match rented.replay {
                true => StatusCode::OK,
                false => StatusCode::ACCEPTED,
            };
            Ok((status, Json(rented)).into_response())
        }
        NewMachine::Add {
            name,
            address,
            fingerprint,
        } => {
            let added = api.machines.add(&name, &address, &fingerprint).await;
            Ok(Json(added.map_err(Refusal)?).into_response())
        }
    }
}

/// 202 while its Hub has not affirmed the end, 200 once it is over.
#[derive(Deserialize)]
pub(crate) struct Force {
    #[serde(default)]
    force: bool,
}

pub(crate) async fn end(
    State(api): State<Arc<Api>>,
    UrlPath(name): UrlPath<String>,
    query: Result<Query<Force>, QueryRejection>,
) -> Result<(StatusCode, Json<Ended>), Refusal> {
    let Query(query) = query.map_err(|e| Refusal(usage(e.body_text())))?;
    let ended = api
        .machines
        .end(&name, query.force)
        .await
        .map_err(Refusal)?;
    let status = match ended.machine.state {
        MachineState::Ending => StatusCode::ACCEPTED,
        _ => StatusCode::OK,
    };
    Ok((status, Json(ended)))
}

pub(crate) async fn keepalive(
    State(api): State<Arc<Api>>,
    UrlPath(name): UrlPath<String>,
) -> Result<Json<Keepalive>, Refusal> {
    Ok(Json(api.machines.keepalive(&name).await.map_err(Refusal)?))
}

#[derive(Deserialize)]
pub(crate) struct LogQuery {
    /// The newest N whole lines; 0: all it keeps.
    #[serde(default)]
    lines: u64,
    /// Only lines from this instant on (unix ms): those before the first stamped at or after it
    /// are left out.
    since_ms: Option<i64>,
    /// Following: on from this batch's place (or `Last-Event-ID`).
    after: Option<String>,
}

/// Calcifer's own log, `calcifer` on `local`, as a machine's: its whole lines, or followed as it is written, across a rotation.
async fn calcifer_log(api: &Arc<Api>, name: &str, query: LogQuery, headers: &HeaderMap) -> Result<Response, Refusal> {
    if !name.eq_ignore_ascii_case("local") {
        let said = format!("{name} has no Cozy log. Only this computer has one.");
        return Err(Refusal(Error::new(Kind::Usage, "log_not_kept", said).next("cozy machine logs local calcifer")));
    }
    let home = api.machines.hubs.home();
    let log = Log::rotated(home.calcifer(), "health.log");
    let unreadable = |e: &dyn std::fmt::Display| Refusal(Error::new(Kind::Internal, "log_unreadable", format!("{}: {e}", home.calcifer_health().display())));
    let mut since = Since(query.since_ms);
    if !follows(headers) {
        let read = tokio::task::spawn_blocking({
            let (log, lines) = (log, query.lines);
            move || log.read(Start::Last(lines))
        });
        let text = read.await.map_err(|e| unreadable(&e))?.map_err(|e| unreadable(&e))?;
        return Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], since.keep(&text)).into_response());
    }
    let start = match after(query.after, headers) {
        Some(place) => Start::After(place.parse().map_err(|why: String| Refusal(usage(why)))?),
        None => Start::Last(query.lines),
    };
    let follow = log.follow(start).map_err(|e| match e.is_unsupported() {
        true => Refusal(Error::new(Kind::Unavailable, "log_follow_unsupported", e.to_string())),
        false => unreadable(&e),
    })?;
    let (sender, batches) = tokio::sync::mpsc::channel(16);
    tokio::task::spawn_blocking(move || follow.run(|batch| sender.blocking_send(batch).is_ok(), || !sender.is_closed()));
    let events = futures_util::stream::unfold(batches, |mut batches| async move { batches.recv().await.map(|batch| (batch, batches)) })
        .map(move |batch| {
            let after = batch.after.to_string();
            let lines = LogEvent::Lines { text: since.keep(&batch.text), after: after.clone() };
            let data = serde_json::to_string(&lines).expect("an event serializes");
            Ok::<_, Infallible>(sse::Event::default().id(after).data(data))
        })
        .take_until(api.stopped());
    Ok(Sse::new(events).keep_alive(KeepAlive::default()).into_response())
}

/// Whether a log's reader asked to follow it.
fn follows(headers: &HeaderMap) -> bool {
    (headers.get(header::ACCEPT).and_then(|v| v.to_str().ok())).is_some_and(|v| v.contains("text/event-stream"))
}

/// The place a log's follower goes on from: `?after=`, else `Last-Event-ID`.
fn after(asked: Option<String>, headers: &HeaderMap) -> Option<String> {
    asked.or_else(|| headers.get("last-event-id").and_then(|v| v.to_str().ok()).map(str::to_owned))
}

/// A machine's log: its whole lines, or with `Accept: text/event-stream` those and then each
/// batch as the machine appends it, as `lines` events whose id (`after`) resumes the follow.
pub(crate) async fn logs(
    State(api): State<Arc<Api>>,
    UrlPath((name, log)): UrlPath<(String, String)>,
    query: Result<Query<LogQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Result<Response, Refusal> {
    let Query(query) = query.map_err(|e| Refusal(usage(e.body_text())))?;
    let log = match log.as_str() {
        "calcifer" => return calcifer_log(&api, &name, query, &headers).await,
        "tensord" => MachineLog::Tensord,
        "tensorfs" => MachineLog::Tensorfs,
        "memory" => MachineLog::Memory,
        other => {
            return Err(Refusal(Error::new(
                Kind::NotFound,
                "machine_log_unknown",
                format!("Machines have no {other:?} log. Ask for tensord, tensorfs or memory, or calcifer on local."),
            )));
        }
    };
    let mut since = Since(query.since_ms);
    if !follows(&headers) {
        let bytes = api
            .machines
            .logs(&name, log, query.lines)
            .await
            .map_err(Refusal)?;
        let text = since.keep(&String::from_utf8_lossy(&bytes));
        return Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response());
    }
    let after = after(query.after, &headers);
    let lines = api
        .machines
        .follow_logs(&name, log, query.lines, after.as_deref())
        .await
        .map_err(Refusal)?;
    // Ends when the machine's stream does, or Calcifer stops: the follower resumes from its id.
    let events = lines
        .take_while(|batch| std::future::ready(batch.is_ok()))
        .filter_map(move |batch| {
            let event = batch.ok().map(|batch| {
                let lines = LogEvent::Lines { text: since.keep(&batch.text), after: batch.after.clone() };
                let data = serde_json::to_string(&lines).expect("an event serializes");
                Ok::<_, Infallible>(sse::Event::default().id(&batch.after).data(data))
            });
            std::future::ready(event)
        })
        .take_until(api.stopped());
    Ok(Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response())
}

/// `GET /v1/machines/{name}/gpus`, as SSE (cl-1100): `gpus` events, each a `GpuUse` the machine read that second, for as long as the
/// caller follows. Nothing is read while nobody does.
pub(crate) async fn gpus(State(api): State<Arc<Api>>, UrlPath(name): UrlPath<String>) -> Result<Response, Refusal> {
    let readings = api.machines.follow_gpus(&name).await.map_err(Refusal)?;
    let events = readings
        .take_while(|reading| std::future::ready(reading.is_ok()))
        .filter_map(|reading| {
            let event = reading.ok().map(|reading| {
                let data = serde_json::to_string(&reading).expect("a reading serializes");
                Ok::<_, Infallible>(sse::Event::default().event("gpus").data(data))
            });
            std::future::ready(event)
        })
        .take_until(api.stopped());
    Ok(Sse::new(events).keep_alive(KeepAlive::default()).into_response())
}

/// A log read from an instant on: lines before the first stamped at or after it are left out. A
/// line's stamp is its first word, an RFC 3339 instant (TensorD's own log) or Unix seconds (the
/// TensorFS and memory logs); a line without one goes with the line before it.
struct Since(Option<i64>);

impl Since {
    fn keep(&mut self, text: &str) -> String {
        let Some(cutoff) = self.0 else {
            return text.to_owned();
        };
        let mut kept = String::new();
        for line in text.split_inclusive('\n') {
            if self.0.is_some() {
                if stamp(line).is_none_or(|at| at < cutoff) {
                    continue;
                }
                self.0 = None;
            }
            kept.push_str(line);
        }
        kept
    }
}

fn stamp(line: &str) -> Option<i64> {
    let word = line.split_whitespace().next()?;
    match word.parse::<jiff::Timestamp>() {
        Ok(at) => Some(at.as_millisecond()),
        Err(_) => word.parse::<f64>().ok().map(|seconds| (seconds * 1000.0) as i64),
    }
}

fn usage(message: String) -> Error {
    Error::new(Kind::Usage, "query_invalid", message)
}

/// How to reach the TensorD whose `api-ready.json` is at `path`.
pub(crate) fn endpoint_of(path: &Path) -> Result<Endpoint, Error> {
    #[derive(serde::Deserialize)]
    struct Ready {
        address: String,
        worker_id: String,
        cert_pem: String,
    }
    let unreadable = |e: String| {
        Error::new(
            Kind::Unavailable,
            "machine_unreadable",
            format!("{}: {e}", path.display()),
        )
    };
    let bytes = std::fs::read(path).map_err(|e| unreadable(e.to_string()))?;
    let ready: Ready = serde_json::from_slice(&bytes).map_err(|e| unreadable(e.to_string()))?;
    Ok(Endpoint {
        worker_id: ready.worker_id,
        address: ready.address,
        pin: pin_of(&ready.cert_pem).ok_or_else(|| unreadable("no certificate".into()))?,
    })
}

/// The sha256 of a PEM certificate's DER.
pub(crate) fn pin_of(pem: &str) -> Option<[u8; 32]> {
    let body: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .ok()
        .filter(|der| !der.is_empty())?;
    Some(Sha256::digest(der).into())
}

/// Merges a warm run's items into the machine's warm set, keyed by source and entrypoint: the
/// spec then names the whole set. Level 0 removes an item. The merged set is what the machine
/// holds once it takes the run, so the next warm run merges into it.
pub(crate) fn merge_warm(current: &mut Vec<v1::WarmItem>, spec: &mut v1::RunSpec) {
    if spec.kind != v1::RunKind::Warm {
        return;
    }
    let Some(set) = spec.set.as_mut() else {
        return;
    };
    for item in std::mem::take(&mut set.items) {
        current.retain(|held| (&held.source, &held.entrypoint) != (&item.source, &item.entrypoint));
        if item.level != v1::WarmLevel::Off {
            current.push(item);
        }
    }
    set.items = current
        .iter()
        .map(|held| v1::WarmItem {
            holds: String::new(),
            held_back: String::new(),
            ..held.clone()
        })
        .collect();
    *current = set.items.clone();
}

/// An installed environment as a machine's Status lists it, its Hub by this controller's name for it.
fn environment(env: &v1::Environment, hubs: &Hubs) -> wire::Environment {
    wire::Environment {
        installation: env.installation.clone(),
        package: env.package.clone(),
        release: env.release.clone(),
        hub: hubs.named(&env.hub),
        runtime: env.runtime.clone(),
        level: env.level.clone(),
        warning: env.warning.clone(),
    }
}

fn status(frame: &v1::StatusFrame, hubs: &Hubs) -> MachineStatus {
    let level = |level: v1::WarmLevel| match level {
        v1::WarmLevel::Off => "",
        v1::WarmLevel::Installed => "installed",
        v1::WarmLevel::Downloaded => "downloaded",
        v1::WarmLevel::Imported => "imported",
        v1::WarmLevel::Host => "host",
        v1::WarmLevel::Gpu => "gpu",
    };
    MachineStatus {
        worker_id: frame.worker_id.clone(),
        version: frame.version.clone(),
        commit: frame.commit.clone(),
        dev: frame.dev,
        platform: frame.platform.clone(),
        phase: frame.phase.clone(),
        update: String::new(),
        runtime: frame.runtime.clone(),
        tensorfs: frame.tensorfs.clone(),
        torch: frame.torch.clone(),
        started_at_ms: frame.started_at_unix_ms,
        idle_deadline_ms: deadline(frame),
        gpus: frame
            .gpus
            .iter()
            .map(|gpu| wire::Gpu {
                index: gpu.index,
                name: gpu.name.clone(),
                memory_bytes: gpu.memory_bytes,
                driver: gpu.driver.clone(),
                family: Some(gpu.family.clone()).filter(|f| !f.is_empty()),
                memory_class: Some(gpu.memory_class.clone()).filter(|c| !c.is_empty()),
                memory_limit_bytes: gpu.memory_limit_bytes,
                display: gpu.display,
                out_of_service: gpu.out_of_service.clone(),
            })
            .collect(),
        mode: frame.mode.clone(),
        driver_floor: frame.driver_floor.clone(),
        gpu_problems: frame.gpu_problems.clone(),
        toolchain: Some(frame.toolchain.clone()).filter(|t| !t.state.is_empty()),
        kernels: frame.kernels.clone(),
        stuck: frame.stuck.clone(),
        heat: frame.heat.clone(),
        runtime_kernels: frame.runtime_kernels.clone(),
        busy_until_ms: None,
        disk_total_bytes: frame.disk.as_ref().map_or(0, |d| d.total_bytes),
        disk_free_bytes: frame.disk.as_ref().map_or(0, |d| d.free_bytes),
        disk_models_bytes: frame.disk.as_ref().map_or(0, |d| d.models_bytes),
        disk_environments_bytes: frame.disk.as_ref().map_or(0, |d| d.environments_bytes),
        disk_runtime_bytes: frame.disk.as_ref().map_or(0, |d| d.runtime_bytes),
        disk_runs_bytes: frame.disk.as_ref().map_or(0, |d| d.runs_bytes),
        runs: vec![],
        activity: vec![],
        models: frame
            .models
            .iter()
            .map(|model| wire::Model {
                model: model.model.clone(),
                total_bytes: model.total_bytes,
                unique_bytes: model.unique_bytes,
                checkpoints: model
                    .checkpoints
                    .iter()
                    .map(|c| wire::Checkpoint {
                        release: c.release.clone(),
                        lane: c.lane.clone(),
                        precision: c.precision.clone(),
                        manifest: c.manifest.clone(),
                        role: c.role.clone(),
                        complete: c.complete,
                    })
                    .collect(),
            })
            .collect(),
        models_bytes: frame.models_bytes,
        environments: frame.environments.iter().map(|env| environment(env, hubs)).collect(),
        warm: frame
            .warm
            .iter()
            .map(|item| wire::WarmItem {
                source: match &item.source {
                    Some(v1::WarmSource::Release(r)) if r.release.is_empty() => r.package.clone(),
                    Some(v1::WarmSource::Release(r)) => format!("{}@{}", r.package, r.release),
                    Some(v1::WarmSource::Installation(id)) => id.clone(),
                    None => String::new(),
                },
                entrypoint: item.entrypoint.clone(),
                level: level(item.level).into(),
                holds: item.holds.clone(),
                held_back: item.held_back.clone(),
            })
            .collect(),
    }
}

fn deadline(frame: &v1::StatusFrame) -> Option<i64> {
    (frame.idle_deadline_unix_ms > 0).then_some(frame.idle_deadline_unix_ms)
}

/// A machine name: 1-32 of `a-z0-9-`, a letter first; `local`, `any` and `new` are taken.
pub(crate) fn valid_name(name: &str) -> Result<(), Error> {
    let mut chars = name.chars();
    let valid = name.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !["local", "any", "new"].contains(&name);
    if valid {
        Ok(())
    } else {
        Err(Error::new(
            Kind::Usage,
            "machine_name_invalid",
            format!(
                "{name:?} can't be a machine name. Use 1-32 lowercase letters, digits and hyphens, \
                 starting with a letter, and not local, any or new."
            ),
        ))
    }
}

/// Mints an Ed25519 key at `path`.
pub(crate) fn mint_key(path: &Path) -> Result<SigningKey, Error> {
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).expect("the OS provides randomness");
    let key = SigningKey::from_bytes(&seed);
    write_key(path, &key)?;
    Ok(key)
}

/// Keeps `key` at `path`: PKCS#8 PEM, owner-only.
pub(crate) fn write_key(path: &Path, key: &SigningKey) -> Result<(), Error> {
    let pem = key
        .to_pkcs8_pem(Default::default())
        .map_err(|e| internal(e.to_string()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    durable::private_dir(dir)
        .and_then(|()| durable::replace(path, pem.as_bytes(), 0o600))
        .map_err(|e| internal(e.to_string()))
}

/// The key a machine's calls are signed with: a PKCS#8 PEM (this controller's owner key, a rental
/// it ordered), or this computer's device key at a Hub (a rental another client ordered).
pub(crate) fn signer(path: &Path) -> Result<SigningKey, Error> {
    let raw = std::fs::read(path).ok();
    let pem = raw.as_deref().and_then(|raw| SigningKey::from_pkcs8_pem(std::str::from_utf8(raw).ok()?).ok());
    pem.or_else(|| Some(::api::hub::DeviceKey::read(raw.as_deref()?, "").ok()?.signing_key().clone()))
        .ok_or_else(|| {
            Error::new(
                Kind::Credential,
                "key_unreadable",
                format!("Cozy can't read the signing key {}.", path.display()),
            )
        })
}

pub(crate) fn log_name(log: MachineLog) -> &'static str {
    match log {
        MachineLog::Tensord => "tensord",
        MachineLog::Tensorfs => "tensorfs-transport",
        MachineLog::Memory => "memory",
    }
}

fn log_refusal(name: &str, log: MachineLog, e: LinkError) -> Error {
    match e {
        // A TensorD serving in dev mode logs to its stderr only.
        LinkError::NotFound if log == MachineLog::Tensord => Error::new(
            Kind::NotFound,
            "machine_log_unsupported",
            format!("{name}'s TensorD doesn't keep a log of its own."),
        )
        .remedy("Its tensorfs and memory logs are there.")
        .next(format!("cozy machine logs {name} tensorfs")),
        e => refusal(name, e),
    }
}

/// A machine's refusal, as the API answers it.
pub(crate) fn refusal(name: &str, e: LinkError) -> Error {
    match e {
        LinkError::Refused { code, message, remedy, next } => {
            let named = |text: String| text.replace("<machine>", name);
            let mut error = Error::new(Kind::Conflict, "machine_refused", format!("{name}: {}", named(message))).cause(code);
            error.remedy = remedy.map(named);
            error.next = next.into_iter().map(named).collect();
            match error.next.is_empty() {
                true => error.next(format!("cozy machine show {name}")),
                false => error,
            }
        }
        LinkError::Unauthorized(message) => Error::new(
            Kind::Credential,
            "machine_unauthorized",
            format!("{name}: {message}"),
        )
        .remedy("Authorize this computer's key on that computer, then try again.")
        .next("cozy machine show local"),
        LinkError::Released(message) => Error::new(
            Kind::Unavailable,
            "machine_released",
            format!("{name}: {message}"),
        )
        .next("cozy machine ls"),
        LinkError::NoRun | LinkError::NotFound => Error::new(
            Kind::NotFound,
            "not_found",
            format!("{name} doesn't have that."),
        )
        .next(format!("cozy machine show {name}")),
        LinkError::Unreachable(message) => Error::new(
            Kind::Unavailable,
            "machine_unreachable",
            format!("{name}: {message}"),
        )
        .next(format!("cozy machine show {name}")),
        LinkError::Broken { code, message } => Error::new(Kind::Unavailable, "machine_failed", format!("{name}: {message}"))
            .cause(code)
            .next(format!("cozy machine logs {name}")),
        LinkError::Unanswered(message) => Error::new(
            Kind::Unavailable,
            "machine_unanswered",
            format!("{name} hasn't answered: {message}"),
        )
        .next(format!("cozy machine show {name}")),
    }
}

fn ended_refusal(name: &str, reason: &str) -> Error {
    Error::new(Kind::Conflict, "machine_ended", format!("{name} has ended ({reason})."))
        .remedy("Run it on another machine, or rent a new one.")
        .next("cozy machine ls")
}

/// Why machine `name` is broken (cl-977): its own Status's word, else this computer's.
fn broken_in(name: &str, seen: &Seen) -> Option<v1::Broken> {
    // Stopped, this computer's machine's last word is not what it is now: its next start checks again.
    let said = seen.frame.as_ref().filter(|_| !(seen.stopped && name == "local")).and_then(|frame| frame.broken.clone());
    said.or_else(|| seen.silent.clone())
}

pub(crate) fn broken_refusal(name: &str, reason: &str) -> Error {
    Error::new(Kind::Unavailable, "machine_broken", format!("{name} isn't working ({reason})."))
        .remedy("It won't take work, and a rental ends. Run it on another machine, or rent a new one.")
        .next(format!("cozy machine show {name}"))
}

fn offline_refusal(name: &str, reason: &str) -> Error {
    Error::new(Kind::Unavailable, "machine_offline", format!("{name} is offline ({reason})."))
        .remedy("It comes back if its provider resumes it. Until then, run it on another machine, or rent a new one.")
        .next(format!("cozy machine show {name}"))
}

/// `local` is never ended.
fn local_refusal() -> Error {
    Error::new(
        Kind::Validation,
        "machine_local",
        "This computer's machine starts when a run needs it and stops after 15 minutes idle, so there's nothing to end.",
    )
    .next("cozy machine show local")
}

/// The refusal of a name only ended machines had: a Hub gives a name again once its rental ends,
/// so an ended machine is named by its rental id.
fn ended_by_name(text: &str, ended: &[MachineRow]) -> Error {
    let ids: Vec<&str> = ended.iter().filter_map(|row| row.rental_id.as_deref()).collect();
    let mut error = Error::new(
        Kind::NotFound,
        "machine_name_ended",
        format!("The machine named {text:?} has ended."),
    )
    .remedy("Use its rental id to look at an ended machine.");
    for id in ids {
        error = error.next(format!("cozy machine show {id}"));
    }
    error.next("cozy machine ls")
}

/// The refusal of a name several live machines have, naming each by its rental id.
fn ambiguous(text: &str, found: &[MachineRow]) -> Error {
    let named: Vec<String> = (found.iter())
        .map(|row| row.rental_id.clone().unwrap_or_else(|| format!("{} (this controller's)", row.name)))
        .collect();
    let mut error = Error::new(
        Kind::Conflict,
        "machine_name_ambiguous",
        format!("{} machines are named {text:?}: {}.", named.len(), named.join(", ")),
    )
    .remedy("Use a rental id to pick one.");
    for id in named.iter().filter(|id| !id.contains(' ')) {
        error = error.next(format!("cozy machine show {id}"));
    }
    error
}

pub(crate) fn not_found(text: &str) -> Error {
    Error::new(
        Kind::NotFound,
        "machine_not_found",
        format!("There's no machine named {text:?}."),
    )
    .next("cozy machine ls")
}

/// What an ending machine waits for: the outputs of its runs not landed here yet.
fn exporting(owed: &[crate::outputs::Owed]) -> String {
    let (moved, length) = (owed.iter()).fold((0, 0), |(m, l), o| (m + o.moved, l + o.length));
    let outputs = match owed.len() {
        1 => "1 result".to_owned(),
        n => format!("{n} results"),
    };
    format!(
        "{outputs} from its runs still downloading to this computer, {} of {}",
        crate::bytes(moved as f64),
        crate::bytes(length as f64)
    )
}

pub(crate) fn internal(e: impl ToString) -> Error {
    Error::new(Kind::Internal, "calcifer_store", e.to_string())
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
