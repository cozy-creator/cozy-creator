//! Current-directory project lookup and explicit project creation.
use std::{io::IsTerminal as _, path::PathBuf, process::ExitCode};

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

/// Identify the current project without evaluating its canvas or checking its files.
pub(crate) async fn identity(ctx: &Context) -> Result<api::calcifer::project::Address, Error> {
    let directory = directory()?;
    let path = crate::calcifer_client::path(&["v1", "projects", &directory, "address"]);
    match ctx.calcifer().await?.get(&path).await {
        Err(error) if error.code == "project_not_found" => Err(Error::new(
            Kind::Usage,
            "not_in_a_project",
            format!("{directory} is outside a Cozy project"),
        ).next("cozy new")),
        answer => answer,
    }
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
    /// Explicitly create ordinary local storage without encryption.
    #[arg(long, conflicts_with_all = ["passphrase_file", "recovery_out"])]
    unencrypted: bool,
    /// Read the new passphrase from a file or - for stdin instead of a local terminal prompt.
    #[arg(long, value_name = "FILE")]
    passphrase_file: Option<PathBuf>,
    /// Save the recovery secret to a new private file outside the project. Otherwise it is shown once.
    #[arg(long, value_name = "FILE")]
    recovery_out: Option<PathBuf>,
}

pub(crate) async fn create(ctx: &Context, args: New) -> Result<ExitCode, Error> {
    let directory = std::path::absolute(&args.directory)
        .map_err(|error| Error::new(Kind::Usage, "project_path", error.to_string()))?;
    let passphrase = if args.unencrypted { None } else {
        if args.passphrase_file.is_none() && !std::io::stdin().is_terminal() {
            return Err(Error::new(Kind::Usage, "vault_credential", "new projects are encrypted; a noninteractive caller must supply a passphrase file or explicitly choose unencrypted storage")
                .next("cozy new --passphrase-file <file>").next("cozy new --unencrypted"));
        }
        if let Some(output) = &args.recovery_out {
            if output.exists() { return Err(Error::new(Kind::Conflict, "vault_recovery_exists", "the recovery output file already exists; choose a new path")); }
            let status = api::calcifer::project::VaultStatus { encrypted: true, state: "new".into(), path: directory.clone(), view: None,
                expires_at_ms: None, idle_timeout_s: 900, next: vec![] };
            vault::recovery_location(output, &status)?;
        }
        Some(vault::credential(args.passphrase_file.as_ref(), "New project passphrase: ", true)?)
    };
    let request = Open { path: Some(directory.clone()), unencrypted: args.unencrypted, passphrase, ..Open::default() };
    let client = ctx.calcifer().await?;
    let key = args.unencrypted.then(mint_key);
    let (id, code, path, created, mut recovery_secret) = match client.post::<_, Opened>("/v1/projects", &request, key.as_deref()).await {
        Ok(opened) => (opened.id, opened.code, opened.path, true, opened.recovery_secret),
        Err(error) if error.code == "project_exists" => {
            let path = crate::calcifer_client::path(&["v1", "projects", &directory.to_string_lossy()]);
            let state: State = client.get(&format!("{path}?assets=false")).await?;
            (state.id, state.code, state.path, false, None)
        }
        Err(error) => return Err(error),
    };
    let label = if code.is_empty() { &id } else { &code };
    let message = if created {
        format!("Created project {label} at {}.", path.display())
    } else {
        format!("Project not created. {} already contains project {label}.", path.display())
    };
    if let (Some(output), Some(secret)) = (&args.recovery_out, &recovery_secret) {
        vault::save_recovery(output, secret)?;
        recovery_secret = None;
        output::say(ctx, &format!("recovery secret saved to {}; keep a copy off this computer", output.display()));
    }
    let mut document = serde_json::json!({ "id": id, "code": code, "path": path, "created": created, "message": message });
    if ctx.json {
        if let Some(secret) = recovery_secret { document["recovery_secret"] = secret.into(); }
        output::document(ctx, &document);
    } else {
        output::view(ctx, &document, &["message"]);
        if let Some(secret) = recovery_secret { vault::show_recovery(ctx, &secret); }
    }
    Ok(ExitCode::SUCCESS)
}
