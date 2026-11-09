//! `cozy run ls`: every run, newest first: a live view on a terminal, else a short table or
//! JSON (`--full`: every column and field).
use std::process::ExitCode;

use api::{
    calcifer::{Kind as RunKind, Run, RunList, Status},
    error::Error,
};
use serde::Serialize;

use super::{follow, live};
use crate::{Context, output, tui};

pub(crate) const HEADERS: [&str; 7] = ["#", "TARGET", "PROJECT", "MACHINE", "STATUS", "PROGRESS", "TIME"];
/// The newest runs a list shows unless `--limit` says otherwise.
const SHOWN: u64 = 20;
/// What `--json` keeps of a run in a list (`--full`: all of it): the table's facts, by the record's names.
pub(crate) const FIELDS: &[&str] = &[
    "older", "machine", "unreadable", "runs.id", "runs.number", "runs.kind", "runs.target", "runs.project", "runs.project_id", "runs.machine",
    "runs.status", "runs.queue_position", "runs.waiting", "runs.progress", "runs.stalled", "runs.reason.code",
    "runs.reason.message", "runs.created_at_ms", "runs.accepted_at_ms", "runs.started_at_ms", "runs.settled_at_ms",
    "runs.execution_ms", "others.id", "others.machine_number", "others.target", "others.machine", "others.status",
    "others.machine_state", "others.waiting", "others.reason.code", "others.reason.message", "others.accepted_at_ms",
    "others.started_at_ms", "others.settled_at_ms",
];
/// A target or progress is one table cell; `cozy run show` has it whole.
const CELL: usize = 72;

/// `GET /v1/runs`' filters.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct Query {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// A project's id: only its runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

pub(crate) async fn exec(ctx: &Context, mut query: Query) -> Result<ExitCode, Error> {
    let limit = query.limit.unwrap_or(SHOWN);
    query.limit = (limit > 0).then_some(limit);
    if !ctx.json && ctx.term.stdout {
        let view = live::Runs::open(ctx, query, limit).await?;
        tui::show(ctx, tui::Screen::Runs(view)).await?;
        return Ok(ExitCode::SUCCESS);
    }
    let list = runs(ctx, &query).await?;
    if ctx.json {
        output::view(ctx, &list, FIELDS);
        return Ok(ExitCode::SUCCESS);
    }
    for run in &list.unreadable {
        println!("left out {run}");
    }
    let listed: Vec<&Run> = list.runs.iter().chain(&list.others).filter(|run| shown(run)).collect();
    if listed.is_empty() {
        let only = query.status.is_some() || query.package.is_some() || query.machine.is_some();
        match (only, &query.project) {
            (true, _) => println!("runs: none match"),
            (false, Some(_)) => {
                println!("runs: none in this project");
                output::next(ctx, &["cozy run ls --all"]);
            }
            (false, None) => println!("runs: none"),
        }
        return Ok(ExitCode::SUCCESS);
    }
    table(ctx, &listed, query.project.is_none()).print();
    if list.older > 0 {
        let own = listed.len() - list.others.len();
        println!("{own} newest of {}: cozy run ls --limit 0 lists them all", own as u64 + list.older);
    }
    if let Some(machine) = list.machine.as_ref().filter(|_| !list.others.is_empty()) {
        println!("{machine}/N: sent by another client of this account; `cozy run show {machine}/N` follows one");
    }
    Ok(ExitCode::SUCCESS)
}

/// The runs as a view prints them: what each is, where, how it stands and its time, then its
/// progress or why it ended, last as it is the one that varies; with `projects`, each one's. `--full`:
/// the live view's columns.
pub(crate) fn table(ctx: &Context, runs: &[&Run], projects: bool) -> output::Table {
    let rows = runs.iter().map(|run| row(run));
    if ctx.full {
        return rows.fold(output::Table::new(ctx, HEADERS), output::Table::row);
    }
    let columns: Vec<usize> = [0, 1, 2, 3, 4, 6, 5].into_iter().filter(|&i| projects || i != 2).collect();
    let table = output::Table::new(ctx, columns.iter().map(|&i| HEADERS[i]));
    rows.map(|cells| columns.iter().map(|&i| cells[i].clone()).collect::<Vec<_>>()).fold(table, output::Table::row)
}

/// The runs `query` asks for, page by page.
pub(crate) async fn runs(ctx: &Context, query: &Query) -> Result<RunList, Error> {
    let calcifer = ctx.calcifer().await?;
    let mut out = RunList {
        runs: vec![],
        older: 0,
        machine: None,
        unreadable: vec![],
        others: vec![],
    };
    let mut page = Query {
        limit: Some(query.limit.unwrap_or(500).min(500)),
        ..query.clone()
    };
    loop {
        let list = calcifer.runs(&page).await?;
        let last = list.runs.last().map(|r| r.number);
        let more = list.older > 0;
        out.older = list.older;
        out.machine = list.machine;
        out.unreadable.extend(list.unreadable);
        out.runs.extend(list.runs);
        out.others.extend(list.others);
        let wanted = query
            .limit
            .is_none_or(|limit| (out.runs.len() as u64) < limit);
        match last {
            Some(number) if more && wanted => page.before = Some(number),
            _ => return Ok(out),
        }
    }
}

/// Whether `run` is one `query` lists: its state, package and machine.
pub(crate) fn matches(query: &Query, run: &Run) -> bool {
    if !shown(run) {
        return false;
    }
    let status = serde_json::to_value(run.status).ok();
    let state = query
        .status
        .as_ref()
        .is_none_or(|s| status.as_ref().and_then(|v| v.as_str()) == Some(s.as_str()));
    let package = query
        .package
        .as_ref()
        .is_none_or(|p| run.target.starts_with(p.as_str()));
    let machine = query
        .machine
        .as_ref()
        .is_none_or(|m| run.machine.as_ref() == Some(m));
    let project = query.project.as_ref().is_none_or(|p| run.project_id.as_ref() == Some(p));
    state && package && machine && project
}

/// `text` cut to one cell's width.
fn clip(text: String) -> String {
    match text.chars().count() > CELL {
        true => format!("{}…", text.chars().take(CELL - 1).collect::<String>()),
        false => text,
    }
}

/// A target cut to one cell, its `@version` always kept.
fn clip_target(target: String) -> String {
    let Some(at) = target.rfind('@').filter(|_| target.chars().count() > CELL) else {
        return clip(target);
    };
    let version = &target[at..];
    let keep = CELL.saturating_sub(version.chars().count() + 1);
    format!("{}…{version}", target[..at].chars().take(keep).collect::<String>())
}

/// The runs as rows of the table and the live view: # · TARGET · PROJECT · MACHINE · STATUS · PROGRESS ·
/// TIME. A queued run's STATUS is its place in its machine's line (`queue: 1` is next: only the
/// running work is ahead of it). PROGRESS is the stage while it runs, and a failed or canceled
/// run's reason in one line; `cozy run show` has it whole. TIME is its own, never its queue's.
pub(crate) fn rows(runs: &[Run]) -> Vec<Vec<String>> {
    runs.iter().map(row).collect()
}

/// Whether the list people read shows it: what a walk sends ahead of itself is `cozy project`'s own (`run show` and `--json` have it).
pub(crate) fn shown(run: &Run) -> bool {
    !run.target.starts_with(api::calcifer::project::AHEAD)
}

fn row(run: &Run) -> Vec<String> {
    let mut target = run.target.strip_prefix("_/").unwrap_or(&run.target).to_owned();
    if run.kind != RunKind::Call {
        target = format!("{} {target}", run.kind.as_str());
    }
    let status = match run.queue_position {
        Some(n) => format!("queue: {n}"),
        None => follow::status(run),
    };
    let progress = match (run.status, &run.progress) {
        _ if run.queue_position.is_some() => String::new(),
        (Status::InProgress, Some(p)) => {
            let mut shown = match p.done_fraction {
                Some(f) => format!("{} {:.0}%", p.stage, f * 100.0),
                None => p.stage.clone(),
            };
            if let Some(left) = follow::left(p) {
                shown = format!("{shown} · {left}");
            }
            let shown = match (&run.stalled, follow::still(p)) {
                (Some(stalled), _) => format!("{shown} · {}", follow::stalled_words(stalled)),
                (None, Some(still)) => format!("{shown} · no progress for {}", output::span(still)),
                (None, None) => shown,
            };
            // What it waits on says first (an update waiting for the runs in flight).
            match &run.waiting {
                Some(waiting) => format!("{waiting} · {shown}"),
                None => shown,
            }
        }
        (Status::InProgress, None) => {
            let shown = run.waiting.clone().or_else(|| run.machine_state.clone()).unwrap_or_default();
            match &run.stalled {
                Some(stalled) => format!("{shown} · {}", follow::stalled_words(stalled)),
                None => shown,
            }
        }
        (Status::Failed | Status::Canceled, _) => run.reason.as_ref().map(|r| r.message.clone()).unwrap_or_default(),
        _ => String::new(),
    };
    let progress = progress.split_whitespace().collect::<Vec<_>>().join(" ");
    // Its own time, never its queue's: since it began executing, then its execution's total.
    let time = match (run.status, run.execution_ms, run.started_at_ms) {
        (Status::InProgress, _, Some(at)) => seconds(now_ms().saturating_sub(at.max(0) as u64)),
        (Status::InProgress | Status::Queued, ..) => "—".into(),
        (_, Some(ms), _) => seconds(ms),
        (_, None, Some(at)) => run.settled_at_ms.map_or_else(|| "—".into(), |end| seconds(end.saturating_sub(at).max(0) as u64)),
        _ => "—".into(),
    };
    vec![
        run.name(),
        clip_target(target),
        run.project.clone().unwrap_or_else(|| "—".into()),
        run.machine.clone().unwrap_or_else(|| "—".into()),
        status,
        clip(progress),
        time,
    ]
}

fn seconds(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}
