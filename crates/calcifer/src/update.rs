//! `POST /v1/machines/{name}/update`: a machine's Runtime, TensorFS and TensorD (its `cozy`
//! binary) in one update run and one restart. Each is a release the machine fetches itself (PyPI
//! for the wheels, else their GitHub prerelease; the GitHub release for `cozy`), or a file on this
//! computer, a dev build staged with the run. With none named, each moves to its Hub's target. On
//! `local`, a `cozy` that `cozy update` installed and `local` does not run yet goes with it. The
//! run holds new runs behind it and waits for those in flight; it succeeds only when the machine
//! then runs what it asked for (`verified`).
use std::{path::Path, sync::Arc};
use sha2::Digest as _;

use axum::{
    Json,
    body::Bytes,
    extract::{Path as UrlPath, State},
    http::StatusCode,
};
use protocol::{
    calcifer::{self as wire, Component, Machine, MachineKind, MachineStatus, MachineUpdate, On, StagedInput, SubmitRun, Updating},
    error::{Error, Kind},
    ids::Digest,
    machine as v1,
};
use serde_json::{Value, json};

use crate::{
    api::{Api, IdempotencyKey, Refusal, parse},
    machines::{Machines, internal},
    rentals::hub_refusal,
    store::RunRow,
};

impl Machines {
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
        let (mut input_owners, mut original_paths) = (vec![], vec![]);
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
                Component::File(path) => path,
            };
            original_paths.push(path.clone());
            let Measured { file, input, version, bundle } = tokio::task::spawn_blocking(move || measured(&path, distribution))
                .await
                .map_err(internal)??;
            input_owners.extend(bundle);
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
        // Binary, limit and mode changes all use the ordinary drain and restart.
        let warm = m.status.as_ref().is_some_and(|s| s.warm.iter().any(|w| !w.level.is_empty()));
        if (restarts || ask.gpu_memory_limit.is_some() || ask.mode.is_some()) && warm {
            warnings.push(format!("{}'s warm models reload after it restarts", m.name));
        }
        let target = named.join(", ");
        let submit = SubmitRun {
            agreed: None,
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
            out: None,
            warnings: vec![],
        };
        // Generated bundle directories are custody, not request identity. Keep the
        // original file names plus actual packed digests stable across same-key retries.
        let digest = submission_identity(&submit, &original_paths);
        let submitted = self.calcifer.submit_with_input_owners(key, &digest, submit, input_owners)
            .await.map_err(|e| Refusal::from(e).0)?;
        let updating = Updating { machine: m, run: Some(submitted.run), target, warnings };
        Ok((updating, !submitted.replay))
    }

    /// An update run's outcome as it settles: one that succeeded while its machine does not run
    /// what it asked for fails `update_unapplied`, with its log's line about the update. A machine
    /// restarting again (for an update queued behind this one) is judged by what this update's
    /// outcome says it left; a dev build's flag is its status's alone.
    pub(crate) async fn verified(self: &Arc<Self>, row: &RunRow, mut outcome: v1::Outcome) -> v1::Outcome {
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
                    origin: "machine".into(), detail: String::new(),
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
            detail: String::new(),
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

/// The release a `cozy` binary says it is, when it runs here.
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
    let base = match distribution {
        "cozy_runtime" => format!("https://github.com/cozy-creator/cozy-runtime/releases/download/v{version}"),
        _ => format!("https://github.com/cozy-creator/cozy-creator/releases/download/tensorfs-v{version}"),
    };
    let sums = http.get(format!("{base}/SHA256SUMS")).send().await.map_err(failed)?;
    if !sums.status().is_success() {
        return Err(Error::new(
            Kind::NotFound,
            "release_not_found",
            format!("{project} {version} is neither on PyPI nor a GitHub prerelease ({base}/SHA256SUMS: HTTP {})", sums.status()),
        ));
    }
    let sums = sums.text().await.map_err(failed)?;
    let prefix = format!("{distribution}-{version}-");
    let wheels: Vec<(&str, &str)> = sums
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .map(|(sha256, file)| (sha256.trim(), file.trim().trim_start_matches('*')))
        .filter(|(sha256, file)| sha256.len() == 64 && file.starts_with(&prefix) && file.ends_with(".whl"))
        .collect();
    let (os, arch) = platform.split_once('/').unwrap_or((platform, ""));
    // A version-specific (`cp310-cp310`) wheel fits one Python only; the stable-ABI one fits all.
    let native = |file: &str| file.contains("-abi3-") && match (os, arch) {
        ("linux", arch) => file.contains("manylinux") && file.contains(arch),
        ("macos", "aarch64") => file.contains("macosx") && file.contains("arm64"),
        ("macos", arch) => file.contains("macosx") && file.contains(arch),
        ("windows", "x86_64") => file.contains("win_amd64"),
        ("windows", _) => file.contains("win_arm64"),
        _ => false,
    };
    let chosen = (wheels.iter().find(|(_, file)| native(file)))
        .or_else(|| wheels.iter().find(|(_, file)| file.ends_with("-none-any.whl")))
        .ok_or_else(|| {
            Error::new(Kind::NotFound, "release_wheel_absent", format!("{project} {version}'s prerelease holds no wheel for {platform}"))
        })?;
    Ok(Some((format!("{base}/{}", chosen.1), chosen.0.to_owned())))
}

fn submission_identity(submit: &SubmitRun, originals: &[std::path::PathBuf]) -> String {
    let mut identity = submit.clone();
    assert_eq!(identity.inputs.len(), originals.len());
    for (input, original) in identity.inputs.iter_mut().zip(originals) {
        input.path = original.clone();
    }
    format!("sha256:{}", hex::encode(sha2::Sha256::digest(
        serde_json::to_vec(&identity).expect("submission JSON"),
    )))
}

/// A local wheel of `distribution` (or, empty, a `cozy` binary), measured once: its file name, what
/// the run stages, and for a `cozy` binary the release it says it is when it runs here.
struct Measured {
    file: String,
    input: StagedInput,
    version: Option<String>,
    bundle: Option<tempfile::TempDir>,
}

fn measured(path: &Path, distribution: &str) -> Result<Measured, Error> {
    let refused = |why: String| Error::new(Kind::Validation, "wheel_refused", why);
    let file = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let regular = std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file());
    let named = file.starts_with(&format!("{distribution}-")) && file.ends_with(".whl");
    if !path.is_absolute() || !regular || (!distribution.is_empty() && !named) {
        return Err(refused(match distribution {
            "" => format!("{} is not a file named by its absolute path", path.display()),
            _ => format!("{} is not a {distribution} wheel file named by its absolute path ({distribution}-<version>-….whl)", path.display()),
        }));
    }
    // Read the original runnable binary's version, never try to execute its archive.
    let version = distribution.is_empty().then(|| says(path)).flatten();
    let mut source = path.to_path_buf();
    let mut bundle = None;
    if distribution.is_empty() {
        let failed = |error: std::io::Error| Error::new(Kind::Validation, "native_bundle_failed", error.to_string());
        let temporary = tempfile::Builder::new().prefix("cozy-native-").tempdir().map_err(failed)?;
        if let Some(packed) = platform::native_bundle::pack_binary(path, &temporary.path().join("cozy-native.tar.gz"))
            .map_err(failed)?
        {
            source = packed;
            bundle = Some(temporary);
        }
    }
    let bytes = std::fs::read(&source).map_err(|e| refused(format!("{}: {e}", source.display())))?;
    let file = source.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
    let input = StagedInput { path: source, digest: Digest::of(&bytes), length: bytes.len() as u64 };
    Ok(Measured { file, input, version, bundle })
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

/// Whether release `a` is newer than `b`, by their leading dotted numbers.
fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> { v.split('.').map_while(|p| p.parse().ok()).collect() };
    parts(a) > parts(b)
}
