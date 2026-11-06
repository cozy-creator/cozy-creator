//! Browser sessions (CALCIFER-API.md, Session). The holder's client asks for a single-use code, and a
//! page trades it for a `token`, sent as `Authorization` on every call, and a `media` secret for the
//! URLs of what can't send a header (`<img>`, `<video>`). No cookie: a cookie reaches every port of a
//! host, and `localStorage` only its own. Only digests are kept, in `sessions.json`, so a session
//! outlives a restart and the file opens nothing.
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::{
    Json,
    body::Bytes,
    extract::{State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode, header},
};
use platform::durable;
use protocol::{
    calcifer::{BrowserSession, RedeemCode, SessionCode},
    error::{Error, Kind},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tracing::warn;

use crate::{
    api::{self, Api, Refusal},
    store::now_ms,
};

const CODE_LIVES_MS: i64 = 60_000;
/// A session unused this long has ended.
const UNUSED_MS: i64 = 30 * 24 * 3600 * 1000;
/// How far behind the file's `used_at_ms` may fall: far less than [`UNUSED_MS`], and no write per call.
const NOTED_EVERY_MS: i64 = 3600 * 1000;

#[derive(Serialize, Deserialize)]
struct Session {
    token: String,
    media: String,
    used_at_ms: i64,
}

/// The sessions of `sessions.json`, and the codes not yet traded, which no restart needs to keep.
pub(crate) struct Sessions {
    path: PathBuf,
    live: Mutex<Vec<Session>>,
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
    /// The sessions kept at `path`. One it can't read costs only a new link.
    pub(crate) fn open(path: PathBuf) -> Self {
        let live: Vec<Session> = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                warn!("{}: {e}; browser sessions start over", path.display());
                vec![]
            }),
            Err(_) => vec![],
        };
        let now = now_ms();
        let live = live.into_iter().filter(|session| now - session.used_at_ms < UNUSED_MS).collect();
        Self { path, live: Mutex::new(live), codes: Mutex::default() }
    }

    /// A durable save outlasts a crash; the rest never waits on the disk's write-back.
    fn save(&self, live: &[Session], durable: bool) -> platform::Result<()> {
        let bytes = serde_json::to_vec(live).expect("sessions serialize");
        if durable { durable::replace(&self.path, &bytes, 0o600) } else { durable::swap(&self.path, &bytes, 0o600) }
    }

    fn mint_code(&self) -> (String, i64) {
        let (code, now) = (secret(), now_ms());
        let mut codes = self.codes.lock().expect("session codes");
        codes.retain(|_, expires_ms| now < *expires_ms);
        codes.insert(digest(&code), now + CODE_LIVES_MS);
        (code, now + CODE_LIVES_MS)
    }

    /// `code`, spent whether or not this goes on to succeed.
    fn start(&self, code: &str) -> Result<BrowserSession, Refusal> {
        let now = now_ms();
        let expires_ms = self.codes.lock().expect("session codes").remove(&digest(code));
        if !expires_ms.is_some_and(|expires_ms| now < expires_ms) {
            let said = "this sign-in code is unknown, used or expired";
            return Err(Refusal(Error::new(Kind::Validation, "session_code_invalid", said).next("cozy open")));
        }
        let (token, media) = (secret(), secret());
        let mut live = self.live.lock().expect("sessions");
        live.retain(|session| now - session.used_at_ms < UNUSED_MS);
        live.push(Session { token: digest(&token), media: digest(&media), used_at_ms: now });
        self.save(&live, true).map_err(|e| {
            live.pop();
            api::refuse(Kind::Internal, "session_unwritable", e.to_string())
        })?;
        Ok(BrowserSession { token, media })
    }

    /// Whether the session `of` picks is live, noting its use.
    fn used(&self, of: impl Fn(&Session) -> bool) -> bool {
        let now = now_ms();
        let mut live = self.live.lock().expect("sessions");
        let Some(at) = live.iter().position(of) else { return false };
        let since = now - live[at].used_at_ms;
        let ended = since >= UNUSED_MS;
        if ended {
            live.swap_remove(at);
        } else if since >= NOTED_EVERY_MS {
            live[at].used_at_ms = now;
        } else {
            return true;
        }
        if let Err(e) = self.save(&live, ended) {
            warn!("noting a browser session's use: {e}");
        }
        !ended
    }

    pub(crate) fn admits_token(&self, presented: &str) -> bool {
        let presented = digest(presented);
        self.used(|session| session.token == presented)
    }

    pub(crate) fn admits_media(&self, presented: &str) -> bool {
        let presented = digest(presented);
        self.used(|session| session.media == presented)
    }

    /// Ends the session `token` opens; any other token names none, and nothing happens.
    fn end(&self, token: &str) -> platform::Result<()> {
        let token = digest(token);
        let mut live = self.live.lock().expect("sessions");
        let before = live.len();
        live.retain(|session| session.token != token);
        if live.len() == before { Ok(()) } else { self.save(&live, true) }
    }
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
    Ok(Json(api.sessions.start(&code)?))
}

/// `DELETE /v1/session`: the session whose token this carries has ended.
pub(crate) async fn end(State(api): State<Arc<Api>>, headers: HeaderMap) -> Result<StatusCode, Refusal> {
    if let Some(token) = api::bearer(&headers) {
        api.sessions.end(token).map_err(|e| api::refuse(Kind::Internal, "session_unwritable", e.to_string()))?;
    }
    Ok(StatusCode::NO_CONTENT)
}
