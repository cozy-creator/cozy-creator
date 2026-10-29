//! `cozy model ls --on`, `download` and `upload`: models on machines. Download and upload are warm runs on
//! the machine `--on` names. The machine fetches,
//! makes and publishes the bytes itself, so no weight passes through this computer; a local file
//! goes to the machine as the run's input.
use std::io::Read as _;
use std::path::Path;
use std::process::ExitCode;

use protocol::calcifer::{Kind as RunKind, On, Run, StagedInput, Status, SubmitRun};
use protocol::error::{Error, Kind};
use protocol::ids::Digest;
use protocol::machine::{InputFile, ModelChoice, RunKind as SpecKind, RunSpec};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use super::refs::{ModelRef, Repo};
use super::{Download, Upload};
use crate::Context;
use crate::context::hub_answer;
use crate::output::{self, Record, Table};

fn usage(code: &str, message: impl Into<String>) -> Error {
    Error::new(Kind::Usage, code, message)
}

struct ModelInput {
    choice: ModelChoice,
    staged: Vec<StagedInput>,
    files: Vec<InputFile>,
}

impl From<(ModelChoice, Option<StagedInput>)> for ModelInput {
    fn from((choice, staged): (ModelChoice, Option<StagedInput>)) -> Self {
        Self { choice, staged: staged.into_iter().collect(), files: vec![] }
    }
}

/// A plain CozyTensors folder travels as an ordinary retained input tree. The machine admits
/// its headers without importing or fetching the tensor objects a thin folder omits.
fn folder(path: &Path, profiles: &[String], manifests: &Path, memo: &Path) -> Result<ModelInput, Error> {
    if !profiles.is_empty() {
        return Err(usage("model_profile_unused", "a CozyTensors folder is already converted; omit --source-format"));
    }
    let staged = crate::run::tree::stage(path, Some(memo), manifests)?;
    let input = InputFile {
        field: "model".into(),
        digest: staged.manifest.digest.to_string(),
        length: staged.manifest.length,
        media_type: protocol::tree::MEDIA.into(),
        order: 0,
    };
    let mut files = staged.files;
    files.push(staged.manifest);
    Ok(ModelInput {
        choice: ModelChoice { parameter: "model".into(), source: "input://model".into(), ..Default::default() },
        staged: files,
        files: vec![input],
    })
}

/// The model a warm run makes present: a reference, or a local `.safetensors` file.
fn source(raw: &str, profiles: &[String]) -> Result<(ModelChoice, Option<StagedInput>), Error> {
    let (mut choice, staged) = if raw.starts_with('.') || Path::new(raw).is_absolute() {
        let staged = measure(Path::new(raw))?;
        let name = Path::new(raw).file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
        let choice = ModelChoice {
            parameter: "model".into(),
            source: format!("object://{}/{}", staged.digest, escape(&name)),
            ..Default::default()
        };
        (choice, Some(staged))
    } else {
        (ModelRef::parse(raw)?.choice("model"), None)
    };
    if !profiles.is_empty() && choice.source.is_empty() {
        return Err(usage("model_profile_unused", "--source-format converts a provider source or a local file"));
    }
    choice.profiles = profiles.to_vec();
    Ok((choice, staged))
}

/// A local model file, measured once: one regular `.safetensors` file.
fn measure(path: &Path) -> Result<StagedInput, Error> {
    let refused = |why: String| Error::new(Kind::Validation, "model_file_refused", why);
    let regular = std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file());
    if !regular || !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("safetensors")) {
        return Err(refused(format!("{} is not one regular .safetensors file", path.display())));
    }
    let mut file = std::fs::File::open(path).map_err(|e| refused(e.to_string()))?;
    let (mut hash, mut buf, mut length) = (Sha256::new(), vec![0; 1 << 20], 0u64);
    loop {
        match file.read(&mut buf).map_err(|e| refused(e.to_string()))? {
            0 => break,
            n => {
                hash.update(&buf[..n]);
                length += n as u64;
            }
        }
    }
    Ok(StagedInput {
        path: std::path::absolute(path).map_err(|e| refused(e.to_string()))?,
        digest: Digest::from_bytes(hash.finalize().into()),
        length,
    })
}

/// A file name as one URI path segment.
fn escape(name: &str) -> String {
    name.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' | b'+' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Submits one warm run of `choice` to `destination` (a `local/` alias, a Hub repository, or
/// none) and follows it; `lane` labels the commit an upload makes.
async fn warm(
    ctx: &Context,
    on: On,
    target: String,
    input: ModelInput,
    destination: String,
    lane: String,
) -> Result<ExitCode, Error> {
    let spec = RunSpec {
        kind: SpecKind::Warm,
        lane,
        models: vec![input.choice],
        inputs: input.files,
        weights_destination: destination,
        ..Default::default()
    };
    let submit = SubmitRun {
        agreed: None,
        on,
        hub: ctx.hub().clone().into(),
        kind: RunKind::Warm,
        target,
        spec,
        inputs: input.staged,
        out: None,
        warnings: vec![],
    };
    let submitted = crate::run::submit(ctx, submit, None).await?;
    let run = crate::run::follow(ctx, submitted.run).await?;
    made(ctx, &run);
    Ok(crate::run::exit_code(&run))
}

/// What the machine made (TensorD's `result.models[0]`), after `follow`'s record; `--json`
/// carries it in the run's result.
fn made(ctx: &Context, run: &Run) {
    let Some(model) = run.result.as_ref().map(|r| &r["models"][0]).filter(|m| m.is_object()) else {
        return;
    };
    if ctx.json || run.status != Status::Completed {
        return;
    }
    let text = |key: &str| model[key].as_str().filter(|s| !s.is_empty());
    let mut record = Record::new();
    for key in ["repository", "release", "lane", "precision", "checkpoint", "resolved", "format"] {
        if let Some(value) = text(key) {
            record = record.field(key, value);
        }
    }
    let published = &model["published"];
    let (destination, commit) = (published["destination"].as_str(), published["commit"].as_str());
    if let (Some(destination), Some(commit)) = (destination, commit) {
        record = record.field("commit", format!("{destination}@{commit}"));
    }
    record.print(ctx);
    if let (Some(destination), Some(commit)) = (destination, commit) {
        output::next(ctx, &[&format!("cozy model publish {destination} <version> {commit}")]);
    }
}

pub(crate) async fn download(ctx: &Context, args: Download) -> Result<ExitCode, Error> {
    let on = args.placement.on()?;
    let destination = match args.alias.as_deref().map(Repo::parse).transpose()? {
        Some(repo) if repo.org == "local" => repo.to_string(),
        Some(_) => return Err(usage("model_alias_invalid", "the name a machine keeps a model under is local/<name>")),
        None => String::new(),
    };
    let mut made = source(&args.model, &args.profiles)?;
    if made.1.is_some() && destination.is_empty() {
        return Err(usage("model_alias_missing", "a local file is kept under a name: add local/<name>"));
    }
    if let Some(lane) = args.lane {
        if made.0.repository.is_empty() || made.0.repository.starts_with("local/") {
            return Err(usage("model_lane_unused", "--lane names a lane of a Hub model"));
        }
        made.0.lane = lane;
    }
    warm(ctx, on, args.model.clone(), made.into(), destination, String::new()).await
}

pub(crate) async fn upload(ctx: &Context, args: Upload) -> Result<ExitCode, Error> {
    let destination = Repo::hub(&args.destination)?.to_string();
    if let Some(page) = &args.mirror {
        mirror(ctx, &args.source, page, &Repo::hub(&args.destination)?).await?;
    }
    if let Some(reference) = args.source.strip_prefix("run:") {
        return super::from_run::upload(ctx, reference, &destination, &args.lane).await;
    }
    // A Hub commit is already at the Hub: the destination records it there, and nothing moves.
    if let Ok(ModelRef::Commit { repo, commit }) = ModelRef::parse(&args.source) {
        return recorded(ctx, &repo, &commit, &Repo::hub(&args.destination)?, &args.lane).await;
    }
    let on = args.placement.on()?;
    // The machine publishes under this computer's sign-in; refuse before anything is made.
    ctx.hub_session(ctx.hub())?;
    let path = Path::new(&args.source);
    let made = if (args.source.starts_with('.') || path.is_absolute()) && path.is_dir() {
        folder(path, &args.profiles, &ctx.home.root().join("cache/trees"), &ctx.home.root().join("cache/digests"))?
    } else {
        source(&args.source, &args.profiles)?.into()
    };
    let target = format!("{} → {destination} ({})", args.source, args.lane);
    warm(ctx, on, target, made, destination, args.lane).await
}

/// Commit `commit` of `from` as a commit of `to` on the same Hub: the Hub records it when this
/// account may read its whole closure, so no machine pulls it and no byte uploads.
async fn recorded(ctx: &Context, from: &Repo, commit: &str, to: &Repo, lane: &str) -> Result<ExitCode, Error> {
    let client = ctx.hub_signed_in(ctx.hub())?;
    let card = hub_answer(client.get_model(&from.org, &from.name).await)?;
    let lanes = card.releases.iter().flat_map(|r| r.lanes.iter().map(|(lane, l)| (lane.as_str(), l.commit.as_str(), l.length)));
    let (held, length) = card.commits.iter().map(|c| (c.lane.as_str(), c.id.as_str(), c.length)).chain(lanes)
        .find(|(_, id, _)| *id == commit)
        .map(|(lane, _, length)| (lane.to_owned(), length))
        .ok_or_else(|| Error::new(Kind::NotFound, "commit_not_found", format!("{from} has no commit {commit} this account reads")))?;
    if held != lane {
        return Err(Error::new(Kind::Usage, "commit_lane_mismatch", format!("{from}@{commit} is lane {held}, not {lane}"))
            .next(format!("cozy model upload {from}@{commit} {to} --lane {held}")));
    }
    let made = hub_answer(client.create_model_commit(&to.org, &to.name, commit, length).await)?;
    if ctx.json {
        output::json(&serde_json::json!({"model": to.to_string(), "commit": made.id, "lane": made.lane, "from": format!("{from}@{commit}")}));
        return Ok(ExitCode::SUCCESS);
    }
    Record::new().field("commit", &format!("{to}@{}", made.id)).field("lane", &made.lane)
        .field("from", &format!("{from}@{commit}")).note("recorded at the Hub: nothing uploaded").print(ctx);
    output::next(ctx, &[&format!("cozy model publish {to} <version> {}", made.id)]);
    Ok(ExitCode::SUCCESS)
}

/// Records the destination as a mirror of its author's model before anything uploads: bare
/// `--mirror` names the hf:// or civitai:// source itself.
async fn mirror(ctx: &Context, source: &str, page: &str, repo: &Repo) -> Result<(), Error> {
    let page = match page {
        "" if source.starts_with("hf://") || source.starts_with("civitai://") => source,
        "" => {
            return Err(Error::new(Kind::Usage, "mirror_source_required", format!(
                "{source:?} is not an hf:// or civitai:// source: name the upstream page, --mirror <URL>")));
        }
        page => page,
    };
    let answer = hub_answer(ctx.hub_signed_in(ctx.hub())?.mirror_model(&repo.org, &repo.name, page).await)?;
    let org = answer.author.as_ref().map(|a| a.org.clone()).unwrap_or_else(|| repo.org.clone());
    let by = answer.author.map(|a| a.reserved.unwrap_or(format!("credits {} ({})", a.name, a.org)));
    output::say(ctx, &format!("{} mirrors {}{}", answer.model, answer.source, by.map(|b| format!(". {b}")).unwrap_or_default()));
    if let Some(handle) = answer.added_handle {
        output::say(ctx, &format!("added {handle} to {org}'s claim handles"));
    }
    Ok(())
}

/// One checkpoint a machine holds, and its repository's bytes there.
#[derive(Serialize)]
struct Held {
    machine: String,
    model: String,
    release: String,
    lane: String,
    /// Its header's precision lane (`int8`, …).
    precision: String,
    checkpoint: String,
    bytes: u64,
    /// The repository's bytes other held models share.
    shared_bytes: u64,
}

/// What every machine holds (or the one `--on` names), from its latest Status through Calcifer.
/// The models machines hold: `on` one, else every machine.
pub(crate) async fn list(ctx: &Context, on: Option<String>) -> Result<ExitCode, Error> {
    let on = match &on {
        Some(on) => Some(crate::machine::machine(ctx, on).await?.name),
        None => None,
    };
    let machines = ctx.calcifer().await?.machines().await?.machines;
    let (mut rows, mut stores, mut silent) = (vec![], vec![], vec![]);
    for machine in machines.iter().filter(|m| on.as_ref().is_none_or(|on| *on == m.name)) {
        let Some(status) = &machine.status else {
            silent.push(machine.name.as_str());
            continue;
        };
        stores.push(format!("{}: {} in {} models", machine.name, output::bytes(status.models_bytes), status.models.len()));
        for model in &status.models {
            let held = |release: &str, lane: (&str, &str), checkpoint: &str| Held {
                machine: machine.name.clone(),
                model: model.repository.clone(),
                release: release.into(),
                lane: lane.0.into(),
                precision: lane.1.into(),
                checkpoint: checkpoint.into(),
                bytes: model.total_bytes,
                shared_bytes: model.total_bytes.saturating_sub(model.unique_bytes),
            };
            if model.checkpoints.is_empty() {
                rows.push(held("", ("", ""), ""));
            }
            rows.extend(model.checkpoints.iter().map(|c| held(&c.release, (&c.lane, &c.precision), &c.manifest)));
        }
    }
    if ctx.json {
        output::json(&serde_json::json!({ "models": rows }));
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = Table::new(ctx, ["machine", "model", "release", "lane", "size", "shared"]);
    for row in &rows {
        let lane = match row.precision.is_empty() {
            true => row.lane.clone(),
            false => format!("{} ({})", row.lane, row.precision),
        };
        let (size, shared) = (output::bytes(row.bytes), output::bytes(row.shared_bytes));
        table = table.row([row.machine.as_str(), &row.model, &row.release, &lane, &size, &shared]);
    }
    // A store's total counts shared content once.
    for store in stores {
        table = table.note(store);
    }
    if !silent.is_empty() {
        table = table.note(format!("no Status yet from {}", silent.join(", ")));
    }
    table.print();
    Ok(ExitCode::SUCCESS)
}
