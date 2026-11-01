//! `cozy model ls|info|family`: the Hub's model catalog, through Calcifer (CALCIFER-API.md
//! "Models"). Signed in, `ls` also lists the user's own unreleased (private) models and `info` lists
//! the commits of a model of the user's org.
use std::process::ExitCode;

use protocol::error::{Error, Kind};
use protocol::hub::{ModelAuthor, ModelDetail, ModelEdit, ModelFamily, ModelSearch};
use serde::Serialize;

use protocol::model_ref::{ModelRef, Repo};
use super::{Family, Info, Ls};
use crate::Context;
use crate::calcifer_client::path;
use crate::output::{self, Record, Table};

/// An author as a person reads it: its name, and while its org is parked, that it is reserved.
fn by(author: &ModelAuthor) -> String {
    match author.parked {
        true => format!("{} ({}, reserved)", author.name, author.org),
        false => format!("{} ({})", author.name, author.org),
    }
}

#[derive(Serialize)]
struct Searching<'a> {
    hub: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    q: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    family: Option<&'a str>,
    limit: u32,
}

pub(crate) async fn search(ctx: &Context, args: Ls) -> Result<ExitCode, Error> {
    let query = args.query.join(" ");
    let ask = Searching { hub: ctx.hub(), q: &query, family: args.family.as_deref(), limit: args.limit };
    let view: ModelSearch = ctx.calcifer().await?.get_query("/v1/models", &ask).await?;
    let total = view.total;
    if ctx.json {
        output::json(&view);
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = Table::new(ctx, ["model", "family", "release", "lanes", "by"]);
    for row in &view.models {
        let lanes = row.lanes.join(", ");
        let family = row.family.as_deref().unwrap_or("-");
        let release = match row.private {
            true => "private",
            false => row.release.as_deref().unwrap_or("-"),
        };
        let author = row.author.as_ref().map_or_else(|| "-".to_owned(), by);
        table = table.row([row.model.as_str(), family, release, &lanes, &author]);
    }
    let shown = view.models.len() as i64;
    if total > shown {
        table = table.note(format!("{shown} of {total} matching models: raise --limit or narrow the search"));
    }
    table.print();
    match view.models.first() {
        Some(first) => output::next(ctx, &[&format!("cozy model info {}", first.model)]),
        None => output::next(ctx, &["cozy model ls"]),
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) async fn info(ctx: &Context, args: Info) -> Result<ExitCode, Error> {
    let (repo, selected) = match ModelRef::parse(&args.model)? {
        ModelRef::Hub { repo, release } => (repo, release),
        _ => {
            return Err(Error::new(Kind::Usage, "model_ref_invalid", format!("{:?}: info reads org/name[@release]", args.model)));
        }
    };
    let mut query = vec![("hub", ctx.hub().to_string())];
    query.extend(selected.map(|release| ("release", release)));
    let at = path(&["v1", "models", &repo.org, &repo.name]);
    let view: ModelDetail = ctx.calcifer().await?.get_query(&at, &query).await?;
    if ctx.json {
        output::json(&view);
        return Ok(ExitCode::SUCCESS);
    }
    let shown = protocol::names::shown(&repo.to_string());
    let mut record = Record::new().field("model", &view.model).field("family", view.family.as_deref().unwrap_or("-"));
    record = record.field("created", &view.created_at);
    if let Some(author) = &view.author {
        record = record.field("by", &by(author));
    }
    if let Some(source) = &view.source {
        record = record.field("source", source);
    }
    if let Some(trigger) = &view.trigger {
        record = record.field("trigger", trigger);
    }
    if let Some(strength) = view.strength {
        record = record.field("strength", &format!("{strength} (recommended; --lora uses 1.0 unless given)"));
    }
    if let Some(author) = &view.author
        && let Some(reserved) = &author.reserved
    {
        let mirror = repo.org == author.org;
        record = record.note(match mirror {
            true => format!("Mirror of {}. {reserved}", view.source.as_deref().unwrap_or("its author's model")),
            false => format!("{}: {reserved}", author.org),
        });
    }
    record.print(ctx);
    for release in &view.releases {
        let mirror = match (release.mirror, release.by_admin) {
            (true, _) => " (mirror)",
            (false, true) => " (uploaded by Tensorhub admin)",
            _ => "",
        };
        anstream::println!("\n{shown}@{}{mirror}", release.version);
        let mut table = Table::new(ctx, ["lane", "kinds", "bytes", "commit", "recipe"]);
        for lane in &release.lanes {
            let recipe = lane.provenance.as_ref().map_or_else(|| "-".into(), |p| format!("{}@{} {}", protocol::names::shown(&p.package), p.release, p.recipe));
            table = table.row([lane.lane.clone(), lane.kinds.join("+"), output::bytes(lane.bytes as u64), lane.commit.clone(), recipe]);
        }
        // A recipe is its fingerprint: the digest of the plan the lane was written by.
        for (lane, made) in release.lanes.iter().filter_map(|l| Some((l, l.provenance.as_ref()?))) {
            table = table.note(format!("{}: recipe {}, from {}", lane.lane, made.fingerprint, made.source));
        }
        table.print();
    }
    if !view.commits.is_empty() {
        anstream::println!("\n{shown} commits");
        let mut table = Table::new(ctx, ["commit", "lane", "bytes", "releases", "created"]);
        for c in &view.commits {
            let releases = if c.releases.is_empty() { "private".to_owned() } else { c.releases.join(", ") };
            table = table.row([c.commit.clone(), c.lane.clone(), output::bytes(c.bytes as u64), releases, c.created_at.clone()]);
        }
        table.print();
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) async fn family(ctx: &Context, args: Family) -> Result<ExitCode, Error> {
    let repo = Repo::hub(&args.model)?;
    let family = if args.clear { String::new() } else { args.family.unwrap_or_default() };
    let ask = ModelEdit { hub: Some(ctx.hub().to_string()), family: Some(family) };
    let view: ModelFamily = ctx.calcifer().await?.patch(&path(&["v1", "models", &repo.org, &repo.name]), &ask).await?;
    if ctx.json {
        output::json(&view);
    } else {
        Record::of(&view).print(ctx);
    }
    Ok(ExitCode::SUCCESS)
}
