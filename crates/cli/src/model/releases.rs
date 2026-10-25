//! `cozy model publish|delete`: a model's releases and commits, as a member of its org. A release
//! names one commit per lane under a version higher than every earlier one, and is public for
//! good; a commit no release names can be deleted, as can the whole model.
use std::process::ExitCode;

use hub_client::types::ModelReleaseRequest;
use protocol::error::{Error, Kind};
use serde::Serialize;

use super::refs::{self, ModelRef, Repo};
use super::{Delete, Publish};
use crate::Context;
use crate::context::hub_answer;
use crate::output::{self, Record, Table};

fn usage(code: &str, message: impl Into<String>) -> Error {
    Error::new(Kind::Usage, code, message)
}

#[derive(Serialize)]
struct Released {
    model: String,
    version: String,
    created_at: String,
    /// Each lane's commit.
    lanes: Vec<(String, String)>,
}

pub(crate) async fn publish(ctx: &Context, args: Publish) -> Result<ExitCode, Error> {
    let repo = Repo::hub(&args.model)?;
    let version = refs::label("version", &args.version)?;
    let commits = args.commits.iter().map(|c| refs::commit(c)).collect::<Result<Vec<_>, _>>()?;
    let client = ctx.hub_signed_in(ctx.hub())?;
    let request = ModelReleaseRequest { commits };
    let release = hub_answer(client.publish_model_release(&repo.org, &repo.name, &version, &request).await)?;
    let view = Released {
        model: repo.to_string(),
        version: release.version,
        created_at: release.created_at,
        lanes: release.lanes.into_iter().map(|(lane, l)| (lane, l.commit)).collect(),
    };
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

#[derive(Serialize)]
struct Deleted {
    model: String,
    /// One commit; absent when the whole model went.
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
}

/// Never prompts: `--yes` is the confirmation, and without it nothing is sent.
pub(crate) async fn delete(ctx: &Context, args: Delete) -> Result<ExitCode, Error> {
    let (repo, commit) = match ModelRef::parse(&args.model)? {
        ModelRef::Hub { repo, release: None } => (repo, None),
        ModelRef::Commit { repo, commit } => (repo, Some(commit)),
        _ => return Err(usage("model_ref_invalid", format!("{:?}: delete takes [org/]name or [org/]name@sha256:<commit>", args.model))),
    };
    let hub = ctx.hub();
    if !ctx.yes {
        return Err(usage("model_delete_unconfirmed", format!("deleting {} from {} cannot be undone", args.model, hub))
            .remedy("run it again with --yes"));
    }
    let client = ctx.hub_signed_in(&hub)?;
    match &commit {
        Some(id) => drop(hub_answer(client.delete_model_commit(&repo.org, &repo.name, id).await)?),
        None => drop(hub_answer(client.delete_model(&repo.org, &repo.name).await)?),
    }
    let view = Deleted { model: repo.to_string(), commit };
    match ctx.json {
        true => output::json(&view),
        false => Record::of(&view).note("The Hub's GC reclaims the objects nothing else names.").print(ctx),
    }
    Ok(ExitCode::SUCCESS)
}
