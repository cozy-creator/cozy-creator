//! Current-directory project lookup and explicit project creation.
use std::{path::PathBuf, process::ExitCode};

use api::{
    calcifer::{hub::At, project::{Event, Open, Opened, State}},
    error::{Error, Kind},
};

use crate::{Context, output, run::mint_key};

pub(crate) mod vault;

pub(crate) fn directory() -> Result<String, Error> {
    let directory = std::env::current_dir()
        .map_err(|error| Error::new(Kind::Unavailable, "cwd", format!("the current directory: {error}")))?;
    directory.into_os_string().into_string()
        .map_err(|_| Error::new(Kind::Usage, "project_path", "the current directory is not a UTF-8 path"))
}

/// The existing project containing the current directory.
pub(crate) async fn current(ctx: &Context) -> Result<State, Error> {
    let project = directory()?;
    let path = format!("{}?assets={}", crate::calcifer_client::path(&["v1", "projects", &project]), ctx.full);
    match ctx.calcifer().await?.get::<State>(&path).await {
        Err(error) if error.code == "project_not_found" => Err(Error::new(
            Kind::Usage,
            "not_in_a_project",
            format!("{project} is outside a Cozy project"),
        ).next("cozy new")),
        answer => answer,
    }
}

/// Create a project in the directory named, or in the current directory.
#[derive(clap::Args, Debug)]
pub(crate) struct New {
    /// The project directory, created when it does not exist.
    #[arg(default_value = ".", value_hint = clap::ValueHint::DirPath)]
    directory: PathBuf,
}

pub(crate) async fn create(ctx: &Context, args: New) -> Result<ExitCode, Error> {
    let directory = std::path::absolute(&args.directory)
        .map_err(|error| Error::new(Kind::Usage, "project_path", error.to_string()))?;
    let request = Open { path: Some(directory), ..Open::default() };
    let mut opened: Opened = ctx.calcifer().await?.post("/v1/projects", &request, Some(&mint_key())).await?;
    opened.next.clear();
    if ctx.json {
        output::json(&opened);
    } else {
        println!("Created project {}", opened.path.display());
        opened.said.iter().for_each(|said| output::warn(said));
    }
    Ok(ExitCode::SUCCESS)
}

/// Sync the current project's files through its existing transfer API.
pub(crate) async fn transfer(ctx: &Context, verb: &str, moved: &str) -> Result<ExitCode, Error> {
    let project = current(ctx).await?;
    let calcifer = ctx.calcifer().await?;
    let events_path = crate::calcifer_client::path(&["v1", "projects", &project.id, "events"]);
    let action_path = crate::calcifer_client::path(&["v1", "projects", &project.id, verb]);
    let mut events = calcifer.watch::<Event>(&events_path).await?;
    let ask = At { hub: Some(ctx.hub().to_string()) };
    let _: Opened = calcifer.post(&action_path, &ask, None).await?;
    while let Some(event) = events.next().await {
        match event {
            Event::Moved { file: Some(file), bytes } => output::say(ctx, &format!("{moved} {file} ({} so far)", output::bytes(bytes))),
            Event::Moved { file: None, bytes } => output::say(ctx, &format!("{} so far", output::bytes(bytes))),
            Event::Transferred { error: Some(error), .. } => return Err(error),
            Event::Transferred { done: Some(done), .. } => {
                if ctx.json {
                    output::json(&done);
                } else {
                    let rest = done.files - done.moved - done.skipped.len() as u64;
                    println!("{} files: {} {moved} ({}), {rest} already present", done.files, done.moved, output::bytes(done.bytes));
                }
                for file in &done.skipped {
                    output::warn(&format!("skipped {}: {}", file.path, file.why));
                }
                return Ok(if done.skipped.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(3) });
            }
            _ => {}
        }
    }
    let message = format!("Calcifer stopped before the {verb} ended; run it again to continue");
    Err(Error::new(Kind::Unavailable, "project_events_ended", message).next(format!("cozy {verb}")))
}
