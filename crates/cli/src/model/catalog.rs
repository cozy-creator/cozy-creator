//! `cozy model search|info|family`: the Hub's model catalog. Signed in, `info` also lists the
//! commits of a model of the user's org.
use std::collections::BTreeMap;
use std::process::ExitCode;

use protocol::error::{Error, Kind};
use hub_client::types::{ModelCard, ModelLaneProvenance};
use serde::Serialize;

use super::refs::{ModelRef, Repo};
use super::{Family, Info, Search};
use crate::Context;
use crate::context::hub_answer;
use crate::output::{self, Record, Table};

/// One search row: the model's newest release, and its lanes.
#[derive(Serialize)]
struct Found {
    model: String,
    family: Option<String>,
    release: Option<String>,
    lanes: Vec<String>,
}

#[derive(Serialize)]
struct Searched {
    models: Vec<Found>,
    /// How many models matched, shown or not.
    total: i64,
}

fn found(card: &ModelCard) -> Found {
    // The Hub lists releases newest first.
    let release = card.releases.first();
    Found {
        model: protocol::names::shown(&format!("{}/{}", card.model.org, card.model.name)),
        family: card.model.family.clone().filter(|f| !f.is_empty()),
        release: release.map(|r| r.version.clone()),
        lanes: release.map(|r| r.lanes.keys().cloned().collect()).unwrap_or_default(),
    }
}

pub(crate) async fn search(ctx: &Context, args: Search) -> Result<ExitCode, Error> {
    let client = ctx.hub_anonymous(ctx.hub());
    let exact = match (&args.family, args.query.as_slice()) {
        // A single word is search text (a root name among its matches, first); org/name is one model.
        (None, [one]) if one.contains('/') => Repo::parse(one).ok(),
        _ => None,
    };
    let (cards, total) = match exact {
        Some(repo) => (vec![hub_answer(client.get_model(&repo.org, &repo.name).await)?], 1),
        None => {
            let query = args.query.join(" ");
            let q = Some(query.as_str()).filter(|q| !q.is_empty());
            let answer = hub_answer(client.search_models(q, args.family.as_deref(), args.limit).await)?;
            // One card read per model, all at once.
            let mut reads = tokio::task::JoinSet::new();
            for (i, model) in answer.models.into_iter().take(args.limit as usize).enumerate() {
                let client = client.clone();
                reads.spawn(async move { (i, client.get_model(&model.org, &model.name).await) });
            }
            let mut cards = BTreeMap::new();
            while let Some(read) = reads.join_next().await {
                let (i, card) = read.map_err(|e| Error::new(Kind::Internal, "card_read", e.to_string()))?;
                cards.insert(i, hub_answer(card)?);
            }
            (cards.into_values().collect(), answer.search.total)
        }
    };
    let view = Searched {
        models: cards.iter().map(found).collect(),
        total,
    };
    if ctx.json {
        output::json(&view);
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = Table::new(ctx, ["model", "family", "release", "lanes"]);
    for row in &view.models {
        let lanes = row.lanes.join(", ");
        let family = row.family.as_deref().unwrap_or("-");
        table = table.row([row.model.as_str(), family, row.release.as_deref().unwrap_or("-"), &lanes]);
    }
    let shown = view.models.len() as i64;
    if total > shown {
        table = table.note(format!("{shown} of {total} matching models: raise --limit or narrow the search"));
    }
    table.print();
    match view.models.first() {
        Some(first) => output::next(ctx, &[&format!("cozy model info {}", first.model)]),
        None => output::next(ctx, &["cozy model search"]),
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Serialize)]
struct Lane {
    lane: String,
    /// The commit, `sha256:…`.
    commit: String,
    bytes: i64,
    /// Its header's schemes and dtypes.
    kinds: Vec<String>,
    /// The recipe that wrote it, as its header records.
    provenance: Option<ModelLaneProvenance>,
}

#[derive(Serialize)]
struct Release {
    version: String,
    created_at: String,
    lanes: Vec<Lane>,
}

#[derive(Serialize)]
struct Commit {
    commit: String,
    lane: String,
    bytes: i64,
    /// The releases naming it; none while private.
    releases: Vec<String>,
    created_at: String,
}

#[derive(Serialize)]
struct Detail {
    model: String,
    family: Option<String>,
    created_at: String,
    releases: Vec<Release>,
    /// Every commit, newest first, to the model's members.
    commits: Vec<Commit>,
}

pub(crate) async fn info(ctx: &Context, args: Info) -> Result<ExitCode, Error> {
    let (repo, selected) = match ModelRef::parse(&args.model)? {
        ModelRef::Hub { repo, release } => (repo, release),
        _ => {
            return Err(Error::new(Kind::Usage, "model_ref_invalid", format!("{:?}: info reads org/name[@release]", args.model)));
        }
    };
    let card = hub_answer(ctx.hub_client(ctx.hub())?.get_model(&repo.org, &repo.name).await)?;
    let releases: Vec<Release> = card
        .releases
        .into_iter()
        .filter(|r| selected.as_ref().is_none_or(|s| *s == r.version))
        .map(|r| Release {
            version: r.version,
            created_at: r.created_at,
            lanes: r.lanes.into_iter().map(|(lane, l)| Lane { lane, commit: l.commit, bytes: l.bytes, kinds: l.kinds, provenance: l.provenance }).collect(),
        })
        .collect();
    if let Some(release) = selected.filter(|_| releases.is_empty()) {
        return Err(Error::new(Kind::NotFound, "model_release_not_found", format!("{repo} has no release {release:?}"))
            .next(format!("cozy model info {repo}")));
    }
    let commits = card.commits.into_iter()
        .map(|c| Commit { commit: c.id, lane: c.lane, bytes: c.bytes, releases: c.releases, created_at: c.created_at })
        .collect();
    let view = Detail {
        model: repo.to_string(),
        family: card.model.family.filter(|f| !f.is_empty()),
        created_at: card.model.created_at,
        releases,
        commits,
    };
    if ctx.json {
        output::json(&view);
        return Ok(ExitCode::SUCCESS);
    }
    let shown = protocol::names::shown(&repo.to_string());
    let mut record = Record::new().field("model", &view.model).field("family", view.family.as_deref().unwrap_or("-"));
    record = record.field("created", &view.created_at);
    record.print(ctx);
    for release in &view.releases {
        anstream::println!("\n{shown}@{}", release.version);
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

#[derive(Serialize)]
struct FamilySet {
    model: String,
    family: Option<String>,
}

pub(crate) async fn family(ctx: &Context, args: Family) -> Result<ExitCode, Error> {
    let repo = Repo::hub(&args.model)?;
    let family = if args.clear { String::new() } else { args.family.unwrap_or_default() };
    let client = ctx.hub_signed_in(ctx.hub())?;
    let answer = hub_answer(client.set_model_family(&repo.org, &repo.name, &family).await)?;
    let view = FamilySet {
        model: repo.to_string(),
        family: answer.model.family.filter(|f| !f.is_empty()),
    };
    if ctx.json {
        output::json(&view);
    } else {
        Record::of(&view).print(ctx);
    }
    Ok(ExitCode::SUCCESS)
}
