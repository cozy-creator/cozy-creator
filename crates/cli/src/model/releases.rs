//! `cozy model publish|delete`: a model's releases and commits, as a member of its org. A release
//! names one commit per lane under a version higher than every earlier one, and is public for
//! good; a commit no release names can be deleted, as can the whole model.
use std::process::ExitCode;

use protocol::error::{Error, Kind};
use protocol::hub::{At, ModelDeleted, ModelReleaseAsk, ModelReleased};

use protocol::model_ref::{ModelRef, Repo};
use super::{Delete, Publish};
use crate::Context;
use crate::calcifer_client::path;
use crate::output::{self, Record, Table};

fn usage(code: &str, message: impl Into<String>) -> Error {
    Error::new(Kind::Usage, code, message)
}

pub(crate) async fn publish(ctx: &Context, args: Publish) -> Result<ExitCode, Error> {
    let repo = Repo::hub(&args.model)?;
    let ask = ModelReleaseAsk { hub: Some(ctx.hub().to_string()), version: args.version, commits: args.commits };
    let at = path(&["v1", "models", &repo.org, &repo.name, "releases"]);
    let view: ModelReleased = ctx.calcifer().await?.post(&at, &ask, None).await?;
    if ctx.json {
        output::json(&view);
        return Ok(ExitCode::SUCCESS);
    }
    Record::new().field("model", &view.model).field("release", &view.version).print(ctx);
    let mut table = Table::new(ctx, ["lane", "commit"]);
    for (lane, commit) in &view.lanes {
        table = table.row([lane.as_str(), commit.as_str()]);
    }
    table.print();
    output::next(ctx, &[&format!("cozy model info {}@{}", view.model, view.version)]);
    Ok(ExitCode::SUCCESS)
}

/// Never prompts: `--yes` is the confirmation, and without it nothing is sent.
pub(crate) async fn delete(ctx: &Context, args: Delete) -> Result<ExitCode, Error> {
    let (repo, release, commit) = match ModelRef::parse(&args.model)? {
        ModelRef::Hub { repo, release } => (repo, release, None),
        ModelRef::Commit { repo, commit } => (repo, None, Some(commit)),
        _ => return Err(usage("model_ref_invalid", format!(
            "{:?}: delete takes [org/]name, [org/]name@<version> or [org/]name@sha256:<commit>", args.model))),
    };
    let hub = ctx.hub();
    if !ctx.yes {
        return Err(usage("model_delete_unconfirmed", format!("deleting {} from {} cannot be undone", args.model, hub))
            .remedy("run it again with --yes"));
    }
    let mut at = vec!["v1", "models", repo.org.as_str(), repo.name.as_str()];
    let note = match (&release, &commit) {
        (Some(version), _) => {
            at.extend(["releases", version.as_str()]);
            format!("{version} is never published again; what only it released is private again.")
        }
        (None, Some(id)) => {
            at.extend(["commits", id.as_str()]);
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
