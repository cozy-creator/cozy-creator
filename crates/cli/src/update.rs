//! `cozy update [VERSION]` (design §5): replaces this binary with a release's by `api::update`, the path Calcifer's own update for
//! a package takes too (cl-978): verified as `platform::release` verifies one, or Homebrew's by `brew upgrade`. It restarts
//! Calcifer on it; the local machine moves to it once idle. `download_url` (`COZY_DOWNLOAD_URL`, which dist's installers read too)
//! serves one release's files in place of GitHub's.
use std::{
    io::{BufRead as _, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, SystemTime},
};

use api::error::{Error, Kind};
use serde::{Deserialize, Serialize};

use crate::{Context, machine::Adopted};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Release to install. Without a version, update to the latest release; an older one is installed only when named.
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
    update(ctx, args.version.as_deref()).await?;
    Ok(ExitCode::SUCCESS)
}

/// `cozy update [wanted]`: the binary it put in place; none when this is the newest release.
async fn update(ctx: &Context, wanted: Option<&str>) -> Result<Option<PathBuf>, Error> {
    // Completion is a convenience: one that can't be written is left out.
    let _ = crate::completion::install(&ctx.user_home);
    let current = ::api::VERSION;
    let binary = installed()?;
    let wanted = wanted.map(|v| v.trim_start_matches('v'));
    let download_url = ctx.config.download_url.as_ref().map(|url| url.as_str().to_owned());
    let dir = api::update::dir(download_url.as_deref(), wanted)?;
    let release = api::update::release(&dir, wanted).await?;
    let version = release.version.clone();
    if wanted.is_none() {
        match moves(&version, current) {
            Move::Up => {}
            Move::Stay => {
                println!("cozy {current} is the newest release");
                return Ok(None);
            }
            Move::Down => {
                let said = format!("the Latest release, cozy {version}, is older than this cozy {current}: an update never goes back unless the version is named");
                return Err(Error::new(Kind::Conflict, "cozy_downgrade", said).next(format!("cozy update {version}")));
            }
        }
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
            Ok(true) => Adopted::Moves,
            Ok(false) => Adopted::MovesWhenIdle,
            // It moves once idle all the same.
            Err(_) => Adopted::MovesWhenIdle,
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
        if let Adopted::Moves = updated.machine {
            crate::output::next(ctx, &["cozy machine show local"]);
        }
    }
    Ok(Some(updated.binary))
}

/// The newest release as last read, and the one its person declined: kept in the cozy home (`update-offer.json`).
#[derive(Serialize, Deserialize, Default)]
struct Offered {
    read_s: u64,
    latest: String,
    declined: String,
}

/// At the start of a command a person runs, a newer patch or minor release is offered (cl-1069, Paul 11-16: "ask them if they want to
/// update, rather than forcibly updating them … that gives them the ability to remain on the older version"): yes installs it as
/// `cozy update` does and the command goes on as it; no keeps this one until the next release. The newest is read at most once a day,
/// waiting at most 3 s. A dev build, `--yes`, `--json`, or no terminal on stdin, stdout and stderr is offered nothing.
pub(crate) async fn offer(ctx: &Context) {
    if ::api::prerelease::dev(::api::VERSION) || ctx.yes || !crate::output::can_ask(ctx) || !ctx.term.stdout {
        return;
    }
    let path = ctx.home.root().join("update-offer.json");
    let mut offered: Offered = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if now.abs_diff(offered.read_s) >= 86_400 {
        let download_url = ctx.config.download_url.as_ref().map(|url| url.as_str().to_owned());
        let read = async { api::update::release(&api::update::dir(download_url.as_deref(), None)?, None).await };
        // Read or not, not again for a day: offline, the 3 s are waited once a day.
        offered.read_s = now;
        if let Ok(Ok(release)) = tokio::time::timeout(Duration::from_secs(3), read).await {
            offered.latest = release.version;
        }
        keep(&path, &offered);
    }
    let current = ::api::VERSION;
    let major = |version: &str| semver::Version::parse(version).map(|v| v.major).ok();
    if offered.latest == offered.declined || !matches!(moves(&offered.latest, current), Move::Up) || major(&offered.latest) != major(current) {
        return;
    }
    anstream::eprint!("cozy {} is out (this is {current}). Update now? [Y/n] ", offered.latest);
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    if !matches!(std::io::stdin().lock().read_line(&mut answer), Ok(1..)) {
        anstream::eprintln!();
        return;
    }
    if !matches!(answer.trim(), "" | "y" | "Y" | "yes") {
        offered.declined = offered.latest.clone();
        keep(&path, &offered);
        anstream::eprintln!("staying on cozy {current}; `cozy update` updates it later");
        return;
    }
    match update(ctx, Some(&offered.latest)).await {
        Ok(Some(binary)) => crate::output::warn(&again(&binary).message),
        Ok(None) => {}
        Err(e) => crate::output::warn(&format!("cozy's update: {}", e.message)),
    }
}

fn keep(path: &Path, offered: &Offered) {
    if let Ok(bytes) = serde_json::to_vec(offered) {
        let _ = platform::durable::replace(path, &bytes, 0o600);
    }
}

/// This command again as the cozy at `binary`: on Unix this process becomes it; on Windows it runs it and exits as it does. What
/// it answers is why it didn't.
fn again(binary: &Path) -> Error {
    let failed = |e: std::io::Error| Error::new(Kind::Internal, "cozy_continue_failed", format!("{} didn't take this command over: {e}", binary.display()));
    let mut command = std::process::Command::new(binary);
    command.args(std::env::args_os().skip(1));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        failed(command.exec())
    }
    #[cfg(not(unix))]
    {
        match command.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(e) => failed(e),
        }
    }
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
        return Err(kept.unwrap_or_else(|| Error::new(Kind::Unavailable, "cozy_update_failed", "cozy's update didn't finish".to_owned()).next("cozy update")));
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
    Err(again(&binary).next("cozy run ls"))
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

enum Move {
    Up,
    Stay,
    Down,
}

/// What release `version` is to `current`: newer, or a release over a dev build (the RC's `0.26.0-dev+g<sha>`, a release's
/// `+dev.g<sha>`) that doesn't sort below it, moves up; one that sorts below it moves down; anything else stays.
fn moves(version: &str, current: &str) -> Move {
    let (Ok(version), Ok(current)) = (semver::Version::parse(version), semver::Version::parse(current)) else {
        return Move::Stay;
    };
    match version.cmp_precedence(&current) {
        std::cmp::Ordering::Less => Move::Down,
        std::cmp::Ordering::Greater => Move::Up,
        std::cmp::Ordering::Equal if !current.build.is_empty() || !current.pre.is_empty() => Move::Up,
        std::cmp::Ordering::Equal => Move::Stay,
    }
}

fn adopted(adopted: &Adopted) -> String {
    match adopted {
        Adopted::NotRunning => "not running; it starts on the new binary when used".into(),
        Adopted::MovesWhenIdle => {
            "running; it moves to the new binary after 15 minutes idle, or at `cozy machine update local`".into()
        }
        Adopted::Moves => "running; it moves to the new binary once its runs end".into(),
    }
}

fn step(step: &Step) -> &'static str {
    match step {
        Step::Restarted => "restarted on the new binary",
        Step::NotRunning => "not running",
        Step::NotPorted => "not in this build yet",
    }
}

