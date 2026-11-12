//! A package release runs only on a Runtime its `cozy-runtime` requirement admits (cl-952, `api::release::choose`). A run whose
//! release needs a newer Runtime than its machine's moves the machine to the newest that runs it, a release (PyPI's) or on the
//! RC the RC's build, before it runs: an update under a `floor-` key, which holds the machine on nothing. A machine held by hand
//! refuses instead.
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use api::{
    calcifer::{Component, MachineKind, MachineUpdate, Note, NoteLevel, Run, Status},
    error::{Error, Kind},
    prerelease,
};
use serde::Deserialize;

use crate::{api::Refusal, machines::Machines};

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
        let released = (project.releases.into_iter())
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
    /// The Runtimes `name` (on `frame`) can move to for a package release: PyPI's, and on the RC (`rc`) the RC's build for its
    /// platform. None when it moves for none: held by hand, or another computer's (`api::release::Moves`).
    pub(crate) async fn runtimes(&self, name: &str, frame: &api::machine::StatusFrame, rc: bool) -> (Vec<String>, api::release::Moves) {
        let moves = match self.named(name).ok().flatten() {
            Some(row) if row.kind == MachineKind::Added => api::release::Moves::No,
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
        if rc {
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
