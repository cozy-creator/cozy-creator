//! `cozy model ls|edit`: the Hub's model catalog, through Calcifer (CALCIFER-API.md
//! "Models"). Signed in, `ls` also lists the user's own unreleased (private) models.
use std::{io::IsTerminal as _, process::ExitCode};

use api::error::{Error, Kind};
use api::calcifer::hub::{ModelDetail, ModelEdit, ModelEdited, ModelSearch};
use serde::Serialize;

use api::model_ref::Repo;
use super::{Edit, Ls};
use crate::Context;
use crate::calcifer_client::path;
use crate::output::{self, Table};

#[derive(Clone, Serialize)]
pub(super) struct Searching {
    pub(super) hub: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(super) q: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) family: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(super) tag: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(super) no_tag: String,
    pub(super) limit: u32,
    pub(super) offset: u32,
    pub(super) role: String,
}

pub(crate) async fn search(ctx: &Context, args: Ls, role: &str) -> Result<ExitCode, Error> {
    let query = args.query.join(" ");
    let ask = Searching {
        hub: ctx.hub().to_string(),
        q: query,
        family: args.family,
        tag: args.tags.join(","),
        no_tag: args.no_tags.join(","),
        limit: output::at_most("--limit", args.limit, 100),
        offset: args.offset,
        role: role.to_owned(),
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
    let mut table = Table::new(ctx, ["model", "family", "size", "local"]);
    for row in browser.rows() { table = table.row(row); }
    table.note(browser.title()).print();
    Ok(ExitCode::SUCCESS)
}

pub(super) async fn local(ctx: &Context, args: Ls, role: &str) -> Result<ExitCode, Error> {
    let query = Searching { hub: ctx.hub().to_string(), q: args.query.join(" "), family: None, tag: String::new(), no_tag: String::new(), limit: output::at_most("--limit", args.limit, 100), offset: args.offset, role: role.into() };
    let inventory: api::calcifer::MachineModels = ctx.calcifer().await?.get("/v1/machines/local/models").await?;
    if inventory.silent {
        return Err(Error::new(Kind::Unavailable, "local_inventory_unavailable", "The local machine has not reported its model inventory yet").next("cozy machine ls"));
    }
    crate::tui::show(ctx, crate::tui::Screen::Models(super::live::Catalog::from_local(query, inventory.models))).await
}

pub(super) async fn read(ctx: &Context, ask: &Searching) -> Result<ModelSearch, Error> {
    ctx.calcifer().await?.get_query("/v1/models", ask).await
}

pub(super) async fn detail(ctx: &Context, model: &str) -> Result<ModelDetail, Error> {
    let repo = Repo::hub(model)?;
    ctx.calcifer().await?.get_query(&path(&["v1", "models", &repo.org, &repo.name]), &[("hub", ctx.hub().to_string())]).await
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
    anstream::println!("Updated {}.", api::names::shown(&repo.to_string()));
    Ok(ExitCode::SUCCESS)
}
