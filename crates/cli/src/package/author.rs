//! `cozy package publish|yank|delete`: authoring, through Calcifer (CALCIFER-API.md
//! "Packages"). The package goes by content, and Calcifer locks and publishes its copy with uv:
//! package code never runs, and the authored files never change but `uv.lock`, which the CLI writes.
use std::process::ExitCode;

use api::{
    error::{Error, Kind},
    calcifer::hub::{At, PackageDeleted, PackagePublish, PackageUnyanked, PackageYanked, Published, job},
};

use super::PackageRef;
use crate::{Context, calcifer_client::path, output, run};

/// Send the current package directory by content for locking and publication.
async fn sent(ctx: &Context) -> Result<(std::path::PathBuf, api::code::LocalCode), Error> {
    let dir = std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let code = crate::send::Sender::independent_code(ctx, &dir).await?;
    Ok((dir, code))
}

/// The `uv.lock` Calcifer made, written into the package's folder `dir` whole: its path.
pub(crate) fn written(dir: &std::path::Path, lock: &str) -> Result<std::path::PathBuf, Error> {
    let path = dir.join("uv.lock");
    platform::durable::replace(&path, lock.as_bytes(), 0o644)
        .map_err(|e| Error::new(Kind::Unavailable, "lock_unwritable", format!("{}: {e}", path.display())))?;
    Ok(path)
}

pub(crate) async fn publish(ctx: &Context, root: bool, yanked: bool) -> Result<ExitCode, Error> {
    let (dir, code) = sent(ctx).await?;
    let going = run::job(ctx, job::PACKAGE_PUBLISH, &PackagePublish { root, yanked }, None, Some(code), None).await?;
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

/// `--yes` explicitly confirms permanent deletion before any request is sent.
pub(crate) async fn delete(ctx: &Context, package: &PackageRef) -> Result<ExitCode, Error> {
    if package.release.is_some() {
        return Err(Error::new(Kind::Usage, "package_release_not_taken", "Delete removes the whole package; yank withdraws one release")
            .next(format!("cozy package yank {package}")));
    }
    if !ctx.yes {
        return Err(Error::new(Kind::Usage, "package_delete_unconfirmed", format!("Deleting {package} and all its releases cannot be undone"))
            .next(format!("cozy package delete {package} --yes")));
    }
    let at = path(&["v1", "packages", &package.org, &package.name]);
    let result: PackageDeleted = ctx.calcifer().await?.delete_query(&at, &At { hub: Some(ctx.hub().to_string()) }).await?;
    if ctx.json {
        output::json(&result);
    } else {
        output::Record::of(&result).note("Existing installed copies remain on their machines.").print(ctx);
    }
    Ok(ExitCode::SUCCESS)
}
