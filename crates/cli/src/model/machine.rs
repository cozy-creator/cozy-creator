//! `cozy model ls --on`, `download` and `upload`: models on machines, through Calcifer
//! (CALCIFER-API.md "Models"). Download and upload are warm runs on the machine `--on` names. The
//! machine fetches, makes and publishes the bytes itself, so no weight passes through this
//! computer; a local file goes to the machine as the run's input. A Hub commit is recorded at the
//! Hub: nothing moves.
use std::process::ExitCode;

use protocol::calcifer::{Run, Status};
use protocol::error::Error;
use protocol::hub::{Displayed, Mirrored, ModelCommitAsk, ModelDownload, ModelUpload, ModelUploading, RecordedCommit};
use serde::Serialize;

use protocol::model_ref::{ModelRef, Repo};
use super::{Download, Upload};
use crate::Context;
use crate::ask::{absolute, start};
use crate::calcifer_client::path;
use crate::output::{self, Record, Table};

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
    // A composition: its base's commit, the components the source took, and what was sent.
    if let Some(commit) = model["base"]["tree"].as_str() {
        record = record.field("base", format!("{}@{commit}", model["base"]["repository"].as_str().unwrap_or_default()));
    }
    for (name, from) in model["components"].as_object().into_iter().flatten() {
        record = record.field(name, from.as_str().unwrap_or_default());
    }
    if let Some(bytes) = model["published"]["sent"]["bytes"].as_u64() {
        record = record.field("sent", output::bytes(bytes));
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
    let mut ask = ModelDownload {
        hub: Some(ctx.hub().to_string()),
        model: absolute(&args.model)?,
        alias: args.alias,
        lane: args.lane,
        source_format: args.format,
        on: on.clone(),
        agreed: None,
    };
    let submitted: protocol::calcifer::Submitted =
        start(ctx, "/v1/model-downloads", "/v1/model-download-quotes", &on, &mut ask, |a, agreed| a.agreed = Some(agreed)).await?;
    let run = crate::run::follow(ctx, submitted.run).await?;
    made(ctx, &run);
    Ok(crate::run::exit_code(&run))
}

pub(crate) async fn upload(ctx: &Context, args: Upload) -> Result<ExitCode, Error> {
    let destination = Repo::hub(&args.destination)?;
    // A Hub commit is already at the Hub: the destination records it there, and nothing moves (on
    // a base it is composed instead).
    if let (Ok(ModelRef::Commit { .. }), None) = (ModelRef::parse(&args.source), &args.base) {
        return recorded(ctx, args, &destination).await;
    }
    let on = args.placement.on()?;
    let from_run = args.source.starts_with("run:");
    let mut ask = ModelUpload {
        hub: Some(ctx.hub().to_string()),
        source: absolute(&args.source)?,
        destination: destination.to_string(),
        lane: args.lane,
        source_format: args.format,
        mirror: args.mirror,
        trained_on: args.trained_on,
        trigger: args.trigger,
        strength: args.strength,
        base: args.base,
        on: on.clone(),
        agreed: None,
    };
    let uploading: ModelUploading =
        start(ctx, "/v1/model-uploads", "/v1/model-upload-quotes", &on, &mut ask, |a, agreed| a.agreed = Some(agreed)).await?;
    if let Some(mirrored) = &uploading.mirrored {
        mirror(ctx, mirrored);
    }
    let run = crate::run::follow(ctx, uploading.run).await?;
    if !from_run {
        made(ctx, &run);
    }
    if run.status == Status::Completed
        && let Some(shown) = after(&run).display
    {
        display(ctx, &shown);
    }
    Ok(crate::run::exit_code(&run))
}

/// What Calcifer did once the run completed.
fn after(run: &Run) -> protocol::hub::AfterRun {
    (run.result.as_ref()).and_then(|r| serde_json::from_value(r["after"].clone()).ok()).unwrap_or_default()
}

/// Says the trigger and strength the Hub took, or warns that it did not: the upload stands.
fn display(ctx: &Context, shown: &Displayed) {
    if let Some(error) = &shown.error {
        output::warn(&format!("{}: the Hub did not take the trigger and strength: {error}", shown.model));
        return;
    }
    let mut said = vec![];
    if let Some(trigger) = &shown.trigger {
        said.push(format!("trigger {trigger:?}"));
    }
    if let Some(strength) = shown.strength {
        said.push(format!("recommended strength {strength}"));
    }
    output::say(ctx, &format!("{}: {}", shown.model, said.join(", ")));
}

/// A Hub commit recorded as a commit of `to`: the Hub records it when this account may read its
/// whole closure, so no machine pulls it and no byte uploads.
async fn recorded(ctx: &Context, args: Upload, to: &Repo) -> Result<ExitCode, Error> {
    let ask = ModelCommitAsk {
        hub: Some(ctx.hub().to_string()),
        from: args.source,
        lane: args.lane,
        mirror: args.mirror,
        trigger: args.trigger,
        strength: args.strength,
    };
    let at = path(&["v1", "models", &to.org, &to.name, "commits"]);
    let made: RecordedCommit = ctx.calcifer().await?.post(&at, &ask, None).await?;
    if let Some(mirrored) = &made.mirrored {
        mirror(ctx, mirrored);
    }
    if ctx.json {
        output::json(&made);
        return Ok(ExitCode::SUCCESS);
    }
    Record::new().field("commit", format!("{to}@{}", made.commit)).field("lane", &made.lane)
        .field("from", &made.from).note("recorded at the Hub: nothing uploaded").print(ctx);
    output::next(ctx, &[&format!("cozy model publish {to} <version> {}", made.commit)]);
    if let Some(shown) = &made.display {
        display(ctx, shown);
    }
    Ok(ExitCode::SUCCESS)
}

/// The destination as a mirror of its author's model, recorded before anything uploaded.
fn mirror(ctx: &Context, mirrored: &Mirrored) {
    let by = mirrored.by.as_ref().map(|b| format!(". {b}")).unwrap_or_default();
    output::say(ctx, &format!("{} mirrors {}{by}", mirrored.model, mirrored.source));
    if let Some(handle) = &mirrored.added_handle {
        output::say(ctx, &format!("added {handle} to {}'s claim handles", mirrored.org));
    }
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
