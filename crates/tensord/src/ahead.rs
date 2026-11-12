//! What a caller expects to send this machine (cl-967, Paul 11-12): its upcoming jobs, nearest first (`PUT /v1/upcoming`), and what
//! to have ready with no job yet when it expects demand (`PUT /v1/prepare`). Each item is a function's warm run of the machine's own,
//! as a project's fetch ahead was: its code installed, then what its call reads up front and its guesses while they fit, behind every
//! run in the machine's lines, a prepare behind every upcoming job (`origin::priority`). A list is the caller's whole one: what it
//! drops is canceled. Their ids are kept out of every listing; their downloads show in Status as any, under these ids.
use std::sync::Arc;

use api::{error::Error, machine as v1};

use crate::{api::auth::VerifiedActor, runs::Runs};

/// The prefixes of the ids an upcoming job's and a prepare item's runs take.
pub(crate) const UPCOMING: &str = "cozy:upcoming:";
pub(crate) const PREPARE: &str = "cozy:prepare:";

/// A run of a list's: never listed.
pub(crate) fn hidden(id: &str) -> bool {
    id.starts_with(UPCOMING) || id.starts_with(PREPARE)
}

/// Replaces `actor`'s list under `prefix` with `items`, in their order: each new one admitted as a warm run, each it drops canceled.
pub(crate) fn put(runs: &Arc<Runs>, actor: VerifiedActor, prefix: &str, items: Vec<v1::Ahead>) -> Result<v1::AheadAnswer, Error> {
    let requests: Vec<v1::RunRequest> = (items.into_iter())
        .map(|item| {
            let spec = v1::RunSpec {
                kind: v1::RunKind::Warm,
                source: item.source,
                entrypoint: item.entrypoint,
                models: item.models,
                lane: item.lane,
                hub: item.hub,
                ..Default::default()
            };
            v1::RunRequest { id: format!("{prefix}{}", item.key), spec }
        })
        .collect();
    for id in runs.live_ids(actor)?.into_iter().filter(|id| id.starts_with(prefix)) {
        if !requests.iter().any(|request| request.id == id) {
            runs.cancel(actor, &id)?;
        }
    }
    let items = (runs.accept(actor, requests)?.into_iter())
        .map(|admitted| match admitted {
            Ok((created, state)) => v1::Admitted { created, state: Some(state), refusal: None },
            Err(error) => v1::Admitted { refusal: Some(error), ..Default::default() },
        })
        .collect();
    Ok(v1::AheadAnswer { items })
}
