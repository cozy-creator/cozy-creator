//! `cozy package publish|yank|delete`: authoring, through Calcifer (CALCIFER-API.md
//! "Packages"). The package goes by content, and Calcifer locks and publishes its copy with uv:
//! package code never runs, and the authored files never change but `uv.lock`, which the CLI writes.
use std::process::ExitCode;

use api::{
    error::{Error, Kind},
    calcifer::hub::{At, PackageDeleted, PackagePublish, PackageUnyanked, PackageYanked, Published, job},
    calcifer::project::{Benchmarks, BenchmarksAsk},
};

use super::PackageRef;
use crate::{Context, calcifer_client::path, output, run};

/// Send the current package directory by content for locking and publication. Publishing a project's package is an
/// explicit export: it goes from that project, and its published copy is outside the project's protection.
async fn sent(ctx: &Context) -> Result<(std::path::PathBuf, api::code::LocalCode, Option<String>), Error> {
    let dir = std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let (code, project) = crate::send::Sender::local_code(ctx, &dir).await?;
    if project.is_some() {
        output::warn("publishing exports this package's source: the published copy leaves its project's protection");
    }
    Ok((dir, code, project))
}

/// The `uv.lock` Calcifer made, written into the package's folder `dir` whole: its path.
pub(crate) fn written(dir: &std::path::Path, lock: &str) -> Result<std::path::PathBuf, Error> {
    let path = dir.join("uv.lock");
    platform::durable::replace(&path, lock.as_bytes(), 0o644)
        .map_err(|e| Error::new(Kind::Unavailable, "lock_unwritable", format!("{}: {e}", path.display())))?;
    Ok(path)
}

pub(crate) async fn publish(ctx: &Context, root: bool) -> Result<ExitCode, Error> {
    let (dir, code, project) = sent(ctx).await?;
    let going = run::job(ctx, job::PACKAGE_PUBLISH, &PackagePublish { root }, None, Some(code), project).await?;
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

/// `cozy package benchmarks` (cl-1024): `package`'s benchmarks from this project's warm runs of its functions on GPUs, as Calcifer
/// reads them (`GET /v1/projects/{id}/benchmarks`), written to `out` (default `benchmarks.json` here), which `cozy package publish`
/// attaches to the next release. Nothing is run.
pub(crate) async fn benchmarks(ctx: &Context, package: Option<String>, out: Option<std::path::PathBuf>) -> Result<ExitCode, Error> {
    let dir = std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let package = match package {
        Some(package) => package,
        None => named_here(&dir)?,
    };
    let Some(project) = crate::project::here(ctx).await? else {
        return Err(Error::new(Kind::Usage, "not_in_a_project", "benchmarks come from a project's runs: run this inside the project that ran them")
            .next("cozy run ls"));
    };
    let at = path(&["v1", "projects", &project, "benchmarks"]);
    let found: Benchmarks = ctx.calcifer().await?.get_query(&at, &BenchmarksAsk { package: package.clone() }).await?;
    if found.benchmarks.is_empty() {
        return Err(Error::new(Kind::NotFound, "no_benchmarks", format!("this project has no warm run of {package} on a GPU"))
            .remedy("run each function to measure twice on the card, the second warm")
            .next(format!("cozy run {package}/<function> … --on <machine>")));
    }
    let path = out.unwrap_or_else(|| dir.join("benchmarks.json"));
    let text = serde_json::to_string_pretty(&found.benchmarks).expect("benchmarks serialize") + "\n";
    platform::durable::replace(&path, text.as_bytes(), 0o644)
        .map_err(|e| Error::new(Kind::Unavailable, "benchmarks_unwritable", format!("{}: {e}", path.display())))?;
    match ctx.json {
        true => output::json(&found),
        false => output::say(ctx, &format!("wrote {} benchmarks of {package} to {}", found.benchmarks.len(), path.display())),
    }
    Ok(ExitCode::SUCCESS)
}

/// The package `dir` is: its pyproject.toml's `[project] name`.
fn named_here(dir: &std::path::Path) -> Result<String, Error> {
    let none = || Error::new(Kind::Usage, "package_unnamed", "no package here: name one, or run this in a package's folder").next("cozy package benchmarks <package>");
    let text = std::fs::read_to_string(dir.join("pyproject.toml")).map_err(|_| none())?;
    let document: toml::Table = text.parse().map_err(|_| none())?;
    document.get("project").and_then(|project| project.get("name")).and_then(toml::Value::as_str).map(str::to_owned).ok_or_else(none)
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
