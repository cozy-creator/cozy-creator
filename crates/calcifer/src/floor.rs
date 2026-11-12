//! A package release runs only on a Runtime its `cozy-runtime` requirement admits (cl-952, `api::release::choose`). A run whose
//! release needs a newer Runtime than its machine's moves the machine to the newest that runs it, a release (PyPI's) or on the
//! RC the RC's build, before it runs: an update under a `floor-` key, which holds the machine on nothing. A machine held by hand
//! refuses instead.
use std::{
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
use pep440_rs::{Operator, Version};
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
    /// The line the run says: `upgrading vanitas's Runtime 0.45.1 → 0.46.0.dev312 for minimax-h3 0.12.0rc3`.
    pub(crate) fn note(&self) -> Note {
        let (from, to) = (prerelease::shown(&self.from), prerelease::shown(&self.to));
        Note { level: NoteLevel::Info, message: format!("upgrading {}'s Runtime {from} → {to} for {}", self.machine, self.release) }
    }
}

impl Machines {
    /// The Runtimes `name` (on `frame`) can move to for a package release: PyPI's, and on the RC (`rc`, or held off it) the RC's
    /// build for its platform; and whether it moves (`api::release::Moves`): not when held by hand, nor when another computer's (added here,
    /// or a rental another client ordered).
    pub(crate) async fn runtimes(&self, name: &str, frame: &api::machine::StatusFrame, rc: bool) -> (Vec<String>, api::release::Moves) {
        let moves = match self.named(name).ok().flatten() {
            Some(row) if row.kind == MachineKind::Added || (row.kind == MachineKind::Rental && !self.ordered(&row)) => api::release::Moves::No,
            _ if self.held_by_hand(name, frame).is_some() => api::release::Moves::Held,
            _ => api::release::Moves::Yes,
        };
        let mut runtimes: Vec<String> = match released().await {
            Ok(released) => released.iter().map(|release| release.version.clone()).collect(),
            Err(e) => {
                tracing::warn!("the Runtime's releases: {}", e.message);
                vec![]
            }
        };
        // A machine held off the RC by hand counts the RC's build too: refused, it names the update that moves it there.
        if rc || self.rc_machine(name, frame) {
            if let Ok(sums) = self.rc.now().await {
                runtimes.extend(sums.build("runtime", &frame.platform).map(|(build, _)| build.version));
            }
        }
        (runtimes, moves)
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
            for (member, version) in sums.behind(&frame.platform, [frame.runtime.as_str(), frame.tensorfs.as_str(), frame.version.as_str()]) {
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
struct Wheel {
    version: String,
    url: String,
    sha256: String,
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
/// meeting its `cozy-runtime` requirement runs it. The floor is the oldest release that requirement admits, or, when it names
/// the open RC's build (`>=0.46.0.dev312`), the build its lock takes by URL, which it must be. The refusal names each name and
/// a floor that has them: its lock's Runtime, else the RC's. A package that names no `cozy-runtime` version claims none.
pub(crate) async fn check(api: &Arc<Api>, uv: &Path, python: &str, src: &Path, package: &Path, label: &str) -> Result<(), Error> {
    let refuse = |code: &str, why: String| Error::new(Kind::Validation, code, why);
    let pyproject: toml::Value = std::fs::read_to_string(src.join("pyproject.toml")).ok().and_then(|text| toml::from_str(&text).ok()).unwrap_or(toml::Value::Table(Default::default()));
    let dependencies: Vec<&str> = pyproject.get("project").and_then(|p| p.get("dependencies")).and_then(toml::Value::as_array).into_iter().flatten().filter_map(toml::Value::as_str).collect();
    let Some(requirement) = api::release::required(dependencies, "cozy-runtime") else { return Ok(()) };
    let lock = std::fs::read_to_string(src.join("uv.lock")).ok().and_then(|lock| locked(&lock));
    let floors = || requirement.iter().filter(|s| matches!(s.operator(), Operator::GreaterThanEqual | Operator::GreaterThan | Operator::Equal | Operator::TildeEqual | Operator::ExactEqual));
    let floor = match floors().filter(|s| s.version().is_dev()).map(|s| s.version().clone()).max() {
        // An RC floor names the build its lock holds: the build its RC's machines run at least.
        Some(named) => match lock.as_ref().filter(|lock| Version::from_str(&lock.version).is_ok_and(|locked| locked.without_local() == named)) {
            Some(lock) => Wheel { version: lock.version.clone(), url: lock.url.clone(), sha256: lock.sha256.clone() },
            None => {
                let held = lock.as_ref().map_or_else(|| "none".to_owned(), |lock| lock.version.clone());
                return Err(refuse("runtime_floor_unlocked", format!("{label} names the RC build cozy-runtime {named} as its floor, and its lock holds {held}: lock that build, its pure wheel as cozy-runtime's source")));
            }
        },
        None => {
            let released = released().await?;
            let oldest = (released.iter())
                .filter_map(|release| Some((Version::from_str(&release.version).ok()?, release)))
                .filter(|(version, _)| requirement.contains(version))
                .min_by(|(a, _), (b, _)| a.cmp(b));
            let Some((_, release)) = oldest else {
                return Err(refuse("runtime_floor_unreleased", format!("no cozy-runtime release meets {label}'s cozy-runtime{requirement}")));
            };
            let Some((url, sha256)) = release.pure.clone() else { return Ok(()) };
            Wheel { version: release.version.clone(), url, sha256 }
        }
    };
    let work = tempfile::tempdir().map_err(|e| Error::internal("runtime_floor_unread", e.to_string()))?;
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
        true => format!("cozy-runtime {}, the RC build its floor names,", prerelease::shown(&floor.version)),
        false => format!("cozy-runtime {}, the oldest its cozy-runtime{requirement} admits,", floor.version),
    };
    let error = refuse("runtime_floor_lacks", format!("{label} imports {}{more}, which {floor_said} lacks", named.join(", ")));
    Err(match has {
        Some((version, _)) if !api::prerelease::dev(&version) => error.remedy(format!("raise its floor: cozy-runtime>={version}")),
        Some((version, url)) => error.remedy(format!(
            "raise its floor to the open RC's build, cozy-runtime>={}, with its wheel as cozy-runtime's source in [tool.uv.sources]: {{ url = \"{url}\" }}",
            prerelease::shown(&version)
        )),
        None => error.remedy(format!("import only what cozy-runtime {} has, or raise its floor to a Runtime that has them", floor.version)),
    })
}
