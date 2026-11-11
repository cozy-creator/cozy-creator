//! `cozy model ls --on`, `download` and `upload`: models on machines, through Calcifer
//! (CALCIFER-API.md "Models"). Download and upload are warm runs on the machine `--on` names. The
//! machine fetches, makes and publishes the bytes itself, so no weight passes through this
//! computer; a local file goes to the machine as the run's input. A Hub commit is recorded at the
//! Hub: nothing moves.
use std::process::ExitCode;

use api::calcifer::{Kind as RunKind, MachineModels, Run, Status};
use api::error::Error;
use api::calcifer::hub::{Displayed, ModelDownload, ModelUpload, RecordedCommit, job};

use super::{Download, Upload};
use crate::Context;
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
    let ask = ModelDownload { model: args.model.clone(), name: None, alias: args.alias, lane: args.lane, source_format: args.format };
    download_to(ctx, ask, on).await
}

pub(super) async fn download_to(ctx: &Context, mut ask: ModelDownload, on: Option<api::calcifer::On>) -> Result<ExitCode, Error> {
    crate::send::held(ctx, &mut ask.model, &mut ask.name, false).await?;
    let run = crate::run::job(ctx, job::MODEL_DOWNLOAD, &ask, on, None).await?;
    let run = crate::run::follow(ctx, run).await?;
    made(ctx, &run);
    Ok(crate::run::exit_code(&run))
}

pub(super) async fn remove(ctx: &Context, model: &str) -> Result<ExitCode, Error> {
    let removed = remove_local(ctx, model).await?;
    print_removed(ctx, &removed);
    Ok(ExitCode::SUCCESS)
}

pub(super) async fn remove_local(ctx: &Context, model: &str) -> Result<api::calcifer::RemovedModel, Error> {
    let repo = api::model_ref::Repo::parse(model)?;
    let key = crate::run::mint_key();
    let removed: api::calcifer::RemovedModel = ctx.calcifer().await?.post(
        "/v1/machines/local/models/remove", &api::calcifer::RemoveModel { model: repo.to_string() }, Some(&key),
    ).await?;
    Ok(removed)
}

fn print_removed(ctx: &Context, removed: &api::calcifer::RemovedModel) {
    let model = &removed.model;
    if ctx.json {
        output::json(&removed);
    } else if removed.removed {
        anstream::println!("Removed local copy of {}. Cache bytes removed: {}.", model, output::bytes(removed.cache_bytes_removed));
        if removed.retained_bytes > 0 {
            anstream::println!("{} retained because the data is in use or shared.", output::bytes(removed.retained_bytes));
        }
    } else {
        anstream::println!("No local copy of {model} was found.");
    }
}

pub(crate) async fn upload(ctx: &Context, args: Upload) -> Result<ExitCode, Error> {
    let on = args.placement.on()?;
    let from_run = args.source.starts_with("run:");
    let mut ask = ModelUpload {
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
    };
    crate::send::held(ctx, &mut ask.source, &mut ask.name, true).await?;
    let run = crate::run::job(ctx, job::MODEL_UPLOAD, &ask, on, None).await?;
    // A Hub commit is recorded there by Calcifer: no machine ran.
    if run.kind == RunKind::Here {
        recorded(ctx, &crate::run::done(ctx, run).await?);
        return Ok(ExitCode::SUCCESS);
    }
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

/// The models machines hold: `on` one, read from it now (`GET /v1/machines/{name}/models`), else every machine as Calcifer last saw it.
pub(crate) async fn list(ctx: &Context, on: Option<String>, words: &[String], role: &str, limit: u32, offset: u32) -> Result<ExitCode, Error> {
    let calcifer = ctx.calcifer().await?;
    let machines: Vec<MachineModels> = match &on {
        Some(on) => vec![calcifer.get(&format!("{}/models", crate::machine::route(&on.to_ascii_lowercase())?)).await?],
        None => calcifer.machines().await?.machines.iter().map(|machine| machine.models()).collect(),
    };
    let matching: Vec<_> = machines.iter().flat_map(|machine| &machine.models).filter(|row| output::names(&row.model, words)).collect();
    let unclassified: Vec<_> = matching.iter().copied().filter(|row| !matches!(row.role.as_str(), "model" | "lora")).collect();
    let classified: Vec<_> = matching.into_iter().filter(|row| row.role == role).collect();
    let total = classified.len();
    let rows: Vec<_> = classified.into_iter().skip(offset as usize).take(output::at_most("--limit", limit, 100) as usize).collect();
    let older = total.saturating_sub(offset as usize).saturating_sub(rows.len());
    if ctx.json {
        output::json(&serde_json::json!({ "models": rows, "total": total, "older": older, "unclassified": unclassified }));
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
    if !unclassified.is_empty() { table = table.note(format!("{} local entries could not be classified; --json includes their records", unclassified.len())); }
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
