//! The one typed refusal every cozy surface renders: Calcifer's HTTP refusals (`{"error": …}`),
//! the CLI's `--json` output and its exit code. A run that ends failed is an answer, not a
//! refusal. `kind` is the class a program branches on;
//! `code` is the stable specific name (`idempotency_conflict`, `machine_not_found`).
use serde::{Deserialize, Serialize};

/// A refusal's class: what a program branches on; it sets the HTTP status and exit code.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "ErrorKind"))]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A bad invocation: unknown flag, malformed `key=value`, malformed target.
    Usage,
    /// A typed payload, schema or bounds refusal.
    Validation,
    NotFound,
    /// A missing or refused credential; the remedy names it.
    Credential,
    /// The caller may not do this here (a foreign Host/Origin or a non-loopback peer).
    Forbidden,
    /// A concurrent writer, an existing target, an idempotency key reused for another ask.
    Conflict,
    /// Not enough memory, disk or GPUs anywhere the ask may go.
    Capacity,
    /// Calcifer, a machine or the Hub is unreachable.
    Unavailable,
    /// A bug, never a user condition.
    Internal,
}

/// A typed refusal: its class, stable code and message, with an optional remedy and next commands.
#[derive(
    Clone, Debug, Deserialize, Serialize, PartialEq, Eq, thiserror::Error,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "ErrorBody"))]
#[error("{code}: {message}")]
pub struct Error {
    pub kind: Kind,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remedy: Option<String>,
    /// Commands to try next; at most two.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    /// The originating component's own stable code (a machine's or the Hub's), when not ours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// How long the one refusing said to wait before asking again, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_s: Option<u64>,
    /// The originating component's own details of its refusal, verbatim (a Hub's `metadata`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// A peer's status where the kind's would misname it: its 429 (asked too soon) is `Unavailable` but answers 429.
    /// Never in the body.
    #[serde(skip)]
    pub status: Option<u16>,
}

/// Calcifer's refusal while it stops (a restart): the ask was not carried out, or goes on under its key, so it is sent
/// again to the next Calcifer.
pub const STOPPING: &str = "calcifer_stopping";

/// The HTTP body of a refusal.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Envelope {
    pub error: Error,
}

impl Error {
    /// A refusal with no remedy, next commands, cause, wait or metadata.
    pub fn new(kind: Kind, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind,
            code: code.into(),
            message: message.into(),
            remedy: None,
            next: vec![],
            cause: None,
            retry_after_s: None,
            metadata: None,
            status: None,
        }
    }

    /// A refusal of a typed payload, its schema or its bounds.
    pub fn validation(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Kind::Validation, code, message)
    }
    /// A refusal naming something that does not exist.
    pub fn not_found(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Kind::NotFound, code, message)
    }
    /// A refusal for a missing or refused credential.
    pub fn credential(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Kind::Credential, code, message)
    }
    /// A refusal for a caller that may not do this here.
    pub fn forbidden(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Kind::Forbidden, code, message)
    }
    /// A refusal for a concurrent writer, an existing target or a reused idempotency key.
    pub fn conflict(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Kind::Conflict, code, message)
    }
    /// A refusal for too little memory, disk or GPUs anywhere the ask may go.
    pub fn capacity(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Kind::Capacity, code, message)
    }
    /// A refusal because Calcifer, a machine or the Hub is unreachable.
    pub fn unavailable(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Kind::Unavailable, code, message)
    }
    /// A refusal from a bug, never a user condition.
    pub fn internal(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Kind::Internal, code, message)
    }

    /// The HTTP status it answers with: a peer's 429 as it was, else its kind's.
    pub fn http_status(&self) -> u16 {
        self.status.unwrap_or_else(|| self.kind.http_status())
    }

    /// The wait after this refusal: the stated one, else `otherwise`.
    pub fn wait_or(&self, otherwise: std::time::Duration) -> std::time::Duration {
        self.retry_after_s.map_or(otherwise, std::time::Duration::from_secs)
    }

    pub fn remedy(mut self, remedy: impl Into<String>) -> Self {
        self.remedy = Some(remedy.into());
        self
    }

    pub fn next(mut self, command: impl Into<String>) -> Self {
        if self.next.len() < 2 {
            self.next.push(command.into());
        }
        self
    }

    pub fn cause(mut self, code: impl Into<String>) -> Self {
        self.cause = Some(code.into());
        self
    }

    /// Whether asking again may be answered: a machine or the Hub gave no answer, a slow one, or failed on its side, or another
    /// program holds a project's file (`file_held`, which the page waits out too). Any other refusal is final.
    pub fn transient(&self) -> bool {
        matches!(self.code.as_str(), "machine_unreachable" | "hub_unreachable" | "hub_slow" | "file_held")
            || (self.kind == Kind::Unavailable && self.cause.as_ref() == Some(&self.code))
    }

    /// The process exit code: 2 for usage, 1 for everything else.
    pub fn exit_code(&self) -> i32 {
        if self.kind == Kind::Usage { 2 } else { 1 }
    }
}

impl Kind {
    pub fn http_status(self) -> u16 {
        match self {
            Kind::Usage | Kind::Validation => 400,
            Kind::Credential => 401,
            Kind::Forbidden => 403,
            Kind::NotFound => 404,
            Kind::Conflict => 409,
            Kind::Capacity => 507,
            Kind::Unavailable => 503,
            Kind::Internal => 500,
        }
    }

    /// What a peer's (the Hub's) HTTP refusal names. A timeout, rate limit or server failure is
    /// `Unavailable`: asking again may be answered.
    pub fn of_http(status: u16) -> Kind {
        match status {
            401 => Kind::Credential,
            403 => Kind::Forbidden,
            404 => Kind::NotFound,
            402 | 409 | 410 => Kind::Conflict,
            408 | 429 => Kind::Unavailable,
            // Never answered, however often it's asked: not implemented, an HTTP version it doesn't speak.
            400..=499 | 501 | 505 => Kind::Validation,
            507 => Kind::Capacity,
            _ => Kind::Unavailable,
        }
    }
}
