//! Tensorhub's client: the routes cozy calls, written against the Hub's `api/openapi.yaml` (tensorhub's
//! contract; `scripts/hub-routes.sh` holds every route here to it), with the credentials every cozy
//! process speaks as: a user's device-key login and Bearer, a pod's worker capability, a machine's
//! workload grant (`workload`). Bulk bytes
//! (presigned GET/PUT) are not here: TensorFS moves them.
pub mod cards;
mod client;
mod device;
pub mod types;
pub mod workload;

use crate::error::{Error as Refusal, Kind};
use serde_json::Value;

pub use client::{Client, Credential};
pub use device::{Bearer, DeviceKey, Enrollment, UserSession};

/// What every Hub call goes over: cozy's one outbound client (`tensorfs::fetch::http`: `cozy/<version>` as the
/// User-Agent, by which a Hub can refuse a cozy too old for it; 5 s to connect), no redirects, and a call that
/// receives nothing for 30 s fails. `ca_der` is a private Hub's CA. Clones share one pool.
pub fn http(ca_der: Option<&[u8]>) -> Result<reqwest::Client, String> {
    let extra: Vec<&[u8]> = ca_der.into_iter().collect();
    let builder = tensorfs::fetch::http(&extra).map_err(|e| format!("the Hub's TLS: {}", e.detail))?;
    builder
        .redirect(reqwest::redirect::Policy::none())
        .read_timeout(tensorfs::fetch::STALL)
        .build()
        .map_err(|e| format!("the Hub's TLS: {e}"))
}

/// A refused or failed call. `Hub` carries the Hub's own typed refusal verbatim.
#[derive(Debug, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum Error {
    #[error("{code}: {message}")]
    Hub {
        status: u16,
        code: String,
        message: String,
        remedy: String,
        /// The answer's `Retry-After`, in seconds.
        #[serde(default)]
        retry_after_s: Option<u64>,
        /// The envelope's `metadata`: the refusal's details, verbatim.
        #[serde(default)]
        metadata: Option<Value>,
    },
    #[error("Tensorhub: {0}")]
    Call(String),
    /// The call's credential could not be had (a login refused, a key missing or unusable);
    /// nothing was sent.
    #[error("{0}")]
    Credential(String),
    #[error("an unusable answer: {0}")]
    Answer(String),
}

impl Error {
    /// Whether asking again may be answered: no answer (no connection, a name that doesn't resolve, 30 s of silence), a
    /// timeout (408), rate limit (429) or server failure (5xx but 501 and 505). A refusal, of the credential too, a
    /// certificate that isn't trusted and an answer that can't be read are final. Every call here is safe to ask again:
    /// a read, a write that sets what it names (an edit, a yank, a publish of one release, a worker's report), or
    /// `POST /v1/rentals`, which goes under an idempotency key.
    pub fn retry(&self) -> bool {
        match self {
            Self::Call(_) => true,
            Self::Hub { status, .. } => Kind::of_http(*status) == Kind::Unavailable,
            _ => false,
        }
    }

    /// How long the answer said to wait before asking again.
    pub fn stated_wait(&self) -> Option<std::time::Duration> {
        match self {
            Self::Hub { retry_after_s, .. } => retry_after_s.map(std::time::Duration::from_secs),
            _ => None,
        }
    }

    /// The one reading of a failed call as cozy's refusal: the Hub's code (also the cause),
    /// message and remedy, classed by status, `Unavailable` exactly when [`Error::retry`]. A
    /// refused credential (a login refused, a key missing, unusable or revoked, a 401) is
    /// `hub_credential`, with `login` next.
    pub fn refusal(self, login: impl FnOnce() -> String) -> Refusal {
        let (error, remedy) = match self {
            Self::Hub {
                status: 401,
                code,
                message,
                remedy,
                ..
            } => (
                Refusal::credential("hub_credential", message)
                    .cause(code)
                    .next(login()),
                remedy,
            ),
            Self::Hub {
                status,
                code,
                message,
                remedy,
                retry_after_s,
                metadata,
            } => (
                Refusal { retry_after_s, metadata, ..Refusal::new(Kind::of_http(status), code.clone(), message).cause(code) },
                remedy,
            ),
            Self::Call(message) => {
                // A timeout is the Hub keeping quiet; only what never connected says anything of this computer.
                let (code, remedy) = match message.contains("timed out") {
                    true => ("hub_slow", "the Hub took too long to answer: it is busy or down, and asking again is safe"),
                    false => ("hub_unreachable", "this computer couldn't reach the Hub: its connection, or the Hub being down. Asking again is safe"),
                };
                (Refusal::unavailable(code, message), remedy.into())
            }
            Self::Credential(message) => (
                Refusal::credential("hub_credential", message).next(login()),
                String::new(),
            ),
            Self::Answer(message) => (Refusal::internal("hub_answer", message), String::new()),
        };
        match remedy.is_empty() {
            true => error,
            false => error.remedy(remedy),
        }
    }

    /// A call that got no answer, in a person's words: where it went and the innermost cause
    /// (`dns error: …`, `Connection refused`), not reqwest's "error sending request for url (…)".
    pub(crate) fn unanswered(error: &reqwest::Error) -> Self {
        let mut cause: &dyn std::error::Error = error;
        while let Some(next) = cause.source() {
            cause = next;
        }
        // Asking again meets the same certificate.
        if cause.to_string().contains("certificate") {
            let origin = error.url().map_or_else(|| "the Hub".to_owned(), |url| url.origin().ascii_serialization());
            return Self::Answer(format!("{origin}'s certificate isn't trusted: {cause}"));
        }
        Self::Call(match error.url() {
            Some(url) => format!(
                "no answer from {}: {cause}",
                url.origin().ascii_serialization()
            ),
            None => format!("no answer: {cause}"),
        })
    }

    /// An error answer: the Hub's envelope (`{"error": {code, message, remedy}}`) or OAuth's
    /// (`{"error": code, "error_description": …}`), its missing fields made whole; anything else
    /// (a proxy's page, an empty body) is `http_<status>` with the start of what came back.
    async fn refused(response: reqwest::Response) -> Self {
        let status = response.status();
        let retry_after_s = (response.headers().get(reqwest::header::RETRY_AFTER))
            .and_then(|value| value.to_str().ok()?.trim().parse().ok());
        let body = match response.bytes().await {
            Ok(body) => body,
            Err(e) => return Self::unanswered(&e),
        };
        let said: Value = serde_json::from_slice(&body).unwrap_or_default();
        let text = |value: &Value| value.as_str().map(str::to_owned);
        let (code, message, remedy) = match &said["error"] {
            Value::String(code) => (
                Some(code.clone()),
                Some(text(&said["error_description"]).unwrap_or_default()),
                None,
            ),
            error @ Value::Object(_) => (
                text(&error["code"]),
                text(&error["message"]),
                text(&error["remedy"]),
            ),
            _ => {
                let start: String = String::from_utf8_lossy(&body)
                    .trim()
                    .chars()
                    .take(256)
                    .collect();
                (None, Some(start).filter(|s| !s.is_empty()), None)
            }
        };
        Self::Hub {
            status: status.as_u16(),
            code: code.unwrap_or_else(|| format!("http_{}", status.as_u16())),
            message: message.unwrap_or_else(|| status.to_string()),
            remedy: remedy.unwrap_or_default(),
            retry_after_s,
            metadata: Some(said["error"]["metadata"].clone()).filter(|m| m.is_object()),
        }
    }
}
