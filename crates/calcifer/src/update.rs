//! `POST /v1/machines/{name}/update`: a machine's Runtime, TensorFS and TensorD (its `cozy`
//! binary) in one update run and one restart. Each is a release the machine fetches itself (PyPI
//! for the wheels, else their GitHub prerelease; the GitHub release for `cozy`), or a file on this
//! computer, a dev build staged with the run. With none named, each moves to its Hub's target. On
//! `local`, a `cozy` that `cozy update` installed and `local` does not run yet goes with it. The
//! run holds new runs behind it and waits for those in flight; it succeeds only when the machine
//! then runs what it asked for (`verified`). Work goes only to a machine on this cozy's minor: one on
//! another is updated to it first (`current`), never while it has work going.
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

/// The release this cozy is, or a dev build was built from: what a machine can fetch.
pub(crate) fn this_release() -> &'static str {
    env!("CARGO_PKG_VERSION").split('+').next().unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// Whether this cozy is a dev build (`+dev.g…`): no release of it to fetch.
pub(crate) fn dev_build() -> bool {
    env!("CARGO_PKG_VERSION").contains('+')
}

/// The command that moves `name` to this cozy: its release, or a dev build's own binary.
pub(crate) fn update_command(name: &str) -> String {
    let tensord = match (dev_build(), std::env::current_exe()) {
        (true, Ok(path)) => path.display().to_string(),
        _ => this_release().to_owned(),
    };
    format!("cozy machine update {name} --tensord {tensord}")
}

/// This very `cozy`, held as a file an update stages (`--tensord <path>`): what a machine of this computer's platform runs to be on
/// this build when it has no release to fetch.
async fn this_build() -> Result<DevBuild, Error> {
    let unreadable = |e: std::io::Error| Error::new(Kind::Unavailable, "cozy_unreadable", format!("this cozy's binary: {e}"));
    let exe = std::env::current_exe().map_err(unreadable)?;
    let digest = tokio::task::spawn_blocking(move || crate::runs::files::hold(&std::fs::read(&exe).map_err(unreadable)?))
        .await
        .map_err(|e| Error::internal("cozy_unreadable", e.to_string()))??;
    Ok(DevBuild { sha256: digest, name: "cozy".into(), version: Some(env!("CARGO_PKG_VERSION").into()) })
}

/// A machine on another minor than this cozy, which isn't updated unasked, or whose update failed: no work goes there.
pub(crate) const OTHER_MINOR: &str = "machine_other_minor";

/// Whether releases `a` and `b` share a major and minor: what a machine and this Calcifer must, for work to go there.
pub(crate) fn same_minor(a: &str, b: &str) -> bool {
    let minor = |release: &str| {
        let mut parts = release.trim_start_matches('v').split(['.', '+', '-']);
        (parts.next().unwrap_or_default().to_owned(), parts.next().unwrap_or_default().to_owned())
    };
    minor(a) == minor(b)
}

impl Machines {
    /// Whether work may go to `name` now (cl-816; Paul 11-07: pre-launch, no legacy support): only to a machine on this cozy's minor.
    /// One on another is updated to this release first, as `cozy machine update` does, once per boot for all that send there: its
    /// update run, to wait for. One added from another computer, or with work going, is left as it is: the refusal names the command.
    pub(crate) async fn current(self: &Arc<Self>, name: &str) -> Result<Option<Run>, Error> {
        let frame = match self.frame(name) {
            Some(frame) => frame,
            None => {
                let (_, link) = self.linked(name)?;
                link.status().await.map_err(|e| Error::new(Kind::Unavailable, "machine_unreachable", format!("{name}: {e}")))?
            }
        };
        let own = this_release();
        if same_minor(&frame.version, own) {
            // A dev home's rental moves to the open RC first, when it is behind it and has no work going (cl-922).
            return Ok(self.follow_rc(name, true).await);
        }
        let row = self.named(name)?;
        let id = row.as_ref().and_then(|row| row.rental_id.clone()).unwrap_or_else(|| name.to_owned());
        let key = format!("minor-{id}-{}-{own}", frame.boot_id);
        let refused = |why: String| {
            Error::new(Kind::Conflict, OTHER_MINOR, format!("{name} runs cozy {}, and this cozy is {own}: {why}", frame.version))
                .next(update_command(name))
        };
        if let Some(row) = self.store().keyed(&key).map_err(internal)? {
            let run = self.calcifer.record(&row).map_err(|e| Refusal::from(e).0)?;
            return match run.status {
                Status::Completed => Ok(None),
                Status::Failed | Status::Canceled => {
                    let why = run.reason.as_ref().map_or_else(|| "it did not finish".to_owned(), |reason| reason.message.clone());
                    Err(refused(format!("its update failed: {why}")))
                }
                _ => Ok(Some(run)),
            };
        }
        if row.as_ref().is_some_and(|row| row.kind == MachineKind::Added) {
            return Err(refused("it is its owner's to update".into()));
        }
        if self.store().working(name).map_err(internal)? > 0 {
            return Err(refused("it has work going, which an update would hold new runs behind".into()));
        }
        // What it takes to run this build: `local`, the cozy installed here; else this release, or a dev build's own binary.
        let m = self.show(name).await?;
        let tensord = match placed(self, &m).await {
            Some(installed) => Component::Version(installed),
            None if !dev_build() => Component::Version(own.into()),
            None if m.status.as_ref().is_some_and(|status| status.platform == format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH)) => {
                Component::File(this_build().await?)
            }
            None => return Err(refused("this cozy is a dev build of another platform than it, with no release to fetch".into())),
        };
        let ask = MachineUpdate { tensord: Some(tensord), ..Default::default() };
        let (updating, _) = self.update(&key, name, ask).await.map_err(|e| refused(format!("its update was refused: {}", e.message)))?;
        Ok(updating.run)
    }

    /// Starts the update `ask` of the machine `text` names under `key`: the same key again answers
    /// the run it made.
    pub(crate) async fn update(self: &Arc<Self>, key: &str, text: &str, ask: MachineUpdate) -> Result<(Updating, bool), Error> {
        wire::local_usable(text)?;
        if let Some(row) = self.store().keyed(key).map_err(internal)? {
            let run = self.calcifer.record(&row).map_err(|e| Refusal::from(e).0)?;
            let machine = self.show(row.machine.as_deref().unwrap_or(text)).await?;
            return Ok((Updating { machine, run: Some(run), target: row.target, warnings: vec![] }, false));
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
        let flagless = asked.is_empty() && ask.gpu_memory_limit.is_none() && ask.mode.is_none();
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
                return Err(Error::new(Kind::Unavailable, "machine_unanswered", format!("{} has not reported the software it runs", m.name))
                    .next(format!("cozy machine show {}", m.name)));
            };
            if self.dev && m.kind == MachineKind::Rental {
                // A dev home's rental moves to the open RC's builds for its platform (cl-922).
                let sums = self.rc.now().await?;
                for (member, version) in sums.behind(&status.platform, [status.runtime.as_str(), status.tensorfs.as_str(), status.version.as_str()]) {
                    let distribution = match member { "runtime" => "cozy_runtime", "tensorfs" => "tensorfs", _ => "" };
                    asked.push((member, distribution, Component::Version(version)));
                }
            } else {
                let target = self.hubs.anonymous(&hub)?.get_software().await.map_err(|e| hub_refusal(&hub, e))?;
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
            if asked.is_empty() && placed.is_none() {
                return Ok((Updating { machine: m, run: None, target: String::new(), warnings: vec![] }, true));
            }
        }
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
                    // The RC's Runtime by its version alone (`0.46.0.dev312`) too.
                    let same = |build: &api::prerelease::Build| build.version == version || build.version.split_once('+').is_some_and(|(public, _)| public == version);
                    let Some((build, url)) = sums.build(member, &platform).filter(|(build, _)| same(build)) else {
                        let has = sums.build(member, &platform).map_or_else(|| format!("no build of it for {platform}"), |(build, _)| build.version);
                        return Err(Error::new(Kind::NotFound, "rc_build_absent", format!("{member} {version} is not the open RC's build: rc-{} holds {has}", sums.rc))
                            .remedy(format!("with no version named, `cozy machine update {}` moves it to the open RC's builds", m.name)));
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
                    // A wheel version PyPI lacks is its GitHub prerelease, an exact version being an
                    // explicit ask: the machine fetches the wheel itself, held to its digest.
                    let fetched = match member {
                        "tensord" => None,
                        _ => prerelease(distribution, &version, &platform).await?,
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
        let restarts = payload.contains_key("tensord") || payload.contains_key("cozy");
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
        let mut warnings = vec![];
        // A dev build says the release it was built from, so `--tensord` naming that looks like no
        // change. It replaces the dev build, and says so.
        if let Some(status) = m.status.as_ref().filter(|status| status.dev)
            && let Some(release) = payload.get("tensord").and_then(Value::as_str).filter(|_| !inputs.iter().any(|input: &v1::InputFile| input.field == "tensord"))
        {
            warnings.push(format!("{} runs a dev build of cozy {}: `--tensord {release}` replaces it with the release; without `--tensord` it stays", m.name, status.version));
        }
        // Binary, limit and mode changes all use the ordinary drain and restart.
        let warm = m.status.as_ref().is_some_and(|s| s.warm.iter().any(|w| !w.level.is_empty()));
        if (restarts || ask.gpu_memory_limit.is_some() || ask.mode.is_some()) && warm {
            warnings.push(format!("{}'s warm models reload after it restarts", m.name));
        }
        let target = named.join(", ");
        let submit = SubmitRun {
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
        let submitted = self.calcifer.submit(key, submit).await.map_err(|e| Refusal::from(e).0)?;
        let updating = Updating { machine: m, run: Some(submitted.run), target, warnings };
        Ok((updating, !submitted.replay))
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
        let m = match self.show(name).await {
            Ok(machine) => machine,
            Err(error) if want.iter().any(|(field, _)| *field == "mode") => {
                outcome.status = "failed".into();
                outcome.reason = Some(v1::Reason {
                    code: "update_unapplied".into(),
                    message: format!("{name}'s update could not verify the requested mode: {error}"),
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

/// The release of this computer's installed cozy when `m` is `local` and runs another: the binary
/// `cozy update` renamed over its launch path, which it moves to at its next update.
async fn placed(machines: &Machines, m: &Machine) -> Option<String> {
    let running = m.status.as_ref().filter(|_| m.kind == MachineKind::Local)?.version.clone();
    let installed = machines.home.launcher()?;
    let version = tokio::task::spawn_blocking(move || says(&installed)).await.ok()??;
    (version != running).then_some(version)
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
        false => Err(Error::new(Kind::Validation, "component_refused", format!("{text:?} is not a release version"))),
    }
}

/// `distribution` `version`'s prerelease wheel for `platform` (`linux/x86_64`), when PyPI does not
/// hold that version: the GitHub prerelease (cozy-runtime `v<version>`, cozy-creator
/// `tensorfs-v<version>`) names it in its `SHA256SUMS`, the platform's own wheel first, else the
/// pure one. Its URL and sha256; `None` for a PyPI release, which the machine fetches from PyPI.
async fn prerelease(distribution: &str, version: &str, platform: &str) -> Result<Option<(String, String)>, Error> {
    let failed = |e: reqwest::Error| Error::new(Kind::Unavailable, "release_unreachable", e.to_string());
    let http = tensorfs::fetch::http(&[]).expect("the webpki roots alone").read_timeout(tensorfs::fetch::STALL).build().map_err(failed)?;
    let project = distribution.replace('_', "-");
    let pypi = http.get(format!("https://pypi.org/pypi/{project}/{version}/json")).send().await.map_err(failed)?;
    if pypi.status().is_success() {
        return Ok(None);
    }
    let base = api::prerelease::base(distribution, version);
    let sums = http.get(format!("{base}/SHA256SUMS")).send().await.map_err(failed)?;
    if !sums.status().is_success() {
        return Err(Error::new(
            Kind::NotFound,
            "release_not_found",
            format!("{project} {version} is neither on PyPI nor a GitHub prerelease ({base}/SHA256SUMS: HTTP {})", sums.status()),
        )
        .remedy("with no version named, `cozy machine update <machine>` moves it to its Hub's target"));
    }
    let sums = sums.text().await.map_err(failed)?;
    let (file, sha256) = api::prerelease::wheel(&sums, distribution, version, platform).ok_or_else(|| {
        Error::new(Kind::NotFound, "release_wheel_absent", format!("{project} {version}'s prerelease holds no wheel for {platform}"))
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
        return Err(refused(format!("{name} is not a {distribution} wheel file ({distribution}-<version>-….whl)")));
    }
    let Some(path) = crate::runs::files::held(&sha256) else {
        return Err(Error::new(Kind::NotFound, "file_not_held", format!("Calcifer holds no file {sha256} ({name})")).remedy(format!("PUT its bytes to /v1/files/{sha256} first")));
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
        "{}'s update succeeded but it runs cozy {} ({build} {}), cozy-runtime {}, tensorfs {}, mode {}, not the requested {}",
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
