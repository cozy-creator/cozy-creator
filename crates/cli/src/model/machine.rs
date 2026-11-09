//! `cozy model ls --on`, `download` and `upload`: models on machines, through Calcifer
//! (CALCIFER-API.md "Models"). Download and upload are warm runs on the machine `--on` names. The
//! machine fetches, makes and publishes the bytes itself, so no weight passes through this
//! computer; a local file goes to the machine as the run's input. A Hub commit is recorded at the
//! Hub: nothing moves.
use std::process::ExitCode;

use api::calcifer::{MachineModels, Run, Status};
use api::error::Error;
use api::calcifer::hub::{Displayed, Mirrored, ModelDownload, ModelUpload, ModelUploading, RecordedCommit};

use super::{Download, Upload};
use crate::Context;
use crate::ask::start;
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
        model: args.model.clone(),
        name: None,
        alias: args.alias,
        lane: args.lane,
        source_format: args.format,
        on: on.clone(),
        agreed: None,
    };
    crate::send::held(ctx, &mut ask.model, &mut ask.name, false).await?;
    let submitted: api::calcifer::Submitted =
        start(ctx, "/v1/model-downloads", "/v1/model-download-quotes", &on, &mut ask, |a, agreed| a.agreed = Some(agreed)).await?;
    let run = crate::run::follow(ctx, submitted.run).await?;
    made(ctx, &run);
    Ok(crate::run::exit_code(&run))
}

pub(crate) async fn upload(ctx: &Context, args: Upload) -> Result<ExitCode, Error> {
    let on = args.placement.named();
    let from_run = args.source.starts_with("run:");
    let mut ask = ModelUpload {
        hub: Some(ctx.hub().to_string()),
        source: args.source.clone(),
        name: None,
        destination: args.destination,
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
    crate::send::held(ctx, &mut ask.source, &mut ask.name, true).await?;
    let uploading: ModelUploading =
        start(ctx, "/v1/model-uploads", "/v1/model-upload-quotes", &on, &mut ask, |a, agreed| a.agreed = Some(agreed)).await?;
    if let Some(mirrored) = &uploading.mirrored {
        mirror(ctx, mirrored);
    }
    if let Some(made) = &uploading.recorded {
        recorded(ctx, made);
        return Ok(ExitCode::SUCCESS);
    }
    let run = uploading.run.ok_or_else(|| Error::internal("model_upload_answer", "Calcifer answered an upload with neither a run nor a commit"))?;
    let run = crate::run::follow(ctx, run).await?;
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
fn after(run: &Run) -> api::calcifer::hub::AfterRun {
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

/// A Hub commit recorded as a commit of another model: no machine pulled it and no byte uploaded.
fn recorded(ctx: &Context, made: &RecordedCommit) {
    made.warnings.iter().for_each(|warning| output::warn(warning));
    if ctx.json {
        output::json(made);
        return;
    }
    Record::new().field("commit", format!("{}@{}", made.model, made.commit)).field("lane", &made.lane)
        .field("from", &made.from).note("recorded at the Hub: nothing uploaded").print(ctx);
    output::nexts(ctx, &made.next);
    if let Some(shown) = &made.display {
        display(ctx, shown);
    }
}

/// The destination as a mirror of its author's model, recorded before anything uploaded.
fn mirror(ctx: &Context, mirrored: &Mirrored) {
    let by = mirrored.by.as_ref().map(|b| format!(". {b}")).unwrap_or_default();
    output::say(ctx, &format!("{} mirrors {}{by}", mirrored.model, mirrored.source));
    if let Some(handle) = &mirrored.added_handle {
        output::say(ctx, &format!("added {handle} to {}'s claim handles", mirrored.org));
    }
}

/// The models machines hold: `on` one, read from it now (`GET /v1/machines/{name}/models`), else every machine as Calcifer last saw it.
pub(crate) async fn list(ctx: &Context, on: Option<String>, words: &[String]) -> Result<ExitCode, Error> {
    let calcifer = ctx.calcifer().await?;
    let machines: Vec<MachineModels> = match &on {
        Some(on) => vec![calcifer.get(&format!("{}/models", crate::machine::route(&on.to_ascii_lowercase())?)).await?],
        None => calcifer.machines().await?.machines.iter().map(|machine| machine.models()).collect(),
    };
    let rows = machines.iter().flat_map(|machine| &machine.models).filter(|row| output::names(&row.model, words));
    if ctx.json {
        output::json(&serde_json::json!({ "models": rows.collect::<Vec<_>>() }));
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = Table::new(ctx, ["machine", "model", "release", "lane", "size", "shared"]);
    for row in rows {
        let lane = match row.precision.is_empty() {
            true => row.lane.clone(),
            false => format!("{} ({})", row.lane, row.precision),
        };
        let (size, shared) = (output::bytes(row.bytes), output::bytes(row.shared_bytes));
        table = table.row([row.machine.as_str(), &row.model, &row.release, &lane, &size, &shared]);
    }
    // A store's total counts shared content once.
    for machine in machines.iter().filter(|machine| !machine.silent) {
        // What its store holds besides them, once it is a hundredth of the store or more.
        let other = machine.stored_bytes.saturating_sub(machine.bytes);
        let other = match other > 0 && other >= machine.stored_bytes / 100 {
            true => format!(", and {} no model names (cut downloads, evicted objects still open), freed when the disk needs it", output::bytes(other)),
            false => String::new(),
        };
        table = table.note(format!("{}: {} in {} models{other}", machine.machine, output::bytes(machine.bytes), machine.repositories));
    }
    let silent: Vec<&str> = machines.iter().filter(|machine| machine.silent).map(|machine| machine.machine.as_str()).collect();
    if !silent.is_empty() {
        table = table.note(format!("no Status yet from {}", silent.join(", ")));
    }
    table.print();
    Ok(ExitCode::SUCCESS)
}
