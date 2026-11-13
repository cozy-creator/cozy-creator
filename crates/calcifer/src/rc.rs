//! A dev home's rentals run the open RC (cl-922; `dev = true`, crates/cli/DEV.md): its newest CI builds of the Runtime
//! (with its kernels), TensorFS and the `cozy` a machine runs, which each push to `rc/<x.y.z>` puts on the rolling
//! prerelease `rc-<x.y.z>` of cozy-runtime and cozy-creator (their rc-dev.yaml). A rental this computer ordered moves to
//! them once it is ready, before its first run, and again as the RC moves, only while it has no work. One an update by
//! hand named software for (any version, the RC's builds too) keeps it until `cozy machine update <machine>` names nothing,
//! which in a dev home is the RC.
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

use crate::{floor::FLOOR, machines::Machines};

/// How long a read of the RC's prereleases stands.
const FRESH: Duration = Duration::from_secs(60);
/// How often, while one is up, the rentals are looked over besides when one becomes ready: GitHub says nothing when the RC
/// moves.
const EVERY: Duration = Duration::from_secs(30);
/// How long this computer is at rest, no run going or waiting and none ended, before `local` moves to the RC (xs-195): never during
/// a run, nor between a render's steps.
const AT_REST: Duration = Duration::from_secs(60);

/// The open RC: the minor after this cozy's release (`0.25.0` → `0.26.0`), or a dev version's own (`0.26.0-dev` → `0.26.0`,
/// cl-976); RCs are minor bumps.
pub(crate) fn open() -> String {
    let release = crate::update::this_release();
    let mut parts = release.split(['.', '-']);
    let major = parts.next().unwrap_or("0");
    let minor: u64 = parts.next().and_then(|minor| minor.parse().ok()).unwrap_or(0);
    format!("{major}.{}.0", minor + u64::from(!release.contains('-')))
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

    /// The RC's pure Runtime wheel and its URL: what a lock and a publish's floor check read on any platform (cl-952).
    pub(crate) fn pure_runtime(&self) -> Option<(Build, String)> {
        let build = prerelease::rc_pure(&self.runtime, "cozy_runtime")?;
        let url = format!("{}/{}", prerelease::rc("cozy-runtime", &self.rc), build.file);
        Some((build, url))
    }

    /// Each member of the RC a machine on `platform` running `[runtime, tensorfs, cozy]` doesn't run, with the RC's
    /// version of it. `local`'s cozy is the one installed here, never the RC's build of it (xs-195).
    pub(crate) fn behind(&self, platform: &str, running: [&str; 3], local: bool) -> Vec<(&'static str, String)> {
        ["runtime", "tensorfs", "tensord"]
            .into_iter()
            .zip(running)
            .filter(|(member, _)| !(local && *member == "tensord"))
            .filter_map(|(member, running)| {
                let (build, _) = self.build(member, platform)?;
                (build.version != running).then_some((member, build.version))
            })
            .collect()
    }

    /// The RC's builds for `platform`, in words: `Runtime 0.47.0.dev2260 · TensorFS 1a2b3c4d · cozy 5e6f7a8b`; `local`'s, no cozy.
    fn said(&self, platform: &str, local: bool) -> String {
        [("runtime", "Runtime"), ("tensorfs", "TensorFS"), ("tensord", "cozy")]
            .into_iter()
            .filter(|(member, _)| !(local && *member == "tensord"))
            .filter_map(|(member, label)| Some(format!("{label} {}", prerelease::shown(&self.build(member, platform)?.0.version))))
            .collect::<Vec<_>>()
            .join(" · ")
    }
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
        let (runtime, creator) = (prerelease::rc("cozy-runtime", &rc), prerelease::rc("cozy-creator", &rc));
        let (runtime, creator) = tokio::try_join!(text(&runtime), text(&creator))?;
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

/// Whether an update's words (`runtime 0.45.0 (prerelease), mode dedicated`) read as software named by hand: a component
/// not marked `(RC)` or `(Hub)`, as one with nothing named moves it. Only a store from before holds were kept reads words
/// this way (`hold_of`); an update now says whether it named anything (`mind_hold`).
fn by_hand(target: &str) -> bool {
    target.split(", ").any(|named| software(named) && !named.ends_with("(RC)") && !named.ends_with("(Hub)"))
}

/// Whether an update under `key` is one Calcifer made itself: to the RC, to this cozy's minor, for a package's floor.
fn calcifers(key: &str) -> bool {
    ["rc-", "minor-", FLOOR].iter().any(|prefix| key.starts_with(prefix))
}

/// The hold a completed update (under `key`, named `target`) leaves its machine: its words when it named software by hand, none
/// when it named software otherwise, and nothing to say when it named none. What a store from before holds were kept reads
/// its machines' holds from (`store::held_column`).
pub(crate) fn hold_of(key: &str, target: &str) -> Option<Option<String>> {
    if calcifers(key) || !target.split(", ").any(software) {
        return None;
    }
    Some(by_hand(target).then(|| target.to_owned()))
}

/// Whether an update's named component is software (`runtime 0.45.0`), not a mode or a GPU memory limit.
fn software(named: &str) -> bool {
    ["runtime ", "tensorfs ", "tensord ", "cozy "].iter().any(|member| named.starts_with(member))
}

impl Machines {
    /// The update by hand that holds `name` on its software, in its words: kept in the store across Calcifer's restarts (cl-952),
    /// set by an update that named software by hand and let go by one that named nothing (`cozy machine update <name>`). The
    /// updates Calcifer makes itself (the RC's, this cozy's minor, a package's floor) leave it; a rental placed again under the
    /// same name starts unheld. A held machine moves to neither the RC nor a Runtime a package needs.
    pub(crate) fn held_by_hand(&self, name: &str) -> Option<String> {
        self.store().held(name).ok().flatten()
    }

    /// Keeps or lets go of `name`'s hold for an update under `key` named `target` (`flagless`: nothing named): see `held_by_hand`.
    /// Software named by version holds it whatever that version is: an RC build named by hand (`(RC)`) is a pin, not the RC.
    pub(crate) fn mind_hold(&self, name: &str, key: &str, target: &str, flagless: bool) {
        if calcifers(key) {
            return;
        }
        let words = match (flagless, target.split(", ").any(software)) {
            (true, _) => None,
            (false, true) => Some(target),
            (false, false) => return,
        };
        if let Err(e) = self.store().hold(name, words) {
            warn!(machine = name, "its hold: {e}");
        }
    }

    /// Whether `name` follows the RC: a rental of this dev home that this computer ordered, up; or `local`, where a dev home runs
    /// its first-party packages from a checkout of the RC's source, which the RC's Runtime runs (xs-195).
    fn follows(&self, name: &str) -> Option<crate::store::MachineRow> {
        let row = self.store().machine(name).ok()??;
        let rental = row.kind == MachineKind::Rental && row.state == MachineState::Ready && self.ordered(&row);
        let local = row.kind == MachineKind::Local && crate::checkout::configured();
        (self.dev && (rental || local)).then_some(row)
    }

    /// Moves `name` to the open RC's builds when it follows the RC, is behind it, isn't held by hand and has no work going (nor,
    /// unless `sending`, any waiting for it): the update run to wait for. None when nothing moves, or the RC can't be read
    /// (its runs go on what it runs).
    pub(crate) async fn follow_rc(self: &Arc<Self>, name: &str, sending: bool) -> Option<Run> {
        let row = self.follows(name)?;
        let frame = self.frame(name).filter(|frame| frame.phase == "ready" && frame.update.is_none())?;
        let store = self.store();
        // `local` moves only at rest, never ahead of a run (xs-195).
        let local = row.kind == MachineKind::Local;
        if local && (sending || !store.at_rest(AT_REST.as_millis() as i64).unwrap_or(false)) {
            return None;
        }
        let queued = store.machine_load().ok()?.get(name).map_or(0, |(_, queued)| *queued);
        if store.working(name).ok()? > 0 || store.updating(name).ok()? || (!sending && queued > 0) {
            return None;
        }
        if self.held_by_hand(name).is_some() {
            return None;
        }
        // A send reads what the look-over last read rather than wait on GitHub.
        let sums = match self.rc.last().filter(|_| sending) {
            Some(sums) => sums,
            None => self.rc.now().await.inspect_err(|e| warn!(machine = name, "the open RC's builds: {}", e.message)).ok()?,
        };
        let behind = sums.behind(&frame.platform, [frame.runtime.as_str(), frame.tensorfs.as_str(), frame.version.as_str()], local);
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
                    let rest = if local { format!(", at rest for {} s", AT_REST.as_secs()) } else { String::new() };
                    info!(machine = name, run = run.number, "to the open RC {}{rest}: {}", sums.rc, updating.target);
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

    /// Looks over this dev home's rentals (and `local`, when it follows the RC) whenever one becomes ready, and each [`EVERY`]
    /// while one is: each idle one behind the RC moves. With none up, nothing wakes until one is.
    pub(crate) async fn follow_rcs(self: Arc<Self>) {
        loop {
            let up = match self.store().machines() {
                Ok(rows) => {
                    let local = |row: &&crate::store::MachineRow| {
                        row.kind == MachineKind::Local && crate::checkout::configured() && self.frame(&row.name).is_some_and(|frame| frame.phase == "ready")
                    };
                    let ready: Vec<_> = rows.iter().filter(|row| (row.kind == MachineKind::Rental && row.state == MachineState::Ready) || local(row)).collect();
                    for row in &ready {
                        self.follow_rc(&row.name, false).await;
                    }
                    !ready.is_empty()
                }
                Err(_) => true,
            };
            match up {
                true => tokio::select! {
                    () = tokio::time::sleep(EVERY) => {}
                    () = self.rc_ready.notified() => {}
                },
                false => self.rc_ready.notified().await,
            }
        }
    }

    /// How `name` stands to the RC, for `machine show` and the panel: None outside a dev home or for a machine that doesn't
    /// follow it. Read from what the RC's prereleases last named, never fetched here.
    pub(crate) fn rc_follow(&self, name: &str, frame: Option<&v1::StatusFrame>) -> Option<RcFollow> {
        let local = self.follows(name)?.kind == MachineKind::Local;
        let frame = frame?;
        let follow = |state: &str, detail: String| Some(RcFollow { rc: open(), state: state.into(), detail });
        if let Some(target) = self.held_by_hand(name) {
            return follow("held", format!("an update by hand gave it {target}; `cozy machine update {name}` moves it to the RC again"));
        }
        let Some(sums) = self.rc.last() else {
            return follow("none", "its builds haven't been read yet".into());
        };
        let said = sums.said(&frame.platform, local);
        if said.is_empty() {
            return follow("none", format!("rc-{} holds no build for {}", sums.rc, frame.platform));
        }
        match sums.behind(&frame.platform, [frame.runtime.as_str(), frame.tensorfs.as_str(), frame.version.as_str()], local).is_empty() {
            true => follow("current", said),
            false if local => follow("behind", format!("moves to {said} once this computer has been at rest for {} s", AT_REST.as_secs())),
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
            return built && self.follows(machine).is_some() && self.held_by_hand(machine).is_none();
        }
        self.dev && machine.is_empty() && matches!(on, api::calcifer::On::New(_))
    }

    /// Whether `name` (on `frame`) runs the open RC's Runtime, or would were it not held by hand: a dev build now, or a rental of
    /// this dev home that follows the RC. Such a machine can move to the RC's build for a package that needs it (cl-952).
    pub(crate) fn rc_machine(&self, name: &str, frame: &v1::StatusFrame) -> bool {
        prerelease::dev(&frame.runtime) || self.follows(name).is_some()
    }

    /// The line a run on `name` says when it runs dev builds: `RC: Runtime 0.47.0.dev2260 · cozy 1a2b3c4d`.
    pub(crate) fn rc_line(&self, name: &str) -> Option<String> {
        let frame = self.frame(name)?;
        let built: Vec<String> = [("Runtime", &frame.runtime), ("TensorFS", &frame.tensorfs), ("cozy", &frame.version)]
            .into_iter()
            .filter(|(_, version)| prerelease::dev(version))
            .map(|(label, version)| format!("{label} {}", prerelease::shown(version)))
            .collect();
        (!built.is_empty()).then(|| format!("{name} runs RC builds: {}", built.join(" · ")))
    }
}
