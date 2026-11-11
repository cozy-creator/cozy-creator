//! `cozy package lock|publish|yank`: authoring, through Calcifer (CALCIFER-API.md
//! "Packages"). The package goes by content, and Calcifer locks and publishes its copy with uv:
//! package code never runs, and the authored files never change but `uv.lock`, which the CLI writes.
use std::process::ExitCode;

use api::{
    error::{Error, Kind},
    calcifer::hub::{At, Locked, PackageLock, PackagePublish, PackageUnyanked, PackageYanked, Published, job},
};

use super::PackageRef;
use crate::{Context, calcifer_client::path, output, run};

fn here() -> Result<std::path::PathBuf, Error> {
    std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))
}

/// The package folder `path` names (else this one), sent by content.
async fn sent(ctx: &Context, path: Option<std::path::PathBuf>) -> Result<(std::path::PathBuf, api::code::LocalCode), Error> {
    let dir = match path {
        Some(path) => here()?.join(path),
        None => here()?,
    };
    let code = crate::send::Sender::new(ctx).await?.code(&dir).await?;
    Ok((dir, code))
}

/// The `uv.lock` Calcifer made, written into the package's folder `dir` whole: its path.
fn written(dir: &std::path::Path, lock: &str) -> Result<std::path::PathBuf, Error> {
    let path = dir.join("uv.lock");
    platform::durable::replace(&path, lock.as_bytes(), 0o644)
        .map_err(|e| Error::new(Kind::Unavailable, "lock_unwritable", format!("{}: {e}", path.display())))?;
    Ok(path)
}

pub(crate) async fn lock(ctx: &Context, path: Option<std::path::PathBuf>, upgrade: &[String], root: bool) -> Result<ExitCode, Error> {
    let (dir, code) = sent(ctx, path).await?;
    // Calcifer's own run, which says uv's notices as it goes.
    let going = run::job(ctx, job::PACKAGE_LOCK, &PackageLock { root, upgrade: upgrade.to_vec() }, None, Some(code)).await?;
    let result: Locked = run::done(ctx, going).await?;
    let lock = match result.changed {
        true => written(&dir, &result.lock)?,
        false => dir.join("uv.lock"),
    };
    match ctx.json {
        true => output::json(&serde_json::json!({"lock": lock, "account_index": result.account_index, "changed": result.changed})),
        false => output::Record::new()
            .field("lock", lock.display())
            .field("account index", result.account_index.as_deref().unwrap_or("none"))
            .field("changed", result.changed)
            .print(ctx),
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) async fn publish(ctx: &Context, root: bool, yanked: bool) -> Result<ExitCode, Error> {
    let (dir, code) = sent(ctx, None).await?;
    let going = run::job(ctx, job::PACKAGE_PUBLISH, &PackagePublish { root, yanked }, None, Some(code)).await?;
    let result: Published = run::done(ctx, going).await?;
    // Locked first, it holds the lock it was published with.
    if let Some(lock) = &result.lock {
        let path = written(&dir, lock)?;
        output::say(ctx, &format!("wrote {}", path.display()));
    }
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

pub(crate) async fn yank(ctx: &Context, package: &PackageRef, undo: bool) -> Result<ExitCode, Error> {
    let Some(release) = &package.release else {
        return Err(Error::new(
            Kind::Usage,
            "yank_release_required",
            "Specify a release to yank",
        ).next(format!("cozy package yank {package}@<release>")));
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
