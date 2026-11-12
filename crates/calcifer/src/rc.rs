//! A dev home's rentals run the open RC (cl-922; `dev = true`, crates/cli/DEV.md): its newest CI builds of the Runtime
//! (with its kernels), TensorFS and the `cozy` a machine runs, which each push to `rc/<x.y.z>` puts on the rolling
//! prerelease `rc-<x.y.z>` of cozy-runtime and cozy-creator (their rc-dev.yaml). A rental this computer ordered moves to
//! them once it is ready, before its first run, and again as the RC moves, only while it has no work. One an update by
//! hand gave other software keeps it until `cozy machine update <machine>` names nothing, which in a dev home is the RC.
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use api::{
    calcifer::{DevBuild, MachineKind, MachineState, MachineUpdate, RcFollow, Run, Status},
    error::{Error, Kind},
    ids::Digest,
    machine as v1,
    prerelease::{self, Build},
};
use sha2::{Digest as _, Sha256};
use tracing::{info, warn};

use crate::machines::Machines;

/// How long a read of the RC's prereleases stands.
const FRESH: Duration = Duration::from_secs(60);
/// How often the rentals are looked over, besides when one becomes ready.
const EVERY: Duration = Duration::from_secs(30);

/// The open RC: the minor after this cozy's release (`0.24.0` → `0.25.0`); RCs are minor bumps.
pub(crate) fn open() -> String {
    let mut parts = crate::update::this_release().split('.');
    let major = parts.next().unwrap_or("0");
    let minor: u64 = parts.next().and_then(|minor| minor.parse().ok()).unwrap_or(0);
    format!("{major}.{}.0", minor + 1)
}

/// What the RC's prereleases name, as read.
pub(crate) struct Sums {
    pub rc: String,
    runtime: String,
    creator: String,
}

impl Sums {
    /// The RC's build of `member` (`runtime`, `tensorfs`, `tensord`) for `platform`, and its URL.
    pub(crate) fn build(&self, member: &str, platform: &str) -> Option<(Build, String)> {
        let (sums, repo, distribution) = match member {
            "runtime" => (&self.runtime, "cozy-runtime", "cozy_runtime"),
            "tensorfs" => (&self.creator, "cozy-creator", "tensorfs"),
            "tensord" => (&self.creator, "cozy-creator", "cozy"),
            _ => return None,
        };
        let build = prerelease::rc_build(sums, distribution, platform)?;
        let url = format!("{}/{}", prerelease::rc(repo, &self.rc), build.file);
        Some((build, url))
    }

    /// Each member of the RC a machine on `platform` running `[runtime, tensorfs, cozy]` doesn't run, with the RC's
    /// version of it.
    pub(crate) fn behind(&self, platform: &str, running: [&str; 3]) -> Vec<(&'static str, String)> {
        ["runtime", "tensorfs", "tensord"]
            .into_iter()
            .zip(running)
            .filter_map(|(member, running)| {
                let (build, _) = self.build(member, platform)?;
                (build.version != running).then_some((member, build.version))
            })
            .collect()
    }

    /// The RC's builds for `platform`, in words: `Runtime b976b092 · TensorFS 1a2b3c4d · cozy 5e6f7a8b`.
    fn said(&self, platform: &str) -> String {
        [("runtime", "Runtime"), ("tensorfs", "TensorFS"), ("tensord", "cozy")]
            .into_iter()
            .filter_map(|(member, label)| Some(format!("{label} {}", commit(&self.build(member, platform)?.0.version))))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// A dev build's commit (`b976b092` of `0.45.0+dev.gb976b092`), else the version.
pub(crate) fn commit(version: &str) -> &str {
    version.split_once("+dev.g").map_or(version, |(_, commit)| commit)
}

/// The RC's prereleases, read at most once a [`FRESH`].
#[derive(Default)]
pub(crate) struct Rc {
    read: tokio::sync::Mutex<Option<(Instant, Arc<Sums>)>>,
    last: Mutex<Option<Arc<Sums>>>,
}

impl Rc {
    /// What the RC's prereleases name now.
    pub(crate) async fn now(&self) -> Result<Arc<Sums>, Error> {
        let mut read = self.read.lock().await;
        if let Some((_, sums)) = read.as_ref().filter(|(at, _)| at.elapsed() < FRESH) {
            return Ok(sums.clone());
        }
        let rc = open();
        let (runtime, creator) = tokio::try_join!(text(&prerelease::rc("cozy-runtime", &rc)), text(&prerelease::rc("cozy-creator", &rc)))?;
        let sums = Arc::new(Sums { rc, runtime, creator });
        *read = Some((Instant::now(), sums.clone()));
        *self.last.lock().expect("rc lock") = Some(sums.clone());
        Ok(sums)
    }

    /// What they named when last read, without reading them.
    pub(crate) fn last(&self) -> Option<Arc<Sums>> {
        self.last.lock().expect("rc lock").clone()
    }
}

fn client(timeout: Duration) -> Result<reqwest::Client, Error> {
    let failed = |e: String| Error::new(Kind::Unavailable, "rc_unreadable", e);
    let builder = tensorfs::fetch::http(&[]).map_err(|e| failed(e.to_string()))?;
    builder.timeout(timeout).build().map_err(|e| failed(e.to_string()))
}

/// A prerelease's SHA256SUMS; one with none yet names nothing.
async fn text(base: &str) -> Result<String, Error> {
    let failed = |e: reqwest::Error| Error::new(Kind::Unavailable, "rc_unreadable", format!("{base}/SHA256SUMS: {e}"));
    let answer = client(Duration::from_secs(30))?.get(format!("{base}/SHA256SUMS")).send().await.map_err(failed)?;
    if answer.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(String::new());
    }
    answer.error_for_status().map_err(failed)?.text().await.map_err(failed)
}

/// The RC's `cozy` for a machine, held here to stage with its update: fetched once, held to its sha256.
pub(crate) async fn held_cozy(build: &Build, url: &str) -> Result<DevBuild, Error> {
    let refused = |why: String| Error::new(Kind::Unavailable, "rc_cozy_unfetched", format!("{url}: {why}"));
    let digest: Digest = format!("sha256:{}", build.sha256).parse().map_err(|e: &str| refused(e.into()))?;
    let held = DevBuild { sha256: digest, name: "cozy".into(), version: Some(build.version.clone()) };
    if crate::runs::files::held(&digest).is_some() {
        return Ok(held);
    }
    let answer = client(Duration::from_secs(600))?.get(url).send().await.and_then(reqwest::Response::error_for_status);
    let bytes = answer.map_err(|e| refused(e.to_string()))?.bytes().await.map_err(|e| refused(e.to_string()))?;
    let found = hex::encode(Sha256::digest(&bytes));
    if found != build.sha256 {
        return Err(refused(format!("its sha256 is {found}, not the {} its SHA256SUMS names", build.sha256)));
    }
    tokio::task::spawn_blocking(move || crate::runs::files::hold(&bytes))
        .await
        .map_err(|e| Error::internal("rc_cozy_unheld", e.to_string()))??;
    Ok(held)
}

/// Whether an update's words (`runtime 0.45.0 (prerelease), mode dedicated`) gave its machine software other than the
/// RC's: each component it named that isn't marked `(RC)`. A mode or a GPU memory limit gives none.
fn by_hand(target: &str) -> bool {
    target.split(", ").any(|named| software(named) && !named.ends_with("(RC)"))
}

/// Whether an update's named component is software (`runtime 0.45.0`), not a mode or a GPU memory limit.
fn software(named: &str) -> bool {
    ["runtime ", "tensorfs ", "tensord ", "cozy "].iter().any(|member| named.starts_with(member))
}

impl Machines {
    /// The update by hand that holds `name` (on `frame`) off the RC, in its words: the newest that changed its software,
    /// while it runs what that gave it, unless it was the RC's or this cozy's own (an update to its minor before work). A
    /// rental since placed on the same host under the same name runs its image's again, and isn't held.
    pub(crate) fn held_by_hand(&self, name: &str, frame: &v1::StatusFrame) -> Option<String> {
        let updates = self.store().updates(name, 20).ok()?;
        let (row, key) = updates.into_iter().find(|(row, _)| row.target.split(", ").any(software))?;
        let running = serde_json::json!({ "runtime": frame.runtime, "tensorfs": frame.tensorfs, "version": frame.version, "dev": frame.dev, "mode": frame.mode });
        let runs = crate::update::wanted(&row.spec).iter().all(|(field, value)| running[*field] == *value);
        (!key.starts_with("rc-") && !key.starts_with("minor-") && by_hand(&row.target) && runs).then_some(row.target)
    }

    /// Whether `name` is a rental of this dev home that follows the RC: one this computer ordered, up.
    fn follows(&self, name: &str) -> Option<crate::store::MachineRow> {
        let row = self.store().machine(name).ok()??;
        (self.dev && row.kind == MachineKind::Rental && row.state == MachineState::Ready && self.ordered(&row)).then_some(row)
    }

    /// Moves `name` to the open RC's builds when it follows the RC, is behind it, isn't held by hand and has no work going (nor,
    /// unless `sending`, any waiting for it): the update run to wait for. None when nothing moves, or the RC can't be read
    /// (its runs go on what it runs).
    pub(crate) async fn follow_rc(self: &Arc<Self>, name: &str, sending: bool) -> Option<Run> {
        let row = self.follows(name)?;
        let frame = self.frame(name).filter(|frame| frame.phase == "ready" && frame.update.is_none())?;
        let store = self.store();
        let queued = store.machine_load().ok()?.get(name).map_or(0, |(_, queued)| *queued);
        if store.working(name).ok()? > 0 || store.updating(name).ok()? || (!sending && queued > 0) {
            return None;
        }
        if self.held_by_hand(name, &frame).is_some() {
            return None;
        }
        // A send reads what the look-over last read rather than wait on GitHub.
        let sums = match self.rc.last().filter(|_| sending) {
            Some(sums) => sums,
            None => self.rc.now().await.inspect_err(|e| warn!(machine = name, "the open RC's builds: {}", e.message)).ok()?,
        };
        let behind = sums.behind(&frame.platform, [frame.runtime.as_str(), frame.tensorfs.as_str(), frame.version.as_str()]);
        if behind.is_empty() {
            return None;
        }
        let rental = row.rental_id.as_deref().unwrap_or(name);
        let wanted: Vec<&str> = behind.iter().map(|(_, version)| version.as_str()).collect();
        let key = format!("rc-{rental}-{}-{}", frame.boot_id, wanted.join("-"));
        match self.update(&key, name, MachineUpdate::default()).await {
            Ok((updating, fresh)) => {
                let run = updating.run?;
                if fresh {
                    info!(machine = name, run = run.number, "to the open RC {}: {}", sums.rc, updating.target);
                }
                // One that failed isn't asked again for the same builds: its runs go on what it runs.
                matches!(run.status, Status::Queued | Status::InProgress).then_some(run)
            }
            Err(e) => {
                warn!(machine = name, "to the open RC {}: {}", sums.rc, e.message);
                None
            }
        }
    }

    /// Looks over this dev home's rentals each [`EVERY`] and whenever one becomes ready: each idle one behind the RC moves.
    pub(crate) async fn follow_rcs(self: Arc<Self>) {
        let mut every = tokio::time::interval(EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = every.tick() => {}
                () = self.rc_ready.notified() => {}
            }
            let Ok(rows) = self.store().machines() else { continue };
            for row in rows.iter().filter(|row| row.kind == MachineKind::Rental && row.state == MachineState::Ready) {
                self.follow_rc(&row.name, false).await;
            }
        }
    }

    /// How `name` stands to the RC, for `machine show` and the panel: None outside a dev home or for a machine that doesn't
    /// follow it. Read from what the RC's prereleases last named, never fetched here.
    pub(crate) fn rc_follow(&self, name: &str, frame: Option<&v1::StatusFrame>) -> Option<RcFollow> {
        self.follows(name)?;
        let frame = frame?;
        let follow = |state: &str, detail: String| Some(RcFollow { rc: open(), state: state.into(), detail });
        if let Some(target) = self.held_by_hand(name, frame) {
            return follow("held", format!("an update by hand gave it {target}; `cozy machine update {name}` moves it to the RC again"));
        }
        let Some(sums) = self.rc.last() else {
            return follow("none", "its builds haven't been read yet".into());
        };
        let said = sums.said(&frame.platform);
        if said.is_empty() {
            return follow("none", format!("rc-{} holds no build for {}", sums.rc, frame.platform));
        }
        match sums.behind(&frame.platform, [frame.runtime.as_str(), frame.tensorfs.as_str(), frame.version.as_str()]).is_empty() {
            true => follow("current", said),
            false => follow("behind", format!("moves to {said} once it has no work")),
        }
    }

    /// Whether a run to `machine` (`on`'s, or none yet for a rental to come) runs the open RC's Runtime: it does now, or in a
    /// dev home it is (or will be) a rental that moves to the RC before its run. It takes packages built for the RC.
    pub(crate) fn takes_rc(&self, machine: &str, on: &api::calcifer::On) -> bool {
        if let Some(frame) = self.frame(machine) {
            if prerelease::dev(&frame.runtime) {
                return true;
            }
            let built = self.rc.last().is_some_and(|sums| sums.build("runtime", &frame.platform).is_some());
            return built && self.follows(machine).is_some() && self.held_by_hand(machine, &frame).is_none();
        }
        self.dev && machine.is_empty() && matches!(on, api::calcifer::On::New(_))
    }

    /// The line a run on `name` says when it runs dev builds: `RC: Runtime b976b092 · cozy 1a2b3c4d`.
    pub(crate) fn rc_line(&self, name: &str) -> Option<String> {
        let frame = self.frame(name)?;
        let built: Vec<String> = [("Runtime", &frame.runtime), ("TensorFS", &frame.tensorfs), ("cozy", &frame.version)]
            .into_iter()
            .filter(|(_, version)| prerelease::dev(version))
            .map(|(label, version)| format!("{label} {}", commit(version)))
            .collect();
        (!built.is_empty()).then(|| format!("{name} runs RC builds: {}", built.join(" · ")))
    }
}
