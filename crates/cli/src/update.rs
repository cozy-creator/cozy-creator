//! `cozy update [VERSION]` (design §5): replaces this binary with a release's, verified as
//! `platform::release` verifies one, and restarts Calcifer on it; the local machine moves to it once
//! idle. `download_url` (`COZY_DOWNLOAD_URL`, which dist's installers read too) serves one release's
//! files in place of GitHub's.
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    process::ExitCode,
};

use platform::release::{self, Release, TARGET};
use protocol::error::{Error, Kind};
use serde::Serialize;

use crate::{Context, machine::Adopted};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// The release to install, even when it is this one (default: the newest, when newer).
    version: Option<String>,
}

#[derive(Serialize)]
struct Updated {
    from: &'static str,
    to: String,
    binary: PathBuf,
    archive: String,
    sha256: String,
    signer: String,
    calcifer: Step,
    machine: Adopted,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Step {
    Restarted,
    NotRunning,
    NotPorted,
}

pub async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    if let Err(e) = crate::completion::install(&ctx.user_home) {
        crate::output::warn(&format!("shell completion not installed: {e}"));
    }
    let current = env!("CARGO_PKG_VERSION");
    let binary = installed()?;
    if let Some(brew) = crate::calcifer_client::homebrew(&binary) {
        return Err(Error::new(
            Kind::Conflict,
            "homebrew_managed",
            format!("{} is Homebrew's cozy; Homebrew updates it", brew.display()),
        )
        .next("brew upgrade cozy"));
    }
    let wanted = args.version.as_deref().map(|v| v.trim_start_matches('v'));
    let dir = match &ctx.config.download_url {
        Some(url) => url.as_str().trim_end_matches('/').to_owned(),
        None => release::dir(wanted).map_err(refused)?,
    };
    let manifest = text(&format!("{dir}/dist-manifest.json")).await?;
    let release = Release::read(&dir, &manifest, wanted).map_err(refused)?;
    let version = release.version.clone();
    if wanted.is_none() && !newer(&version, current) {
        println!("cozy {current} is the newest release");
        return Ok(ExitCode::SUCCESS);
    }
    let url = match &ctx.config.download_url {
        Some(_) => format!("{dir}/{}", release.archive),
        None => release.url(),
    };
    let bundles = format!("{url}.sigstore.jsonl");
    let signer = release
        .signer(&bundles, &text(&bundles).await?)
        .map_err(refused)?;
    crate::output::say(ctx, &format!("fetching cozy {version}: {url}"));
    let part = binary.with_file_name(format!(
        ".cozy-{}.part",
        release.sha256.get(..12).unwrap_or_default()
    ));
    let (archive, sha256) = crate::tools::download(ctx, &url, &part)
        .await
        .map_err(|e| unavailable(&url, e))?;
    if sha256 != release.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(Error::new(
            Kind::Validation,
            "checksum_mismatch",
            format!("{url} has sha256 {sha256}, not the one its release names"),
        ));
    }
    let new = unpack(&release, archive, &binary).map_err(|e| local("unpack", e))?;
    let _ = std::fs::remove_file(&part);
    let runs = std::process::Command::new(&new)
        .arg("--version")
        .output()
        .map_err(|e| local("run the new binary", e))?;
    if String::from_utf8_lossy(&runs.stdout).trim() != format!("cozy {version}") {
        return Err(Error::new(
            Kind::Validation,
            "new_binary_refused",
            format!(
                "cozy {version} for {TARGET} does not run here: {}",
                String::from_utf8_lossy(&runs.stderr).trim()
            ),
        ));
    }
    keep_previous(&binary).map_err(|e| local("keep the binary it replaces", e))?;
    self_replace::self_replace(&new).map_err(|e| local("replace this binary", e))?;

    let calcifer = match crate::daemon::restart(ctx).await {
        Ok(true) => Step::Restarted,
        Ok(false) => Step::NotRunning,
        Err(e) if e.code == "not_ported" => Step::NotPorted,
        Err(e) => {
            return Err(Error {
                remedy: Some(format!(
                    "{} is cozy {version} now; `cozy doctor` restarts Calcifer on it",
                    binary.display()
                )),
                ..e
            });
        }
    };
    let machine = crate::machine::adopt(ctx)?;
    let updated = Updated {
        from: current,
        to: version,
        binary,
        archive: release.archive,
        sha256,
        signer,
        calcifer,
        machine,
    };
    if ctx.json {
        println!(
            "{}",
            serde_json::to_string(&updated).expect("an update serializes")
        );
    } else {
        println!(
            "cozy {} → {}: {}",
            updated.from,
            updated.to,
            updated.binary.display()
        );
        println!(
            "  {} sha256 {}, signed by {}",
            updated.archive, updated.sha256, updated.signer
        );
        println!("  calcifer: {}", step(&updated.calcifer));
        println!("  machine: {}", adopted(&updated.machine));
    }
    Ok(ExitCode::SUCCESS)
}

/// Keeps the binary being replaced beside it as `.cozy-previous`, a machine's rollback when the
/// new one never serves (design §5).
fn keep_previous(binary: &Path) -> io::Result<()> {
    let previous = binary.with_file_name(".cozy-previous");
    match std::fs::remove_file(&previous) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    std::fs::hard_link(binary, &previous).or_else(|_| std::fs::copy(binary, &previous).map(drop))
}

/// This binary's own file, through any link to it.
fn installed() -> Result<PathBuf, Error> {
    let exe = std::env::current_exe().map_err(|e| local("find this binary", e))?;
    // On Windows, canonicalize answers a `\\?\` path.
    if cfg!(windows) {
        return Ok(exe);
    }
    exe.canonicalize().map_err(|e| local("find this binary", e))
}

/// Writes the archive's `cozy` beside `binary` (a file there can run, and the swap stays on one disk).
fn unpack(release: &Release, archive: File, binary: &Path) -> io::Result<tempfile::TempPath> {
    let mut file = tempfile::Builder::new()
        .prefix(".cozy-update-")
        .suffix(std::env::consts::EXE_SUFFIX)
        .tempfile_in(binary.parent().unwrap_or(Path::new(".")))?;
    release.unpack(archive, &mut file)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }
    // Closed: neither OS runs a file another handle still writes.
    Ok(file.into_temp_path())
}

/// A release document, read whole by TensorFS's one GET rule; the archive it names is held to its digest.
async fn text(url: &str) -> Result<String, Error> {
    let mut body = Vec::new();
    let mut sink = |at: u64, frame: &[u8]| -> tensorfs::err::Result<()> {
        body.truncate(at as usize);
        body.extend_from_slice(frame);
        Ok(())
    };
    match tensorfs::fetch::get_pinned(url, 0, &mut sink).await {
        Ok(()) => String::from_utf8(body).map_err(|e| unavailable(url, e)),
        Err(e) if e.code == tensorfs::err::Code::REF_NOT_FOUND => {
            Err(not_found(format!("{url}: not found")))
        }
        Err(e) => Err(unavailable(url, e)),
    }
}

/// Whether release `version` replaces `current`: a newer release, or any release over a dev build
/// (`+dev.g<sha>`), even the release it was built from.
fn newer(version: &str, current: &str) -> bool {
    match (
        semver::Version::parse(version),
        semver::Version::parse(current),
    ) {
        (Ok(_), Ok(current)) if current.build.as_str().starts_with("dev") => true,
        (Ok(version), Ok(current)) => version > current,
        _ => false,
    }
}

fn adopted(adopted: &Adopted) -> &'static str {
    match adopted {
        Adopted::NotRunning => "not running; it starts on the new binary when used",
        Adopted::MovesWhenIdle => "running; it moves to the new binary once idle",
    }
}

fn step(step: &Step) -> &'static str {
    match step {
        Step::Restarted => "restarted on the new binary",
        Step::NotRunning => "not running",
        Step::NotPorted => "not in this build yet",
    }
}

fn not_found(message: String) -> Error {
    Error::new(Kind::NotFound, "release_not_found", message)
}

fn refused(e: release::Refused) -> Error {
    let kind = match e.code {
        "release_not_found" => Kind::NotFound,
        "unattested" => Kind::Validation,
        _ => Kind::Unavailable,
    };
    Error::new(kind, e.code, e.message)
}

fn unavailable(url: &str, error: impl std::fmt::Display) -> Error {
    Error::new(
        Kind::Unavailable,
        "release_unavailable",
        format!("{url}: {error}"),
    )
}

fn local(op: &str, error: io::Error) -> Error {
    Error::new(Kind::Internal, "update_failed", format!("{op}: {error}"))
}
