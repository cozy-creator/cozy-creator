//! Project status, the logs `cozy machine logs` reads, and Calcifer's restart (`cozy update`, `cozy doctor`). Any service command starts
//! Calcifer; restarting it never stops work: machines run what they accepted, and the next Calcifer
//! resumes what it owed.
use std::process::ExitCode;

use clap::ValueEnum;
use api::error::{Error, Kind};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use platform::tail::{Log, Start};

use crate::{Context, calcifer_client::Client, output};

/// Stops a running Calcifer and starts the installed `cozy`'s in its place, after something
/// replaced that binary. `false`: none was running, and none is started.
pub(crate) async fn restart(ctx: &Context) -> Result<bool, Error> {
    let Some(client) = Client::running(ctx).await? else {
        return Ok(false);
    };
    client.stop().await?;
    client.ready().await?;
    Ok(true)
}

#[derive(clap::Args, Debug)]
pub(crate) struct Restart {}

/// `cozy restart`: Calcifer stopped and started on the installed cozy, whoever uses it (cl-962). Runs on machines go on,
/// pages reconnect, and the new Calcifer resumes what the old one owed.
pub(crate) async fn restart_calcifer(ctx: &Context, _args: Restart) -> Result<ExitCode, Error> {
    let restarted = restart(ctx).await?;
    let holder = Client::connect(ctx).await?.ready().await?;
    if ctx.json {
        output::json(&json!({"restarted": restarted, "version": holder.version, "pid": holder.pid}));
    } else {
        let did = if restarted { "restarted" } else { "started" };
        output::say(ctx, &format!("Calcifer {did} on cozy {} (pid {})", holder.version, holder.pid));
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(clap::Args, Debug)]
pub(crate) struct Status {}

/// The current project's health and actionable warnings.
pub(crate) async fn status(ctx: &Context, _args: Status) -> Result<ExitCode, Error> {
    let project = crate::project::current(ctx).await?;
    let attention = project.absent > 0 || !project.missing.is_empty() || !project.stale.is_empty()
        || !project.warnings.is_empty() || !project.unsent.is_empty();
    let mut next = Vec::new();
    next.push("cozy open");
    if !attention {
        next.push("cozy run --help");
    }
    if !ctx.full && (project.warnings.len() > 10 || project.unsent.len() > 5) {
        next.push("cozy status --full");
    }
    let mut document = json!({
        "project": if ctx.full {
            json!({
                "id": project.id, "code": project.code, "path": project.path, "assets": project.assets,
                "missing": project.missing, "stale": project.stale, "orphans": project.orphans,
                "absent": project.absent, "run_files": project.run_files, "machines": project.machines,
            })
        } else { json!({"code": project.code, "path": project.path, "machines": project.machines}) },
        "health": if attention { "needs_attention" } else { "ready" },
        "counts": {
            "unrendered_assets": project.missing.len(),
            "outdated_assets": project.stale.len(),
            "missing_files": project.absent,
            "warnings": project.warnings.len(),
        },
        "warnings": project.warnings.iter().take(if ctx.full { usize::MAX } else { 10 }).collect::<Vec<_>>(),
        "next": next,
    });
    if !project.unsent.is_empty() {
        document["counts"]["errors"] = json!(project.unsent.len());
        document["errors"] = json!(project.unsent.iter().take(if ctx.full { usize::MAX } else { 5 }).collect::<Vec<_>>());
    }
    // What a box's text mentions that its tool can't use: left out of its render, said once each, nothing to act on (cl-944).
    if !project.notes.is_empty() {
        document["notes"] = json!(project.notes.iter().map(|note| &note.message).collect::<Vec<_>>());
    }
    output::view(ctx, &document, &["project", "health", "counts", "warnings", "errors", "notes", "next"]);
    Ok(ExitCode::SUCCESS)
}

/// Which of a machine's logs, and how much of it.
#[derive(clap::Args, Debug)]
pub(crate) struct LogQuery {
    /// Choose the machine, transfer, memory or local Calcifer log.
    #[arg(value_enum, ignore_case = true, default_value_t = LogName::Tensord)]
    log: LogName,
    /// Keep printing as it grows.
    #[arg(short, long)]
    follow: bool,
    /// The newest N lines; 0 prints every line it keeps.
    #[arg(short = 'n', long, value_name = "N", default_value_t = 200)]
    lines: usize,
    /// Only lines from the last DURATION, such as 90s, 30m or 2h.
    #[arg(long, value_name = "DURATION", value_parser = since)]
    since: Option<SignedDuration>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LogName {
    Tensord,
    Tensorfs,
    Memory,
    Calcifer,
}

impl LogName {
    fn as_str(self) -> &'static str {
        match self {
            LogName::Tensord => "tensord",
            LogName::Tensorfs => "tensorfs",
            LogName::Memory => "memory",
            LogName::Calcifer => "calcifer",
        }
    }
}

fn since(text: &str) -> Result<SignedDuration, String> {
    text.parse()
        .map_err(|_| format!("{text:?} is not a duration such as 90s, 30m or 2h"))
}

/// A machine's log through Calcifer: its newest lines from the last `--since`, then, with
/// `--follow`, each line as the machine writes it (`GET /v1/machines/{name}/logs/{log}`). A machine
/// keeps its logs, so reading never needs it to have run anything here.
pub(crate) async fn machine_log(
    ctx: &Context,
    name: &str,
    q: &LogQuery,
) -> Result<ExitCode, Error> {
    if let LogName::Calcifer = q.log {
        if !name.eq_ignore_ascii_case("local") {
            return Err(Error::new(
                Kind::Usage,
                "log_not_kept",
                format!("Calcifer logs are local; {name} has none"),
            )
            .next("cozy machine logs local calcifer"));
        }
        return calcifer_log(ctx, q).await;
    }
    api::calcifer::local_usable(name)?;
    let mut path = format!("{}/logs/{}", crate::machine::route(name)?, q.log.as_str());
    if let Some(window) = q.since {
        let at = Timestamp::now().saturating_sub(window).unwrap_or_default();
        path += &format!("?since_ms={}", at.as_millisecond());
    }
    let calcifer = ctx.calcifer().await?;
    if !q.follow {
        let sep = if path.contains('?') { '&' } else { '?' };
        let text = calcifer.text(&format!("{path}{sep}lines={}", q.lines)).await?;
        for line in text.lines().filter(|l| !l.is_empty()) {
            print_log_line(ctx, name, q.log, line);
        }
        if !text.lines().any(|l| !l.is_empty()) {
            output::say(
                ctx,
                &format!("{name} has logged nothing in its {} log", q.log.as_str()),
            );
        }
        return Ok(ExitCode::SUCCESS);
    }
    let mut lines = calcifer.follow_log(&path, q.lines as u64);
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(ExitCode::SUCCESS),
            batch = lines.next() => {
                for line in batch?.text.lines().filter(|l| !l.is_empty()) {
                    print_log_line(ctx, name, q.log, line);
                }
            }
        }
    }
}

fn stamp(line: &str) -> Option<Timestamp> {
    let word = line.split_whitespace().next()?;
    word.parse().ok().or_else(|| {
        let seconds: f64 = word.parse().ok()?;
        Timestamp::from_millisecond((seconds * 1000.0) as i64).ok()
    })
}

fn print_log_line(ctx: &Context, machine: &str, log: LogName, line: &str) {
    if ctx.json {
        output::ndjson(&json!({ "machine": machine, "log": log.as_str(), "text": line }));
    } else {
        println!("{line}");
    }
}

/// Calcifer's log, read from its file: there whether or not Calcifer is, its newest `lines` from
/// the last `since`. Followed, each line as it is written, woken by a watch on its directory
/// (`tail -F`, across a rotation to `calcifer.log.1`).
async fn calcifer_log(ctx: &Context, q: &LogQuery) -> Result<ExitCode, Error> {
    let path = ctx.home.calcifer_log();
    if !path.exists() {
        return Err(Error::new(
            Kind::NotFound,
            "log_not_found",
            format!(
                "Calcifer log not found: {}",
                path.display()
            ),
        )
        .next("cozy doctor"));
    }
    let unreadable = |e: &dyn std::fmt::Display| {
        Error::new(
            Kind::Internal,
            "log_unreadable",
            format!("{}: {e}", path.display()),
        )
    };
    let log = Log::rotated(ctx.home.calcifer(), "calcifer.log");
    let start = Start::Last(q.lines as u64);
    // Lines before `since` are skipped up to the first one stamped at or after it.
    let mut cutoff = q
        .since
        .map(|window| Timestamp::now().saturating_sub(window).unwrap_or_default());
    let mut emit = |text: &str| {
        for line in text.lines().filter(|l| !l.is_empty()) {
            if let Some(at) = cutoff {
                if stamp(line).is_none_or(|stamped| stamped < at) {
                    continue;
                }
                cutoff = None;
            }
            print_line(ctx, line);
        }
    };
    if !q.follow {
        emit(&log.read(start).map_err(|e| unreadable(&e))?);
        return Ok(ExitCode::SUCCESS);
    }
    let follow = log
        .follow(start)
        .map_err(|e| match e.is_unsupported() {
            true => Error::new(Kind::Unavailable, "log_follow_unsupported", e.to_string()),
            false => unreadable(&e),
        })?;
    // Dropped after `batches`: the follower, woken, finds no reader.
    let _leave = follow.on_leave();
    let (sender, mut batches) = tokio::sync::mpsc::channel(16);
    let reading = tokio::task::spawn_blocking(move || {
        follow.run(
            |batch| sender.blocking_send(batch).is_ok(),
            || !sender.is_closed(),
        )
    });
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(ExitCode::SUCCESS),
            batch = batches.recv() => match batch {
                Some(batch) => emit(&batch.text),
                None => {
                    return match reading.await {
                        Ok(Err(e)) => Err(unreadable(&e)),
                        _ => Ok(ExitCode::SUCCESS),
                    };
                }
            },
        }
    }
}

fn print_line(ctx: &Context, line: &str) {
    if ctx.json {
        output::ndjson(&json!({ "text": line }));
    } else {
        println!("{line}");
    }
}
