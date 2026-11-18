//! `POST /v1/machines/{name}/update`: a machine's Runtime, TensorFS and TensorD (its `cozy`
//! binary) in one update run and one restart. Each is a release the machine fetches itself (PyPI
//! for the wheels, else their GitHub prerelease; the GitHub release for `cozy`), or a file on this
//! computer, a dev build staged with the run. With none named, each moves to its Hub's target. On
//! `local`, a `cozy` that `cozy update` installed and `local` does not run yet goes with it. The
//! run holds new runs behind it and waits for those in flight; it succeeds only when the machine
//! then runs what it asked for (`verified`). Work goes to a machine of any version (cl-1069): one on an
//! older minor that this computer may move is updated to this release first (`current`), never back.
use std::{path::Path, sync::Arc};

use axum::{
    Json,
    body::Bytes,
    extract::{Path as UrlPath, State},
    http::StatusCode,
};
use api::{
    calcifer::{self as wire, Component, DevBuild, Machine, MachineKind, MachineStatus, MachineUpdate, On, Run, StagedInput, Status, SubmitRun, Updating},
    error::{Error, Kind},
    machine as v1,
};
use serde_json::{Value, json};

use crate::{
    api::{Api, IdempotencyKey, Refusal, parse},
    machines::{Machines, internal},
    rentals::hub_refusal,
    store::RunRow,
};

/// The release this cozy is, or a dev build was built from (`0.25.0`, the RC's `0.26.0-dev`): what a release can fetch.
pub(crate) fn this_release() -> &'static str {
    ::api::VERSION.split('+').next().unwrap_or(::api::VERSION)
}

/// Whether this cozy is a dev build (the RC's `0.26.0-dev+g…`, a release's `+dev.g…`): no release of it to fetch.
pub(crate) fn dev_build() -> bool {
    ::api::prerelease::dev(::api::VERSION)
}

/// Whether this cozy is the open RC's dev version (`0.26.0-dev+g…`, cl-976): a minor no release or worker image is on yet.
pub(crate) fn rc_version() -> bool {
    this_release().contains('-')
}

/// This very `cozy`, held as a file an update stages (`--tensord <path>`): what a machine of this computer's platform runs to be on
/// this build when it has no release to fetch.
async fn this_build() -> Result<DevBuild, Error> {
    let unreadable = |e: std::io::Error| Error::new(Kind::Unavailable, "cozy_unreadable", format!("Cozy couldn't read its own program file: {e}."));
    let exe = platform::exe::running().map_err(crate::cozy::updated)?;
    let digest = tokio::task::spawn_blocking(move || crate::runs::files::hold(&std::fs::read(&exe).map_err(unreadable)?))
        .await
        .map_err(|e| Error::internal("cozy_unreadable", e.to_string()))??;
    Ok(DevBuild { sha256: digest, name: "cozy".into(), version: Some(::api::VERSION.into()) })
}

/// Whether releases `a` and `b` share a major and minor.
pub(crate) fn same_minor(a: &str, b: &str) -> bool {
    let minor = |release: &str| {
        let mut parts = release.trim_start_matches('v').split(['.', '+', '-']);
        (parts.next().unwrap_or_default().to_owned(), parts.next().unwrap_or_default().to_owned())
    };
    minor(a) == minor(b)
}

/// Whether release `machine` is of an older major or minor than `own`: a machine moves up for work, never down. One that doesn't
/// read is taken as older.
fn older_minor(machine: &str, own: &str) -> bool {
    let minor = |release: &str| -> Option<(u64, u64)> {
        let mut parts = release.trim_start_matches('v').split(['.', '+', '-']);
        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
    };
    match (minor(machine), minor(own)) {
        (Some(machine), Some(own)) => machine < own,
        _ => true,
    }
}

impl Machines {
    /// What work for `name` waits for first (cl-1069; Paul 11-16: "It should be a standing rule that we allow version skew"): work
    /// goes to a machine of any version. One on an older minor that this computer may move (`local`, or a rental it ordered, not held
    /// by hand, with no work going) moves up to this release first, once per boot: its update run, to wait for. One newer than this
    /// cozy is never moved back, and one it can't move, or whose update fails, gets work as it is.
    pub(crate) async fn current(self: &Arc<Self>, name: &str) -> Result<Option<Run>, Error> {
        if self.booting_for_rc(name) {
            return Err(Error::new(Kind::Unavailable, "machine_booting", format!("waiting for {name} to start")));
        }
        let frame = match self.frame(name) {
            Some(frame) => frame,
            None => {
                let (_, link) = self.linked(name)?;
                link.status().await.map_err(|e| Error::new(Kind::Unavailable, "machine_unreachable", format!("Cozy couldn't reach {name}: {e}.")))?
            }
        };
        let own = this_release();
        // The open RC's dev version moves no released machine to its minor: work goes to one as it runs, as when the RC's builds
        // said the last release (cl-976). A dev home's rental not on the open RC's builds yet moves to them first, when it has no
        // work going (cl-922); one on them is never moved ahead of work (cl-1123).
        if same_minor(&frame.version, own) || rc_version() {
            return Ok(self.follow_rc(name, true).await);
        }
        if !older_minor(&frame.version, own) {
            return Ok(None);
        }
        let row = self.named(name)?;
        // Not this computer's to move: added from another, or a rental another client ordered.
        let others = row.as_ref().is_some_and(|row| row.kind == MachineKind::Added || (row.kind == MachineKind::Rental && !self.ordered(row)));
        if others || self.held_by_hand(name).is_some() {
            return Ok(None);
        }
        let id = row.as_ref().and_then(|row| row.rental_id.clone()).unwrap_or_else(|| name.to_owned());
        let key = format!("minor-{id}-{}-{own}", frame.boot_id);
        let as_is = |why: String| {
            tracing::warn!(machine = name, "work goes to it as it is, on cozy {}: {why}", frame.version);
            Ok(None)
        };
        if let Some(row) = self.store().keyed(&key).map_err(internal)? {
            let run = self.calcifer.record(&row).map_err(|e| Refusal::from(e).0)?;
            return match run.status {
                Status::Completed => Ok(None),
                Status::Failed | Status::Canceled => {
                    let why = run.reason.as_ref().map_or_else(|| "it did not finish".to_owned(), |reason| reason.message.clone());
                    as_is(format!("its update to cozy {own} failed: {why}"))
                }
                _ => Ok(Some(run)),
            };
        }
        // An update would hold new runs behind the work it has going: they go as it is.
        if self.calcifer.each_store(|store|store.working(name)).map_err(internal)?.into_iter().sum::<u32>() > 0 {
            return Ok(None);
        }
        // What it takes to run this build: `local`, the cozy installed here; else this release, or a dev build's own binary.
        let m = self.show(name).await?;
        let tensord = match placed(self, &m).await {
            Some(installed) => Component::Version(installed),
            None if !dev_build() => Component::Version(own.into()),
            None if m.status.as_ref().is_some_and(|status| status.platform == format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH)) => {
                Component::File(this_build().await?)
            }
            None => return as_is("this cozy is a dev build of another platform, with no release to fetch".into()),
        };
        // `local` below this cozy's Runtime minor takes this cozy's own Runtime in the same update, unless held by hand (cl-1055): a
        // project's run then finds it on the minor, with no second update.
        let runtime = match self.held_by_hand(name) {
            Some(_) => None,
            None => self.own_runtime(name, &frame).await.filter(|runtime| !api::prerelease::dev(runtime)),
        };
        let ask = MachineUpdate { tensord: Some(tensord), runtime: runtime.map(Component::Version), ..Default::default() };
        match self.update(&key, name, ask).await {
            Ok((updating, _)) => Ok(updating.run),
            Err(e) => as_is(format!("its update to cozy {own} was refused: {}", e.message)),
        }
    }

    /// Starts the update `ask` of the machine `text` names under `key`: the same key again answers
    /// the run it made.
    pub(crate) async fn update(self: &Arc<Self>, key: &str, text: &str, ask: MachineUpdate) -> Result<(Updating, bool), Error> {
        crate::private_io::context(None,self.update_independent(key,text,ask)).await
    }

    async fn update_independent(self: &Arc<Self>, key:&str, text:&str, ask:MachineUpdate)->Result<(Updating,bool),Error> {
        wire::local_usable(text)?;
        if let Some(row) = self.store().keyed(key).map_err(internal)? {
            let run = self.calcifer.record(&row).map_err(|e| Refusal::from(e).0)?;
            let machine = self.show(row.machine.as_deref().unwrap_or(text)).await?;
            return Ok((Updating { machine, run: Some(run), target: row.target }, false));
        }
        let mut asked = vec![];
        for (member, distribution, component) in [
            ("runtime", "cozy_runtime", ask.runtime),
            ("tensorfs", "tensorfs", ask.tensorfs),
            ("tensord", "", ask.tensord),
        ] {
            if let Some(component) = component {
                asked.push((member, distribution, component));
            }
        }
        let m = self.show(text).await?;
        let hub = match m.hub.as_str() {
            "" => self.hubs.default_origin(),
            hub => hub.to_owned(),
        };
        let flagless = asked.is_empty() && ask.gpu_memory_limit.is_none() && ask.mode.is_none() && !ask.torch;
        // The Hub's PyTorch, once read for this update.
        let mut hub_torch = None;
        // The cozy `cozy update` installed here and `local` has not moved to yet: it goes with the
        // update, unless another `cozy` is named.
        let mut placed = placed(self, &m).await;
        if let Some(version) = &placed {
            asked.retain(|(member, _, c)| !(*member == "tensord" && matches!(c, Component::Version(v) if v.trim_start_matches('v') == version)));
            if asked.iter().any(|(member, _, _)| *member == "tensord") {
                placed = None;
            }
        }
        if flagless {
            let Some(status) = &m.status else {
                return Err(Error::new(Kind::Unavailable, "machine_unanswered", format!("{} hasn't reported what software it runs yet.", m.name))
                    .next(format!("cozy machine show {}", m.name)));
            };
            let local = m.kind == MachineKind::Local;
            if self.dev && (m.kind == MachineKind::Rental || (local && crate::checkout::configured())) {
                // A dev home's rental moves to the open RC's builds for its platform (cl-922), and its `local` where its first-party
                // packages run from the RC's source (xs-195): its Runtime and TensorFS, its TensorD the cozy installed here.
                let sums = self.rc.now().await?;
                for (member, version) in sums.behind(&status.platform, [status.runtime.as_str(), status.tensorfs.as_str(), status.version.as_str()], local) {
                    let distribution = match member { "runtime" => "cozy_runtime", "tensorfs" => "tensorfs", _ => "" };
                    asked.push((member, distribution, Component::Version(version)));
                }
            } else {
                // The Hub unreachable, its target as this computer last read it (cl-1127).
                let target = match self.hubs.anonymous(&hub)?.get_software().await {
                    Ok(software) => {
                        crate::hub::held::keep_software(self.hubs.home(), &hub, &software);
                        software
                    }
                    Err(e) => {
                        let refused = hub_refusal(&hub, e);
                        match crate::hub::held::software(self.hubs.home(), &hub).filter(|_| refused.transient()) {
                            Some(kept) => kept,
                            None => return Err(refused),
                        }
                    }
                };
                hub_torch = Some(target.torch.clone());
                // An unpinned target older than the machine's software is no reason to go back.
                let moves = |running: &str, wanted: &str| running != wanted && (target.pinned || newer(wanted, running));
                let cozy = target.cozy.as_deref().filter(|cozy| moves(&status.version, cozy) && placed.is_none());
                for (member, distribution, running, wanted) in [
                    ("runtime", "cozy_runtime", status.runtime.as_str(), Some(target.runtime.as_str())),
                    ("tensorfs", "tensorfs", status.tensorfs.as_str(), Some(target.tensorfs.as_str())),
                    ("tensord", "", status.version.as_str(), cozy),
                ] {
                    if let Some(wanted) = wanted.filter(|wanted| moves(running, wanted)) {
                        asked.push((member, distribution, Component::Version(wanted.into())));
                    }
                }
            }
        }
        let torch = self.torch(&m, &hub, ask.torch || hub_torch.is_some(), hub_torch).await?;
        let settings = ask.gpu_memory_limit.is_some() || ask.mode.is_some();
        if asked.is_empty() && placed.is_none() && torch.is_none() && !settings {
            // Nothing to move, and nothing named: it lets go of a hold by hand all the same (cl-952).
            if flagless {
                self.mind_hold(&m.name, key, "", true);
            }
            return Ok((Updating { machine: m, run: None, target: String::new() }, true));
        }
        rc_pairing(&m, &asked, placed.as_deref())?;
        let platform = m.status.as_ref().map(|s| s.platform.clone()).unwrap_or_default();
        // Nothing named, off the RC: each moves to its Hub's target, said `(Hub)`, which holds the machine on nothing.
        let hub_target = if flagless { " (Hub)" } else { "" };
        let (mut payload, mut inputs, mut staged, mut named) = (serde_json::Map::new(), vec![], vec![], vec![]);
        for (member, distribution, component) in asked {
            let (path, built) = match component {
                // A dev build by version is the open RC's (cl-922): a wheel the machine fetches from its prerelease, held to
                // its digest; the `cozy` fetched here and staged with the run.
                Component::Version(version) if api::prerelease::dev(&version) => {
                    let version = release(&version)?;
                    let sums = self.rc.now().await?;
                    // The RC's Runtime by its version alone (`0.47.0.dev2260`) too.
                    let same = |build: &api::prerelease::Build| build.version == version || build.version.split_once('+').is_some_and(|(public, _)| public == version);
                    let Some((build, url)) = sums.build(member, &platform).filter(|(build, _)| same(build)) else {
                        let has = sums.build(member, &platform).map_or_else(|| format!("no build of it for {platform}"), |(build, _)| build.version);
                        return Err(Error::new(Kind::NotFound, "rc_build_absent", format!("{member} {version} isn't the open RC's build. rc-{} has {has}.", sums.rc))
                            .remedy("Leave out the version to move it to the open RC's builds.")
                            .next(format!("cozy machine update {}", m.name)));
                    };
                    named.push(format!("{member} {version} (RC)"));
                    if member != "tensord" {
                        payload.insert(member.into(), json!({ "url": url, "sha256": build.sha256 }));
                        continue;
                    }
                    (crate::rc::held_cozy(&build, &url).await?, "RC")
                }
                Component::Version(version) => {
                    let version = release(&version)?;
                    // A wheel version PyPI lacks is its GitHub prerelease, its SHA256SUMS signed by its publish workflow
                    // (xs-196): named by hand, or the Hub's target on a dev computer (`dev = true`) only. The machine fetches
                    // the wheel itself, held to its digest.
                    let fetched = match member {
                        "tensord" => None,
                        _ => prerelease(distribution, &version, &platform, !flagless || self.dev).await?,
                    };
                    match fetched {
                        Some((url, sha256)) => {
                            named.push(format!("{member} {version} (prerelease){hub_target}"));
                            payload.insert(member.into(), json!({ "url": url, "sha256": sha256 }));
                        }
                        None => {
                            named.push(format!("{member} {version}{hub_target}"));
                            payload.insert(member.into(), version.into());
                        }
                    }
                    continue;
                }
                Component::File(build) => (build, "dev"),
            };
            // The pure Runtime wheel carries no kernels: a machine whose GPUs run them is refused it (cl-913).
            if member == "runtime" && path.name.ends_with("-none-any.whl") && m.status.as_ref().is_some_and(MachineStatus::takes_runtime_kernels) {
                let commit = path.name.split_once("+dev.g").map(|(_, rest)| rest.chars().take_while(char::is_ascii_hexdigit).collect::<String>());
                return Err(wire::kernel_less_runtime(&m.name, &format!("{}, the pure wheel,", path.name), commit.as_deref().filter(|c| !c.is_empty())));
            }
            let (file, input, version) = measured(path, distribution)?;
            if built == "dev" {
                named.push(format!("{member} {} (dev)", version.as_deref().unwrap_or(&file)));
            }
            inputs.push(v1::InputFile {
                field: member.into(),
                digest: input.digest.to_string(),
                length: input.length,
                media_type: match distribution {
                    "" => "application/octet-stream",
                    _ => "application/zip",
                }
                .into(),
                order: 0,
            });
            // A wheel by its file's name; the machine's `cozy` by the release it says it is, which
            // `verified` holds it to (`dev` when it does not run here).
            payload.insert(member.into(), match member {
                "tensord" => version.unwrap_or_else(|| "dev".into()),
                _ => file,
            }.into());
            staged.push(input);
        }
        if let Some(version) = placed {
            // The guard every TensorD carries out: the binary at its launch path, that release, activates.
            named.push(format!("cozy {version} (installed)"));
            payload.insert("cozy".into(), version.into());
        }
        if let Some(version) = torch {
            named.push(match version.as_str() {
                "" => "its PyTorch".into(),
                version => format!("PyTorch {version}{hub_target}"),
            });
            payload.insert("torch".into(), version.into());
        }
        if let Some(limit) = ask.gpu_memory_limit {
            named.push(match limit {
                0 => "no GPU memory limit".into(),
                bytes => format!("a GPU memory limit of {}", crate::bytes(bytes as f64)),
            });
            payload.insert("gpu_memory_limit".into(), limit.into());
        }
        if let Some(mode) = ask.mode {
            named.push(format!("mode {}", mode.name()));
            payload.insert("mode".into(), mode.name().into());
        }
        let target = named.join(", ");
        // One of Calcifer's own moves (the RC's) that an update by hand pinned meanwhile, while it fetched the RC's `cozy`: the pin
        // wins (cl-1203: a candidate TensorD replaced a minute later on mucheng and lucario).
        if crate::rc::calcifers(key) && self.held_by_hand(&m.name).is_some() {
            return Err(Error::new(Kind::Conflict, "machine_held", format!("{} was held by hand meanwhile; it stays as it is", m.name)));
        }
        let submit = SubmitRun { project: None,
            agreed: None,
            stays: vec![],
            on: On::Machine(m.name.clone()),
            hub,
            kind: wire::Kind::Update,
            target: target.clone(),
            spec: v1::RunSpec {
                kind: v1::RunKind::Update,
                payload: Some(v1::Json::parse(Value::Object(payload).to_string()).expect("JSON")),
                inputs,
                ..Default::default()
            },
            inputs: staged,
            held: vec![],
            out: None,
            warnings: vec![],
        };
        let submitted = self.calcifer.submit(key, submit, crate::store::Task::Update).await.map_err(|e| Refusal::from(e).0)?;
        // Software named by hand holds the machine on it, across restarts, until an update names nothing (cl-952).
        if !submitted.replay {
            self.mind_hold(&m.name, key, &target, flagless);
        }
        let updating = Updating { machine: m, run: Some(submitted.run), target };
        Ok((updating, !submitted.replay))
    }

    /// The PyTorch `m` installs as its own base when `asked` (cl-1030): the version its Hub names (`named`, else read
    /// now), when it holds another; with none named (an older Hub, or none answering, as at a first run), the newest, when
    /// it holds none. Only a machine on this cozy's minor knows the member; a rental's is its worker image's.
    async fn torch(&self, m: &Machine, hub: &str, asked: bool, named: Option<Option<String>>) -> Result<Option<String>, Error> {
        let Some(status) = m.status.as_ref().filter(|_| asked && m.kind != MachineKind::Rental) else {
            return match asked && m.kind != MachineKind::Rental {
                true => Err(Error::new(Kind::Unavailable, "machine_unanswered", format!("{} hasn't reported what software it runs yet.", m.name))
                    .next(format!("cozy machine show {}", m.name))),
                false => Ok(None),
            };
        };
        if !same_minor(&status.version, this_release()) {
            return Ok(None);
        }
        let named = match named {
            Some(named) => named,
            None => match self.hubs.anonymous(hub)?.get_software().await {
                Ok(software) => software.torch,
                Err(e) => {
                    tracing::warn!(machine = %m.name, "{hub} did not name its PyTorch: {e}");
                    None
                }
            },
        };
        let held = status.torch.split('+').next().unwrap_or_default();
        Ok(match named {
            Some(version) if version != held => Some(version),
            None if held.is_empty() => Some(String::new()),
            _ => None,
        })
    }

    /// An update run's outcome as it settles: one that succeeded while its machine does not run
    /// what it asked for fails `update_unapplied`, with its log's line about the update. A machine
    /// restarting again (for an update queued behind this one) is judged by what this update's
    /// outcome says it left; a dev build's flag is its status's alone.
    pub(crate) async fn verified(self: &Arc<Self>, row: &RunRow, mut outcome: v1::Outcome) -> v1::Outcome {
        // A machine can't know what it's called here.
        if let (Some(name), Some(reason)) = (row.machine.as_deref(), outcome.reason.as_mut()) {
            reason.message = reason.message.replace("<machine>", name);
            if reason.next.is_empty() {
                reason.next = commands(&reason.message);
            }
            if reason.code == "does_not_fit" && reason.next.is_empty() {
                reason.next.push(format!("cozy run {} … --on new", row.target));
            }
            for next in &mut reason.next {
                *next = next.replace("<machine>", name);
            }
            reason.next.dedup();
            reason.next.truncate(2);
        }
        let Some(name) = row.machine.as_deref().filter(|_| row.kind == wire::Kind::Update && outcome.status == "succeeded") else {
            return outcome;
        };
        let want = wanted(&row.spec);
        // Boxed, as the run drivers' futures are (`refloored`).
        let m = match Box::pin(self.came_back(name, &want)).await {
            Ok(machine) => machine,
            Err(error) if want.iter().any(|(field, _)| *field == "mode") => {
                outcome.status = "failed".into();
                outcome.reason = Some(v1::Reason {
                    code: "update_unapplied".into(),
                    message: format!("{name} updated, but Cozy couldn't check its mode: {error}"),
                    origin: "machine".into(), detail: String::new(), next: Vec::new(),
                });
                return outcome;
            }
            Err(_) => return outcome,
        };
        let status = m.status.clone().unwrap_or_default();
        if m.unreachable.is_none() && runs(&status, &want) {
            return outcome;
        }
        let to = outcome.result.as_ref().and_then(|result| serde_json::from_str::<Value>(result.0.get()).ok()).map(|result| result["to"].clone());
        let left = to.map(|to| MachineStatus {
            version: to["cozy"].as_str().unwrap_or_default().into(),
            runtime: to["runtime"].as_str().unwrap_or_default().into(),
            tensorfs: to["tensorfs"].as_str().unwrap_or_default().into(),
            ..status.clone()
        });
        let named: Vec<_> = want.iter().filter(|(field, _)| *field != "dev").cloned().collect();
        // Software `to` does not report mode. A requested mode requires live status.
        if !want.iter().any(|(field, _)| *field == "mode")
            && left.is_some_and(|left| m.unreachable.is_some() && runs(&left, &named))
        {
            return outcome;
        }
        let log = self.logs(name, wire::MachineLog::Tensord, 1000).await.unwrap_or_default();
        let log = String::from_utf8_lossy(&log);
        let line = m.unreachable.clone().or_else(|| log.lines().rfind(|l| l.contains(row.id.as_str())).map(str::to_owned));
        outcome.status = "failed".into();
        outcome.reason = Some(v1::Reason {
            code: "update_unapplied".into(),
            message: unapplied(&m, &status, &want, line.as_deref()),
            origin: "machine".into(),
            ..Default::default()
        });
        outcome
    }
}

/// How long an update's outcome waits for its machine to come back from the restart the update ends with.
const BACK: std::time::Duration = std::time::Duration::from_secs(180);

impl Machines {
    /// `name` once the restart its update ends with is over: read again as what's known of it changes, until it runs `want` or
    /// `BACK` passed, and the last read stands. Read at once, a machine still restarting answered nothing, and an update of its
    /// mode failed `update_unapplied` (cl-1123, pod-session 44042599506).
    async fn came_back(self: &Arc<Self>, name: &str, want: &[(&str, Value)]) -> Result<Machine, Error> {
        let mut changes = self.calcifer.machine_watch(name);
        let deadline = tokio::time::Instant::now() + BACK;
        loop {
            let read = self.show(name).await;
            let back = read.as_ref().is_ok_and(|m| m.unreachable.is_none() && runs(&m.status.clone().unwrap_or_default(), want));
            let now = tokio::time::Instant::now();
            if back || now >= deadline {
                return read;
            }
            // What it says next, or a look again soon: a machine coming back says nothing until its stream opens again.
            let _ = tokio::time::timeout((deadline - now).min(std::time::Duration::from_secs(5)), changes.changed()).await;
        }
    }
}

/// `POST /v1/machines/{name}/update`: 202 with the update run, 200 when the key had made it or the
/// machine runs its Hub's target already.
pub(crate) async fn update(
    State(api): State<Arc<Api>>,
    UrlPath(name): UrlPath<String>,
    IdempotencyKey(key): IdempotencyKey,
    body: Bytes,
) -> Result<(StatusCode, Json<Updating>), Refusal> {
    let ask: MachineUpdate = parse(&body)?;
    let (updating, fresh) = api.machines.update(&key, &name, ask).await.map_err(Refusal)?;
    let status = match fresh && updating.run.is_some() {
        true => StatusCode::ACCEPTED,
        false => StatusCode::OK,
    };
    Ok((status, Json(updating)))
}

/// Refuses an update that would leave `m` on the open RC's Runtime (a dev release, `0.47.0.dev2259`) under a released cozy
/// (cl-952): only the RC's own TensorD runs released packages over their caps on it, so a release of cozy there refuses them.
/// The RC follower and a package's floor move the three together; this holds for an update by hand.
fn rc_pairing(m: &Machine, asked: &[(&str, &str, Component)], placed: Option<&str>) -> Result<(), Error> {
    let status = m.status.clone().unwrap_or_default();
    let named = |member: &str| asked.iter().find(|(asked, _, _)| *asked == member).map(|(_, _, component)| component);
    let runtime = match named("runtime") {
        Some(Component::Version(version)) => version.clone(),
        Some(Component::File(build)) => build.name.split('-').nth(1).unwrap_or_default().to_owned(),
        None => status.runtime.clone(),
    };
    let (cozy, dev) = match (named("tensord"), placed) {
        (Some(Component::Version(version)), _) => (version.clone(), api::prerelease::dev(version)),
        (Some(Component::File(_)), _) => ("a dev build".to_owned(), true),
        (None, Some(installed)) => (installed.to_owned(), api::prerelease::dev(installed)),
        (None, None) => (status.version.clone(), status.dev || api::prerelease::dev(&status.version)),
    };
    let rc = runtime.parse::<pep440_rs::Version>().is_ok_and(|version| version.is_dev() && !version.is_pre() && !version.is_post());
    if !rc || dev || cozy.is_empty() {
        return Ok(());
    }
    Err(Error::new(
        Kind::Conflict,
        "rc_runtime_needs_rc_cozy",
        format!("{} would run the open RC's Runtime {} under cozy {cozy}, which is a release. Only the RC's cozy can run released packages with it.", m.name, api::prerelease::shown(&runtime)),
    )
    .next(format!("cozy machine update {}", m.name)))
}

/// The release of this computer's installed cozy when `m` is `local` and runs another: the binary
/// `cozy update` renamed over its launch path, which it moves to at its next update.
async fn placed(machines: &Machines, m: &Machine) -> Option<String> {
    let running = m.status.as_ref().filter(|_| m.kind == MachineKind::Local)?.version.clone();
    installed(machines).await.filter(|version| *version != running)
}

/// The release this computer's installed `cozy` says it is: what `local` runs (xs-195).
pub(crate) async fn installed(machines: &Machines) -> Option<String> {
    let binary = machines.home.launcher()?;
    tokio::task::spawn_blocking(move || said_by(&binary)).await.ok()?
}

/// What the installed `cozy` last said it is, without asking it: None until asked, or once its binary changed.
pub(crate) fn installed_known(machines: &Machines) -> Option<String> {
    let binary = machines.home.launcher()?;
    let stamp = stamp(&binary)?;
    SAID.lock().expect("said lock").as_ref().filter(|(at, _)| *at == stamp).map(|(_, said)| said.clone())
}

type Stamp = (std::path::PathBuf, u64, std::time::SystemTime);
/// The installed `cozy`'s release as it said it, by its binary's path, size and time: asked again only once an install replaced it.
static SAID: std::sync::Mutex<Option<(Stamp, String)>> = std::sync::Mutex::new(None);

fn stamp(binary: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(binary).ok()?;
    Some((binary.to_owned(), meta.len(), meta.modified().ok()?))
}

/// The release the `cozy` at `binary` says it is, asked once each time it changes.
fn said_by(binary: &Path) -> Option<String> {
    let at = stamp(binary)?;
    if let Some((_, said)) = SAID.lock().expect("said lock").as_ref().filter(|(was, _)| *was == at) {
        return Some(said.clone());
    }
    let said = says(binary)?;
    *SAID.lock().expect("said lock") = Some((at, said.clone()));
    Some(said)
}

/// The release this computer's installed `cozy` says it is.
fn says(binary: &Path) -> Option<String> {
    let out = std::process::Command::new(binary).arg("--version").stdin(std::process::Stdio::null()).output().ok()?;
    let said = String::from_utf8_lossy(&out.stdout);
    let version = said.trim().strip_prefix("cozy ")?;
    (out.status.success() && !version.is_empty()).then(|| version.to_owned())
}

/// A release named for a component: `0.33.1`, or `v0.33.1`.
fn release(text: &str) -> Result<String, Error> {
    let version = text.trim_start_matches('v');
    match !version.is_empty() && version.bytes().all(|c| c.is_ascii_alphanumeric() || b".-+".contains(&c)) {
        true => Ok(version.to_owned()),
        false => Err(Error::new(Kind::Validation, "component_refused", format!("{text:?} isn't a release version."))),
    }
}

/// `distribution` `version`'s prerelease wheel for `platform` (`linux/x86_64`), when PyPI does not
/// hold that version: the GitHub prerelease (cozy-runtime `v<version>`, cozy-creator
/// `tensorfs-v<version>`) names it in its `SHA256SUMS`, the platform's own wheel first, else the
/// pure one. Its URL and sha256; `None` for a PyPI release, which the machine fetches from PyPI.
async fn prerelease(distribution: &str, version: &str, platform: &str, taken: bool) -> Result<Option<(String, String)>, Error> {
    let failed = |e: reqwest::Error| Error::new(Kind::Unavailable, "release_unreachable", e.to_string());
    let http = tensorfs::fetch::http(&[]).expect("the webpki roots alone").read_timeout(tensorfs::fetch::STALL).build().map_err(failed)?;
    let project = distribution.replace('_', "-");
    let pypi = http.get(format!("https://pypi.org/pypi/{project}/{version}/json")).send().await.map_err(failed)?;
    if pypi.status().is_success() {
        return Ok(None);
    }
    if !taken {
        return Err(Error::new(Kind::Conflict, "prerelease_refused", format!("The Hub's target {project} {version} isn't on PyPI, and this computer only installs prereleases when its config has `dev = true`."))
            .remedy("Name a released version.")
            .next("cozy machine update <machine> --runtime <version> --tensorfs <version>"));
    }
    let base = api::prerelease::base(distribution, version);
    let sums = http.get(format!("{base}/SHA256SUMS")).send().await.map_err(failed)?;
    if !sums.status().is_success() {
        return Err(Error::new(
            Kind::NotFound,
            "release_not_found",
            format!("{project} {version} isn't on PyPI or a GitHub prerelease ({base}/SHA256SUMS answered HTTP {}).", sums.status()),
        )
        .remedy("Leave out the version to move it to the Hub's target.")
        .next("cozy machine update <machine>"));
    }
    let sums = sums.text().await.map_err(failed)?;
    let bundles = format!("{base}/SHA256SUMS.sigstore.jsonl");
    let answer = http.get(&bundles).send().await.map_err(failed)?;
    let lines = match answer.status().is_success() {
        true => answer.text().await.map_err(failed)?,
        false => String::new(),
    };
    platform::release::prerelease_sums(distribution, version, &sums, &bundles, &lines)
        .map_err(|e| Error::new(Kind::Validation, e.code, format!("{project} {version}'s prerelease isn't signed by its publish workflow: {}", e.message)))?;
    let (file, sha256) = api::prerelease::wheel(&sums, distribution, version, platform).ok_or_else(|| {
        Error::new(Kind::NotFound, "release_wheel_absent", format!("{project} {version}'s prerelease has no wheel for {platform}."))
    })?;
    Ok(Some((format!("{base}/{file}"), sha256.to_owned())))
}

/// A held dev build of `distribution` (or, empty, a `cozy` binary): its file name, what the run stages, and for a `cozy` binary the
/// release its client says it is.
fn measured(build: DevBuild, distribution: &str) -> Result<(String, StagedInput, Option<String>), Error> {
    let refused = |why: String| Error::new(Kind::Validation, "wheel_refused", why);
    let DevBuild { sha256, name, version } = build;
    let named = name.starts_with(&format!("{distribution}-")) && name.ends_with(".whl") && !name.contains(['/', '\\']);
    if !distribution.is_empty() && !named {
        return Err(refused(format!("{name} isn't a {distribution} wheel file ({distribution}-<version>-….whl).")));
    }
    let Some(path) = crate::runs::files::held(&sha256) else {
        return Err(Error::new(Kind::NotFound, "file_not_held", format!("Cozy doesn't have the file {name} ({sha256}).")).remedy(format!("Upload it first (PUT /v1/files/{sha256}).")));
    };
    let length = std::fs::metadata(&path).map_err(|e| refused(format!("{name}: {e}")))?.len();
    let version = version.filter(|_| distribution.is_empty());
    Ok((name, StagedInput { path, digest: sha256, length }, version))
}

/// What a machine that carried out update `spec` reports: each wheel's version (a staged one's
/// from its file name, a fetched one's from its URL's), and its `cozy`'s release, or `dev` for a
/// dev binary that did not say its release here.
pub(crate) fn wanted(spec: &v1::RunSpec) -> Vec<(&'static str, Value)> {
    let payload: Value = spec.payload.as_ref().and_then(|p| serde_json::from_str(p.0.get()).ok()).unwrap_or_default();
    let wheel = |file: &str| file.strip_suffix(".whl").map(|w| w.split('-').nth(1).unwrap_or_default().to_owned());
    let mut want = vec![];
    for member in ["runtime", "tensorfs"] {
        let version = match &payload[member] {
            Value::String(named) => wheel(named).unwrap_or_else(|| named.clone()),
            Value::Object(fetched) => fetched["url"].as_str().and_then(|url| wheel(url.rsplit('/').next()?)).unwrap_or_default(),
            _ => continue,
        };
        want.push((member, Value::from(version)));
    }
    let dev = spec.inputs.iter().any(|input| input.field == "tensord");
    for member in ["tensord", "cozy"] {
        match payload[member].as_str() {
            Some("dev") if dev => want.push(("dev", true.into())),
            Some(version) => want.push(("version", version.into())),
            None => {}
        }
    }
    if let Some(mode) = payload["mode"].as_str() {
        want.push(("mode", mode.into()));
    }
    want
}

fn runs(status: &MachineStatus, want: &[(&str, Value)]) -> bool {
    let status = serde_json::to_value(status).unwrap_or_default();
    want.iter().all(|(field, value)| status[*field] == *value)
}

fn unapplied(m: &Machine, s: &MachineStatus, want: &[(&str, Value)], line: Option<&str>) -> String {
    let asked: Vec<String> = (want.iter())
        .map(|(field, value)| match (*field, value.as_str()) {
            ("runtime", Some(version)) => format!("cozy-runtime {version}"),
            ("version", Some(version)) => format!("cozy {version}"),
            (field, Some(version)) => format!("{field} {version}"),
            _ => "dev build".into(),
        })
        .collect();
    let build = if s.dev { "dev build" } else { "release" };
    let message = format!(
        "{}'s update finished, but it still runs cozy {} ({build} {}), cozy-runtime {}, tensorfs {} and mode {}, not the requested {}",
        m.name, s.version, s.commit.get(..12).unwrap_or(&s.commit), s.runtime, s.tensorfs,
        if s.mode.is_empty() { "unknown" } else { &s.mode }, asked.join(", ")
    );
    match line {
        Some(line) => format!("{message}: {}", line.trim()),
        None => message,
    }
}

/// The exact commands a machine's words name, as it writes them: `cozy …` in backticks, with no
/// placeholder left.
fn commands(message: &str) -> Vec<String> {
    let exact = |said: &&str| said.starts_with("cozy ") && *said != "cozy run" && !said.contains(['<', '…', '{']);
    message.split('`').skip(1).step_by(2).filter(exact).map(str::to_owned).collect()
}

/// Whether release `a` is newer than `b`, by their leading dotted numbers.
/// Whether release `a` is after `b`, by semver: build metadata (`+dev.g…`) is no part of it, and a pre-release comes before its release.
fn newer(a: &str, b: &str) -> bool {
    let read = |v: &str| {
        let core = v.split('+').next().unwrap_or(v);
        let (numbers, pre) = core.split_once('-').map_or((core, None), |(numbers, pre)| (numbers, Some(pre)));
        let numbers: Vec<u64> = numbers.split('.').map(|part| part.parse().unwrap_or(0)).collect();
        (numbers, pre.is_none())
    };
    read(a) > read(b)
}
