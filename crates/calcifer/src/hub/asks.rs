//! Asks that become runs: model downloads and uploads, package installs and quantizations. Each
//! builds the run it submits under the request's `Idempotency-Key` (`202` with it, `200` when the
//! key had), keeping what Calcifer does once it completes (`after`). Its quote route takes the same
//! body and answers what a `new` run would rent, renting nothing.
use std::sync::Arc;

use axum::{
    Json,
    body::Bytes,
    extract::{State, rejection::BytesRejection},
    http::StatusCode,
};
use protocol::calcifer::{RunQuote, SubmitRun, Submitted};
use serde::de::DeserializeOwned;

use super::after::After;
use crate::api::{Api, IdempotencyKey, Refusal, parse};

/// The run an ask submits, and what follows it.
pub(super) struct Prepared {
    pub submit: SubmitRun,
    pub after: Option<After>,
}

pub(super) trait Ask: DeserializeOwned + Send + 'static {
    /// The run, refused before anything is recorded; nothing here changes the Hub or a machine.
    fn prepare(self, api: &Arc<Api>) -> impl Future<Output = Result<Prepared, Refusal>> + Send;
}

pub(super) async fn create<A: Ask>(
    State(api): State<Arc<Api>>,
    IdempotencyKey(key): IdempotencyKey,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Submitted>), Refusal> {
    let prepared = parse::<A>(&body?)?.prepare(&api).await?;
    submit(&api, &key, prepared).await
}

/// Submits `prepared` under `key`, its step kept first.
pub(super) async fn submit(
    api: &Arc<Api>,
    key: &str,
    Prepared { submit, after }: Prepared,
) -> Result<(StatusCode, Json<Submitted>), Refusal> {
    // A key already recorded is answered as it is: its step, if any, is kept or done.
    let recorded = api.calcifer.store().keyed(key)?.is_some();
    let after = after.filter(|_| !recorded);
    if let Some(after) = &after {
        api.calcifer.plan(key, after).map_err(Refusal)?;
    }
    let submitted = api.calcifer.submit(key, submit).await;
    if submitted.is_err() && after.is_some() {
        api.calcifer.unplan(key);
    }
    let submitted = submitted?;
    let status = if submitted.replay { StatusCode::OK } else { StatusCode::ACCEPTED };
    Ok((status, Json(submitted)))
}

pub(super) async fn quote<A: Ask>(
    State(api): State<Arc<Api>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<RunQuote>, Refusal> {
    let Prepared { submit, .. } = parse::<A>(&body?)?.prepare(&api).await?;
    api.machines.hubs.client(&submit.hub).map_err(Refusal)?;
    Ok(Json(api.machines.run_quote(&submit).await.map_err(Refusal)?))
}
