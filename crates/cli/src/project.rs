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

/// Identify the current project without evaluating its canvas or checking its files, unlocking it first at a terminal when it is locked.
pub(crate) async fn identity(ctx: &Context) -> Result<api::calcifer::project::Address, Error> {
    unlocking(ctx, || address(ctx)).await
}

/// The current project's address as it stands, asking nothing: for what reads a project only when one is open.
pub(crate) async fn address(ctx: &Context) -> Result<api::calcifer::project::Address, Error> {
    let directory = directory()?;
    let path = crate::calcifer_client::path(&["v1", "projects", &directory, "address"]);
    ctx.calcifer().await?.get(&path).await.map_err(|error: Error| match error.code.as_str() {
        "project_not_found" => not_in_a_project(&directory),
        _ => error,
    })
}

/// `read`, with a locked project unlocked first at a terminal, as `cozy open` does (cl-1002): every verb goes on. `--json` and pipes never
/// ask for a credential: they get `project_locked`, which names `cozy vault unlock`. A project busy locking or converting is not locked:
/// its refusal stands, and nothing is asked.
async fn unlocking<T, F, Fut>(ctx: &Context, read: F) -> Result<T, Error>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, Error>>,
{
    match read().await {
        Err(error) if error.code == "project_locked" && !ctx.json && std::io::stdin().is_terminal() && ctx.term.stderr => {
            if !vault::locked(ctx).await {
                return Err(error);
            }
            vault::unlock_for_open(ctx).await?;
            read().await
        }
        answer => answer,
    }
}

/// The current project's id, for a run: outside any project, refused as a run is (cl-1028: every run is kept in its project).
pub(crate) async fn for_run(ctx: &Context) -> Result<String, Error> {
    match identity(ctx).await {
        Ok(project) => Ok(project.id),
        Err(error) if error.code == "not_in_a_project" => Err(api::calcifer::project::unnamed()),
        Err(error) => Err(error),
    }
}

/// The current project's id; None outside any project.
pub(crate) async fn here(ctx: &Context) -> Result<Option<String>, Error> {
    match identity(ctx).await {
        Ok(project) => Ok(Some(project.id)),
        Err(error) if error.code == "not_in_a_project" => Ok(None),
        Err(error) => Err(error),
    }
}

/// The existing project containing the current directory.
pub(crate) async fn current(ctx: &Context) -> Result<State, Error> {
    let project = directory()?;
    let path = format!("{}?assets={}", crate::calcifer_client::path(&["v1", "projects", &project]), ctx.full);
    unlocking(ctx, || async { ctx.calcifer().await?.get::<State>(&path).await }).await.map_err(|error| match error.code.as_str() {
        "project_not_found" => not_in_a_project(&project),
        _ => error,
    })
}

fn not_in_a_project(directory: &str) -> Error {
    let name = std::path::Path::new(directory).file_name().and_then(|name| name.to_str()).unwrap_or(directory);
    Error::new(Kind::Usage, "not_in_a_project", format!("{name:?} is not a Cozy Creator project."))
        .remedy("Navigate to a project directory, or create one with cozy new.")
        .next("cozy new")
}

/// Show or change the current project's key (cl-987): its runs are `KEY-N`, `SE-5`, named so from any folder.
#[derive(clap::Args, Debug)]
pub(crate) struct KeyArgs {
    /// Its new key: 2 to 10 letters and digits, a letter first. The key it had still names its runs.
    key: Option<String>,
}

pub(crate) async fn key(ctx: &Context, args: KeyArgs) -> Result<ExitCode, Error> {
    let project = identity(ctx).await?.id;
    let path = crate::calcifer_client::path(&["v1", "projects", &project, "key"]);
    let client = ctx.calcifer().await?;
    let answer: api::calcifer::project::Key = match args.key {
        None => client.get(&path).await?,
        Some(key) => client.put(&path, &api::calcifer::project::Key { key, former: vec![] }).await?,
    };
    if ctx.json {
        output::json(&answer);
        return Ok(ExitCode::SUCCESS);
    }
    println!("{}: its runs are {}-1, {}-2, …", answer.key, answer.key, answer.key);
    if !answer.former.is_empty() {
        println!("formerly {}, which name them still", answer.former.join(", "));
    }
    Ok(ExitCode::SUCCESS)
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
    // Repeating New on an existing project reads its identity; it never asks for a new credential or changes storage.
    let existing = if ["project.db", "project.vault.keys.json"].iter().any(|file| directory.join(file).exists()) {
        let client = ctx.calcifer().await?;
        let path = crate::calcifer_client::path(&["v1", "projects", &directory.to_string_lossy(), "address"]);
        Some(client.get::<api::calcifer::project::Address>(&path).await?)
    } else { None };
    let client = ctx.calcifer().await?;
    let passphrase = if args.unencrypted || existing.is_some() { None } else {
        if args.passphrase_file.is_none() && !std::io::stdin().is_terminal() {
            return Err(Error::new(Kind::Usage, "vault_credential", "new projects are encrypted; a noninteractive caller must supply a passphrase file or explicitly choose unencrypted storage")
                .next("cozy new --passphrase-file <file>").next("cozy new --unencrypted"));
        }
        if let Some(output) = &args.recovery_out {
            if output.exists() { return Err(Error::new(Kind::Conflict, "vault_recovery_exists", "the recovery output file already exists; choose a new path")); }
            let status = api::calcifer::project::VaultStatus { encrypted: true, state: "new".into(), path: directory.clone(), view: None,
                expires_at_ms: None, idle_timeout_s: api::calcifer::project::DEFAULT_VAULT_IDLE_SECONDS, next: vec![] };
            vault::recovery_location(output, &status)?;
        }
        Some(vault::credential(args.passphrase_file.as_ref(), "New project passphrase: ", true)?)
    };
    let creating = (!args.unencrypted && existing.is_none()).then(|| {
        let meter = crate::progress::Transfer::stderr(ctx.term.stderr, "Creating encrypted project");
        meter.finalizing("Preparing encrypted storage");
        meter
    });
    let request = Open { path: directory.clone(), unencrypted: args.unencrypted, passphrase, ..Open::default() };
    let key = args.unencrypted.then(mint_key);
    if existing.is_none() && request.passphrase.is_some() { client.own_account().await?; }
    let (id, code, key, path, created, mut recovery_secret) = if let Some(address) = existing {
        (address.id, address.code, address.key, directory.clone(), false, None)
    } else { match if args.unencrypted {
        client.post::<_, Opened>("/v1/projects", &request, key.as_deref()).await
    } else {
        client.private_post::<_, Opened>("/v1/projects", &request).await
    } {
        Ok(opened) => (opened.id, opened.code, opened.key, opened.path, true, opened.recovery_secret),
        Err(error) if error.code == "project_exists" => {
            let path = crate::calcifer_client::path(&["v1", "projects", &directory.to_string_lossy()]);
            let state: State = client.get(&format!("{path}?assets=false")).await?;
            (state.id, state.code, state.key, state.path, false, None)
        }
        Err(error) => return Err(error),
    } };
    // Its readable view, when it has one now; the recovery secret it was made with is kept whatever the project's state.
    let view = match created && !args.unencrypted {
        true => {
            let status_path = crate::calcifer_client::path(&["v1", "projects", &directory.to_string_lossy(), "vault"]);
            client.get::<api::calcifer::project::VaultStatus>(&status_path).await.ok().and_then(|status| status.view)
        }
        false => None,
    };
    if let Some(meter) = &creating { meter.finish_and_clear(); }
    let label = if code.is_empty() { &id } else { &code };
    let message = if created {
        format!("Created project {label} at {}. Its runs are {key}-1, {key}-2, ….", path.display())
    } else {
        format!("Project not created. {} already contains project {label}, whose runs are {key}-N.", path.display())
    };
    if let (Some(output), Some(secret)) = (&args.recovery_out, &recovery_secret) {
        vault::save_recovery(output, secret)?;
        recovery_secret = None;
        output::say(ctx, &format!("recovery secret saved to {}; keep a copy off this computer", output.display()));
    }
    let mut document = serde_json::json!({ "id": id, "code": code, "key": key, "path": path, "created": created, "message": message });
    if let Some(view) = view { document["view"] = serde_json::json!(view); }
    if ctx.json {
        if let Some(secret) = recovery_secret { document["recovery_secret"] = secret.into(); }
        output::document(ctx, &document);
    } else {
        output::view(ctx, &document, &["message"]);
        if let Some(view) = document["view"].as_str() { output::say(ctx, &format!("readable view: {}", output::link(ctx, std::path::Path::new(view)))); }
        if let Some(secret) = recovery_secret { vault::show_recovery(ctx, &secret); }
    }
    Ok(ExitCode::SUCCESS)
}
