//! `cozy package ls|info`: the Hub's catalog, as this computer's account sees it (anonymous when
//! signed out), through Calcifer (CALCIFER-API.md "Packages").
use std::process::ExitCode;

use protocol::{
    error::Error,
    hub::{PackageDetail, PackageSearch},
};

use super::PackageRef;
use crate::{
    Context,
    calcifer_client::path,
    output::{self, Table},
};

pub async fn search(ctx: &Context, query: &str, limit: u32) -> Result<ExitCode, Error> {
    let hub = ctx.hub();
    let query = query.trim();
    let mut ask = vec![("hub", hub.to_string()), ("limit", limit.to_string())];
    if !query.is_empty() {
        ask.push(("q", query.to_owned()));
    }
    let result: PackageSearch = ctx.calcifer().await?.get_query("/v1/packages", &ask).await?;
    if ctx.json {
        output::json(&result);
        return Ok(ExitCode::SUCCESS);
    }
    if result.packages.is_empty() {
        println!("No packages on {} match.", hub.as_str());
        let next = if query.is_empty() {
            "cozy package publish --help"
        } else {
            "cozy package ls"
        };
        output::next(ctx, &[next]);
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = Table::new(ctx, ["PACKAGE", "LATEST", "CREATED"]);
    for p in &result.packages {
        let latest = p.latest_release.as_deref().unwrap_or("-");
        table = table.row([p.package.as_str(), latest, p.created_at.as_str()]);
    }
    table.print();
    if result.older > 0 {
        let shown = result.total.min(200);
        println!("{} more not shown. Use --limit {shown} to show them.", result.older);
    }
    Ok(ExitCode::SUCCESS)
}

pub async fn info(ctx: &Context, package: &PackageRef) -> Result<ExitCode, Error> {
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
