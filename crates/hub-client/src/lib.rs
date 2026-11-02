//! Tensorhub's client: the routes cozy calls, written against the Hub's `api/openapi.yaml` (tensorhub's
//! contract; `scripts/hub-routes.sh` holds every route here to it), with the credentials every cozy
//! process speaks as: a user's device-key login and Bearer, a pod's worker capability, a machine's
//! workload grant (`workload`). Bulk bytes
//! (presigned GET/PUT) are not here: TensorFS moves them.
#![forbid(unsafe_code)]
mod client;
mod device;
pub mod latest;
pub mod types;
pub mod workload;

use protocol::error::{Error as Refusal, Kind};
use serde_json::Value;

pub use client::{Client, Credential};
pub use device::{Bearer, DeviceKey, Enrollment, UserSession};

/// A refused or failed call. `Hub` carries the Hub's own typed refusal verbatim.
#[derive(Debug, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum Error {
    #[error("{code}: {message}")]
    Hub {
        status: u16,
        code: String,
        message: String,
        remedy: String,
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
    /// Whether asking again may be answered: no answer, a timeout (408), rate limit (429) or
    /// server failure. A refusal, of the credential too, is final.
    pub fn retry(&self) -> bool {
        match self {
            Self::Call(_) => true,
            Self::Hub { status, .. } => Kind::of_http(*status) == Kind::Unavailable,
            _ => false,
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
            } => (
                Refusal::new(Kind::of_http(status), code.clone(), message).cause(code),
                remedy,
            ),
            Self::Call(message) => (
                Refusal::unavailable("hub_unreachable", message),
                "check this computer's internet connection, then try again".into(),
            ),
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
    pub fn unanswered(error: &reqwest::Error) -> Self {
        let mut cause: &dyn std::error::Error = error;
        while let Some(next) = cause.source() {
            cause = next;
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
        }
    }
}
