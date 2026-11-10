//! Following a run (G/API.md): its log as a stream, and what an output-limited grant sees of it.
//! Status and software-update runs are `machine_status`'s and `machine_update`'s.
use super::{auth::VerifiedActor, capability::Grant, http::blocking, v1};
use crate::runs::{Log, Runs};
use api::error::Error;
use std::sync::Arc;
use tokio::sync::mpsc;

/// What an output-limited grant sees of an event: its outputs' products, and the outcome's
/// status and inventory of them. Results, reasons, measurements, adjustments, memo facts, waiting
/// details, progress and logs stay the run owner's.
fn visible(grant: &Grant, event: &mut v1::RunEvent) -> bool {
    let allows = |p: &v1::Product| grant.allows(&grant.run, &p.output, (p.index > 0).then_some(p.index));
    match &mut event.event {
        v1::Event::Product(product) => allows(&*product),
        v1::Event::Outcome(outcome) => {
            let (status, mut outputs) = (std::mem::take(&mut outcome.status), std::mem::take(&mut outcome.outputs));
            outputs.retain(allows);
            *outcome = v1::Outcome { status, outputs, ..Default::default() };
            true
        }
        v1::Event::State(state) => {
            state.waiting.clear();
            true
        }
        _ => false,
    }
}

/// A run's log after `cursor`, its current state first, until its outcome or the caller leaves;
/// it holds no admission. An output-limited grant (`limit`) sees only what [`visible`] keeps.
pub(super) async fn follow(
    runs: Arc<Runs>,
    actor: VerifiedActor,
    id: String,
    cursor: u64,
    limit: Option<Grant>,
) -> Result<mpsc::Receiver<Result<v1::RunEvent, Error>>, Error> {
    let run = id.clone();
    let current = blocking(&runs, move |runs| runs.state(actor, &run)).await?;
    let (sender, receiver) = mpsc::channel(256);
    let snapshot = v1::RunEvent { sequence: 0, at_ms: 0, event: v1::Event::State(current) };
    let _ = sender.try_send(Ok(snapshot));
    let owner = limit.is_none();
    tokio::spawn(async move {
        if let Err(error) = log(runs.clone(), actor, id.clone(), cursor, limit, &sender).await {
            tracing::warn!("run {id}: its log can't be read: {error}");
            // An outcome that can't be made never can be: said as its outcome, the run ends (cl-773).
            if matches!(error.code.as_str(), "custody_corrupt" | "result_unreadable") {
                let reason = v1::Reason { code: error.code, message: error.message, origin: "machine".into(), ..Default::default() };
                let outcome = v1::Outcome { status: "failed".into(), reason: Some(reason), ..Default::default() };
                let _ = sender.send(Ok(v1::RunEvent { sequence: cursor + 1, at_ms: 0, event: v1::Event::Outcome(outcome) })).await;
                return;
            }
            // A finished run's stream says why its outcome isn't sent, then ends: its controller says
            // so and asks again (cl-806). A body broken off after it would lose that frame unsent.
            let run = id.clone();
            if let Ok(mut state) = blocking(&runs, move |runs| runs.state(actor, &run)).await {
                if matches!(state.state.as_str(), "succeeded" | "failed" | "canceled") && owner {
                    state.waiting = format!("its outcome can't be read: {}: {}", error.code, error.message);
                    let _ = sender.send(Ok(v1::RunEvent { sequence: 0, at_ms: 0, event: v1::Event::State(state) })).await;
                    return;
                }
            }
            // Any other error breaks the stream off, and its controller attaches again.
            let _ = sender.send(Err(error)).await;
        }
    });
    Ok(receiver)
}

async fn log(
    runs: Arc<Runs>,
    actor: VerifiedActor,
    id: String,
    cursor: u64,
    limit: Option<Grant>,
    sender: &mpsc::Sender<Result<v1::RunEvent, Error>>,
) -> Result<(), Error> {
    // Read from its start (`Log`); only entries past the cursor are sent.
    let mut log = Log::default();
    loop {
        let run = id.clone();
        let (events, read) = blocking(&runs, move |runs| {
            let events = runs.read(actor, &run, &mut log)?;
            Ok((events, log))
        })
        .await?;
        log = read;
        let mut ended = false;
        for mut event in events {
            ended |= matches!(event.event, v1::Event::Outcome(_));
            let hidden = limit.as_ref().is_some_and(|grant| !visible(grant, &mut event));
            if event.sequence <= cursor || hidden {
                continue;
            }
            if sender.send(Ok(event)).await.is_err() {
                return Ok(());
            }
        }
        if ended || sender.is_closed() {
            return Ok(());
        }
    }
}
