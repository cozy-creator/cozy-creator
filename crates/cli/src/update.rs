//! `cozy update [VERSION]` (design §5): replaces this binary with a release's by `api::update`, the path Calcifer's own update for
//! a package takes too (cl-978): verified as `platform::release` verifies one, or Homebrew's by `brew upgrade`. It restarts
//! Calcifer on it; the local machine moves to it once idle. `download_url` (`COZY_DOWNLOAD_URL`, which dist's installers read too)
//! serves one release's files in place of GitHub's.
use std::{
    path::PathBuf,
    process::ExitCode,
};

use api::error::{Error, Kind};
use serde::Serialize;

use crate::{Context, machine::Adopted};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Release to install. Without a version, update to the latest release.
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

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    if let Err(e) = crate::completion::install(&ctx.user_home) {
        crate::output::warn(&format!("shell completion not installed: {e}"));
    }
    let current = ::api::VERSION;
    let binary = installed()?;
    let wanted = args.version.as_deref().map(|v| v.trim_start_matches('v'));
    let download_url = ctx.config.download_url.as_ref().map(|url| url.as_str().to_owned());
    let dir = api::update::dir(download_url.as_deref(), wanted)?;
    let release = api::update::release(&dir, wanted).await?;
    let version = release.version.clone();
    if wanted.is_none() && !newer(&version, current) {
        println!("cozy {current} is the newest release");
        return Ok(ExitCode::SUCCESS);
    }
    crate::output::say(ctx, &format!("fetching cozy {version}"));
    let bar = crate::progress::Transfer::stderr(ctx.term.stderr, "Downloading Cozy");
    bar.phase("Downloading and checking archive");
    let mut moved = |at: u64| bar.update(crate::progress::TransferCounts { files_done: 0, files_total: Some(1), bytes_done: at, bytes_total: None, bytes_moved: at });
    let installed = api::update::install(&dir, download_url.is_some(), &release, &binary, &mut moved).await;
    bar.finish_and_clear();
    let installed = installed?;
    let version = installed.to.clone();

    let calcifer = match crate::daemon::restart(ctx).await {
        Ok(true) => Step::Restarted,
        Ok(false) => Step::NotRunning,
        Err(e) if e.code == "not_ported" => Step::NotPorted,
        Err(e) => {
            return Err(Error {
                remedy: Some(format!(
                    "{} is cozy {version} now; `cozy doctor` restarts Calcifer on it",
                    installed.binary.display()
                )),
                ..e
            });
        }
    };
    let machine = match crate::machine::adopt(ctx)? {
        Adopted::MovesWhenIdle => match crate::machine::activate(ctx, &version).await {
            Ok(Some(run)) => Adopted::Moves { run },
            Ok(None) => Adopted::MovesWhenIdle,
            Err(e) => {
                crate::output::warn(&format!("local did not take the update to cozy {version}: {}", e.message));
                Adopted::MovesWhenIdle
            }
        },
        adopted => adopted,
    };
    let updated = Updated {
        from: current,
        to: version,
        binary: installed.binary,
        archive: installed.archive,
        sha256: installed.sha256,
        signer: installed.signer,
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
        if !updated.archive.is_empty() {
            println!("  {} sha256 {}, signed by {}", updated.archive, updated.sha256, updated.signer);
        }
        println!("  calcifer: {}", step(&updated.calcifer));
        println!("  machine: {}", adopted(&updated.machine));
        if let Adopted::Moves { run } = updated.machine {
            crate::output::next(ctx, &[&format!("cozy run watch {run}")]);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// What a command that waited behind cozy's own update (cl-978) does once it ended: completed, this command again on the installed
/// cozy, as that cozy (on Unix this process becomes it; on Windows it runs it and exits as it does), unless this is that cozy; failed,
/// its refusal, whole. Another run: nothing.
pub(crate) fn after_update(ctx: &Context, run: &api::calcifer::Run) -> Result<(), Error> {
    if run.target != api::calcifer::hub::job::COZY_UPDATE {
        return Ok(());
    }
    if run.status != api::calcifer::Status::Completed {
        let kept = run.result.as_ref().and_then(|result| serde_json::from_value::<Error>(result["error"].clone()).ok());
        return Err(kept.unwrap_or_else(|| Error::new(Kind::Unavailable, "cozy_update_failed", format!("cozy's update (run {}) didn't finish", run.number)).next("cozy update")));
    }
    let binary = crate::calcifer_client::installed(&ctx.home)?;
    let said = std::process::Command::new(&binary).arg("--version").output().map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned());
    if said.as_deref().is_ok_and(|said| said == format!("cozy {}", ::api::VERSION)) {
        return Ok(());
    }
    // Calcifer restarts on it once nothing else is going: the new cozy then asks that one. One still busy is waited for by
    // the new cozy, which it refuses until then.
    let record = platform::lock::LockFile::new(ctx.home.calcifer_lock());
    let moved = || record.holder().ok().flatten().and_then(|raw| serde_json::from_slice::<api::calcifer::Holder>(&raw).ok()).is_some_and(|holder| holder.version != ::api::VERSION);
    let began = std::time::Instant::now();
    while !moved() && began.elapsed() < std::time::Duration::from_secs(30) {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let failed = |e: std::io::Error| {
        Error::new(Kind::Internal, "cozy_continue_failed", format!("{} didn't take this command over: {e}", binary.display())).next("cozy run ls")
    };
    let mut command = std::process::Command::new(&binary);
    command.args(std::env::args_os().skip(1));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        Err(failed(command.exec()))
    }
    #[cfg(not(unix))]
    {
        let status = command.status().map_err(failed)?;
        std::process::exit(status.code().unwrap_or(1))
    }
}

/// This binary's own file, through any link to it.
fn installed() -> Result<PathBuf, Error> {
    let local = |e: std::io::Error| Error::new(Kind::Internal, "update_failed", format!("find this binary: {e}"));
    let exe = std::env::current_exe().map_err(local)?;
    // On Windows, canonicalize answers a `\\?\` path.
    if cfg!(windows) {
        return Ok(exe);
    }
    exe.canonicalize().map_err(local)
}

/// Whether release `version` replaces `current`: a newer release, or any release over a dev build
/// (the RC's `0.26.0-dev+g<sha>`, a release's `+dev.g<sha>`), even the release it was built from.
fn newer(version: &str, current: &str) -> bool {
    match (
        semver::Version::parse(version),
        semver::Version::parse(current),
    ) {
        (Ok(_), Ok(current)) if !current.build.is_empty() || !current.pre.is_empty() => true,
        (Ok(version), Ok(current)) => version > current,
        _ => false,
    }
}

fn adopted(adopted: &Adopted) -> String {
    match adopted {
        Adopted::NotRunning => "not running; it starts on the new binary when used".into(),
        Adopted::MovesWhenIdle => {
            "running; it moves to the new binary after 15 minutes idle, or at `cozy machine update local`".into()
        }
        Adopted::Moves { run } => format!("running; it moves to the new binary once its runs end (run {run})"),
    }
}

fn step(step: &Step) -> &'static str {
    match step {
        Step::Restarted => "restarted on the new binary",
        Step::NotRunning => "not running",
        Step::NotPorted => "not in this build yet",
    }
}

