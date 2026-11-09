//! `cozy status`, the logs `cozy machine logs` reads (Calcifer's too), and Calcifer's restart (`cozy update`, `cozy doctor`). Any cozy command starts
//! Calcifer; restarting it never stops work: machines run what they accepted, and the next Calcifer
//! resumes what it owed.
use std::process::ExitCode;

use clap::ValueEnum;
use api::{
    calcifer::{CalciferStatus, Holder, MachineState},
    error::{Error, Kind},
    calcifer::project::{Open, Opened, State},
};
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
pub(crate) struct Status {}

/// This cozy, Calcifer, COZY_PROJECT's project, the runs not yet settled, the machines and what they cost:
/// `cozy` alone too. Never starts Calcifer.
pub(crate) async fn status(ctx: &Context, _args: Status) -> Result<ExitCode, Error> {
    let exe = std::env::current_exe().map(|path| path.display().to_string()).unwrap_or_default();
    let Some(client) = Client::running(ctx).await? else {
        if ctx.json {
            output::json(&json!({ "calcifer": { "running": false } }));
        } else {
            println!("cozy {} {exe}", env!("CARGO_PKG_VERSION"));
            println!("calcifer: not running; any other cozy command starts it");
            output::next(ctx, &["cozy doctor"]);
        }
        return Ok(ExitCode::SUCCESS);
    };
    let here: CalciferStatus = client.get("/v1/status").await?;
    let holder = client.holder().expect("a running client has a record");
    // Calcifer's picture: it never waits on a Hub here.
    let mut list = client.machines().await?;
    list.machines.retain(|m| m.state != MachineState::Ended);
    let mut runs = vec![];
    for status in ["queued", "in_progress"] {
        runs.extend(client.runs(&[("status", status)]).await?.runs);
    }
    runs.retain(crate::run::list::shown);
    runs.sort_by_key(|run| run.number);
    let project = project(&client).await;
    if ctx.json {
        let project = match &project {
            Some(Ok(state)) => json!(state),
            Some(Err(error)) => json!({ "error": error }),
            None => serde_json::Value::Null,
        };
        let doc = json!({
            "calcifer": running(ctx, &holder, &here), "project": project, "machines": list.machines, "rentals": list.rentals,
            "unanswered": list.unanswered, "signed_out": list.signed_out, "runs": runs,
        });
        let project = crate::project::FIELDS.iter().map(|path| format!("project.{path}"));
        let mut paths: Vec<String> = ["calcifer".into(), "project.error".into()].into_iter().chain(project).collect();
        paths.extend(crate::machine::list::FIELDS.iter().chain(crate::run::list::FIELDS).map(|path| path.to_string()));
        output::view(ctx, &doc, &paths.iter().map(String::as_str).collect::<Vec<_>>());
        return Ok(ExitCode::SUCCESS);
    }
    println!("cozy {} {exe}", env!("CARGO_PKG_VERSION"));
    match ctx.full {
        true => calcifer_record(ctx, &holder, &here).print(ctx),
        false => {
            // Signed out is said with the machines, by Hub.
            let version = Some(&here.version).filter(|v| *v != env!("CARGO_PKG_VERSION")).map(|v| format!(" on cozy {v}"));
            println!("calcifer: up {}{}", output::age(holder.started_at_ms), version.unwrap_or_default());
        }
    }
    let next = match &project {
        Some(Ok(state)) => {
            let name = state.path.file_name().map_or_else(|| state.id.clone(), |name| name.to_string_lossy().into_owned());
            println!("project: {name}: {}", crate::project::totals(state));
            state.warnings.iter().for_each(|warning| println!("warning: {}", warning.message));
            state.next.clone()
        }
        Some(Err(error)) => {
            println!("project: {} ({})", error.message, error.code);
            error.next.clone()
        }
        None => {
            println!("project: none named (COZY_PROJECT)");
            vec!["cozy project ls".into(), "cozy project new <name>".into()]
        }
    };
    if !runs.is_empty() {
        crate::run::list::table(ctx, &runs.iter().collect::<Vec<_>>(), true).print();
    }
    crate::machine::list::print(ctx, &list);
    output::next(ctx, &next.iter().map(String::as_str).collect::<Vec<_>>());
    Ok(ExitCode::SUCCESS)
}

/// COZY_PROJECT's project, as Calcifer reads it: none when none is named.
async fn project(client: &Client) -> Option<Result<State, Error>> {
    let project = crate::project::chosen(None)?;
    let opened: Result<Opened, Error> = client.post("/v1/projects", &Open { project: Some(project), ..Open::default() }, None).await;
    match opened {
        Ok(opened) => Some(client.get(&format!("/v1/projects/{}", opened.id)).await),
        Err(error) => Some(Err(error)),
    }
}

/// Which of a machine's logs, and how much of it.
#[derive(clap::Args, Debug)]
pub(crate) struct LogQuery {
    /// The machine's own log, the TensorFS transport or memory log it keeps, or (on `local`)
    /// Calcifer's, this computer's controller.
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
                format!("{name} keeps no Calcifer log: Calcifer is this computer's"),
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
                "no Calcifer has run here: {} does not exist",
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

fn running(ctx: &Context, holder: &Holder, here: &CalciferStatus) -> serde_json::Value {
    json!({
        "running": true,
        "address": holder.address,
        "pid": holder.pid,
        "version": here.version,
        "os": here.os,
        "local_usable": here.local_usable,
        "signed_in": here.signed_in,
        "heap_bytes": here.heap_bytes,
        "started_at_ms": holder.started_at_ms,
        "log": ctx.home.calcifer_log(),
    })
}

fn calcifer_record(ctx: &Context, holder: &Holder, here: &CalciferStatus) -> output::Record {
    output::Record::new()
        .field(
            "calcifer",
            format!("running, up {}", output::age(holder.started_at_ms)),
        )
        .field("address", holder.address)
        .field("pid", holder.pid)
        .field("version", &here.version)
        .field("signed in", if here.signed_in { "yes" } else { "no" })
        .field("holds", output::bytes(here.heap_bytes))
        .field("log", output::link(ctx, &ctx.home.calcifer_log()))
}
