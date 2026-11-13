//! What a caller expects to send this machine (cl-967, Paul 11-12): its upcoming jobs, nearest first (`PUT /v1/upcoming`), and what
//! to have ready with no job yet when it expects demand (`PUT /v1/prepare`). Each item is a function's warm run of the machine's own,
//! as a project's fetch ahead was: its code installed, then what its call reads up front and its guesses while they fit, behind every
//! run in the machine's lines, a prepare behind every upcoming job (`origin::priority`). A list is the caller's whole one: what it
//! drops is canceled. Their ids are kept out of every listing; their downloads show in Status as any, under these ids.
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use api::{error::Error, machine as v1};

use crate::{api::auth::VerifiedActor, runs::Runs};

/// The prefixes of the ids an upcoming job's and a prepare item's runs take.
pub(crate) const UPCOMING: &str = "cozy:upcoming:";
pub(crate) const PREPARE: &str = "cozy:prepare:";

/// A caller, in the execution scope it asked from (a project's own engine, or the machine's), and a list's prefix.
type List = (String, &'static str);

/// Each caller's latest lists' run ids: what its Status says of them.
static LISTED: LazyLock<Mutex<HashMap<List, Vec<String>>>> = LazyLock::new(Mutex::default);

/// The run ids of `actor`'s latest lists in this request's scope: a project's own lists are its engine's alone.
pub(crate) fn listed(actor: &str) -> Vec<String> {
    let actor = crate::private_io::resource_key(actor);
    let listed = LISTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    listed.iter().filter(|((owner, _), _)| *owner == actor).flat_map(|(_, ids)| ids.clone()).collect()
}

/// A run of a list's: never listed.
pub(crate) fn hidden(id: &str) -> bool {
    id.starts_with(UPCOMING) || id.starts_with(PREPARE)
}

/// Replaces `actor`'s list under `prefix` with `items`, in their order: each new one admitted as a warm run, each it drops canceled.
pub(crate) fn put(runs: &Arc<Runs>, actor: VerifiedActor, prefix: &'static str, items: Vec<v1::Ahead>) -> Result<v1::AheadAnswer, Error> {
    let requests: Vec<v1::RunRequest> = (items.into_iter())
        .map(|item| {
            // Its Hub's transport for TensorFS too, as a run's spec carries it.
            let tensorfs = (item.hub.as_ref().filter(|hub| !hub.origin.is_empty()))
                .map(|hub| v1::TensorFSOptions { endpoint: hub.origin.clone(), ca_der: hub.ca_der.clone(), ..Default::default() });
            let spec = v1::RunSpec {
                kind: v1::RunKind::Warm,
                source: item.source,
                entrypoint: item.entrypoint,
                models: item.models,
                lane: item.lane,
                hub: item.hub,
                tensorfs,
                ..Default::default()
            };
            v1::RunRequest { id: format!("{prefix}{}", item.key), spec }
        })
        .collect();
    let ids: Vec<String> = requests.iter().map(|request| request.id.clone()).collect();
    let owner = crate::private_io::resource_key(&crate::runs::actor_id(actor));
    LISTED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert((owner, prefix), ids);
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
