//! Model releases select one checkpoint per lane. Owners may unpublish a release,
//! delete an unreferenced checkpoint, or permanently delete the model.
use std::process::ExitCode;

use api::error::{Error, Kind};
use api::calcifer::hub::{At, ModelDeleted, ModelReleaseAsk, ModelReleased};

use api::model_ref::{ModelRef, ModelId};
use super::{Delete, Publish, Unpublish};
use crate::Context;
use crate::calcifer_client::path;
use crate::output::{self, Record, Table};

fn usage(code: &str, message: impl Into<String>) -> Error {
    Error::new(Kind::Usage, code, message)
}

pub(crate) async fn publish(ctx: &Context, args: Publish, noun: &str) -> Result<ExitCode, Error> {
    let model_id = ModelId::hub(&args.model)?;
    let ask = ModelReleaseAsk { hub: Some(ctx.hub().to_string()), version: args.version, checkpoints: args.checkpoints };
    let at = path(&["v1", "models", &model_id.org, &model_id.name, "releases"]);
    let view: ModelReleased = ctx.calcifer().await?.post(&at, &ask, None).await?;
    view.warnings.iter().for_each(|warning| output::warn(warning));
    if ctx.json {
        output::json(&view);
        return Ok(ExitCode::SUCCESS);
    }
    Record::new().field("model", &view.model).field("release", &view.version).print(ctx);
    let mut table = Table::new(ctx, ["lane", "checkpoint"]);
    for (lane, checkpoint) in &view.lanes {
        table = table.row([lane.as_str(), checkpoint.as_str()]);
    }
    table.print();
    output::next(ctx, &[&format!("cozy {noun} ls {}", view.model)]);
    Ok(ExitCode::SUCCESS)
}

pub(crate) async fn unpublish(ctx: &Context, args: Unpublish, noun: &str) -> Result<ExitCode, Error> {
    let (model, version) = match ModelRef::parse(&args.model)? {
        ModelRef::Hub { model, release: Some(version) } => (model, version),
        _ => return Err(usage("model_release_required", "Choose a model release to unpublish")
            .next(format!("cozy {noun} unpublish <org/name>@<version>"))),
    };
    let at = path(&["v1", "models", &model.org, &model.name, "releases", &version, "unpublish"]);
    let result: api::calcifer::hub::ModelUnpublished = ctx.calcifer().await?.post(&at, &At { hub: Some(ctx.hub().to_string()) }, None).await?;
    if ctx.json {
        output::json(&result);
    } else {
        let status = if result.unpublished { "Unpublished" } else { "Already private" };
        anstream::println!("{status}: {}@{}", result.model, result.release);
        output::next(ctx, &[&format!("cozy {noun} ls {}", result.model)]);
    }
    Ok(ExitCode::SUCCESS)
}

/// Never prompts: `--yes` is the confirmation, and without it nothing is sent.
pub(crate) async fn delete(ctx: &Context, args: Delete, noun: &str) -> Result<ExitCode, Error> {
    let (model_id, release, checkpoint) = match ModelRef::parse(&args.model)? {
        ModelRef::Hub { model: model_id, release } => (model_id, release, None),
        ModelRef::Checkpoint { model: model_id, checkpoint } => (model_id, None, Some(checkpoint)),
        _ => return Err(usage("model_ref_invalid", format!("Invalid model reference: {:?}", args.model))
            .next(format!("cozy {noun} delete --help"))),
    };
    let hub = ctx.hub();
    if !ctx.yes {
        return Err(usage("model_delete_unconfirmed", format!("deleting {} from {} cannot be undone", args.model, hub))
            .next(format!("cozy {noun} delete {} --yes", args.model)));
    }
    let mut at = vec!["v1", "models", model_id.org.as_str(), model_id.name.as_str()];
    let note = match (&release, &checkpoint) {
        (Some(version), _) => {
            at.extend(["releases", version.as_str()]);
            format!("{version} is never published again; what only it released is private again.")
        }
        (None, Some(id)) => {
            at.extend(["checkpoints", id.as_str()]);
            "The Hub's GC reclaims the objects nothing else names.".into()
        }
        (None, None) => "The Hub's GC reclaims the objects nothing else names.".into(),
    };
    let view: ModelDeleted = ctx.calcifer().await?.delete_query(&path(&at), &At { hub: Some(hub.to_string()) }).await?;
    match ctx.json {
        true => output::json(&view),
        false => Record::of(&view).note(note).print(ctx),
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unpublish_requires_a_release_before_contacting_calcifer() {
        let root = tempfile::tempdir().unwrap();
        let ctx = Context::new(false, false, false, api::config::Loaded {
            config: Default::default(), home: platform::paths::Home::under(root.path()), user_home: root.path().into(), warnings: vec![],
        }, output::Term::detect());
        for model in ["author/model".to_owned(), format!("author/model@sha256:{}", "a".repeat(64))] {
            let error = unpublish(&ctx, Unpublish { model }, "model").await.unwrap_err();
            assert_eq!(error.code, "model_release_required");
        }
        assert!(!root.path().join("calcifer/calcifer.lock.holder").exists());
    }
}
