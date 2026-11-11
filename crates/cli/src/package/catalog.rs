//! `cozy package ls|info`: the Hub's catalog, as this computer's account sees it (anonymous when
//! signed out), through Calcifer (CALCIFER-API.md "Packages").
use std::{io::IsTerminal as _, process::ExitCode};

use api::{
    error::Error,
    calcifer::hub::{PackageDetail, PackageSearch},
};

use super::PackageRef;
use crate::{
    Context,
    calcifer_client::path,
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

pub(crate) async fn info(ctx: &Context, package: &PackageRef) -> Result<ExitCode, Error> {
    let hub = ctx.hub();
    let mut ask = vec![("hub", hub.to_string())];
    ask.extend(package.release.clone().map(|release| ("release", release)));
    let at = path(&["v1", "packages", &package.org, &package.name]);
    let info: PackageDetail = ctx.calcifer().await?.get_query(&at, &ask).await?;
    if ctx.json {
        output::json(&info);
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "{} on {}, latest {}",
        info.package,
        hub.as_str(),
        info.latest_release.as_deref().unwrap_or("none")
    );
    let mut table = Table::new(ctx, ["RELEASE", "PUBLISHED", "YANKED"]);
    for r in &info.releases {
        let yanked = if r.yanked { "yes" } else { "no" };
        table = table.row([
            r.release.as_str(),
            r.published.as_deref().unwrap_or("-"),
            yanked,
        ]);
    }
    for warning in &info.warnings {
        table = table.note(format!("warning: {warning}"));
    }
    table.print();
    Ok(ExitCode::SUCCESS)
}
