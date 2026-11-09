//! This machine's Calcifer. A rental has no login to start one, so its TensorD keeps one from boot,
//! and starts it again when it exits. It is detached: a restart of this service leaves it running,
//! and its orphan reparents to the stable parent, which reaps it. One holds the home at a time
//! (`calcifer.lock`), so a pod shell's `cozy` finds this one and never makes a second, and whoever
//! starts one loses to whoever got there first. A laptop or an added machine keeps its own start
//! (Calcifer's login service); the door starts one there only when none runs.
use super::binary;
use platform::{
    durable,
    lock::{Kind, LockFile},
    paths::Home,
    process::Detached,
    service::{ServiceSpec, Services},
};
use api::calcifer::Holder;
use std::{
    path::PathBuf,
    process::ExitStatus,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// After a Calcifer that failed, the first wait and the longest. One that ran this long is a new
/// failure, whose wait starts over.
const FIRST: Duration = Duration::from_secs(1);
const MOST: Duration = Duration::from_secs(60);
/// How often the door looks at a start it waits for.
const SAMPLE: Duration = Duration::from_millis(100);

pub struct Calcifer {
    home: Home,
    services: Services,
    program: PathBuf,
    rental: bool,
    /// The door's client: loopback, HTTP/1.1, and nowhere an answer redirects it.
    pub(crate) http: reqwest::Client,
    /// The record last read.
    holder: Mutex<Option<Holder>>,
    /// One start at a time on a machine whose door starts it.
    starting: tokio::sync::Mutex<()>,
}

impl Calcifer {
    /// None in a process that never recorded its launch path: it has nothing to start.
    pub fn new(home: &Home, rental: bool) -> Option<Arc<Self>> {
        let user_home = home.root().parent().unwrap_or(home.root());
        let http = reqwest::Client::builder()
            .no_proxy()
            .http1_only()
            .tcp_nodelay(true)
            .redirect(reqwest::redirect::Policy::none())
            .build();
        let http = http.map_err(|error| tracing::warn!("no client for this machine's Calcifer: {error}")).ok()?;
        Some(Arc::new(Self {
            home: home.clone(),
            services: Services::new(home, user_home),
            program: binary::launch()?.path.clone(),
            rental,
            http,
            holder: Mutex::default(),
            starting: tokio::sync::Mutex::new(()),
        }))
    }

    fn lock(&self) -> LockFile {
        LockFile::new(self.home.calcifer_lock())
    }

    /// The directories the lock and the log live in: a fresh pod has neither.
    fn prepared(&self) -> platform::Result<()> {
        self.home.open()?;
        durable::private_dir(&self.home.calcifer())
    }

    fn start(&self) -> platform::Result<Detached> {
        self.services.detach(&ServiceSpec {
            name: "calcifer".into(),
            program: self.program.clone(),
            args: vec!["calcifer".into()],
            log: Some(self.home.calcifer_log()),
        })
    }

    /// A rental's: a thread that keeps one Calcifer running for as long as this service lives.
    pub fn keep(self: &Arc<Self>) -> std::io::Result<()> {
        let me = self.clone();
        std::thread::Builder::new().name("keep-calcifer".into()).spawn(move || me.keeping()).map(drop)
    }

    fn keeping(&self) {
        let mut wait = FIRST;
        loop {
            let began = Instant::now();
            match self.one_life() {
                // Stopped (`POST /v1/down`, a shell's update), or another held the home first.
                Ok(status) if status.success() => {
                    wait = FIRST;
                    continue;
                }
                Ok(status) => tracing::warn!("this machine's Calcifer exited ({status}); it starts again"),
                Err(error) => tracing::warn!("this machine's Calcifer: {error}"),
            }
            if began.elapsed() >= MOST {
                wait = FIRST;
            }
            std::thread::sleep(wait);
            wait = (wait * 2).min(MOST);
        }
    }

    /// Whoever holds the home now (one this service's last run started, or a shell's), until the
    /// kernel says it let go, then one Calcifer from its start to its exit.
    fn one_life(&self) -> platform::Result<ExitStatus> {
        self.prepared()?;
        let lock = self.lock();
        if lock.try_lock(Kind::Shared)?.is_none() {
            drop(lock.lock(Kind::Shared)?);
        }
        self.start()?.wait()
    }

    /// The Calcifer to reach: as last read unless `fresh`, else as its record says now. On a rental
    /// `keep` starts one, so no record means none yet. Elsewhere the first use starts it.
    pub async fn holder(&self, fresh: bool) -> Option<Holder> {
        if !fresh {
            if let Some(holder) = self.holder.lock().unwrap().clone() {
                return Some(holder);
            }
        }
        let found = match self.rental {
            true => self.record(),
            false => self.running().await,
        };
        self.holder.lock().unwrap().clone_from(&found);
        found
    }

    fn record(&self) -> Option<Holder> {
        serde_json::from_slice(&self.lock().holder().ok()??).ok()
    }

    /// The Calcifer holding this home, started if none does. A record names one only while its lock
    /// is held: a killed holder leaves its record behind. Its record or its failed exit ends the
    /// wait, and a client leaving drops it.
    async fn running(&self) -> Option<Holder> {
        let _one = self.starting.lock().await;
        self.prepared().map_err(|error| tracing::warn!("this machine's Calcifer: {error}")).ok()?;
        let mut child = None;
        loop {
            match self.lock().try_lock(Kind::Shared) {
                Ok(None) => {
                    if let Some(holder) = self.record() {
                        if let Some(mut child) = child {
                            // A thread of its own, so it leaves no zombie, and nothing waits for it.
                            let reaped = std::thread::Builder::new()
                                .name("reap-calcifer".into())
                                .spawn(move || drop(Detached::wait(&mut child)));
                            if let Err(error) = reaped {
                                tracing::warn!("reaping this machine's Calcifer: {error}");
                            }
                        }
                        return Some(holder);
                    }
                }
                Ok(Some(free)) => {
                    drop(free);
                    if child.is_none() {
                        child = Some(self.start().map_err(|error| tracing::warn!("starting this machine's Calcifer: {error}")).ok()?);
                    }
                }
                Err(error) => {
                    tracing::warn!("this machine's Calcifer: {error}");
                    return None;
                }
            }
            if let Some(status) = child.as_mut().and_then(|child: &mut Detached| child.try_wait().ok().flatten()) {
                // Success: another start got there first, so its record comes, or it lets go.
                if !status.success() {
                    tracing::warn!("this machine's Calcifer exited ({status}) before it served");
                    return None;
                }
                child = None;
            }
            tokio::time::sleep(SAMPLE).await;
        }
    }
}
