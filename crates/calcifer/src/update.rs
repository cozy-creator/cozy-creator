//! `POST /v1/machines/{name}/update`: a machine's Runtime, TensorFS and TensorD (its `cozy`
//! binary) in one update run and one restart. Each is a release the machine fetches itself (PyPI
//! for the wheels, else their GitHub prerelease; the GitHub release for `cozy`), or a file on this
//! computer, a dev build staged with the run. With none named, each moves to its Hub's target. On
//! `local`, a `cozy` that `cozy update` installed and `local` does not run yet goes with it. The
//! run holds new runs behind it and waits for those in flight; it succeeds only when the machine
//! then runs what it asked for (`verified`). A machine too old for what is asked of it is brought up
//! here too (`brought_up`), never while it has work: an update waits for measured idleness on the machine.
use std::{path::Path, sync::Arc};

use axum::{
    Json,
    body::Bytes,
    extract::{Path as UrlPath, State},
    http::StatusCode,
};
use protocol::{
    calcifer::{self as wire, Component, DevBuild, Machine, MachineKind, MachineStatus, MachineUpdate, On, Reason, Run, StagedInput, Status, SubmitRun, Updating, Warning},
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

/// Whether its TensorD says it meets a package's Runtime floor in that package's own environment (`ENV_RUNTIME`): one that does
/// refuses a floor for the package's sake, never its own age.
fn meets_floors(status: Option<&MachineStatus>) -> bool {
    status.is_some_and(|status| status.capabilities.iter().any(|c| c == v1::ENV_RUNTIME))
}

/// The warning on the update run that brings a machine up, which is also said to whoever waits on it.
pub(crate) const BRINGING_UP: &str = "machine_brought_up";

/// How far bringing a machine up has got.
pub(crate) enum BroughtUp {
    Going(Box<Run>),
    Done,
}

/// The release this cozy is, or a dev build was built from: what a machine can fetch.
pub(crate) fn this_release() -> &'static str {
    env!("CARGO_PKG_VERSION").split('+').next().unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// The command that moves `name` to this cozy.
pub(crate) fn bring_up_command(name: &str) -> String {
    format!("cozy machine update {name} --tensord {}", this_release())
}

impl Machines {
    /// What a run failing with `reason` on machine `name` comes to, when that is its being older than this Calcifer: its update to
    /// this cozy, going or done, the same one for everything that asks, said in one line. None: it isn't that, and the failure stands.
    /// A machine this computer's user doesn't own, a dev build, or one with work here isn't updated unasked: the refusal is the command.
    pub(crate) async fn brought_up(self: &Arc<Self>, name: &str, reason: &Reason) -> Result<Option<BroughtUp>, Error> {
        // A floor refused typed, by a machine that says it can't meet floors itself: its failure's words are never read.
        if reason.code != "runtime_floor_unmet" {
            return Ok(None);
        }
        let m = self.show(name).await?;
        let own = this_release();
        let key = format!("bring-up-{}-{own}", m.rental_id.as_deref().unwrap_or(name));
        // Once it has been asked, what it came to is the answer, though the machine is no longer older.
        let fresh = self.store().keyed(&key).map_err(internal)?.is_none();
        let running = m.status.as_ref().map_or("", |status| status.version.as_str());
        if fresh && (meets_floors(m.status.as_ref()) || !newer(own, running)) {
            return Ok(None);
        }
        // Its outcome's words already say the command; `next` does here.
        let refused = reason.message.split(&format!("; {name} runs cozy")).next().unwrap_or(&reason.message);
        let by_hand = |why: String| {
            Error::new(Kind::Conflict, "machine_too_old", format!("{refused}; {name} runs cozy {running}, and {why}"))
                .cause(reason.code.clone())
                .next(bring_up_command(name))
        };
        if fresh {
            if m.kind == MachineKind::Added {
                return Err(by_hand("it is its owner's to update".into()));
            }
            if m.status.as_ref().is_some_and(|status| status.dev || status.version.contains("+dev")) {
                return Err(by_hand("it runs a dev build, which nothing replaces unasked".into()));
            }
            let load = self.store().machine_load().map_err(internal)?;
            if load.get(name).is_some_and(|(running, queued)| running + queued > 0) {
                return Err(by_hand("it has work going, which an update would hold new runs behind".into()));
            }
        }
        let ask = MachineUpdate { tensord: Some(Component::Version(own.into())), ..Default::default() };
        let (updating, _) = self.update(&key, name, ask).await.map_err(|e| by_hand(format!("updating it was refused: {}", e.message)))?;
        let Some(mut run) = updating.run else { return Ok(Some(BroughtUp::Done)) };
        if fresh {
            let warning = Warning {
                code: BRINGING_UP.into(),
                message: format!("{name} runs cozy {running}, too old for this: updating it to {own}, then going on"),
            };
            self.calcifer.warn(&run.id, &warning).map_err(|e| Refusal::from(e).0)?;
            run.warnings.push(warning);
        }
        match run.status {
            Status::Completed => Ok(Some(BroughtUp::Done)),
            Status::Failed | Status::Canceled => {
                let why = run.reason.as_ref().map_or_else(|| "its update did not finish".to_owned(), |reason| reason.message.clone());
                Err(by_hand(format!("updating it failed: {why}")))
            }
            _ => Ok(Some(BroughtUp::Going(Box::new(run)))),
        }
    }

    /// Starts the update `ask` of the machine `text` names under `key`: the same key again answers
    /// the run it made.
    pub async fn update(self: &Arc<Self>, key: &str, text: &str, ask: MachineUpdate) -> Result<(Updating, bool), Error> {
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
            if asked.is_empty() && placed.is_none() {
                return Ok((Updating { machine: m, run: None, target: String::new(), warnings: vec![] }, true));
            }
        }
        let platform = m.status.as_ref().map(|s| s.platform.clone()).unwrap_or_default();
        let (mut payload, mut inputs, mut staged, mut named) = (serde_json::Map::new(), vec![], vec![], vec![]);
        for (member, distribution, component) in asked {
            let path = match component {
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
                            named.push(format!("{member} {version} (prerelease)"));
                            payload.insert(member.into(), json!({ "url": url, "sha256": sha256 }));
                        }
                        None => {
                            named.push(format!("{member} {version}"));
                            payload.insert(member.into(), version.into());
                        }
                    }
                    continue;
                }
                Component::File(build) => build,
            };
            let (file, input, version) = measured(&self.calcifer, path, distribution)?;
            named.push(format!("{member} {} (dev)", version.as_deref().unwrap_or(&file)));
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
        // A machine can't know what it's called here, and a refusal for its age says what ends it, wherever the run was headed.
        if let (Some(name), Some(reason)) = (row.machine.as_deref(), outcome.reason.as_mut()) {
            reason.message = reason.message.replace("<machine>", name);
            if reason.next.is_empty() {
                reason.next = commands(&reason.message);
            }
            if reason.code == "does_not_fit" && reason.next.is_empty() {
                reason.next.push(format!("cozy run {} … --on new", row.target));
            }
            if reason.code == "runtime_floor_unmet"
                && let Ok(m) = self.show(name).await
                && let Some(status) = m.status.as_ref().filter(|status| newer(this_release(), &status.version) && !meets_floors(Some(*status)))
            {
                reason.message += &format!("; {name} runs cozy {}: `{}` brings it up", status.version, bring_up_command(name));
                reason.next.insert(0, bring_up_command(name));
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

/// A release's first run on a machine older than this Calcifer that doesn't meet floors itself (`ENV_RUNTIME`), which may refuse what a
/// newer one wouldn't: the install goes first, as local code's does, so a refusal stops short of the run and the machine can be brought
/// up. The run or update going, if any.
pub(crate) async fn release_first(api: &Arc<crate::api::Api>, key: &str, name: &str, hub: &str, package: &str, release: &str) -> Result<Option<Run>, Error> {
    let Ok(m) = api.machines.show(name).await else { return Ok(None) };
    let Some(status) = m.status.as_ref().filter(|status| newer(this_release(), &status.version) && !meets_floors(Some(*status))) else { return Ok(None) };
    if status.environments.iter().any(|env| env.package == package && env.release == release) {
        return Ok(None);
    }
    let submit = || crate::hub::release_install(hub, On::Machine(name.to_owned()), package, release, None);
    Ok(match crate::local_code::install_through(api, &format!("{key}/install"), submit).await? {
        crate::local_code::Install::Going(run) => Some(run),
        // The run says for itself why it can't go.
        crate::local_code::Install::Ended(_) => None,
    })
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
    let http = reqwest::Client::builder()
        .user_agent(concat!("cozy/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(failed)?;
    let project = distribution.replace('_', "-");
    let pypi = http.get(format!("https://pypi.org/pypi/{project}/{version}/json")).send().await.map_err(failed)?;
    if pypi.status().is_success() {
        return Ok(None);
    }
    let base = protocol::prerelease::base(distribution, version);
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
    let (file, sha256) = protocol::prerelease::wheel(&sums, distribution, version, platform).ok_or_else(|| {
        Error::new(Kind::NotFound, "release_wheel_absent", format!("{project} {version}'s prerelease holds no wheel for {platform}"))
    })?;
    Ok(Some((format!("{base}/{file}"), sha256.to_owned())))
}

/// A held dev build of `distribution` (or, empty, a `cozy` binary): its file name, what the run stages, and for a `cozy` binary the
/// release its client says it is.
fn measured(calcifer: &crate::Calcifer, build: DevBuild, distribution: &str) -> Result<(String, StagedInput, Option<String>), Error> {
    let refused = |why: String| Error::new(Kind::Validation, "wheel_refused", why);
    let DevBuild { sha256, name, version } = build;
    let named = name.starts_with(&format!("{distribution}-")) && name.ends_with(".whl") && !name.contains(['/', '\\']);
    if !distribution.is_empty() && !named {
        return Err(refused(format!("{name} is not a {distribution} wheel file ({distribution}-<version>-….whl)")));
    }
    let Some(path) = crate::runs::files::held(calcifer, &sha256) else {
        return Err(Error::new(Kind::NotFound, "file_not_held", format!("Calcifer holds no file {sha256} ({name})")).remedy(format!("PUT its bytes to /v1/files/{sha256} first")));
    };
    crate::runs::files::touch(&path);
    let length = std::fs::metadata(&path).map_err(|e| refused(format!("{name}: {e}")))?.len();
    let version = version.filter(|_| distribution.is_empty());
    Ok((name, StagedInput { path, digest: sha256, length }, version))
}

/// What a machine that carried out update `spec` reports: each wheel's version (a staged one's
/// from its file name, a fetched one's from its URL's), and its `cozy`'s release, or `dev` for a
/// dev binary that did not say its release here.
fn wanted(spec: &v1::RunSpec) -> Vec<(&'static str, Value)> {
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
