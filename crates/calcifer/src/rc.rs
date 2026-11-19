//! A dev home's rentals run the open RC (cl-922; `dev = true`, crates/cli/DEV.md): its newest CI builds of the Runtime
//! (with its kernels), TensorFS and the `cozy` a machine runs, which each push to `rc/<x.y.z>` puts on the rolling
//! prerelease `rc-<x.y.z>` of cozy-runtime and cozy-creator (their rc-dev.yaml). A rental this computer ordered moves to
//! them at its boot and to newer ones once after each use, only while it is idle: no work queued for it, being sent or going
//! there (cl-1201); never just to stay current while idle, which it ends (cl-1203). The RC's `cozy` it takes is fetched while it boots, so a move goes as it is made; until it is sent, work that comes
//! first goes first and the move waits for idle again. A send or a request never moves a machine: work goes to it as it is
//! (cl-1069), or waits for a move already under way. One an update by hand named software for (any version, the RC's builds too)
//! keeps it until `cozy machine update <machine>` names nothing, which in a dev home is the RC.
use std::{
    sync::{Arc, LazyLock, Mutex},
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
/// What a rental's `cozy` is fetched for before its first frame names its platform: every worker image's.
const RENTAL_PLATFORM: &str = "linux/x86_64";
/// The key every move to the RC is made under: `rc-<rental>-<boot>-<versions>`, `~<n>` after one that yielded (`Machines::move_key`).
pub(crate) const MOVE: &str = "rc-";
/// The reason code of a move to the RC that work sent to its machine ended before it was sent (cl-1201).
pub(crate) const YIELDED: &str = "yielded";

/// Whether a machine's `cozy` (`version`) is a build of the open RC: it moved to the RC's builds before.
fn on_rc(version: &str) -> bool {
    prerelease::dev(version) && crate::update::same_minor(version, &open())
}

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

    /// The RC's builds for `platform`, in words: `Runtime 0.47.0.dev2260 · TensorFS 0.27.0.dev3298 · cozy 5e6f7a8b`; `local`'s, no cozy.
    fn said(&self, platform: &str, local: bool) -> String {
        [("runtime", "Runtime"), ("tensorfs", "TensorFS"), ("tensord", "cozy")]
            .into_iter()
            .filter(|(member, _)| !(local && *member == "tensord"))
            .filter_map(|(member, label)| Some(format!("{label} {}", prerelease::shown(&self.build(member, platform)?.0.version))))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// The RC's prereleases, read at most once a [`FRESH`], and each rental's idle deadline when it last moved to them.
#[derive(Clone, Default)]
pub(crate) struct Rc(Arc<Read>, Arc<Mutex<std::collections::HashMap<String, i64>>>);

#[derive(Default)]
struct Read {
    read: tokio::sync::Mutex<Option<(Instant, Arc<Sums>)>>,
    last: Mutex<Option<Arc<Sums>>>,
}

impl Read {
    async fn now(&self) -> Result<Arc<Sums>, Error> {
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
}

impl Rc {
    /// What the RC's prereleases name now: as last read, read again behind the answer when older than [`FRESH`], one read at a time.
    /// Only a Calcifer that never read them waits for a read. A submit on an RC machine waited 12 s behind `follow_rcs`' read, which
    /// held the lock across GitHub (cl-1010).
    pub(crate) async fn now(&self) -> Result<Arc<Sums>, Error> {
        let Some(last) = self.last() else { return self.0.now().await };
        let stale = match self.0.read.try_lock() {
            Ok(read) => read.as_ref().is_none_or(|(at, _)| at.elapsed() >= FRESH),
            // A read is going: what was read stands meanwhile.
            Err(_) => false,
        };
        if stale {
            let read = self.0.clone();
            tokio::spawn(async move {
                if let Err(e) = read.now().await {
                    warn!("the open RC's builds: {}", e.message);
                }
            });
        }
        Ok(last)
    }

    /// What they named when last read, without reading them.
    pub(crate) fn last(&self) -> Option<Arc<Sums>> {
        self.0.last.lock().expect("rc lock").clone()
    }
}

/// How long a read of the RC's Runtime builds stands for a lock or a floor check: each Runtime merge makes a newer one (the
/// prerelease keeps the newest 20, so a lock on one read before stays good while it is there).
const RUNTIME_FRESH: Duration = Duration::from_secs(20);

/// Builds and their wheels' URLs, newest first.
pub(crate) type Builds = Arc<Vec<(Build, String)>>;

/// The open RC's Runtime builds, newest first: each one's pure wheel and that wheel's URL (cl-952), read at most once a
/// [`RUNTIME_FRESH`]. What a package whose floor only the RC meets locks and is checked against (`floor::rc_runtime`, xs-194).
pub(crate) async fn runtime_builds() -> Result<Builds, Error> {
    static READ: Mutex<Option<(Instant, Builds)>> = Mutex::new(None);
    if let Some((at, read)) = READ.lock().expect("rc lock").clone() {
        if at.elapsed() < RUNTIME_FRESH {
            return Ok(read);
        }
    }
    let base = prerelease::rc("cozy-runtime", &open());
    let sums = text(&base).await?;
    let mut read: Vec<(Build, String)> = prerelease::rc_pures(&sums, "cozy_runtime").map(|build| {
        let url = format!("{base}/{}", build.file);
        (build, url)
    }).collect();
    read.sort_by_cached_key(|(build, _)| std::cmp::Reverse(build.version.parse::<pep440_rs::Version>().ok()));
    let read = Arc::new(read);
    *READ.lock().expect("rc lock") = Some((Instant::now(), read.clone()));
    Ok(read)
}

/// The open RC's TensorFS builds for a rental's platform, newest first, each with its wheel's URL: what a lock on an RC Runtime
/// build takes beside it (`floor::rc_tensorfs_wheel`, xs-216). Read as often as the Runtime's.
pub(crate) async fn tensorfs_builds() -> Result<Builds, Error> {
    static READ: Mutex<Option<(Instant, Builds)>> = Mutex::new(None);
    if let Some((at, read)) = READ.lock().expect("rc lock").clone() {
        if at.elapsed() < RUNTIME_FRESH {
            return Ok(read);
        }
    }
    let base = prerelease::rc("cozy-creator", &open());
    let sums = text(&base).await?;
    let mut read: Vec<(Build, String)> = prerelease::rc_natives(&sums, "tensorfs", RENTAL_PLATFORM).map(|build| {
        let url = format!("{base}/{}", build.file);
        (build, url)
    }).collect();
    // By dev number; builds of one version (`0.26.0+dev.g…`) stay as listed, newest first.
    read.sort_by_cached_key(|(build, _)| std::cmp::Reverse(build.version.parse::<pep440_rs::Version>().ok().map(|version| version.without_local())));
    let read = Arc::new(read);
    *READ.lock().expect("rc lock") = Some((Instant::now(), read.clone()));
    Ok(read)
}

fn client(timeout: Duration) -> Result<reqwest::Client, Error> {
    let failed = |e: String| Error::new(Kind::Unavailable, "rc_unreadable", e);
    let builder = tensorfs::fetch::http(&[]).map_err(|e| failed(e.to_string()))?;
    builder.timeout(timeout).build().map_err(|e| failed(e.to_string()))
}

/// A prerelease's SHA256SUMS; while rc-dev swaps it, by rename, its last one, SHA256SUMS.old (cl-1128: a lock that found none for
/// those seconds took no RC build); one with neither yet names nothing.
async fn text(base: &str) -> Result<String, Error> {
    for name in ["SHA256SUMS", "SHA256SUMS.old"] {
        let failed = |e: reqwest::Error| Error::new(Kind::Unavailable, "rc_unreadable", format!("{base}/{name}: {e}"));
        let answer = client(Duration::from_secs(30))?.get(format!("{base}/{name}")).send().await.map_err(failed)?;
        if answer.status() == reqwest::StatusCode::NOT_FOUND {
            continue;
        }
        return answer.error_for_status().map_err(failed)?.text().await.map_err(failed);
    }
    Ok(String::new())
}

/// The RC's `cozy` for a machine, held here to stage with its update: fetched once, one fetch at a time (the one ahead of a boot and
/// the move's own are the same file), held to its sha256.
pub(crate) async fn held_cozy(build: &Build, url: &str) -> Result<DevBuild, Error> {
    static FETCHING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(tokio::sync::Mutex::default);
    let refused = |why: String| Error::new(Kind::Unavailable, "rc_cozy_unfetched", format!("{url}: {why}"));
    let digest: Digest = format!("sha256:{}", build.sha256).parse().map_err(|e: &str| refused(e.into()))?;
    let held = DevBuild { sha256: digest, name: "cozy".into(), version: Some(build.version.clone()) };
    if crate::runs::files::held(&digest).is_some() {
        return Ok(held);
    }
    let _one = FETCHING.lock().await;
    if crate::runs::files::held(&digest).is_some() {
        return Ok(held);
    }
    let began = Instant::now();
    let answer = client(Duration::from_secs(600))?.get(url).send().await.and_then(reqwest::Response::error_for_status);
    let bytes = answer.map_err(|e| refused(e.to_string()))?.bytes().await.map_err(|e| refused(e.to_string()))?;
    let found = hex::encode(Sha256::digest(&bytes));
    if found != build.sha256 {
        return Err(refused(format!("its sha256 is {found}, not the {} its SHA256SUMS names", build.sha256)));
    }
    let length = bytes.len();
    tokio::task::spawn_blocking(move || crate::runs::files::hold(&bytes))
        .await
        .map_err(|e| Error::internal("rc_cozy_unheld", e.to_string()))??;
    info!(version = build.version, ms = began.elapsed().as_millis() as u64, "fetched the open RC's cozy ({})", crate::bytes(length as f64));
    Ok(held)
}

/// The RC's `cozy` for a machine on `platform` held here (`held_cozy`); none to hold when the RC builds none for it.
async fn rc_cozy(sums: &Sums, platform: &str) -> Result<(), Error> {
    match sums.build("tensord", platform) {
        Some((build, url)) => held_cozy(&build, &url).await.map(drop),
        None => Ok(()),
    }
}

/// Whether an update under `key` is one Calcifer made itself: to the RC, to this cozy's minor, for a package's floor.
pub(crate) fn calcifers(key: &str) -> bool {
    [MOVE, "minor-", FLOOR].iter().any(|prefix| key.starts_with(prefix))
}

/// Whether an update's named component is software (`runtime 0.45.0`), not a mode or a GPU memory limit. The cozy installed on this
/// computer (`cozy 0.27.0 (installed)`), which `local` runs anyway and `cozy update` moves it to, is none: it pins nothing.
fn software(named: &str) -> bool {
    let installed = named.starts_with("cozy ") && named.ends_with(" (installed)");
    !installed && ["runtime ", "tensorfs ", "tensord ", "cozy "].iter().any(|member| named.starts_with(member))
}

/// The hold an update under `key` named `target` gives its machine (`flagless`: nothing named): `Some(None)` lets it go, `Some(words)`
/// holds it on them. None leaves it as it is: Calcifer's own updates, and one that names no software, such as `cozy update`'s move of
/// `local` to the cozy it installed (cl-1254). Software named by version holds it whatever that version is: an RC build named by hand
/// (`(RC)`) is a pin, not the RC.
pub(crate) fn hold_of<'a>(key: &str, target: &'a str, flagless: bool) -> Option<Option<&'a str>> {
    if calcifers(key) {
        return None;
    }
    match (flagless, target.split(", ").any(software)) {
        (true, _) => Some(None),
        (false, true) => Some(Some(target)),
        (false, false) => None,
    }
}

/// The software a hold's words name, by member and version: `runtime 0.47.0`, `tensorfs 0.26.0 (prerelease)`, `tensord 0.27.0-dev+g… (RC)`,
/// a wheel file by its version, `cozy 0.27.0-dev+g… (installed)` as `tensord`. A `cozy` named by its file alone names none.
fn held_versions(words: &str) -> Vec<(&'static str, &str)> {
    fn named(item: &str) -> Option<(&'static str, &str)> {
        let mut parts = item.split(' ');
        let member = match parts.next()? {
            "runtime" => "runtime",
            "tensorfs" => "tensorfs",
            "tensord" | "cozy" => "tensord",
            _ => return None,
        };
        let named = parts.next()?;
        let version = match named.strip_suffix(".whl") {
            Some(wheel) => wheel.split('-').nth(1)?,
            None => named,
        };
        version.starts_with(|c: char| c.is_ascii_digit()).then_some((member, version))
    }
    words.split(", ").filter_map(named).collect()
}

/// Whether a machine reporting `running` runs `named`: the same, or its public version when `named` has no local part (`0.48.0.dev2319`
/// runs as `0.48.0.dev2319+g1a2b3c4d`).
fn runs_named(running: &str, named: &str) -> bool {
    running == named || (!named.contains('+') && running.split('+').next() == Some(named))
}

impl Machines {
    /// The update by hand that holds `name` on its software, in its words: kept in the store across Calcifer's restarts (cl-952),
    /// set by an update that named software by hand and let go by one that named nothing (`cozy machine update <name>`). The
    /// updates Calcifer makes itself (the RC's, this cozy's minor, a package's floor) leave it; a rental placed again under the
    /// same name starts unheld. A held machine moves to neither the RC nor a Runtime a package needs.
    pub(crate) fn held_by_hand(&self, name: &str) -> Option<String> {
        self.store().held(name).ok().flatten()
    }

    /// Holds `name` on `words`, or lets it go (none).
    pub(crate) fn set_hold(&self, name: &str, words: Option<&str>) {
        if let Err(e) = self.store().hold(name, words) {
            warn!(machine = name, "its hold: {e}");
        }
    }

    /// Gives `name` the hold its update under `key` named `target` asks (`hold_of`) before that update is sent, then cancels each update
    /// Calcifer asked of it itself that is yet to end (cl-1196): the RC's, this cozy's minor's, a package floor's. One unsent ends here;
    /// one its machine holds is canceled there before it activates, and one activating finishes before the hand's, which its machine
    /// queues behind it. With `superseded` at each send, none goes after the hand's: the hold is set before the cancel looks. The hold
    /// it had, to put back if the update isn't made; None when the update leaves the hold alone.
    pub(crate) fn hold_for(&self, name: &str, key: &str, target: &str, flagless: bool) -> Option<Option<String>> {
        let words = hold_of(key, target, flagless)?;
        let was = self.held_by_hand(name);
        self.set_hold(name, words);
        let why = match words {
            Some(words) => format!("superseded by an update by hand to {words}"),
            None => "superseded by an update by hand".into(),
        };
        match self.calcifer.supersede(name, calcifers, &why) {
            Ok(ended) if !ended.is_empty() => info!(machine = name, "{why}: {} of Cozy's own updates canceled", ended.len()),
            Ok(_) => {}
            Err(e) => warn!(machine = name, "canceling Cozy's own updates: {e}"),
        }
        Some(was)
    }

    /// Whether update `id` of `name`, about to be sent, is one Calcifer asked itself while `name` is held by hand: then it is canceled
    /// here, never sent (cl-1196).
    pub(crate) fn superseded(&self, id: &api::ids::RunId, name: &str) -> bool {
        let Some(words) = self.held_by_hand(name) else { return false };
        if !self.store().key(id.as_str()).ok().flatten().is_some_and(|key| calcifers(&key)) {
            return false;
        }
        match self.calcifer.supersede(name, calcifers, &format!("superseded by an update by hand to {words}")) {
            Ok(ended) => ended.contains(id),
            Err(e) => {
                warn!(machine = name, "canceling Cozy's own update: {e}");
                false
            }
        }
    }

    /// What `name` runs (on `frame`) that its hold by hand names otherwise, in words (`Runtime 0.48.0.dev2319 · cozy 0.27.0-dev`),
    /// while no update of it is going (cl-1196): the hold says what was asked, the machine what it runs, and the machine's word stands.
    /// None: not held, running what the hold names, its Status unread, or an update under way.
    pub(crate) fn unheld(&self, name: &str, frame: Option<&v1::StatusFrame>) -> Option<String> {
        let held = self.held_by_hand(name)?;
        let frame = frame?;
        if self.store().updating(name).unwrap_or(true) {
            return None;
        }
        let differs: Vec<String> = held_versions(&held)
            .into_iter()
            .filter_map(|(member, named)| {
                let (label, running) = match member {
                    "runtime" => ("Runtime", &frame.runtime),
                    "tensorfs" => ("TensorFS", &frame.tensorfs),
                    _ => ("cozy", &frame.version),
                };
                match running.as_str() {
                    running if runs_named(running, named) => None,
                    "" => Some(format!("no {label}")),
                    running => Some(format!("{label} {}", prerelease::shown(running))),
                }
            })
            .collect();
        (!differs.is_empty()).then(|| differs.join(" · "))
    }

    /// Whether `name` follows the RC: a rental of this dev home that this computer ordered, up; or `local`, where a dev home runs
    /// its first-party packages from a checkout of the RC's source, which the RC's Runtime runs (xs-195).
    pub(crate) fn follows(&self, name: &str) -> Option<crate::store::MachineRow> {
        let row = self.store().machine(name).ok()??;
        let rental = row.kind == MachineKind::Rental && row.state == MachineState::Ready && self.ordered_here(&row);
        let local = row.kind == MachineKind::Local && crate::checkout::configured();
        (self.dev && (rental || local)).then_some(row)
    }

    /// Whether `name` is a rental that follows the RC and hasn't said it's ready (cl-1069): until it is, it can't be told behind or
    /// not, and work for it waits for it to start. Ready, work goes to it as it is, and it moves to the RC once idle (cl-1201); a run
    /// whose package needs the RC's Runtime moves it first (`floor`; LAB2-47826, refused on the 0.25.0 image's 0.46.0).
    pub(crate) fn booting_for_rc(&self, name: &str) -> bool {
        self.follows(name).is_some_and(|row| row.kind == MachineKind::Rental)
            && self.held_by_hand(name).is_none()
            && self.frame(name).is_none_or(|frame| frame.phase == "booting")
    }

    /// Moves `name` to the open RC's builds when it follows the RC, is behind it, isn't held by hand and is idle (cl-1201): no work
    /// queued for it, being sent or going there, and no update. At its boot or as the RC moves (on its builds, once after each use,
    /// cl-1203), the same: a move never goes ahead of work. The RC's `cozy` is held here before the move is made (fetched while it booted, `rc_ahead`), so it goes at once; until
    /// it is sent, work that comes for the machine goes first and the move yields (`Calcifer::yield_moves`), to be made again once it
    /// is idle. A send or a request never moves a machine (cl-1123: PRO6-33 waited 92 s for hougetsu's): only this follower does. The
    /// update run, or None when nothing moves or the RC can't be read (its runs go on what it runs).
    pub(crate) async fn follow_rc(self: &Arc<Self>, name: &str) -> Option<Run> {
        let row = self.follows(name)?;
        let frame = self.frame(name).filter(|frame| frame.phase == "ready" && frame.update.is_none())?;
        // `local` moves only at rest, never between a render's steps (xs-195).
        let local = row.kind == MachineKind::Local;
        if local && !self.calcifer.at_rest(AT_REST.as_millis() as i64).unwrap_or(false) {
            return None;
        }
        // Every client's work counts (cl-1224): another computer of the account may be running on it.
        let idle = || {
            self.calcifer.busy(name).is_ok_and(|busy| busy == 0)
                && self.frame(name).is_none_or(|frame| self.others_on(&frame) == 0)
                && self.store().updating(name).is_ok_and(|updating| !updating)
                && self.held_by_hand(name).is_none()
        };
        if !idle() {
            return None;
        }
        // On the RC's builds, a rental moves to newer ones once after each use (a job's end moves its idle deadline), never again
        // while it stays idle: an update is no use, and its idle deadline ends it (cl-1203).
        let deadline = frame.idle_deadline_unix_ms;
        if !local && on_rc(&frame.version) && self.rc.1.lock().expect("rc lock").get(name) == Some(&deadline) {
            return None;
        }
        let sums = self.rc.now().await.inspect_err(|e| warn!(machine = name, "the open RC's builds: {}", e.message)).ok()?;
        let behind = sums.behind(&frame.platform, [frame.runtime.as_str(), frame.tensorfs.as_str(), frame.version.as_str()], local);
        // `local` runs the cozy installed here (xs-195, cl-1029): once an install replaced it, its TensorD moves to it at rest, whether or not
        // the RC's Runtime and TensorFS moved too (the update carries it: `cozy X (installed)`).
        let installed = match local {
            true => crate::update::installed(self).await.filter(|installed| *installed != frame.version),
            false => None,
        };
        if behind.is_empty() && installed.is_none() {
            return None;
        }
        // Held before the move is made, never fetched between this look and its send.
        if behind.iter().any(|(member, _)| *member == "tensord")
            && let Err(e) = rc_cozy(&sums, &frame.platform).await
        {
            warn!(machine = name, "to the open RC {}: {}", sums.rc, e.message);
            return None;
        }
        // Work that came meanwhile goes first.
        if !idle() {
            return None;
        }
        let rental = row.rental_id.as_deref().unwrap_or(name);
        let wanted: Vec<&str> = behind.iter().map(|(_, version)| version.as_str()).chain(installed.as_deref()).collect();
        let key = self.move_key(&format!("{rental}-{}-{}", frame.boot_id, wanted.join("-")))?;
        match self.update(&key, name, MachineUpdate::default()).await {
            Ok((updating, fresh)) => {
                let run = updating.run?;
                if !local {
                    self.rc.1.lock().expect("rc lock").insert(name.to_owned(), deadline);
                }
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

    /// The key of the next move to the builds `base` names (`<rental>-<boot>-<versions>`): `rc-<base>`, then `rc-<base>~2`, … after each
    /// that yielded to work (cl-1201); one under way answers its own. None: one of them failed, or was canceled otherwise (an update by
    /// hand superseded it), on this boot, and these builds aren't asked again.
    fn move_key(&self, base: &str) -> Option<String> {
        let store = self.store();
        for n in 1u32.. {
            let key = match n {
                1 => format!("{MOVE}{base}"),
                n => format!("{MOVE}{base}~{n}"),
            };
            match store.keyed(&key).ok()? {
                None => return Some(key),
                Some(row) if row.state != crate::store::State::Settled => return Some(key),
                Some(row) if row.reason.as_ref().is_some_and(|reason| reason.code == YIELDED) => continue,
                Some(_) => return None,
            }
        }
        None
    }

    /// The move to the open RC under way on `name`, for work sent there to wait for (cl-1201). One not sent yet yields to that work
    /// first: it goes as `name` is, and the follower moves it once it's idle. None: no move under way.
    pub(crate) fn rc_moving(&self, name: &str) -> Option<Run> {
        let row = self.store().moving(name).ok()??;
        if row.state == crate::store::State::Queued && self.calcifer.yield_moves(name) {
            return None;
        }
        self.calcifer.record(&row).ok()
    }

    /// The RC's `cozy` for a rental of this dev home on its way, fetched now in the background (cl-1201): at its boot it is held here,
    /// so its move goes as it is made and no request waits for GitHub. Its first frame names its platform; before it, every rental's.
    fn rc_ahead(&self, frame: Option<&v1::StatusFrame>) {
        let platform = frame.map(|frame| frame.platform.as_str()).filter(|platform| !platform.is_empty()).unwrap_or(RENTAL_PLATFORM).to_owned();
        let rc = self.rc.clone();
        self.spawn(async move {
            let ahead = async { rc_cozy(&*rc.now().await?, &platform).await };
            if let Err(e) = ahead.await {
                warn!(platform, "the open RC's cozy, ahead of a rental's boot: {}", e.message);
            }
        });
    }

    /// Looks over this dev home's rentals (and `local`, when it follows the RC) whenever one is ordered or becomes ready, and each
    /// [`EVERY`] while one is up or on its way: each idle one behind the RC moves, and the RC's `cozy` is fetched for each on its way.
    /// With none, nothing wakes until one is.
    pub(crate) async fn follow_rcs(self: Arc<Self>) {
        loop {
            let took = crate::took::Took::new("follow_rcs");
            let up = match self.store().live_machines() {
                Ok(rows) => {
                    let mut up = false;
                    for row in &rows {
                        let frame = self.frame(&row.name);
                        let ready = frame.as_ref().is_some_and(|frame| frame.phase == "ready");
                        match row.kind {
                            MachineKind::Rental if !self.ordered_here(row) => continue,
                            MachineKind::Rental if row.state == MachineState::Ready && ready => {
                                self.follow_rc(&row.name).await;
                            }
                            MachineKind::Rental => self.rc_ahead(frame.as_ref()),
                            MachineKind::Local if crate::checkout::configured() && ready => {
                                self.follow_rc(&row.name).await;
                            }
                            _ => continue,
                        }
                        up = true;
                    }
                    up
                }
                Err(_) => true,
            };
            drop(took);
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
            let runs = self.unheld(name, Some(frame)).map_or_else(String::new, |runs| format!(", but it runs {runs}"));
            return follow("held", format!("held by hand on {target}{runs}; `cozy machine update {name}` moves it to the RC again"));
        }
        let Some(sums) = self.rc.last() else {
            return follow("none", "its builds haven't been read yet".into());
        };
        let said = sums.said(&frame.platform, local);
        if said.is_empty() {
            return follow("none", format!("rc-{} holds no build for {}", sums.rc, frame.platform));
        }
        // `local`'s cozy is the one installed here, as the follower last read it.
        let installed = local.then(|| crate::update::installed_known(self)).flatten().filter(|installed| *installed != frame.version);
        let behind = sums.behind(&frame.platform, [frame.runtime.as_str(), frame.tensorfs.as_str(), frame.version.as_str()], local);
        match (behind.is_empty(), installed) {
            (true, None) => follow("current", said),
            (current, Some(installed)) => {
                let cozy = format!("cozy {} (installed)", prerelease::shown(&installed));
                let to = if current { cozy } else { format!("{said} · {cozy}") };
                follow("behind", format!("moves to {to} once this computer has been at rest for {} s", AT_REST.as_secs()))
            }
            (false, None) if local => follow("behind", format!("moves to {said} once this computer has been at rest for {} s", AT_REST.as_secs())),
            (false, None) => follow("behind", format!("moves to {said} once it has no work")),
        }
    }

    /// Whether a run to `machine` (`on`'s, or none yet for a rental to come) runs the open RC's Runtime: it does now, or in a
    /// dev home it is (or will be) a rental that moves to the RC before its run. Its package releases resolve as any machine's
    /// (cl-1109): being on the RC takes no prerelease.
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

}
