//! `cozy model ls|info|edit`: the Hub's model catalog, through Calcifer (CALCIFER-API.md
//! "Models"). Signed in, `ls` also lists the user's own unreleased (private) models and `info` lists
//! the commits of a model of the user's org.
use std::{io::IsTerminal as _, process::ExitCode};

use api::error::{Error, Kind};
use api::calcifer::hub::{FamilyFrom, ModelAuthor, ModelDetail, ModelEdit, ModelEdited, ModelSearch};
use serde::Serialize;

use api::model_ref::Repo;
use super::{Edit, Ls};
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

#[derive(Clone, Serialize)]
pub(super) struct Searching {
    pub hub: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub q: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tag: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub no_tag: String,
    pub limit: u32,
    pub offset: u32,
}

pub(crate) async fn search(ctx: &Context, args: Ls) -> Result<ExitCode, Error> {
    let query = args.query.join(" ");
    let ask = Searching {
        hub: ctx.hub().to_string(),
        q: query,
        family: args.family,
        tag: args.tags.join(","),
        no_tag: args.no_tags.join(","),
        limit: output::at_most("--limit", args.limit, 100),
        offset: args.offset,
    };
    let view = read(ctx, &ask).await?;

    if ctx.json {
        output::json(&view);
        return Ok(ExitCode::SUCCESS);
    }
    let browser = super::live::Catalog::new(ask.clone(), view, super::live::local_inventory(ctx).await);
    if ctx.term.stdout && std::io::stdin().is_terminal() {
        return crate::tui::show(ctx, crate::tui::Screen::Models(browser)).await;
    }
    let mut table = Table::new(ctx, ["model", "family", "disk", "local"]);
    for row in browser.rows() { table = table.row(row); }
    table.note(browser.title()).print();
    Ok(ExitCode::SUCCESS)
}

pub(super) async fn read(ctx: &Context, ask: &Searching) -> Result<ModelSearch, Error> {
    ctx.calcifer().await?.get_query("/v1/models", ask).await
}

pub(super) async fn detail(ctx: &Context, model: &str) -> Result<ModelDetail, Error> {
    let repo = Repo::hub(model)?;
    ctx.calcifer().await?.get_query(&path(&["v1", "models", &repo.org, &repo.name]), &[("hub", ctx.hub().to_string())]).await
}

/// A model's family as a person reads it: where it comes from, and what its weights say when its owner's label differs.
fn family(view: &ModelDetail) -> String {
    let Some(family) = &view.family else { return "-".into() };
    match (view.family_from, &view.weights_family) {
        (Some(FamilyFrom::Weights), _) => format!("{family} (from its weights)"),
        (Some(FamilyFrom::Owner), Some(weights)) => format!("{family} (set by its owner; its weights say {weights})"),
        (Some(FamilyFrom::Owner), None) => format!("{family} (set by its owner)"),
        (None, _) => family.clone(),
    }
}

/// What a model's card and header say of it.
fn said(view: &ModelDetail, repo: &Repo) -> Record {
    let mut record = Record::new().field("model", &view.model).field("family", &family(view));
    if let Some(kind) = view.kind {
        use api::calcifer::hub::ModelKind;
        let kind = match kind {
            ModelKind::Lora => "LoRA",
            ModelKind::Weights => "a model's weights",
            ModelKind::Component => "a component (no family's model)",
        };
        record = record.field("kind", kind);
    }
    if !view.tags.is_empty() {
        record = record.field("tags", &view.tags.join(", "));
    }
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
    record
}

pub(crate) async fn edit(ctx: &Context, args: Edit) -> Result<ExitCode, Error> {
    let repo = Repo::hub(&args.model)?;
    let ask = ModelEdit {
        hub: Some(ctx.hub().to_string()),
        family: args.family,
        trigger: args.trigger,
        strength: args.strength.map(|s| s.0),
        add_tags: args.tag,
        remove_tags: args.untag,
    };
    if ask.family.is_none() && ask.trigger.is_none() && ask.strength.is_none() && !ask.tags_change() {
        return Err(Error::new(Kind::Usage, "model_edit_empty", "No model changes specified")
            .next("cozy model edit --help"));
    }
    let edited: ModelEdited = ctx.calcifer().await?.patch(&path(&["v1", "models", &repo.org, &repo.name]), &ask).await?;
    if ctx.json {
        output::json(&edited);
        return Ok(ExitCode::SUCCESS);
    }
    for warning in &edited.warnings {
        output::warn(warning);
    }
    said(&edited.model, &repo).print(ctx);
    Ok(ExitCode::SUCCESS)
}
