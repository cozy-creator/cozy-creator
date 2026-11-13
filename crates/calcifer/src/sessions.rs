//! Browser sessions (CALCIFER-API.md, Session). The holder's client asks for a single-use code, and a
//! page trades it for a `token`, sent as `Authorization` on every call, and a `media` secret for the
//! URLs of what can't send a header (`<img>`, `<video>`). No cookie: a cookie reaches every port of a
//! host, and `localStorage` only its own. Only digests are kept, in `state.db`'s `sessions`, so a
//! session outlives a restart and the store opens nothing.
use std::{
    sync::{Arc, Mutex, Weak},
    collections::HashMap,
};

use axum::{
    Json,
    body::Bytes,
    extract::{State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode, header},
};
use ::api::{
    calcifer::{BrowserSession, RedeemCode, SessionCode},
    error::{Error, Kind},
};
use sha2::{Digest as _, Sha256};
use tracing::warn;

use crate::{
    api::{self, Api, Refusal},
    state::SessionRow,
    store::{Store, StoreError, now_ms},
};

const CODE_LIVES_MS: i64 = 15 * 60 * 1000;
/// A session unused this long has ended.
const UNUSED_MS: i64 = 30 * 24 * 3600 * 1000;
/// How far behind its row's `used_at_ms` may fall: far less than [`UNUSED_MS`], and no write per call.
const NOTED_EVERY_MS: i64 = 3600 * 1000;

/// Browser sessions cached in memory; unredeemed code digests stay in the store across restarts.
pub(crate) struct Sessions {
    live: Mutex<Vec<SessionRow>>,
    owned: Arc<Mutex<HashMap<String, (String, String)>>>,
}

fn secret() -> String {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).expect("the OS provides randomness");
    hex::encode(bytes)
}

fn digest(secret: &str) -> String {
    hex::encode(Sha256::digest(secret))
}

impl Sessions {
    /// The sessions `store` keeps. One it can't read costs only a new link.
    pub(crate) fn open(store: &Store) -> Self {
        let live = store.sessions(now_ms() - UNUSED_MS).unwrap_or_else(|e| {
            warn!("{e}; browser sessions start over");
            vec![]
        });
        Self { live: Mutex::new(live), owned: Arc::default() }
    }

    /// Memory-only authority minted solely for a fresh Cozy-owned private process.
    pub(crate) fn begin_owned(&self) -> (OwnedSessionLease, BrowserSession) {
        let (id, token, media) = (secret(), secret(), secret());
        self.owned.lock().expect("owned sessions").insert(id.clone(), (digest(&token), digest(&media)));
        (OwnedSessionLease { id, sessions: Arc::downgrade(&self.owned) }, BrowserSession { token, media })
    }

    pub(crate) fn admits_owned_token(&self, token: &str) -> bool {
        let token = digest(token);
        self.owned.lock().expect("owned sessions").values().any(|(held, _)| held == &token)
    }

    pub(crate) fn admits_owned_media(&self, media: &str) -> bool {
        let media = digest(media);
        self.owned.lock().expect("owned sessions").values().any(|(_, held)| held == &media)
    }

    fn mint_code(&self, store: &Store) -> Result<(String, i64), Refusal> {
        let (code, now) = (secret(), now_ms());
        let expires_ms = now + CODE_LIVES_MS;
        store.session_code(&digest(&code), now, expires_ms).map_err(unwritable)?;
        Ok((code, expires_ms))
    }

    fn start(&self, store: &Store, code: &str) -> Result<BrowserSession, Refusal> {
        self.start_at(store, code, now_ms())
    }

    fn start_at(&self, store: &Store, code: &str, now: i64) -> Result<BrowserSession, Refusal> {
        let (token, media) = (secret(), secret());
        let session = SessionRow { token: digest(&token), media: digest(&media), used_at_ms: now };
        if !store.session_started(&digest(code), &session, now, now - UNUSED_MS).map_err(unwritable)? {
            let said = "this link's code is unknown, used or expired";
            return Err(Refusal(Error::new(Kind::Validation, "session_code_invalid", said).next("cozy open")));
        }
        let mut live = self.live.lock().expect("sessions");
        live.retain(|session| now - session.used_at_ms < UNUSED_MS);
        live.push(session);
        Ok(BrowserSession { token, media })
    }

    /// Whether the session `of` picks is live, noting its use.
    fn used(&self, store: &Store, of: impl Fn(&SessionRow) -> bool) -> bool {
        let now = now_ms();
        let mut live = self.live.lock().expect("sessions");
        let Some(at) = live.iter().position(of) else { return false };
        let since = now - live[at].used_at_ms;
        let noted = if since >= UNUSED_MS {
            store.session_ended(&live.swap_remove(at).token)
        } else if since >= NOTED_EVERY_MS {
            live[at].used_at_ms = now;
            store.session_used(&live[at].token, now)
        } else {
            return true;
        };
        if let Err(e) = noted {
            warn!("noting a browser session's use: {e}");
        }
        since < UNUSED_MS
    }

    pub(crate) fn admits_token(&self, store: &Store, presented: &str) -> bool {
        if self.admits_owned_token(presented) { return true; }
        let presented = digest(presented);
        self.used(store, |session| session.token == presented)
    }

    pub(crate) fn admits_media(&self, store: &Store, presented: &str) -> bool {
        if self.admits_owned_media(presented) { return true; }
        let presented = digest(presented);
        self.used(store, |session| session.media == presented)
    }

    /// Ends the session `token` opens; any other token names none, and nothing happens.
    fn end(&self, store: &Store, token: &str) -> Result<(), Refusal> {
        let token = digest(token);
        self.owned.lock().expect("owned sessions").retain(|_, (held, _)| held != &token);
        let mut live = self.live.lock().expect("sessions");
        let Some(at) = live.iter().position(|session| session.token == token) else { return Ok(()) };
        store.session_ended(&token).map_err(unwritable)?;
        live.swap_remove(at);
        Ok(())
    }
}

/// One exact owned browser incarnation. It cannot be cloned or recovered from a persisted token.
pub(crate) struct OwnedSessionLease {
    id: String,
    sessions: Weak<Mutex<HashMap<String, (String, String)>>>,
}

impl OwnedSessionLease {
    pub(crate) fn id(&self) -> &str { &self.id }
}

impl Drop for OwnedSessionLease {
    fn drop(&mut self) {
        if let Some(sessions) = self.sessions.upgrade() {
            sessions.lock().expect("owned sessions").remove(&self.id);
        }
    }
}

fn unwritable(e: StoreError) -> Refusal {
    api::refuse(Kind::Internal, "session_unwritable", e.to_string())
}

/// `POST /v1/session-codes`: the holder's bearer only, so a page can't open another.
pub(crate) async fn code(State(api): State<Arc<Api>>, headers: HeaderMap) -> Result<Json<SessionCode>, Refusal> {
    if !api::bearer(&headers).is_some_and(|token| api.admits(token)) {
        let said = "a link's code comes from the bearer in Calcifer's lock record, not a page's session";
        return Err(Refusal(Error::new(Kind::Forbidden, "session_code_needs_bearer", said).next("cozy open")));
    }
    let host = headers.get(header::HOST).and_then(|host| host.to_str().ok()).unwrap_or_default();
    let (code, expires_ms) = api.sessions.mint_code(api.calcifer.store())?;
    Ok(Json(SessionCode { url: format!("http://{host}/#code={code}"), code, expires_ms }))
}

/// `POST /v1/session`, outside `admit`: the code is the credential.
pub(crate) async fn start(
    State(api): State<Arc<Api>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<BrowserSession>, Refusal> {
    let RedeemCode { code } = api::parse(&body?)?;
    Ok(Json(api.sessions.start(api.calcifer.store(), &code)?))
}

/// `DELETE /v1/session`: the session whose token this carries has ended.
pub(crate) async fn end(State(api): State<Arc<Api>>, headers: HeaderMap) -> Result<StatusCode, Refusal> {
    if let Some(token) = api::bearer(&headers) {
        api.sessions.end(api.calcifer.store(), token)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_codes_live_for_fifteen_minutes_and_survive_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let store = Store::open(&path).unwrap();
        let sessions = Sessions::open(&store);
        let before = now_ms();
        let (code, expires_ms) = sessions.mint_code(&store).unwrap();
        assert_eq!(CODE_LIVES_MS, 900_000);
        assert!((before + CODE_LIVES_MS..=now_ms() + CODE_LIVES_MS).contains(&expires_ms));
        let kept: String = store.read(|conn| Ok(conn.query_row("SELECT code_sha256 FROM session_codes", [], |row| row.get(0))?)).unwrap();
        assert_eq!(kept, digest(&code));
        drop(sessions);
        drop(store);

        let store = Store::open(&path).unwrap();
        let sessions = Sessions::open(&store);
        assert!(sessions.start_at(&store, &code, expires_ms - 1).is_ok());
        assert!(sessions.start_at(&store, &code, expires_ms - 1).is_err());
        let (expired, boundary) = sessions.mint_code(&store).unwrap();
        assert!(sessions.start_at(&store, &expired, boundary).is_err());
    }

    #[test]
    fn a_sign_in_code_can_be_redeemed_once_across_concurrent_requests() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&directory.path().join("state.db")).unwrap());
        let sessions = Arc::new(Sessions::open(&store));
        let (code, _) = sessions.mint_code(&store).unwrap();
        let requests: Vec<_> = (0..2).map(|_| {
            let store = Arc::clone(&store);
            let sessions = Arc::clone(&sessions);
            let code = code.clone();
            std::thread::spawn(move || sessions.start(&store, &code).is_ok())
        }).collect();
        let successes = requests.into_iter().map(|request| request.join().unwrap()).filter(|ok| *ok).count();
        assert_eq!(successes, 1);
    }
}
