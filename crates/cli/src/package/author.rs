//! `cozy package lock|publish|yank|delete`: authoring, through Calcifer (CALCIFER-API.md
//! "Packages"). Calcifer locks and publishes this directory's project with uv on this computer:
//! package code never runs here, and the authored files never change but `uv.lock`.
use std::process::ExitCode;

use protocol::{
    error::{Error, Kind},
    hub::{At, Authoring, AuthoringEvent, LockAsk, PackageDeleted, PackageUnyanked, PackageYanked, PublishAsk, WorkState},
};
use serde::Serialize;

use super::PackageRef;
use crate::{Context, calcifer_client::path, output};

fn here() -> Result<std::path::PathBuf, Error> {
    std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))
}

/// Starts the work at `route` and follows it to its end, saying its notices as they come.
async fn authored(ctx: &Context, route: &str, ask: &impl Serialize) -> Result<Authoring, Error> {
    let calcifer = ctx.calcifer().await?;
    let started: Authoring = calcifer.post(route, ask, Some(&crate::run::mint_key())).await?;
    let at = format!("{route}/{}", path(&[&started.id]).trim_start_matches('/'));
    let mut events = calcifer.watch::<AuthoringEvent>(&format!("{at}/events")).await?;
    while let Some(event) = events.next().await {
        match event {
            AuthoringEvent::Notice { message } => output::warn(&message),
            _ => break,
        }
    }
    let done: Authoring = calcifer.get(&at).await?;
    match (done.state, done.error) {
        (WorkState::Failed, Some(error)) => Err(error),
        (WorkState::Running, _) | (WorkState::Failed, None) => Err(Error::new(
            Kind::Unavailable,
            "authoring_interrupted",
            format!("Calcifer stopped before {} ended", started.id),
        )
        .remedy("run it again: it is safe")),
        (WorkState::Done, _) => Ok(Authoring { error: None, ..done }),
    }
}

pub async fn lock(ctx: &Context, upgrade: &[String], root: bool) -> Result<ExitCode, Error> {
    let ask = LockAsk { hub: Some(ctx.hub().to_string()), dir: here()?, root, upgrade: upgrade.to_vec() };
    let done = authored(ctx, "/v1/package-locks", &ask).await?;
    let result = done.locked.ok_or_else(|| Error::new(Kind::Internal, "untyped_answer", "a lock that ended with no lock"))?;
    match ctx.json {
        true => output::json(&result),
        false => output::Record::new()
            .field("lock", result.lock.display())
            .field("account index", result.account_index.as_deref().unwrap_or("none"))
            .field("changed", result.changed)
            .print(ctx),
    }
    Ok(ExitCode::SUCCESS)
}

pub async fn publish(ctx: &Context, root: bool) -> Result<ExitCode, Error> {
    let ask = PublishAsk { hub: Some(ctx.hub().to_string()), dir: here()?, root };
    let done = authored(ctx, "/v1/package-publications", &ask).await?;
    let result = (done.published)
        .ok_or_else(|| Error::new(Kind::Internal, "untyped_answer", "a publication that ended with no release"))?;
    if ctx.json {
        output::json(&result);
    } else {
        let mut record = output::Record::new()
            .field("package", format!("{}@{}", result.package, result.release))
            .field("status", &result.status)
            .field("uploaded", output::bytes(result.uploaded_bytes))
            .field("hub", &result.hub);
        for warning in &result.warnings {
            record = record.note(format!("warning: {warning}"));
        }
        record.print(ctx);
    }
    if !result.changed {
        output::next(ctx, &["bump [project] version in pyproject.toml to publish a change"]);
    }
    Ok(ExitCode::SUCCESS)
}

pub async fn yank(ctx: &Context, package: &PackageRef, undo: bool) -> Result<ExitCode, Error> {
    let Some(release) = &package.release else {
        return Err(Error::new(
            Kind::Usage,
            "yank_release_required",
            "name the release to yank: [org/]name@release",
        ));
    };
    let hub = At { hub: Some(ctx.hub().to_string()) };
    let at = path(&["v1", "packages", &package.org, &package.name, "releases", release, "yank"]);
    let calcifer = ctx.calcifer().await?;
    if undo {
        let result: PackageUnyanked = calcifer.delete_query(&at, &hub).await?;
        match ctx.json {
            true => output::json(&result),
            false => output::Record::new()
                .field("package", format!("{}@{}", result.package, result.release))
                .field(
                    "status",
                    if result.changed {
                        "un-yanked"
                    } else {
                        "not yanked"
                    },
                )
                .note("It is the unversioned default again when it is the newest release.")
                .print(ctx),
        }
        return Ok(ExitCode::SUCCESS);
    }
    let result: PackageYanked = calcifer.post(&at, &hub, None).await?;
    match ctx.json {
        true => output::json(&result),
        false => output::Record::new()
            .field("package", format!("{}@{}", result.package, result.release))
            .field(
                "status",
                if result.changed {
                    "yanked"
                } else {
                    "already yanked"
                },
            )
            .field("yanked at", &result.yanked_at)
            .note("Machines that hold it keep it; only an exact pin installs it.")
            .print(ctx),
    }
    Ok(ExitCode::SUCCESS)
}

/// Never prompts: `--yes` is the confirmation, and without it nothing is sent.
pub async fn delete(ctx: &Context, package: &PackageRef) -> Result<ExitCode, Error> {
    if package.release.is_some() {
        return Err(Error::new(
            Kind::Usage,
            "package_release_not_taken",
            format!("{package} names a release; delete takes the whole package, [org/]name"),
        )
        .remedy(format!("cozy package yank {package}")));
    }
    let hub = ctx.hub();
    if !ctx.yes {
        return Err(Error::new(
            Kind::Usage,
            "package_delete_unconfirmed",
            format!(
                "deleting {package} and every release from {} cannot be undone",
                hub
            ),
        )
        .remedy("run it again with --yes"));
    }
    let at = path(&["v1", "packages", &package.org, &package.name]);
    let result: PackageDeleted = ctx.calcifer().await?.delete_query(&at, &At { hub: Some(hub.to_string()) }).await?;
    match ctx.json {
        true => output::json(&result),
        false => output::Record::of(&result)
            .note("Machines that installed it keep their copies.")
            .print(ctx),
    }
    Ok(ExitCode::SUCCESS)
}
