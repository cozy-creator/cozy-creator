//! Package locks and publications: work that goes on after its POST answers, followed on its
//! events stream. Kept while this Calcifer runs: one that stopped under it is gone, and asking
//! again is safe (a lock writes `uv.lock` whole; a publication finds what the Hub already holds).
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
};

use axum::{
    Json,
    body::Bytes,
    extract::{
        Path, Query, State,
        rejection::{BytesRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
    response::sse::{self, KeepAlive, Sse},
};
use futures_util::Stream;
use protocol::{
    error::Error,
    hub::{Authoring, AuthoringEvent, LockAsk, PublishAsk, WorkState},
};
use serde::{Deserialize, Serialize};

use sha2::{Digest as _, Sha256};
use tokio::sync::watch;

use crate::api::{Api, IdempotencyKey, Refusal, parse};

/// Every lock and publication this Calcifer started, by id, and each key's id.
#[derive(Default)]
pub(crate) struct Works {
    by_id: Mutex<HashMap<String, Arc<Work>>>,
    by_key: Mutex<HashMap<String, (String, String)>>,
}

pub(crate) struct Work {
    id: String,
    events: Mutex<Vec<AuthoringEvent>>,
    told: watch::Sender<usize>,
}

impl Work {
    /// A notice, said as the work goes.
    pub(crate) fn say(&self, message: &str) {
        self.push(AuthoringEvent::Notice { message: message.to_owned() });
    }

    fn push(&self, event: AuthoringEvent) {
        let mut events = self.events.lock().expect("work events");
        events.push(event);
        self.told.send_replace(events.len());
    }

    fn record(&self) -> Authoring {
        let events = self.events.lock().expect("work events");
        let mut record =
            Authoring { id: self.id.clone(), state: WorkState::Running, notices: vec![], locked: None, published: None, error: None };
        for event in events.iter() {
            match event.clone() {
                AuthoringEvent::Notice { message } => record.notices.push(message),
                AuthoringEvent::Locked { locked } => (record.state, record.locked) = (WorkState::Done, Some(locked)),
                AuthoringEvent::Published { published } => (record.state, record.published) = (WorkState::Done, Some(published)),
                AuthoringEvent::Failed { error } => (record.state, record.error) = (WorkState::Failed, Some(error)),
            }
        }
        record
    }
}

impl Works {
    /// The work `key` names, or a new one of `kind` for `ask`; true when new. A key names one ask.
    fn start(&self, kind: &str, key: &str, ask: &impl Serialize) -> Result<(Arc<Work>, bool), Refusal> {
        let digest = hex::encode(Sha256::digest(serde_json::to_vec(ask).expect("an ask serializes")));
        let mut keys = self.by_key.lock().expect("work keys");
        if let Some((id, asked)) = keys.get(key) {
            if *asked != digest {
                return Err(Refusal(crate::api::conflict(key)));
            }
            if let Some(work) = self.by_id.lock().expect("works").get(id) {
                return Ok((work.clone(), false));
            }
        }
        let id = format!("{kind}-{}", hex::encode(crate::random()));
        let work = Arc::new(Work { id: id.clone(), events: Mutex::default(), told: watch::channel(0).0 });
        keys.insert(key.to_owned(), (id.clone(), digest));
        self.by_id.lock().expect("works").insert(id, work.clone());
        Ok((work, true))
    }

    fn get(&self, kind: &str, id: &str) -> Result<Arc<Work>, Refusal> {
        let found = id.starts_with(&format!("{kind}-")).then(|| self.by_id.lock().expect("works").get(id).cloned()).flatten();
        found.ok_or_else(|| {
            Refusal(
                Error::not_found(format!("{kind}_not_found"), format!("no {kind} {id}: this Calcifer did not start it"))
                    .remedy("a Calcifer that stopped forgets its work; ask again, which is safe"),
            )
        })
    }
}

fn answered(fresh: bool, work: &Work) -> (StatusCode, Json<Authoring>) {
    (if fresh { StatusCode::ACCEPTED } else { StatusCode::OK }, Json(work.record()))
}

/// `POST /v1/package-locks`.
pub(super) async fn lock(
    State(api): State<Arc<Api>>,
    IdempotencyKey(key): IdempotencyKey,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Authoring>), Refusal> {
    let ask: LockAsk = parse(&body?)?;
    let (work, fresh) = api.works.start("lock", &key, &ask)?;
    if fresh {
        let (api, running) = (api.clone(), work.clone());
        tokio::spawn(async move {
            let end = match super::author::lock(&api, ask, &running).await {
                Ok(locked) => AuthoringEvent::Locked { locked },
                Err(error) => AuthoringEvent::Failed { error },
            };
            running.push(end);
        });
    }
    Ok(answered(fresh, &work))
}

/// `POST /v1/package-publications`.
pub(super) async fn publish(
    State(api): State<Arc<Api>>,
    IdempotencyKey(key): IdempotencyKey,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Authoring>), Refusal> {
    let ask: PublishAsk = parse(&body?)?;
    let (work, fresh) = api.works.start("publication", &key, &ask)?;
    if fresh {
        let (api, running) = (api.clone(), work.clone());
        tokio::spawn(async move {
            let end = match super::author::publish(&api, ask, &running).await {
                Ok(published) => AuthoringEvent::Published { published },
                Err(error) => AuthoringEvent::Failed { error },
            };
            running.push(end);
        });
    }
    Ok(answered(fresh, &work))
}

pub(super) async fn show_lock(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Json<Authoring>, Refusal> {
    Ok(Json(api.works.get("lock", &id)?.record()))
}

pub(super) async fn show_publication(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Json<Authoring>, Refusal> {
    Ok(Json(api.works.get("publication", &id)?.record()))
}

#[derive(Deserialize)]
pub(super) struct Resume {
    starting_after: Option<usize>,
}

pub(super) async fn lock_events(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    query: Result<Query<Resume>, QueryRejection>,
) -> Result<Sse<impl Stream<Item = Result<sse::Event, Infallible>>>, Refusal> {
    events(&api, "lock", &id, &headers, query?.0)
}

pub(super) async fn publication_events(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    query: Result<Query<Resume>, QueryRejection>,
) -> Result<Sse<impl Stream<Item = Result<sse::Event, Infallible>>>, Refusal> {
    events(&api, "publication", &id, &headers, query?.0)
}

/// The work's events after the cursor (`?starting_after=`, else `Last-Event-ID`), each with its
/// number as its `id:`, then as they come; the stream ends after the last.
fn events(
    api: &Arc<Api>,
    kind: &str,
    id: &str,
    headers: &HeaderMap,
    resume: Resume,
) -> Result<Sse<impl Stream<Item = Result<sse::Event, Infallible>> + use<>>, Refusal> {
    let work = api.works.get(kind, id)?;
    let last = headers.get("last-event-id").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok());
    let sent = resume.starting_after.or(last).unwrap_or(0);
    let told = work.told.subscribe();
    let stop = api.stop_signal();
    let stream = futures_util::stream::unfold((work, told, stop, sent), |(work, mut told, mut stop, sent)| async move {
        loop {
            let next = work.events.lock().expect("work events").get(sent).cloned();
            if let Some(event) = next {
                let ended = event.is_terminal();
                let data = sse::Event::default().id((sent + 1).to_string()).json_data(&event).expect("an event serializes");
                // After the last event, one more turn ends the stream.
                let sent = if ended { usize::MAX } else { sent + 1 };
                return Some((Ok(data), (work, told, stop, sent)));
            }
            if sent == usize::MAX {
                return None;
            }
            tokio::select! {
                changed = told.changed() => if changed.is_err() { return None },
                _ = stop.wait_for(|stopping| *stopping) => return None,
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
