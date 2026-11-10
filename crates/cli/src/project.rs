//! Current-directory project lookup and explicit project creation.
use std::{path::PathBuf, process::ExitCode};

use api::{
    calcifer::project::{Open, Opened, State},
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
    let request = Open { path: Some(directory.clone()), ..Open::default() };
    let client = ctx.calcifer().await?;
    let (id, code, path, created) = match client.post::<_, Opened>("/v1/projects", &request, Some(&mint_key())).await {
        Ok(opened) => (opened.id, opened.code, opened.path, true),
        Err(error) if error.code == "project_exists" => {
            let path = crate::calcifer_client::path(&["v1", "projects", &directory.to_string_lossy()]);
            let state: State = client.get(&format!("{path}?assets=false")).await?;
            (state.id, state.code, state.path, false)
        }
        Err(error) => return Err(error),
    };
    let label = if code.is_empty() { &id } else { &code };
    let message = if created {
        format!("Created project {label} at {}.", path.display())
    } else {
        format!("Project not created. {} already contains project {label}.", path.display())
    };
    let document = serde_json::json!({ "id": id, "code": code, "path": path, "created": created, "message": message });
    if ctx.json {
        output::document(ctx, &document);
    } else {
        output::view(ctx, &document, &["message"]);
    }
    Ok(ExitCode::SUCCESS)
}
