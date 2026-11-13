//! A package release runs only on a Runtime its `cozy-runtime` requirement admits (cl-952, `api::release::choose`). A run whose
//! release needs a newer Runtime than its machine's moves the machine to the newest that runs it, a release (PyPI's) or on the
//! RC the RC's build, before it runs: an update under a `floor-` key, which holds the machine on nothing. A machine held by hand
//! refuses instead. Only a Runtime this cozy's line runs (its TensorFS's protocol or an older one's) is moved to; one only a newer
//! cozy's line runs is reached by cozy updating itself (`crate::cozy`, cl-978).
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use api::{
    calcifer::{Component, MachineKind, MachineUpdate, Note, NoteLevel, Run, Status},
    error::{Error, Kind},
    prerelease,
};
use api::hub::types::Software;
use pep440_rs::{Version, VersionSpecifiers};
use futures_util::StreamExt as _;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use crate::{
    api::{Api, Refusal},
    machines::Machines,
};

/// The key prefix of an update a package's floor asked for.
pub(crate) const FLOOR: &str = "floor-";

/// How long a read of PyPI's Runtime releases stands.
const FRESH: Duration = Duration::from_secs(600);

/// A Runtime release on PyPI, and its pure wheel (URL, sha256).
#[derive(Clone, Debug)]
pub(crate) struct Released {
    pub version: String,
    pub pure: Option<(String, String)>,
}

static RELEASED: Mutex<Option<(Instant, Arc<Vec<Released>>)>> = Mutex::new(None);

/// What each Runtime release requires of TensorFS, as PyPI said it once: a release never changes.
static TENSORFS: Mutex<BTreeMap<String, Option<VersionSpecifiers>>> = Mutex::new(BTreeMap::new());

/// The Hub's software (`/v1/software`), as last read, by Hub.
static SOFTWARE: Mutex<BTreeMap<String, (Instant, Arc<Software>)>> = Mutex::new(BTreeMap::new());

#[derive(Deserialize)]
struct Project {
    releases: std::collections::BTreeMap<String, Vec<File>>,
}

#[derive(Deserialize)]
struct File {
    filename: String,
    url: String,
    digests: Digests,
    #[serde(default)]
    yanked: bool,
}

#[derive(Deserialize)]
struct Digests {
    sha256: String,
}

fn http() -> Result<reqwest::Client, Error> {
    let failed = |e: String| Error::new(Kind::Unavailable, "release_unreachable", e);
    let builder = tensorfs::fetch::http(&[]).map_err(|e| failed(e.to_string()))?;
    builder.read_timeout(tensorfs::fetch::STALL).build().map_err(|e| failed(e.to_string()))
}

/// The Runtime's releases on PyPI (finals, none yanked), read at most once a [`FRESH`]; as last read when PyPI doesn't answer.
pub(crate) async fn released() -> Result<Arc<Vec<Released>>, Error> {
    let kept = RELEASED.lock().expect("released lock").clone();
    if let Some((at, read)) = &kept {
        if at.elapsed() < FRESH {
            return Ok(read.clone());
        }
    }
    let read = async {
        let failed = |e: reqwest::Error| Error::new(Kind::Unavailable, "release_unreachable", format!("PyPI's cozy-runtime releases: {e}"));
        let answer = http()?.get("https://pypi.org/pypi/cozy-runtime/json").send().await.and_then(reqwest::Response::error_for_status).map_err(failed)?;
        let project: Project = answer.json().await.map_err(failed)?;
        let released: Vec<Released> = (project.releases.into_iter())
            .filter(|(version, files)| !files.is_empty() && files.iter().all(|file| !file.yanked) && !api::release::prerelease(version))
            .map(|(version, files)| {
                let pure = files.iter().find(|file| file.filename.ends_with("-py3-none-any.whl")).map(|file| (file.url.clone(), file.digests.sha256.clone()));
                Released { version, pure }
            })
            .collect();
        Ok::<_, Error>(Arc::new(released))
    };
    match (read.await, kept) {
        (Ok(read), _) => {
            *RELEASED.lock().expect("released lock") = Some((Instant::now(), read.clone()));
            Ok(read)
        }
        (Err(_), Some((_, read))) => Ok(read),
        (Err(e), None) => Err(e),
    }
}

/// What Runtime release `version` requires of TensorFS (its `Requires-Dist`), from PyPI, read once: None when it names none, or
/// PyPI didn't answer (asked again next time), which holds nothing back.
fn tensorfs_of(version: String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<VersionSpecifiers>> + Send>> {
    Box::pin(async move { tensorfs_read(&version).await })
}

async fn tensorfs_read(version: &str) -> Option<VersionSpecifiers> {
    if let Some(needs) = TENSORFS.lock().expect("tensorfs lock").get(version) {
        return needs.clone();
    }
    #[derive(Deserialize)]
    struct Read {
        info: Info,
    }
    #[derive(Deserialize)]
    struct Info {
        #[serde(default)]
        requires_dist: Option<Vec<String>>,
    }
    let url = format!("https://pypi.org/pypi/cozy-runtime/{version}/json");
    let read = async { http().ok()?.get(&url).send().await.ok()?.error_for_status().ok()?.json::<Read>().await.ok() };
    let requires = read.await?.info.requires_dist.unwrap_or_default();
    let needs = api::release::required(requires.iter().map(String::as_str), "tensorfs");
    TENSORFS.lock().expect("tensorfs lock").insert(version.to_owned(), needs.clone());
    needs
}

/// What a run's machine does for the package release it takes: it moves to `to` first.
#[derive(Clone, Debug)]
pub(crate) struct Upgrade {
    pub machine: String,
    pub from: String,
    pub to: String,
    /// `minimax-h3 0.12.0rc3`.
    pub release: String,
}

impl Upgrade {
    /// The line the run says: `upgrading vanitas's Runtime 0.45.1 → 0.47.0.dev2260 for minimax-h3 0.12.0rc3`.
    pub(crate) fn note(&self) -> Note {
        let (from, to) = (prerelease::shown(&self.from), prerelease::shown(&self.to));
        Note { level: NoteLevel::Info, message: format!("upgrading {}'s Runtime {from} → {to} for {}", self.machine, self.release) }
    }
}

impl Machines {
    /// The Runtimes `name` (on `frame`) can move to for a package release on this cozy's line: PyPI's that its TensorFS's protocol
    /// speaks, and on the RC (`rc`, or held off it) the RC's build for its platform; whether it moves (`api::release::Moves`): not
    /// when held by hand, nor when another computer's (added here, or a rental another client ordered); and the newer cozy, the
    /// Hub's, whose line runs Runtimes this one's doesn't (cl-978).
    pub(crate) async fn runtimes(&self, name: &str, frame: &api::machine::StatusFrame, rc: bool) -> (Vec<String>, api::release::Moves, Option<api::release::Newer>) {
        let moves = match self.named(name).ok().flatten() {
            Some(row) if row.kind == MachineKind::Added || (row.kind == MachineKind::Rental && !self.ordered(&row)) => api::release::Moves::No,
            _ if self.held_by_hand(name).is_some() => api::release::Moves::Held,
            _ => api::release::Moves::Yes,
        };
        let released: Vec<String> = match released().await {
            Ok(released) => released.iter().map(|release| release.version.clone()).collect(),
            Err(e) => {
                tracing::warn!("the Runtime's releases: {}", e.message);
                vec![]
            }
        };
        // Only those newer than its own are asked what TensorFS they need: a machine moves only up.
        let running = Version::from_str(&frame.runtime).ok();
        let above: Vec<String> = released.into_iter().filter(|release| match (&running, Version::from_str(release)) {
            (Some(running), Ok(release)) => release > *running,
            _ => true,
        }).collect();
        let asked: Vec<_> = above.iter().cloned().map(tensorfs_of).collect();
        let needs: Vec<Option<VersionSpecifiers>> = futures_util::stream::iter(asked).buffered(8).collect().await;
        let ours = tensorfs::VERSION;
        let (mut runtimes, mut beyond) = (vec![], vec![]);
        for (release, needs) in above.into_iter().zip(needs) {
            match api::release::admits_series(needs.as_ref(), ours) {
                true => runtimes.push(release),
                false => beyond.push((release, needs)),
            }
        }
        // A machine held off the RC by hand counts the RC's build too: refused, it names the update that moves it there.
        if rc || self.rc_machine(name, frame) {
            if let Ok(sums) = self.rc.now().await {
                runtimes.extend(sums.build("runtime", &frame.platform).map(|(build, _)| build.version));
            }
        }
        let newer = match beyond.is_empty() {
            true => None,
            false => self.newer(beyond).await,
        };
        (runtimes, moves, newer)
    }

    /// The Hub's cozy, when it is a newer release than this one, and which of `beyond` (Runtimes this cozy's line doesn't run, with
    /// what each requires of TensorFS) its line runs. A dev build never updates itself: its binary is someone's build.
    async fn newer(&self, beyond: Vec<(String, Option<VersionSpecifiers>)>) -> Option<api::release::Newer> {
        if crate::update::dev_build() {
            return None;
        }
        let software = self.software().await?;
        let cozy = software.cozy.clone().filter(|cozy| crate::cozy::newer(cozy, crate::update::this_release()))?;
        let runtimes: Vec<String> = (beyond.into_iter())
            .filter(|(_, needs)| api::release::admits_series(needs.as_ref(), &software.tensorfs))
            .map(|(release, _)| release)
            .collect();
        let major = crate::cozy::major(&cozy) != crate::cozy::major(crate::update::this_release());
        (!runtimes.is_empty()).then_some(api::release::Newer { cozy, runtimes, major })
    }

    /// This home's Hub's software (`/v1/software`), read at most once a [`FRESH`]; as last read when it doesn't answer.
    pub(crate) async fn software(&self) -> Option<Arc<Software>> {
        let hub = self.hubs.default_origin();
        let kept = SOFTWARE.lock().expect("software lock").get(&hub).cloned();
        if let Some((at, read)) = &kept
            && at.elapsed() < FRESH
        {
            return Some(read.clone());
        }
        let read = match self.hubs.anonymous(&hub) {
            Ok(client) => client.get_software().await.map_err(|e| e.to_string()),
            Err(e) => Err(e.message),
        };
        match (read, kept) {
            (Ok(read), _) => {
                let read = Arc::new(read);
                SOFTWARE.lock().expect("software lock").insert(hub, (Instant::now(), read.clone()));
                Some(read)
            }
            (Err(_), Some((_, read))) => Some(read),
            (Err(e), None) => {
                tracing::warn!("{hub}'s software: {e}");
                None
            }
        }
    }

    /// Moves `upgrade.machine` to Runtime `upgrade.to` before the run that needs it, which waits behind it: a release alone
    /// (TensorD keeps or pairs its TensorFS), the RC's build with the RC's other builds it's behind. One update per boot and
    /// Runtime: its run, none once it finished; one that failed is refused with what happened.
    pub(crate) async fn upgrade(self: &Arc<Self>, upgrade: &Upgrade) -> Result<Option<Run>, Error> {
        let name = upgrade.machine.as_str();
        let frame = self.frame(name).ok_or_else(|| Error::new(Kind::Unavailable, "machine_unanswered", format!("{name} hasn't said what it runs")))?;
        let id = self.named(name).ok().flatten().and_then(|row| row.rental_id).unwrap_or_else(|| name.to_owned());
        let key = format!("{FLOOR}{id}-{}-{}", frame.boot_id, upgrade.to);
        let refused = |why: String| {
            Error::new(Kind::Conflict, "runtime_upgrade_failed", format!("{name}'s Runtime {} → {} for {}: {why}", upgrade.from, upgrade.to, upgrade.release))
                .next(format!("cozy machine update {name} --runtime {}", upgrade.to))
        };
        if let Some(row) = self.store().keyed(&key).map_err(crate::machines::internal)? {
            let run = self.calcifer.record(&row).map_err(|e| Refusal::from(e).0)?;
            return match run.status {
                Status::Failed | Status::Canceled => Err(refused(run.reason.map_or_else(|| "its update didn't finish".to_owned(), |reason| reason.message))),
                // What it says it runs may be from before its restart: TensorD holds the release to its floor as ever.
                Status::Completed => Ok(None),
                _ => Ok(Some(run)),
            };
        }
        let mut ask = MachineUpdate { runtime: Some(Component::Version(upgrade.to.clone())), ..Default::default() };
        if prerelease::dev(&upgrade.to) {
            let sums = self.rc.now().await?;
            let local = self.named(name).ok().flatten().is_some_and(|row| row.kind == MachineKind::Local);
            for (member, version) in sums.behind(&frame.platform, [frame.runtime.as_str(), frame.tensorfs.as_str(), frame.version.as_str()], local) {
                match member {
                    "tensorfs" => ask.tensorfs = Some(Component::Version(version)),
                    "tensord" => ask.tensord = Some(Component::Version(version)),
                    _ => {}
                }
            }
        }
        let (updating, _) = self.update(&key, name, ask).await.map_err(|e| refused(e.message))?;
        Ok(updating.run)
    }
}

/// Reads which `cozy_runtime` names a package wheel imports that a Runtime wheel lacks.
const READER: &str = include_str!("floor_reader.py");

/// A Runtime wheel the floor check reads: its version, and its pure wheel (URL, sha256).
pub(crate) struct Wheel {
    pub version: String,
    pub url: String,
    pub sha256: String,
}

/// `src`'s `cozy-runtime` requirement, as its pyproject.toml declares it: None when it names no version.
fn requirement_of(src: &Path) -> Option<VersionSpecifiers> {
    let pyproject: toml::Value = std::fs::read_to_string(src.join("pyproject.toml")).ok().and_then(|text| toml::from_str(&text).ok())?;
    let dependencies = pyproject.get("project").and_then(|p| p.get("dependencies")).and_then(toml::Value::as_array);
    api::release::required(dependencies.into_iter().flatten().filter_map(toml::Value::as_str), "cozy-runtime")
}

/// The Runtime `src` takes where its `cozy-runtime` floor is an RC build that no release meets (`>=0.47.0.dev0`): the open RC's
/// newest, which every machine of the RC runs (xs-194). A package names only that floor, never the RC's wheel, which each
/// Runtime merge replaces: a lock binds the newest build by URL in a private copy, and a publish's floor check reads it.
/// None: it names no RC floor, a release meets it, or no RC build does.
pub(crate) async fn rc_runtime(src: &Path) -> Result<Option<Wheel>, Error> {
    let Some(requirement) = requirement_of(src) else { return Ok(None) };
    if !requirement.iter().any(|s| s.version().is_dev()) {
        return Ok(None);
    }
    let released = released().await?;
    if released.iter().any(|release| Version::from_str(&release.version).is_ok_and(|version| requirement.contains(&version))) {
        return Ok(None);
    }
    let Some((build, url)) = crate::rc::runtime_now().await? else { return Ok(None) };
    let meets = Version::from_str(&build.version).is_ok_and(|version| requirement.contains(&version.without_local()));
    Ok(meets.then_some(Wheel { version: build.version, url, sha256: build.sha256 }))
}

/// The pure wheel of the `cozy-runtime` a lock holds: from an index, or the URL its source names (the open RC's build).
fn locked(lock: &str) -> Option<Wheel> {
    #[derive(Deserialize)]
    struct Lock {
        #[serde(default)]
        package: Vec<Row>,
    }
    #[derive(Deserialize)]
    struct Row {
        name: String,
        #[serde(default)]
        version: String,
        #[serde(default)]
        wheels: Vec<Locked>,
    }
    #[derive(Deserialize)]
    struct Locked {
        url: String,
        #[serde(default)]
        hash: String,
    }
    let lock: Lock = toml::from_str(lock).ok()?;
    let row = lock.package.into_iter().find(|row| row.name == "cozy-runtime")?;
    let wheel = row.wheels.into_iter().find(|wheel| wheel.url.ends_with("-py3-none-any.whl"))?;
    Some(Wheel { version: row.version, url: wheel.url, sha256: wheel.hash.trim_start_matches("sha256:").to_owned() })
}

/// `wheel` fetched into `dir`, held to its sha256.
async fn fetched(wheel: &Wheel, dir: &Path) -> Result<PathBuf, Error> {
    let failed = |why: String| Error::new(Kind::Unavailable, "runtime_unfetched", format!("cozy-runtime {} ({}): {why}", wheel.version, wheel.url));
    let answer = http()?.get(&wheel.url).send().await.and_then(reqwest::Response::error_for_status).map_err(|e| failed(e.to_string()))?;
    let bytes = answer.bytes().await.map_err(|e| failed(e.to_string()))?;
    let found = hex::encode(Sha256::digest(&bytes));
    if !wheel.sha256.is_empty() && found != wheel.sha256 {
        return Err(failed(format!("its sha256 is {found}, not {}", wheel.sha256)));
    }
    let path = dir.join(format!("cozy-runtime-{}.whl", &found[..16]));
    tokio::fs::write(&path, &bytes).await.map_err(|e| failed(e.to_string()))?;
    Ok(path)
}

/// One name a package imports that a Runtime lacks: its module, the name (none: the module itself), and where.
#[derive(Deserialize)]
struct Missing(String, Option<String>, String, u32);

/// What `runtime` lacks of the `cozy_runtime` names the package wheel `package` imports.
fn lacks(uv: &Path, dir: &Path, python: &str, runtime: &Path, package: &Path) -> Result<Vec<Missing>, Error> {
    #[derive(Deserialize)]
    struct Read {
        missing: Vec<Missing>,
    }
    let mut command = crate::local_code::uv(uv, dir);
    command.args(["run", "--no-project", "--no-config", "--python", python, "python", "-I", "-c", READER]).arg(runtime).arg(package);
    let said = tokio::task::block_in_place(|| crate::local_code::run(&mut command, "runtime_floor_unread", "the Runtime floor check"))?;
    let read: Read = serde_json::from_str(said.trim()).map_err(|e| Error::new(Kind::Unavailable, "runtime_floor_unread", format!("the Runtime floor check: {e}")))?;
    Ok(read.missing)
}

/// Publishing (cl-952): refuses a package whose wheel `package` imports from `cozy_runtime` what its floor lacks, so a machine
/// meeting its `cozy-runtime` requirement runs it. The floor is the oldest release that requirement admits, else, for an RC floor
/// (`>=0.47.0.dev0`) no release meets yet, the open RC's newest build (`rc_runtime`). The refusal names each name and a floor that
/// has them: its lock's Runtime, else the RC's. A package that names no `cozy-runtime` version claims none.
pub(crate) async fn check(api: &Arc<Api>, uv: &Path, python: &str, src: &Path, package: &Path, label: &str) -> Result<(), Error> {
    let refuse = |code: &str, why: String| Error::new(Kind::Validation, code, why);
    let Some(requirement) = requirement_of(src) else { return Ok(()) };
    let lock = std::fs::read_to_string(src.join("uv.lock")).ok().and_then(|lock| locked(&lock));
    let floor = match rc_runtime(src).await? {
        // An RC floor no release meets: the RC's newest build, which its machines run.
        Some(rc) => rc,
        None => {
            let released = released().await?;
            let oldest = (released.iter())
                .filter_map(|release| Some((Version::from_str(&release.version).ok()?, release)))
                .filter(|(version, _)| requirement.contains(version))
                .min_by(|(a, _), (b, _)| a.cmp(b));
            let Some((_, release)) = oldest else {
                return Err(refuse("runtime_floor_unreleased", format!("no cozy-runtime release, nor the open RC's newest build, meets {label}'s cozy-runtime{requirement}")));
            };
            let Some((url, sha256)) = release.pure.clone() else { return Ok(()) };
            Wheel { version: release.version.clone(), url, sha256 }
        }
    };
    // A project's package is read where its own staging is.
    let work = match crate::private_io::current() {
        Some(scope) => tempfile::tempdir_in(crate::private_io::cache(Some(&scope), PathBuf::new(), "author")?),
        None => tempfile::tempdir(),
    }
    .map_err(|e| Error::internal("runtime_floor_unread", e.to_string()))?;
    let missing = lacks(uv, work.path(), python, &fetched(&floor, work.path()).await?, package)?;
    if missing.is_empty() {
        return Ok(());
    }
    // A Runtime that has them all: its lock's, else the open RC's.
    let mut has: Option<(String, String)> = None;
    let rc = async {
        let (build, url) = api.machines.rc.now().await.ok()?.pure_runtime()?;
        Some(Wheel { version: build.version, url, sha256: build.sha256 })
    };
    for wheel in [lock.filter(|lock| lock.version != floor.version), rc.await].into_iter().flatten() {
        let Ok(path) = fetched(&wheel, work.path()).await else { continue };
        if lacks(uv, work.path(), python, &path, package).is_ok_and(|missing| missing.is_empty()) {
            has = Some((wheel.version, wheel.url));
            break;
        }
    }
    let named: Vec<String> = (missing.iter().take(3))
        .map(|Missing(module, name, file, line)| match name {
            Some(name) => format!("{name} from {module} ({file}:{line})"),
            None => format!("{module} ({file}:{line})"),
        })
        .collect();
    let more = if missing.len() > 3 { format!(" and {} more", missing.len() - 3) } else { String::new() };
    let floor_said = match api::prerelease::dev(&floor.version) {
        true => format!("cozy-runtime {}, the open RC's newest build,", prerelease::shown(&floor.version)),
        false => format!("cozy-runtime {}, the oldest its cozy-runtime{requirement} admits,", floor.version),
    };
    let error = refuse("runtime_floor_lacks", format!("{label} imports {}{more}, which {floor_said} lacks", named.join(", ")));
    Err(match has {
        Some((version, _)) if !api::prerelease::dev(&version) => error.remedy(format!("raise its floor: cozy-runtime>={version}")),
        // The RC's minor, never its build or wheel: each Runtime merge replaces those.
        Some((version, _)) => {
            let release = Version::from_str(&version).map(|v| v.release().to_vec()).unwrap_or_default();
            let (major, minor) = (release.first().copied().unwrap_or(0), release.get(1).copied().unwrap_or(0));
            error.remedy(format!("raise its floor to the open RC's: cozy-runtime>={major}.{minor}.0.dev0"))
        }
        None => error.remedy(format!("import only what cozy-runtime {} has, or raise its floor to a Runtime that has them", floor.version)),
    })
}
