//! Project status, the logs `cozy machine logs` reads, and Calcifer's restart (`cozy update`, `cozy doctor`). Any service command starts
//! Calcifer; restarting it never stops work: machines run what they accepted, and the next Calcifer
//! resumes what it owed.
use std::{collections::BTreeMap, process::ExitCode};

use clap::ValueEnum;
use api::{
    calcifer::{CalciferStatus, LiveRentals, MachineList, Status as RunStatus},
    error::{Error, Kind},
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

/// The current project's active runs and the machines those runs use.
pub(crate) async fn status(ctx: &Context, _args: Status) -> Result<ExitCode, Error> {
    let project = crate::project::current(ctx).await?;
    let client = ctx.calcifer().await?;
    let mut runs = BTreeMap::new();
    let mut unreadable = Vec::new();
    for status in ["queued", "in_progress"] {
        let listed = crate::run::list::runs(ctx, &crate::run::list::Query {
            status: Some(status.into()),
            project: Some(project.id.clone()),
            ..Default::default()
        }).await?;
        // A queued run can start between the two reads. Keep its latest record.
        runs.extend(listed.runs.into_iter().filter(crate::run::list::shown).map(|run| (run.number, run)));
        unreadable.extend(listed.unreadable);
    }
    let runs: Vec<_> = runs.into_values().collect();
    let mut machines = match runs.is_empty() {
        true => Vec::new(),
        false => client.machines().await?.machines,
    };
    machines.retain_mut(|machine| {
        machine.running = 0;
        machine.queued = 0;
        for run in &runs {
            let Some(name) = run.machine.as_deref() else { continue };
            if name != machine.name && Some(name) != machine.alias.as_deref() && Some(name) != machine.rental_id.as_deref() {
                continue;
            }
            match run.status {
                RunStatus::Queued => machine.queued += 1,
                RunStatus::InProgress => machine.running += 1,
                _ => {}
            }
        }
        machine.running + machine.queued > 0
    });
    let diagnostic = if ctx.full {
        let here: CalciferStatus = client.get("/v1/status").await?;
        let holder = client.holder();
        Some(json!({
            "version": here.version,
            "os": here.os,
            "pid": holder.as_ref().map(|holder| holder.pid),
            "address": holder.as_ref().map(|holder| holder.address.to_string()),
            "heap_bytes": here.heap_bytes,
        }))
    } else {
        None
    };
    if ctx.json {
        // A machine can also run another project's work. Project status carries
        // only this project's counts and hardware details, with no rental totals.
        let machines = machines.iter().map(|machine| json!({
            "name": machine.name,
            "kind": machine.kind,
            "state": machine.state,
            "running": machine.running,
            "queued": machine.queued,
            "unreachable": machine.unreachable,
            "reason": machine.reason,
            "status": machine.status.as_ref().map(|status| json!({
                "version": status.version,
                "phase": status.phase,
                "gpus": status.gpus,
            })),
        })).collect::<Vec<_>>();
        let mut document = json!({
            "project": if ctx.full { serde_json::to_value(&project).expect("project serializes") } else { json!({ "id": project.id, "path": project.path }) },
            "runs": runs,
            "machines": machines,
            "unreadable": unreadable,
        });
        if let Some(diagnostic) = diagnostic {
            document["calcifer"] = diagnostic;
        }
        let mut fields = vec!["project", "machines", "unreadable"];
        fields.extend(crate::run::list::FIELDS.iter().copied());
        output::view(ctx, &document, &fields);
        return Ok(ExitCode::SUCCESS);
    }
    println!("project: {}", project.path.display());
    if let Some(diagnostic) = diagnostic {
        println!("calcifer: {} (pid {}, {})", diagnostic["version"].as_str().unwrap_or_default(), diagnostic["pid"], diagnostic["address"].as_str().unwrap_or_default());
    }
    if runs.is_empty() {
        println!("runs: none active");
    } else {
        crate::run::list::table(ctx, &runs.iter().collect::<Vec<_>>(), false).print();
    }
    if !machines.is_empty() {
        let list = MachineList { machines, hubs: Vec::new(), unanswered: Vec::new(), signed_out: Vec::new(), rentals: LiveRentals::default() };
        let columns = [0, 1, 2, 4, 8, 9];
        let table = output::Table::new(ctx, ["MACHINE", "KIND", "GPUS", "STATE", "RUNNING", "QUEUED"]);
        crate::machine::list::rows(ctx, &list).into_iter()
            .map(|row| columns.map(|index| row[index].clone()))
            .fold(table, output::Table::row).print();
    }
    for warning in &project.warnings {
        output::warn(&warning.message);
    }
    for row in unreadable {
        output::warn(&format!("could not read {row}"));
    }
    Ok(ExitCode::SUCCESS)
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
