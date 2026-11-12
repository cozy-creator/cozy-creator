//! `cozy machine end`, `keepalive` and `update`: asks Calcifer makes of one machine.
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use api::{
    calcifer::{self, Component, Ended, Keepalive, Machine, MachineKind, MachineList, MachineState, MachineStatus, MachineUpdate, Updating},
    error::{Error, Kind},
    ids::Digest,
};

use super::{Following, clock, machine, now_ms, route, usd};
use crate::{Context, calcifer_client::Client, output};

/// Ends a rental, returning once its Hub affirms the pod gone, or removes an added machine
/// (`DELETE /v1/machines/{name}`). Calcifer carries the end out once the outputs of its runs have
/// landed here, unless `force`; this follows it on the machine's stream. Ctrl-C stops following,
/// never the end. Anything short of `ended` is an error: nothing says a pod is gone that its Hub
/// has not affirmed gone.
pub(crate) async fn end(ctx: &Context, name: &str, force: bool) -> Result<ExitCode, Error> {
    let calcifer = ctx.calcifer().await?;
    let path = route(name)?;
    let asked = match force {
        true => format!("{path}?force=true"),
        false => path.clone(),
    };
    let Ended {
        machine,
        changed,
        abandoned,
    } = calcifer.delete(&asked).await?;
    if !abandoned.is_empty() {
        output::warn(&format!(
            "Gave up outputs that had not landed: {}",
            abandoned.join(", ")
        ));
    }
    // An order the Hub has not answered: stopped now, whatever becomes of the Hub.
    let order = machine.kind == MachineKind::Rental && machine.rental_id.is_none();
    let machine = match order {
        true => machine,
        false => confirmed(ctx, &calcifer, machine).await?,
    };
    if ctx.json {
        output::json(&Ended {
            machine,
            changed,
            abandoned,
        });
        return Ok(ExitCode::SUCCESS);
    }
    let said = match (machine.kind, changed) {
        _ if order => "The order is stopped: it buys nothing more. Once the Hub answers, Calcifer ends anything it placed; `cozy machine ls` shows it until then.".to_owned(),
        (MachineKind::Added, true) => format!(
            "{} is no longer one of this computer's machines.",
            machine.name
        ),
        (MachineKind::Added, false) => format!("{} was already removed.", machine.name),
        (_, true) => format!(
            "{} shut down. Temporary pod files are gone. Local outputs and uploaded checkpoints remain.",
            machine.called()
        ),
        (_, false) => format!("{} is already shut down.", machine.called()),
    };
    println!("{said}");
    if let Ok(list) = calcifer.get::<MachineList>("/v1/machines").await {
        let live = &list.rentals;
        println!("Rentals running: {} · {}/hour now", live.count, usd(live.usd_micros_per_hour));
    }
    output::next(ctx, &["cozy machine ls"]);
    Ok(ExitCode::SUCCESS)
}

/// The machine an end was asked of, once it has ended: followed while it ends, and an error for
/// anything else (its end failed, or it is not ending), naming why.
pub(crate) async fn confirmed(ctx: &Context, calcifer: &Client, mut machine: Machine) -> Result<Machine, Error> {
    if machine.state == MachineState::Ending {
        // Followed by its rental id: the name is the Hub's and an alias may be what was typed.
        let id = machine
            .rental_id
            .clone()
            .unwrap_or_else(|| machine.name.clone());
        let (following, now) = Following::open(calcifer, &id).await?;
        machine = ended(ctx, calcifer, following, now, &route(&id)?).await?;
    }
    match machine.state {
        MachineState::Ended => Ok(machine),
        _ => Err(not_ended(&machine)),
    }
}

/// An end that ended nothing: Calcifer's reason when its end failed (this computer's sign-in, the
/// Hub's refusal), else the state it is in.
fn not_ended(m: &Machine) -> Error {
    let (name, handle) = (m.called(), m.handle());
    let why = match &m.end_failed {
        Some(why) => why.clone(),
        None => format!("it is {}, not ended", super::list::state_cell(m)),
    };
    let billing = match m.kind {
        MachineKind::Rental => "Nothing was ended, and it may still be billing",
        _ => "Nothing was ended",
    };
    Error::new(Kind::Conflict, "end_failed", format!("{name} was not shut down: {why}"))
        .remedy(billing)
        .next(format!("cozy machine end {handle}"))
}

/// Follows an ending machine while it ends: while its end waits for its runs' outputs to land,
/// what it waits for; then its release, until its Hub affirms the pod gone or the end fails.
async fn ended(
    ctx: &Context,
    calcifer: &Client,
    mut following: Following,
    mut machine: Machine,
    path: &str,
) -> Result<Machine, Error> {
    let (mut waited, mut releasing) = (String::new(), false);
    while machine.state == MachineState::Ending {
        match &machine.ending {
            Some(waits) if *waits != waited => {
                output::say(ctx, &format!("Waiting to end {}: {waits}", machine.called()));
                waited.clone_from(waits);
            }
            None if !releasing => {
                output::say(ctx, &format!("Shutting down {}…", machine.called()));
                releasing = true;
            }
            _ => {}
        }
        tokio::select! {
            next = following.next(calcifer) => machine = match next? {
                Some(next) => next,
                // Left the list: as Calcifer records it now. An added machine, which has no
                // rental id, is named no more once removed.
                None => match calcifer.get(path).await {
                    Err(_) if machine.kind == MachineKind::Added => Machine { state: MachineState::Ended, ..machine },
                    now => now?,
                },
            },
            _ = tokio::signal::ctrl_c() => {
                let (name, handle, waits) = (machine.called(), machine.handle(), machine.ending.is_some());
                return Err(match waits {
                    true => Error::new(
                        Kind::Unavailable,
                        "end_waiting",
                        format!("Stopped watching; {name} will shut down after its outputs are saved"),
                    )
                    .remedy("To stop immediately and discard unsaved outputs, use --force")
                    .next(format!("cozy machine end {handle} --force")),
                    false => Error::new(
                        Kind::Unavailable,
                        "end_unconfirmed",
                        format!("Stopped watching; {name} is still shutting down"),
                    )
                    .next(format!("cozy machine show {handle}")),
                });
            }
        }
    }
    Ok(machine)
}

/// Resets a rental's idle clock once; it answers only after the machine acknowledged.
pub(crate) async fn keepalive(ctx: &Context, name: &str) -> Result<ExitCode, Error> {
    let path = format!("{}/keepalive", route(name)?);
    let answer: Keepalive = ctx
        .calcifer()
        .await?
        .post(&path, &serde_json::json!({}), None)
        .await?;
    if ctx.json {
        output::json(&answer);
        return Ok(ExitCode::SUCCESS);
    }
    let ends = match answer.idle_deadline_ms {
        Some(at) => format!("in {} unless work arrives", clock(at - now_ms())),
        None => "after its jobs".into(),
    };
    output::Record::new()
        .field("machine", name)
        .field("ends", ends)
        .print(ctx);
    Ok(ExitCode::SUCCESS)
}

/// Updates a machine's Runtime, TensorFS and TensorD (its `cozy` binary) in one run and one
/// restart (`POST /v1/machines/{name}/update`): each named as a release version, or a local path,
/// a dev build sent from here (a wheel or a project to build here; a `cozy` binary or the
/// cozy-creator checkout to build here). With none named, each moves to its Hub's target. The
/// update is a numbered run, followed like any run; Calcifer fails it unless the machine then
/// runs what it asked for.
pub(crate) async fn update(
    ctx: &Context,
    name: &str,
    runtime: Option<String>,
    tensorfs: Option<String>,
    tensord: Option<String>,
    gpu_memory_limit: Option<u64>,
    mode: Option<api::config::MachineMode>,
) -> Result<ExitCode, Error> {
    api::calcifer::local_usable(name)?;
    let ask = MachineUpdate {
        runtime: component(ctx, runtime, "cozy_runtime").await?,
        tensorfs: component(ctx, tensorfs, "tensorfs").await?,
        tensord: component(ctx, tensord, "").await?,
        gpu_memory_limit,
        mode,
    };
    let dev = matches!(ask.tensord, Some(Component::File(_)));
    let named = ask.tensord.is_some();
    let path = format!("{}/update", route(name)?);
    let calcifer = ctx.calcifer().await?;
    let Updating { machine: m, run, warnings, .. } = calcifer.post(&path, &ask, Some(&mint_key())).await?;
    let was = m.status.as_ref().map(|status| status.commit.clone());
    for warning in &warnings {
        output::warn(warning);
    }
    let Some(run) = run else {
        return up_to_date(ctx, &m, &m.status.clone().unwrap_or_default());
    };
    let run = crate::run::follow(ctx, run).await?;
    if run.status != calcifer::Status::Completed {
        return Ok(crate::run::exit_code(&run));
    }
    // What the update left, which Calcifer held it to: the machine's word, unless it is
    // restarting again.
    let now = machine(ctx, &m.name).await?;
    let status = match (&now.unreachable, run.result.as_ref().map(|r| &r["to"])) {
        (Some(_), Some(to)) => MachineStatus {
            version: to["cozy"].as_str().unwrap_or_default().into(),
            runtime: to["runtime"].as_str().unwrap_or_default().into(),
            tensorfs: to["tensorfs"].as_str().unwrap_or_default().into(),
            ..now.status.clone().unwrap_or_default()
        },
        _ => now.status.clone().unwrap_or_default(),
    };
    updated(ctx, &m.name, &status);
    // `local` runs the installed cozy, which just moved: Calcifer runs it too, so the CLI and
    // Calcifer stay one build.
    if named && m.kind == MachineKind::Local && (dev || was.as_ref() != Some(&status.commit)) {
        crate::daemon::restart(ctx).await?;
    }
    if dev && m.kind == MachineKind::Local {
        output::warn(&format!(
            "this computer's cozy is now {}, a dev build; `cozy update` returns it to a release",
            status.version
        ));
    }
    Ok(crate::run::exit_code(&run))
}

/// Asks `local` to move to cozy `version`, the release `cozy update` just installed, once its
/// runs in flight end: an update naming it, submitted and not followed. Its number; none when
/// `local` runs it already.
pub(crate) async fn activate(ctx: &Context, version: &str) -> Result<Option<u64>, Error> {
    let m = machine(ctx, "local").await?;
    if m.status.as_ref().is_some_and(|s| s.version == version) {
        return Ok(None);
    }
    let ask = MachineUpdate {
        tensord: Some(Component::Version(version.into())),
        ..Default::default()
    };
    let path = format!("{}/update", route("local")?);
    let updating: Updating = ctx.calcifer().await?.post(&path, &ask, Some(&mint_key())).await?;
    Ok(updating.run.map(|run| run.number))
}

/// What `cozy machine update` installs of one component: a release version, or a file here, built
/// here first when the path is a project to build, and sent by content. A `cozy` binary says its release.
async fn component(ctx: &Context, value: Option<String>, distribution: &str) -> Result<Option<Component>, Error> {
    let Some(value) = value else { return Ok(None) };
    let path = Path::new(&value);
    let file = match () {
        _ if path.is_file() => path.to_path_buf(),
        _ if path.is_dir() => build(path, distribution)?,
        _ => return Ok(Some(Component::Version(value))),
    };
    let name = file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let version = distribution.is_empty().then(|| says(&file)).flatten();
    let sha256 = crate::send::Sender::new(ctx).await?.file(&file).await?;
    Ok(Some(Component::File(api::calcifer::DevBuild { sha256, name, version })))
}

/// The release a `cozy` binary says it is, when it runs here.
fn says(binary: &Path) -> Option<String> {
    let out = std::process::Command::new(binary).arg("--version").stdin(std::process::Stdio::null()).output().ok()?;
    let said = String::from_utf8_lossy(&out.stdout);
    let version = said.trim().strip_prefix("cozy ")?;
    (out.status.success() && !version.is_empty()).then(|| version.to_owned())
}

fn mint_key() -> String {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("the OS has randomness");
    format!("update-{}", hex::encode(bytes))
}

/// Builds the project at `dir`: a Runtime or TensorFS wheel (`uv build`), else the `cozy` binary
/// of a cozy-creator checkout (`cargo build --release`). The built file.
fn build(dir: &Path, distribution: &str) -> Result<PathBuf, Error> {
    let failed = |why: String| Error::new(Kind::Validation, "build_failed", format!("{}: {why}", dir.display()));
    let out = dir.join("target").join("cozy-update");
    let status = match distribution {
        "" => std::process::Command::new("cargo")
            .args(["build", "--release", "--bin", "cozy"])
            .current_dir(dir)
            .status(),
        _ => std::process::Command::new("uv")
            .args(["build", "--wheel", "--out-dir"])
            .arg(&out)
            .arg(dir)
            .status(),
    };
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => return Err(failed(format!("the build exited {status}"))),
        Err(e) => return Err(failed(e.to_string())),
    }
    if distribution.is_empty() {
        return std::path::absolute(dir.join("target/release").join(if cfg!(windows) { "cozy.exe" } else { "cozy" }))
            .map_err(|e| failed(e.to_string()));
    }
    let newest = std::fs::read_dir(&out)
        .map_err(|e| failed(e.to_string()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&format!("{distribution}-")) && n.ends_with(".whl")))
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok());
    let built = newest.ok_or_else(|| failed(format!("the build left no {distribution} wheel in {}", out.display())))?;
    stamp(&built, &local_version(dir, &built)).map_err(|e| failed(format!("stamping {}: {e}", built.display())))
}

/// A dev build's PEP 440 local version: `dev.g<commit>`, `.dirty` with uncommitted changes; a tree
/// git does not know is named by the built wheel's digest.
fn local_version(dir: &Path, wheel: &Path) -> String {
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    match git(&["rev-parse", "--short=12", "HEAD"]) {
        Some(commit) => {
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
            format!("dev.g{commit}{}", if dirty { ".dirty" } else { "" })
        }
        None => {
            let digest = std::fs::read(wheel).map(|bytes| Digest::of(&bytes).to_string()).unwrap_or_default();
            format!("dev.{}", digest.trim_start_matches("sha256:").get(..12).unwrap_or("0"))
        }
    }
}

/// `wheel` restamped with `local` as its version's local part (`0.31.0+dev.g<commit>`): its file
/// name, `.dist-info` (and `.data`) folder, METADATA and RECORD, so the machine reports which
/// build it runs. The original is removed.
fn stamp(wheel: &Path, local: &str) -> std::io::Result<PathBuf> {
    use base64::Engine as _;
    use sha2::Digest as _;
    use std::io::{Read as _, Write as _};
    let invalid = |why: String| std::io::Error::new(std::io::ErrorKind::InvalidData, why);
    let file = wheel.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let (distribution, rest) = file.strip_suffix(".whl").and_then(|s| s.split_once('-')).ok_or_else(|| invalid(format!("{file} is not a wheel name")))?;
    let (version, tags) = rest.split_once('-').ok_or_else(|| invalid(format!("{file} is not a wheel name")))?;
    let stamped = match version.contains('+') {
        true => format!("{version}.{local}"),
        false => format!("{version}+{local}"),
    };
    let (info, data) = (format!("{distribution}-{version}.dist-info/"), format!("{distribution}-{version}.data/"));
    let new_info = format!("{distribution}-{stamped}.dist-info/");
    let renamed = |name: &str| match (name.strip_prefix(&info), name.strip_prefix(&data)) {
        (Some(rest), _) => format!("{new_info}{rest}"),
        (_, Some(rest)) => format!("{distribution}-{stamped}.data/{rest}"),
        _ => name.to_owned(),
    };
    let mut archive = zip::ZipArchive::new(std::fs::File::open(wheel)?).map_err(std::io::Error::other)?;
    let mut read = |name: &str| -> std::io::Result<String> {
        let mut text = String::new();
        archive.by_name(name).map_err(|_| invalid(format!("{file} has no {name}")))?.read_to_string(&mut text)?;
        Ok(text)
    };
    let (metadata, record) = (read(&format!("{info}METADATA"))?, read(&format!("{info}RECORD"))?);
    // The header's Version line only, never one in the description below it.
    let mut replaced = false;
    let metadata: String = metadata
        .split_inclusive('\n')
        .map(|line| match !replaced && line.starts_with("Version:") {
            true => {
                replaced = true;
                format!("Version: {stamped}\n")
            }
            false => line.to_owned(),
        })
        .collect();
    let hash = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(metadata.as_bytes()));
    let record: String = record
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let path = line.rsplitn(3, ',').nth(2).unwrap_or(line);
            match path.strip_prefix(&info) {
                Some("METADATA") => format!("{new_info}METADATA,sha256={hash},{}\n", metadata.len()),
                Some("RECORD") => format!("{new_info}RECORD,,\n"),
                _ => format!("{}{}\n", renamed(path), &line[path.len()..]),
            }
        })
        .collect();
    let out = wheel.with_file_name(format!("{distribution}-{stamped}-{tags}.whl"));
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&out)?);
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i).map_err(std::io::Error::other)?;
        let name = entry.name().map_err(std::io::Error::other)?.into_owned();
        if name != format!("{info}METADATA") && name != format!("{info}RECORD") {
            writer.raw_copy_file_rename(entry, renamed(&name)).map_err(std::io::Error::other)?;
        }
    }
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file(format!("{new_info}METADATA"), options).map_err(std::io::Error::other)?;
    writer.write_all(metadata.as_bytes())?;
    writer.start_file(format!("{new_info}RECORD"), options).map_err(std::io::Error::other)?;
    writer.write_all(record.as_bytes())?;
    writer.finish().map_err(std::io::Error::other)?;
    std::fs::remove_file(wheel)?;
    Ok(out)
}

/// The machine's three components after an update.
fn updated(ctx: &Context, name: &str, s: &MachineStatus) {
    if ctx.json {
        output::json(&serde_json::json!({"machine": name, "tensord": s.version, "runtime": s.runtime, "tensorfs": s.tensorfs, "mode": s.mode}));
    } else {
        let build = if s.dev { " (dev build)" } else { "" };
        output::say(ctx, &format!("{name} runs TensorD (cozy) {}{build}, Runtime {}, TensorFS {}, mode {}", s.version, s.runtime, s.tensorfs,
            if s.mode.is_empty() { "unknown" } else { &s.mode }));
    }
}

fn up_to_date(ctx: &Context, m: &Machine, s: &MachineStatus) -> Result<ExitCode, Error> {
    if ctx.json {
        output::json(&serde_json::json!({"machine": m.name, "changed": false, "tensord": s.version, "runtime": s.runtime, "tensorfs": s.tensorfs, "mode": s.mode}));
    } else {
        output::say(ctx, &format!(
            "{} runs TensorD (cozy) {}, Runtime {}, TensorFS {}: the Hub's target, nothing to update",
            m.name, s.version, s.runtime, s.tensorfs
        ));
    }
    Ok(ExitCode::SUCCESS)
}
