//! Local encryption credentials are read from a terminal or an explicit file/stdin, never argv or environment variables.
use std::{fs::OpenOptions, io::{Read as _, Write as _}, path::PathBuf, process::ExitCode};
use api::{calcifer::project::{VaultAsk, VaultChanged, VaultStatus}, error::{Error, Kind}};
use crate::{Context, output};

#[derive(clap::Args, Debug)]
pub(super) struct Encrypt {
    /// Read the new passphrase from this file instead of prompting. Use - for stdin.
    #[arg(long, value_name = "FILE")]
    passphrase_file: Option<PathBuf>,
    /// Save the new recovery secret to a new, owner-only file. Otherwise it is shown once.
    #[arg(long, value_name = "FILE")]
    recovery_out: Option<PathBuf>,
    /// Remove the verified plaintext source after conversion. Without this it is preserved and named.
    #[arg(long)]
    remove_plaintext: bool,
    /// Lock after this many seconds without activity. 0 disables idle expiry.
    #[arg(long, default_value_t = 900)]
    idle: u64,
    /// Maximum unlock duration in seconds, even while active. 0 disables the maximum.
    #[arg(long, default_value_t = 0)]
    max_duration: u64,
}

#[derive(clap::Args, Debug)]
pub(super) struct Unlock {
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
pub(super) struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
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
    /// Explicitly export a plaintext copy to a new folder. The source remains encrypted.
    Export {
        #[arg(long, value_name = "DIRECTORY")]
        output: PathBuf,
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
    VaultAsk { action: action.into(), passphrase: None, recovery_secret: None, idle_timeout_s: None, max_duration_s: None, remove_plaintext: false, output: None }
}

fn credential(path: Option<&PathBuf>, prompt: &str, confirm: bool) -> Result<String, Error> {
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

async fn change(ctx: &Context, project: Option<&String>, request: VaultAsk, recovery_out: Option<PathBuf>) -> Result<ExitCode, Error> {
    if recovery_out.as_ref().is_some_and(|path| path.exists()) {
        return Err(Error::new(Kind::Conflict, "vault_recovery_exists", "the recovery output file already exists; choose a new path"));
    }
    let path = super::at(&super::named(project)?, &["vault"]);
    // No client idempotency cache for responses containing a recovery secret.
    let mut changed: VaultChanged = ctx.calcifer().await?.post(&path, &request, None).await?;
    if let (Some(path), Some(secret)) = (&recovery_out, &changed.recovery_secret) {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt as _; options.mode(0o600); }
        let mut file = options.open(path).map_err(|e| Error::new(Kind::Unavailable, "vault_recovery_save", format!("cannot save the recovery secret to {}: {e}; keep the project unlocked and replace the recovery secret", path.display())))?;
        writeln!(file, "{secret}").and_then(|()| file.sync_all()).map_err(|e| Error::new(Kind::Unavailable, "vault_recovery_save", format!("cannot save the recovery secret: {e}")))?;
        changed.recovery_secret = None;
        output::say(ctx, &format!("recovery secret saved to {}; keep a copy off this computer", path.display()));
    }
    if ctx.json { output::json(&changed); return Ok(ExitCode::SUCCESS); }
    show(ctx, &changed.vault);
    if let Some(source) = changed.preserved_source {
        output::warn(&format!("plaintext source preserved at {}; this copy is not protected by the vault", source.display()));
    }
    if let Some(secret) = changed.recovery_secret {
        output::say(ctx, "Save this recovery secret on another device or in your password manager. TensorHub does not receive it:");
        output::say(ctx, &secret);
    }
    Ok(ExitCode::SUCCESS)
}

fn show(ctx: &Context, status: &VaultStatus) {
    output::say(ctx, &format!("{}: {}", status.path.display(), status.state));
    if let Some(view) = &status.view { output::say(ctx, &format!("readable view: {}", view.display())); }
    output::nexts(ctx, &status.next);
}

pub(super) async fn encrypt(ctx: &Context, project: Option<&String>, args: Encrypt) -> Result<ExitCode, Error> {
    let mut request = ask("encrypt");
    request.passphrase = Some(credential(args.passphrase_file.as_ref(), "New project passphrase: ", true)?);
    request.idle_timeout_s = Some(args.idle);
    request.max_duration_s = Some(args.max_duration);
    request.remove_plaintext = args.remove_plaintext;
    change(ctx, project, request, args.recovery_out).await
}

pub(super) async fn unlock(ctx: &Context, project: Option<&String>, args: Unlock) -> Result<ExitCode, Error> {
    let mut request = ask("unlock");
    if let Some(path) = &args.recovery_file { request.recovery_secret = Some(credential(Some(path), "Recovery secret: ", false)?); }
    else { request.passphrase = Some(credential(args.passphrase_file.as_ref(), "Project passphrase: ", false)?); }
    request.idle_timeout_s = Some(args.idle);
    request.max_duration_s = Some(args.max_duration);
    change(ctx, project, request, None).await
}

pub(super) async fn lock(ctx: &Context, project: Option<&String>) -> Result<ExitCode, Error> {
    change(ctx, project, ask("lock"), None).await
}

pub(super) async fn exec(ctx: &Context, project: Option<&String>, args: Args) -> Result<ExitCode, Error> {
    match args.command {
        Command::Status => {
            let path = super::at(&super::named(project)?, &["vault"]);
            let status: VaultStatus = ctx.calcifer().await?.get(&path).await?;
            if ctx.json { output::json(&status); } else { show(ctx, &status); }
            Ok(ExitCode::SUCCESS)
        }
        Command::Passphrase { passphrase_file } => {
            let mut request = ask("passphrase");
            request.passphrase = Some(credential(passphrase_file.as_ref(), "New project passphrase: ", true)?);
            change(ctx, project, request, None).await
        }
        Command::Recovery { recovery_out } => change(ctx, project, ask("recovery"), recovery_out).await,
        Command::Export { output } => { let mut request = ask("export"); request.output = Some(output); change(ctx, project, request, None).await }
        Command::Rotate { passphrase_file, recovery_out } => {
            let mut request = ask("rotate");
            request.passphrase = Some(credential(passphrase_file.as_ref(), "Passphrase for the new key generation: ", true)?);
            change(ctx, project, request, recovery_out).await
        }
    }
}
