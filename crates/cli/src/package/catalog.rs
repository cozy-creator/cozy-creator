//! `cozy package ls`: the Hub's catalog, as this computer's account sees it (anonymous when
//! signed out), through Calcifer (CALCIFER-API.md "Packages").
use std::{io::IsTerminal as _, process::ExitCode};

use api::{
    error::Error,
    calcifer::hub::PackageSearch,
};

use crate::{
    Context,
    output::{self, Table},
};

pub(crate) async fn search(ctx: &Context, query: &str, limit: u32, offset: u32) -> Result<ExitCode, Error> {
    let request = super::browse::Searching { hub: ctx.hub().to_string(), q: query.trim().to_owned(), limit, offset };
    let result = read(ctx, &request).await?;
    if ctx.json {
        output::json(&result);
        return Ok(ExitCode::SUCCESS);
    }
    let view = super::browse::Catalog::new(request, result, super::browse::local_inventory(ctx).await);
    if ctx.term.stdout && std::io::stdin().is_terminal() {
        return crate::tui::show(ctx, crate::tui::Screen::PackageCatalog(view)).await;
    }
    let mut table = Table::new(ctx, super::browse::HEADERS);
    for row in view.rows() { table = table.row(row); }
    table.note(view.title()).print();
    Ok(ExitCode::SUCCESS)
}

pub(super) async fn read(ctx: &Context, request: &super::browse::Searching) -> Result<PackageSearch, Error> {
    ctx.calcifer().await?.get_query("/v1/packages", request).await
}
