//! Runs other clients of this account sent to one of its machines: read on that machine, which on a
//! rental lets each of its owner's keys read every run on it, and never held here. Each is named
//! `<machine>/<number>`, by the machine's own number for it.
use std::convert::Infallible;

use axum::response::sse;
use futures_util::{Stream, StreamExt as _, stream};
use protocol::{
    calcifer::{Event, Kind, Run, Status},
    error::{Error, Kind as Refused},
    ids::RunId,
    machine as v1,
};

use crate::{
    api::Api,
    link::Live,
    machines::{internal, refusal},
    store::MachineRow,
};

/// How many of a machine's other runs a list shows.
const LISTED: u32 = 50;

/// The machine and number `<machine>/<number>` names.
pub(crate) fn named(key: &str) -> Option<(&str, u64)> {
    let (machine, number) = key.rsplit_once('/')?;
    Some((machine, number.parse().ok()?))
}

/// The runs on `row` that no run here is, newest first.
pub(crate) async fn listed(api: &Api, row: &MachineRow) -> Result<Vec<Run>, Error> {
    let (row, link) = api.machines.linked(&row.name)?;
    let listed = link.runs(0, LISTED).await.map_err(|e| refusal(&row.name, e))?;
    let store = api.calcifer.store();
    let mut others = vec![];
    for state in &listed.runs {
        if store.run(&state.id).map_err(internal)?.is_none() {
            others.push(of(&row, state));
        }
    }
    Ok(others)
}

/// The run another client sent to `machine` that it numbers `number`, as it stands now, with its
/// outcome once it has one.
pub(crate) async fn show(api: &Api, machine: &str, number: u64) -> Result<Run, Error> {
    let (mut run, mut frames) = opened(api, machine, number, 0).await?;
    if matches!(run.status, Status::Completed | Status::Failed | Status::Canceled) {
        while let Some(Ok(frame)) = frames.next().await {
            if let v1::Event::Outcome(outcome) = frame.event {
                settle(&mut run, outcome);
                break;
            }
        }
    }
    Ok(run)
}

/// That run's events after the machine's `after`, as this controller's runs' are told, to its end.
pub(crate) async fn events(
    api: &Api,
    machine: &str,
    number: u64,
    after: u64,
) -> Result<impl Stream<Item = Result<sse::Event, Infallible>> + use<>, Error> {
    let (run, frames) = opened(api, machine, number, after).await?;
    let said = stream::unfold(Some((run, frames)), |held| async move {
        let (mut run, mut frames) = held?;
        let frame = frames.next().await?.ok()?;
        let sequence = frame.sequence;
        let (events, ended) = told(&mut run, frame);
        let said: Vec<Result<sse::Event, Infallible>> = (events.into_iter())
            .map(|event| {
                let sent = sse::Event::default().data(serde_json::to_string(&event).expect("an event serializes"));
                Ok(match sequence {
                    0 => sent,
                    sequence => sent.id(sequence.to_string()),
                })
            })
            .collect();
        Some((stream::iter(said), (!ended).then_some((run, frames))))
    });
    Ok(said.flatten())
}

/// The run as its machine's listing says it, and its events after `after` (its state first).
async fn opened(api: &Api, machine: &str, number: u64, after: u64) -> Result<(Run, Live<v1::RunEvent>), Error> {
    let (row, link) = api.machines.linked(machine)?;
    let listed = link.runs(number + 1, 1).await.map_err(|e| refusal(&row.name, e))?;
    let state = (listed.runs.into_iter().find(|run| run.number == number)).ok_or_else(|| {
        Error::new(Refused::NotFound, "run_not_found", format!("{} has no run {number} this computer reads", row.name))
            .next(format!("cozy run ls --on {}", row.name))
    })?;
    let frames = link.events(&state.id, after).await.map_err(|e| refusal(&row.name, e))?;
    Ok((of(&row, &state), frames))
}

/// `state`, which the machine `row` reports, as a run.
fn of(row: &MachineRow, state: &v1::RunState) -> Run {
    let mut run = Run {
        id: RunId::try_from(format!("{}/{}", row.name, state.number)).expect("a machine name and a number"),
        number: 0,
        machine_number: Some(state.number),
        kind: Kind::Call,
        target: state.target.clone(),
        hub: row.hub.clone(),
        machine: Some(row.name.clone()),
        project: None,
        project_id: None,
        status: Status::Queued,
        cancel_requested: false,
        machine_state: None,
        reason: None,
        output: vec![],
        created_at_ms: state.accepted_at_ms as i64,
        accepted_at_ms: Some(state.accepted_at_ms as i64),
        started_at_ms: None,
        settled_at_ms: None,
        waiting: None,
        progress: None,
        queue_position: None,
        warnings: vec![],
        adjustments: vec![],
        result: None,
        execution_ms: None,
        memo: None,
        measurements: None,
        stalled: None,
        calls: vec![],
        report: None,
        request: None,
    };
    stated(&mut run, state);
    run
}

/// The run as its machine's state now says it.
fn stated(run: &mut Run, state: &v1::RunState) {
    let at = |ms: u64| (ms > 0).then_some(ms as i64);
    run.status = match state.state.as_str() {
        "succeeded" => Status::Completed,
        "failed" => Status::Failed,
        "canceled" => Status::Canceled,
        "running" => Status::InProgress,
        _ => Status::Queued,
    };
    run.machine_state = Some(state.state.clone());
    run.waiting = Some(state.waiting.clone()).filter(|waiting| !waiting.is_empty() && *waiting != state.state);
    run.started_at_ms = at(state.started_at_ms).or(run.started_at_ms);
    run.settled_at_ms = at(state.finished_at_ms).or(run.settled_at_ms);
}

/// The run as its outcome leaves it.
fn settle(run: &mut Run, outcome: v1::Outcome) {
    run.status = match outcome.status.as_str() {
        "succeeded" => Status::Completed,
        "canceled" => Status::Canceled,
        _ => Status::Failed,
    };
    run.reason = outcome.reason.filter(|_| run.status != Status::Completed);
    run.result = outcome.result.and_then(|result| serde_json::from_str(result.get()).ok());
    run.execution_ms = (outcome.execution_ms > 0).then_some(outcome.execution_ms);
    run.adjustments = outcome.adjustments;
    run.waiting = None;
    run.settled_at_ms = run.settled_at_ms.or_else(|| Some(crate::store::now_ms()));
}

/// What one of the machine's frames tells of the run, and whether it ended it.
fn told(run: &mut Run, frame: v1::RunEvent) -> (Vec<Event>, bool) {
    let sequence = frame.sequence;
    match frame.event {
        v1::Event::State(state) => {
            stated(run, &state);
            let mut said = vec![Event::lifecycle(sequence, run.clone(), false)];
            said.extend(run.waiting.clone().map(|reason| Event::Waiting { reason }));
            (said, false)
        }
        v1::Event::Progress(progress) => {
            let progress = crate::progress_of(progress, frame.at_ms);
            run.progress = Some(progress.clone());
            (vec![Event::Progress { progress }], false)
        }
        v1::Event::Log(line) => {
            (vec![Event::Log { sequence_number: sequence, at_ms: frame.at_ms, level: line.level, text: line.text }], false)
        }
        v1::Event::Outcome(outcome) => {
            settle(run, outcome);
            (vec![Event::lifecycle(sequence, run.clone(), false)], true)
        }
        v1::Event::Product(_) | v1::Event::Call(_) | v1::Event::Gpu(_) => (vec![], false),
    }
}
