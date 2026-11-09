//! Browser sessions (CALCIFER-API.md, Session). The holder's client asks for a single-use code, and a
//! page trades it for a `token`, sent as `Authorization` on every call, and a `media` secret for the
//! URLs of what can't send a header (`<img>`, `<video>`). No cookie: a cookie reaches every port of a
//! host, and `localStorage` only its own. Only digests are kept, in `state.db`'s `sessions`, so a
//! session outlives a restart and the store opens nothing.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
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

const CODE_LIVES_MS: i64 = 60_000;
/// A session unused this long has ended.
const UNUSED_MS: i64 = 30 * 24 * 3600 * 1000;
/// How far behind its row's `used_at_ms` may fall: far less than [`UNUSED_MS`], and no write per call.
const NOTED_EVERY_MS: i64 = 3600 * 1000;

/// The sessions of `state.db`, as Calcifer reads them on every call, and the codes not yet traded, which no restart needs to keep.
pub(crate) struct Sessions {
    live: Mutex<Vec<SessionRow>>,
    codes: Mutex<HashMap<String, i64>>,
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
        Self { live: Mutex::new(live), codes: Mutex::default() }
    }

    fn mint_code(&self) -> (String, i64) {
        let (code, now) = (secret(), now_ms());
        let mut codes = self.codes.lock().expect("session codes");
        codes.retain(|_, expires_ms| now < *expires_ms);
        codes.insert(digest(&code), now + CODE_LIVES_MS);
        (code, now + CODE_LIVES_MS)
    }

    /// `code`, spent whether or not this goes on to succeed.
    fn start(&self, store: &Store, code: &str) -> Result<BrowserSession, Refusal> {
        let now = now_ms();
        let expires_ms = self.codes.lock().expect("session codes").remove(&digest(code));
        if !expires_ms.is_some_and(|expires_ms| now < expires_ms) {
            let said = "this sign-in code is unknown, used or expired";
            return Err(Refusal(Error::new(Kind::Validation, "session_code_invalid", said).next("cozy open")));
        }
        let (token, media) = (secret(), secret());
        let session = SessionRow { token: digest(&token), media: digest(&media), used_at_ms: now };
        let mut live = self.live.lock().expect("sessions");
        store.session_started(&session, now - UNUSED_MS).map_err(unwritable)?;
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
        let presented = digest(presented);
        self.used(store, |session| session.token == presented)
    }

    pub(crate) fn admits_media(&self, store: &Store, presented: &str) -> bool {
        let presented = digest(presented);
        self.used(store, |session| session.media == presented)
    }

    /// Ends the session `token` opens; any other token names none, and nothing happens.
    fn end(&self, store: &Store, token: &str) -> Result<(), Refusal> {
        let token = digest(token);
        let mut live = self.live.lock().expect("sessions");
        let Some(at) = live.iter().position(|session| session.token == token) else { return Ok(()) };
        store.session_ended(&token).map_err(unwritable)?;
        live.swap_remove(at);
        Ok(())
    }
}

fn unwritable(e: StoreError) -> Refusal {
    api::refuse(Kind::Internal, "session_unwritable", e.to_string())
}

/// `POST /v1/session-codes`: the holder's bearer only, so a page can't sign in another.
pub(crate) async fn code(State(api): State<Arc<Api>>, headers: HeaderMap) -> Result<Json<SessionCode>, Refusal> {
    if !api::bearer(&headers).is_some_and(|token| api.admits(token)) {
        let said = "a sign-in code comes from the bearer in Calcifer's lock record, not a session";
        return Err(Refusal(Error::new(Kind::Forbidden, "session_code_needs_bearer", said).next("cozy open")));
    }
    let host = headers.get(header::HOST).and_then(|host| host.to_str().ok()).unwrap_or_default();
    let (code, expires_ms) = api.sessions.mint_code();
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
