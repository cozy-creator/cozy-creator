//! Local encryption credentials are read from a terminal or an explicit file/stdin, never argv or environment variables.
use std::{io::{Read as _, Write as _}, path::{Path, PathBuf}, process::ExitCode};
use api::{calcifer::project::{VaultAsk, VaultChanged, VaultStatus}, error::{Error, Kind}};
use crate::{Context, output};

#[derive(clap::Args, Debug)]
struct Encrypt {
    /// Read the new passphrase from this file instead of prompting. Use - for stdin.
    #[arg(long, value_name = "FILE")]
    passphrase_file: Option<PathBuf>,
    /// Save the new recovery secret to a new, owner-only file. Otherwise it is shown once.
    #[arg(long, value_name = "FILE")]
    recovery_out: Option<PathBuf>,
    /// Remove the verified plaintext source after conversion. Without this it is preserved and named.
    #[arg(long)]
    remove_plaintext: bool,
    /// Lock after this many seconds without activity; the project's runs and renders are activity. 0 disables idle expiry.
    #[arg(long, default_value_t = 900)]
    idle: u64,
    /// Maximum unlock duration in seconds, even while active; reaching it ends the project's runs, as a lock does. 0 disables the maximum.
    #[arg(long, default_value_t = 0)]
    max_duration: u64,
}

#[derive(clap::Args, Debug)]
struct Unlock {
    /// Read the passphrase from this file instead of prompting. Use - for stdin.
    #[arg(long, value_name = "FILE", conflicts_with = "recovery_file")]
    passphrase_file: Option<PathBuf>,
    /// Use a saved recovery secret from this file. Use - for stdin.
    #[arg(long, value_name = "FILE")]
    recovery_file: Option<PathBuf>,
    #[arg(long, default_value_t = 900)]
    idle: u64,
    #[arg(long, default_value_t = 0)]
    max_duration: u64,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// A project directory when working from elsewhere. On Windows, leave the project directory before encrypting, decrypting or rotating.
    #[arg(long, value_name = "DIRECTORY", value_hint = clap::ValueHint::DirPath)]
    directory: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Encrypt the current project and open its readable view.
    Encrypt(Encrypt),
    /// Unlock with a passphrase or saved recovery secret.
    Unlock(Unlock),
    /// Lock the current project and unmount its readable view, ending its runs and renders first.
    Lock,
    /// Decrypt the unlocked project in place, returning it to ordinary local storage.
    Decrypt,
    /// Encryption status and the readable mount path, if unlocked.
    Status,
    /// Change the passphrase of an unlocked project without rewriting its files.
    Passphrase {
        #[arg(long, value_name = "FILE")]
        passphrase_file: Option<PathBuf>,
    },
    /// Replace a lost recovery secret on an unlocked project. Existing copied old keybags remain outside this change.
    Recovery {
        #[arg(long, value_name = "FILE")]
        recovery_out: Option<PathBuf>,
    },
    /// Re-encrypt all retained local content using a fresh key. Requires the project to be unlocked.
    Rotate {
        #[arg(long, value_name = "FILE")]
        passphrase_file: Option<PathBuf>,
        #[arg(long, value_name = "FILE")]
        recovery_out: Option<PathBuf>,
    },
}

fn ask(action: &str) -> VaultAsk {
    VaultAsk { action: action.into(), passphrase: None, recovery_secret: None, idle_timeout_s: None, max_duration_s: None, remove_plaintext: false }
}

pub(super) fn credential(path: Option<&PathBuf>, prompt: &str, confirm: bool) -> Result<String, Error> {
    let io = |e: std::io::Error| Error::new(Kind::Usage, "vault_credential", format!("cannot read the credential: {e}"));
    let value = if let Some(path) = path {
        let mut bytes = Vec::new();
        if path.as_os_str() == "-" {
            std::io::stdin().take(4097).read_to_end(&mut bytes).map_err(io)?;
        } else {
            std::fs::File::open(path).map_err(io)?.take(4097).read_to_end(&mut bytes).map_err(io)?;
        }
        if bytes.len() > 4096 { return Err(Error::new(Kind::Usage, "vault_credential", "credential exceeds 4096 bytes")); }
        String::from_utf8(bytes).map_err(|_| Error::new(Kind::Usage, "vault_credential", "credential must be UTF-8"))?.trim_end_matches(['\r', '\n']).to_owned()
    } else {
        let value = rpassword::prompt_password(prompt).map_err(io)?;
        if confirm && value != rpassword::prompt_password("Again: ").map_err(io)? {
            return Err(Error::new(Kind::Usage, "vault_passphrase_mismatch", "the passphrases do not match"));
        }
        value
    };
    if value.is_empty() { return Err(Error::new(Kind::Usage, "vault_credential", "an empty credential is not accepted")); }
    Ok(value)
}

pub(super) fn recovery_location(output: &Path, status: &VaultStatus) -> Result<(), Error> {
    let refuse = |why: &str| Error::new(Kind::Usage, "vault_recovery_location", why)
        .remedy("Save the recovery secret outside the project, with a copy on another device");
    let name = output.file_name().ok_or_else(|| refuse("recovery output must name a new file"))?;
    let parent = output.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let parent = platform::durable::canonical(parent).map_err(|_| refuse("the recovery output parent must already exist locally"))?;
    if !parent.is_dir() { return Err(refuse("the recovery output parent must be a local directory")); }
    let destination = parent.join(name);
    // Recovery output belongs to the client; a remote Calcifer's paths may not exist here.
    for root in std::iter::once(&status.path).chain(status.view.iter()) {
        if let Ok(root) = platform::durable::canonical(root)
            && destination.starts_with(root)
        {
            return Err(refuse("The recovery secret cannot be saved inside the project or its unlocked view"));
        }
    }
    Ok(())
}

pub(super) fn save_recovery(path: &Path, secret: &str) -> Result<(), Error> {
    let failed = |e: String| Error::new(Kind::Unavailable, "vault_recovery_save", format!("Cannot save the recovery secret to {}: {e}", path.display()))
        .remedy("Keep the project unlocked and replace its recovery secret")
        .next("cozy vault recovery --recovery-out <file>");
    let mut file = platform::durable::create_private_new(path).map_err(|e| failed(e.to_string()))?;
    writeln!(file, "{secret}").and_then(|()| file.sync_all()).map_err(|e| failed(e.to_string()))
}

pub(super) fn show_recovery(ctx: &Context, secret: &str) {
    output::say(ctx, "Save this recovery secret on another device or in your password manager. TensorHub does not receive it:");
    output::say(ctx, secret);
}

fn vault_path(directory: Option<PathBuf>) -> Result<String, Error> {
    // A locked project has no readable state yet. Address the vault by directory.
    let directory = match directory {
        Some(directory) => std::path::absolute(directory)
            .map_err(|e| Error::new(Kind::Usage, "project_path", e.to_string()))?
            .into_os_string().into_string()
            .map_err(|_| Error::new(Kind::Usage, "project_path", "the directory is not a UTF-8 path"))?,
        None => super::directory()?,
    };
    Ok(crate::calcifer_client::path(&["v1", "projects", &directory, "vault"]))
}

async fn change(ctx: &Context, path: &str, request: VaultAsk, recovery_out: Option<PathBuf>) -> Result<ExitCode, Error> {
    if recovery_out.as_ref().is_some_and(|path| path.exists()) {
        return Err(Error::new(Kind::Conflict, "vault_recovery_exists", "the recovery output file already exists; choose a new path"));
    }
    let client = ctx.calcifer().await?;
    if let Some(output) = &recovery_out {
        let status: VaultStatus = client.get(path).await?;
        recovery_location(output, &status)?;
    }
    let mut changed = request_vault(ctx, &client, path, &request).await?;
    if let (Some(path), Some(secret)) = (&recovery_out, &changed.recovery_secret) {
        save_recovery(path, secret)?;
        changed.recovery_secret = None;
        output::say(ctx, &format!("recovery secret saved to {}; keep a copy off this computer", path.display()));
    }
    for skipped in &changed.skipped_files {
        output::warn(&format!("retained-media link skipped: {}; its project reference was preserved", skipped.display()));
    }
    if ctx.json { output::json(&changed); return Ok(ExitCode::SUCCESS); }
    if !changed.ended.is_empty() {
        let runs: Vec<String> = changed.ended.iter().map(u64::to_string).collect();
        output::say(ctx, &format!("ended the project's runs {} before locking", runs.join(", ")));
    }
    show(ctx, &changed.vault);
    if let Some(source) = changed.preserved_source {
        let protection = if request.action == "encrypt" { "this source is still plaintext" } else { "its original protection is unchanged" };
        output::warn(&format!("source preserved at {}; {protection}", source.display()));
    }
    if let Some(secret) = changed.recovery_secret {
        show_recovery(ctx, &secret);
    }
    Ok(ExitCode::SUCCESS)
}

/// The private stream and progress renderer are shared without command result/status output.
async fn request_vault(ctx: &Context, client: &crate::calcifer_client::Client, path: &str, request: &VaultAsk) -> Result<VaultChanged, Error> {
    // No client idempotency cache for responses containing a recovery secret.
    let meter = crate::progress::Transfer::stderr(ctx.term.stderr, format!("vault {}", request.action));
    let result = client.vault(path, request, |progress| {
        use crate::calcifer_client::VaultProgress;
        match progress {
            VaultProgress::Accepted { .. } => meter.finalizing("accepted"),
            VaultProgress::Notice { message } => meter.println(message),
            VaultProgress::Progress { phase, files_done, bytes_done, files_total, bytes_total } => {
                meter.update(crate::progress::TransferCounts {
                    files_done, files_total: (files_total > 0).then_some(files_total),
                    bytes_done, bytes_total: (bytes_total > 0).then_some(bytes_total), bytes_moved: bytes_done,
                });
                match phase {
                    "copying" | "verifying" => meter.phase(phase),
                    _ => meter.finalizing(phase),
                }
            }
        }
    }).await;
    meter.finish_and_clear();
    result
}

/// Called only after Open verified an interactive terminal. No status or secret preamble reaches stdout.
pub(crate) async fn unlock_for_open(ctx: &Context) -> Result<(), Error> {
    let path = vault_path(None)?;
    let mut request = ask("unlock");
    request.passphrase = Some(credential(None, "Project passphrase: ", false)?);
    let client = ctx.calcifer().await?;
    let changed = request_vault(ctx, &client, &path, &request).await?;
    if !changed.vault.encrypted || changed.vault.state != "unlocked" {
        return Err(Error::new(Kind::Conflict, "project_locked", "the project did not remain unlocked; try unlocking it again")
            .next("cozy vault unlock"));
    }
    Ok(())
}

fn show(ctx: &Context, status: &VaultStatus) {
    output::say(ctx, &format!("{}: {}", status.path.display(), status.state));
    if let Some(view) = &status.view { output::say(ctx, &format!("readable view: {}", output::link(ctx, view))); }
    output::nexts(ctx, &status.next);
}

async fn encrypt(ctx: &Context, path: &str, args: Encrypt) -> Result<ExitCode, Error> {
    output::warn("Encryption does not erase earlier plaintext copies, backups or shared cache copies. Protect or remove them separately.");
    let mut request = ask("encrypt");
    request.passphrase = Some(credential(args.passphrase_file.as_ref(), "New project passphrase: ", true)?);
    request.idle_timeout_s = Some(args.idle);
    request.max_duration_s = Some(args.max_duration);
    request.remove_plaintext = args.remove_plaintext;
    change(ctx, path, request, args.recovery_out).await
}

async fn unlock(ctx: &Context, path: &str, args: Unlock) -> Result<ExitCode, Error> {
    let mut request = ask("unlock");
    if let Some(path) = &args.recovery_file { request.recovery_secret = Some(credential(Some(path), "Recovery secret: ", false)?); }
    else { request.passphrase = Some(credential(args.passphrase_file.as_ref(), "Project passphrase: ", false)?); }
    request.idle_timeout_s = Some(args.idle);
    request.max_duration_s = Some(args.max_duration);
    change(ctx, path, request, None).await
}

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    let path = vault_path(args.directory)?;
    match args.command {
        Command::Encrypt(args) => encrypt(ctx, &path, args).await,
        Command::Unlock(args) => unlock(ctx, &path, args).await,
        Command::Lock => change(ctx, &path, ask("lock"), None).await,
        Command::Decrypt => change(ctx, &path, ask("decrypt"), None).await,
        Command::Status => {
                let status: VaultStatus = ctx.calcifer().await?.get(&path).await?;
            if ctx.json { output::json(&status); } else { show(ctx, &status); }
            Ok(ExitCode::SUCCESS)
        }
        Command::Passphrase { passphrase_file } => {
            let mut request = ask("passphrase");
            request.passphrase = Some(credential(passphrase_file.as_ref(), "New project passphrase: ", true)?);
            change(ctx, &path, request, None).await
        }
        Command::Recovery { recovery_out } => change(ctx, &path, ask("recovery"), recovery_out).await,
        Command::Rotate { passphrase_file, recovery_out } => {
            let mut request = ask("rotate");
            request.passphrase = Some(credential(passphrase_file.as_ref(), "Passphrase for the new key generation: ", true)?);
            change(ctx, &path, request, recovery_out).await
        }
    }
}
