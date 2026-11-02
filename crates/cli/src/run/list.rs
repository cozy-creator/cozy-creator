//! `cozy run ls`: every run, newest first: a live view on a terminal, else a table or
//! JSON.
use std::process::ExitCode;

use protocol::{
    calcifer::{Kind as RunKind, Run, RunList, Status},
    error::Error,
};
use serde::Serialize;

use super::{follow, live};
use crate::{Context, output, tui};

pub(crate) const HEADERS: [&str; 6] = ["#", "TARGET", "MACHINE", "STATUS", "PROGRESS", "TIME"];
/// The newest runs a list shows unless `--limit` says otherwise.
const SHOWN: u64 = 50;
/// A target or progress is one table cell; `cozy run show` has it whole.
const CELL: usize = 72;

/// `GET /v1/runs`' filters.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Query {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

pub async fn exec(ctx: &Context, mut query: Query) -> Result<ExitCode, Error> {
    let limit = query.limit.unwrap_or(SHOWN);
    query.limit = (limit > 0).then_some(limit);
    if !ctx.json && ctx.term.stdout {
        let view = live::Runs::open(ctx, query, limit).await?;
        tui::show(ctx, tui::Screen::Runs(view)).await?;
        return Ok(ExitCode::SUCCESS);
    }
    let list = runs(ctx, &query).await?;
    for run in &list.unreadable {
        output::warn(&format!("left out {run}"));
    }
    if ctx.json {
        output::json(&list);
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = output::Table::new(ctx, HEADERS);
    for row in rows(&list.runs) {
        table = table.row(row);
    }
    if list.older > 0 {
        table = table.note(format!(
            "{} older runs: cozy run ls --limit 0",
            list.older
        ));
    }
    table.print();
    Ok(ExitCode::SUCCESS)
}

/// The runs `query` asks for, page by page.
pub(crate) async fn runs(ctx: &Context, query: &Query) -> Result<RunList, Error> {
    let calcifer = ctx.calcifer().await?;
    let mut out = RunList {
        runs: vec![],
        older: 0,
        machine: None,
        unreadable: vec![],
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
    state && package && machine
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

/// The runs as rows of the table and the live view: # · TARGET · MACHINE · STATUS · PROGRESS ·
/// TIME. A queued run's STATUS is its place in its machine's line (`queue: 1` is next: only the
/// running work is ahead of it). PROGRESS is the stage while it runs, and a failed or canceled
/// run's reason in one line; `cozy run show` has it whole. TIME is its own, never its queue's.
pub(crate) fn rows(runs: &[Run]) -> Vec<Vec<String>> {
    runs.iter().map(row).collect()
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
            let shown = match p.done_fraction {
                Some(f) => format!("{} {:.0}%", p.stage, f * 100.0),
                None => p.stage.clone(),
            };
            let shown = match follow::still(p) {
                Some(still) => format!("{shown} · no progress for {}", output::span(still)),
                None => shown,
            };
            // What it waits on says first (an update waiting for the runs in flight).
            match &run.waiting {
                Some(waiting) => format!("{waiting} · {shown}"),
                None => shown,
            }
        }
        (Status::InProgress, None) => run.waiting.clone().or_else(|| run.machine_state.clone()).unwrap_or_default(),
        (Status::Failed | Status::Canceled, _) => run.reason.as_ref().map(|r| r.message.clone()).unwrap_or_default(),
        _ => String::new(),
    };
    let progress = progress.split_whitespace().collect::<Vec<_>>().join(" ");
    // Its own time, never its queue's: since it began executing, then its execution's total.
    let time = match (run.status, run.execution_ms, run.started_at_ms) {
        (Status::InProgress, _, Some(at)) => seconds(now_ms().saturating_sub(at.max(0) as u64)),
        (Status::InProgress | Status::Queued, ..) => "—".into(),
        (_, Some(ms), _) => seconds(ms),
        _ => "—".into(),
    };
    vec![
        run.number.to_string(),
        clip_target(target),
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
