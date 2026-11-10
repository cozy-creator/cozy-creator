//! The one retry rule, for every request (object_store's and the AWS SDKs'). Asked again: what may answer next time, a
//! transport failure (no connection, a name that doesn't resolve, 30 s with no bytes, a body cut off) or a "not now"
//! answer (408, 425, 429, 499, 5xx but 501 and 505), after an exponential backoff with jitter or the wait the answer
//! states, capped ([`Backoff`]). Failed at once: every other answer (a 4xx is the server's refusal, in its own words), a
//! certificate that isn't trusted, an answer that can't be read. Only what is safe to repeat is asked again: a GET, HEAD,
//! PUT or DELETE, or a POST under an idempotency key or that sets what it names.
//! An attempt that moved bytes starts the count over; [`ATTEMPTS`] in a row that moved nothing end it, and a call a
//! person waits on ends within a minute whatever its attempts took.

use std::hash::{BuildHasher as _, RandomState};
use std::time::{Duration, Instant};

use super::net::{shown, Client, GetError, HeaderMap};
use super::token::TokenProvider;
use crate::err::{Code, Refusal};

/// Attempts in a row that moved nothing before a request fails (object_store's `max_retries`).
pub const ATTEMPTS: usize = 10;

/// An answer that means "not now": 499 is a proxy's own dropped exchange (the Hub's blobs, run
/// 15391). 501 and 505 say the ask can never be answered.
pub(crate) fn weather(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 499 | 500..=599) && !matches!(status, 501 | 505)
}

/// A failure the next attempt may not have.
pub(crate) fn transient(refusal: &Refusal) -> bool {
    matches!(refusal.code, Code::TRANSFER_FAILED | Code::HUB_UNREACHABLE | Code::FD_HEADROOM)
}

/// What a call a person waits on may take in all, attempts and pauses: past it the call fails with its last reason.
const CALL: Duration = Duration::from_secs(60);
/// The longest wait an answer's Retry-After is given.
const STATED: Duration = Duration::from_secs(60);

/// The pauses between one ask's attempts, the same rule wherever cozy asks again: a pause that doubles to a
/// ceiling, lengthened by up to itself again at random, or the wait the answer states (at most [`STATED`]); an attempt
/// that moved bytes starts the count over, and a fixed number in a row that moved nothing end it, or the time it was
/// given in all. Each pause is said.
pub struct Backoff {
    first: Duration,
    most: Duration,
    attempts: usize,
    next: Duration,
    made: usize,
    began: Instant,
    within: Option<Duration>,
}

impl Backoff {
    /// A transfer's request: [`ATTEMPTS`], 100 ms doubling to 15 s.
    pub fn transfer() -> Backoff {
        Backoff::of(ATTEMPTS, Duration::from_millis(100), Duration::from_secs(15), None)
    }

    /// A call a person waits on: 5 attempts, about 1.5, 3, 6 and 12 s apart, and [`CALL`] in all.
    pub fn call() -> Backoff {
        Backoff::of(5, Duration::from_secs(1), Duration::from_secs(16), Some(CALL))
    }

    /// Work no one waits on (an output brought here, a report): 5 attempts, about 3, 6, 12 and 24 s apart.
    pub fn background() -> Backoff {
        Backoff::of(5, Duration::from_secs(2), Duration::from_secs(16), None)
    }

    fn of(attempts: usize, first: Duration, most: Duration, within: Option<Duration>) -> Backoff {
        Backoff { first, most, attempts, next: first, made: 1, began: Instant::now(), within }
    }

    /// How many attempts in a row may move nothing.
    pub fn attempts(&self) -> usize {
        self.attempts
    }

    /// The pause after a failed attempt (`moved`: it gained bytes; `stated`: the answer's
    /// Retry-After), or None once every attempt in a row moved nothing.
    pub fn after(&mut self, moved: bool, stated: Option<Duration>) -> Option<Duration> {
        if moved {
            (self.next, self.made) = (self.first, 1);
        }
        if self.made >= self.attempts {
            return None;
        }
        let pause = self.next;
        // A uniform fraction from the 53 high bits of a fresh random hash.
        let jitter = (RandomState::new().hash_one(()) >> 11) as f64 / (1u64 << 53) as f64;
        let pause = stated.map_or(pause + pause.mul_f64(jitter), |stated| stated.min(STATED));
        // Another attempt would start past the time given in all.
        if self.within.is_some_and(|within| self.began.elapsed() + pause >= within) {
            return None;
        }
        self.made += 1;
        self.next = (self.next * 2).min(self.most);
        Some(pause)
    }

    /// [`Backoff::after`] with the line that says it: `<why>; attempt 2/5 in 3 s`.
    pub fn again(&mut self, why: &str, moved: bool, stated: Option<Duration>) -> Option<(Duration, String)> {
        let pause = self.after(moved, stated)?;
        let wait = match pause.as_secs_f64() {
            s if s < 1.0 => format!("{s:.1}"),
            s => format!("{}", s.round() as u64),
        };
        Some((pause, format!("{why}; attempt {}/{} in {wait} s", self.made, self.attempts)))
    }

    /// The last failure's line once it's given up: `<why>, after 2 attempts in 62 s`.
    pub fn spent(&self, why: &str) -> String {
        format!("{why}, after {} attempts in {} s", self.made, self.began.elapsed().as_secs())
    }
}

/// Where [`get`] puts a body: each frame with the byte it starts at.
pub(crate) type Sink<'a> = dyn FnMut(u64, &[u8]) -> Result<(), Refusal> + Send + 'a;

/// GET `url` (`range`: from, and through) into `sink`, asked again by the rule from the byte reached;
/// a 200 to an open-ended ask starts the body over. The answer's headers, or the status of an answer
/// neither data nor weather.
pub(crate) async fn get(
    client: &Client,
    url: &str,
    headers: &[(String, String)],
    provider: Option<&dyn TokenProvider>,
    range: Option<(u64, Option<u64>)>,
    sink: &mut Sink<'_>,
) -> Result<Result<HeaderMap, u16>, GetError> {
    let (mut at, through) = range.unwrap_or((0, None));
    let mut backoff = Backoff::transfer();
    loop {
        let from = at;
        let (why, wait) = match attempt(client, url, headers, provider, &mut at, through, sink).await {
            Ok(Ok(done)) => return Ok(done),
            Ok(Err((why, wait))) => (why, wait),
            Err(GetError::Exchange(refusal)) if transient(&refusal) => (refusal.detail, None),
            Err(error) => return Err(error),
        };
        tracing::info!(url = %shown(url), at, %why, "asked again");
        match backoff.after(at > from, wait) {
            Some(pause) => tokio::time::sleep(pause).await,
            None => {
                let detail = format!("{why}; {ATTEMPTS} attempts in a row moved nothing");
                return Err(GetError::Exchange(Refusal { code: Code::HUB_UNREACHABLE, detail }));
            }
        }
    }
}

type Tried = Result<Result<HeaderMap, u16>, (String, Option<Duration>)>;

async fn attempt(
    client: &Client,
    url: &str,
    headers: &[(String, String)],
    provider: Option<&dyn TokenProvider>,
    at: &mut u64,
    through: Option<u64>,
    sink: &mut Sink<'_>,
) -> Result<Tried, GetError> {
    let mut headers = headers.to_vec();
    if *at > 0 || through.is_some() {
        let through = through.map_or(String::new(), |t| t.to_string());
        headers.push(("range".into(), format!("bytes={at}-{through}")));
    }
    let mut answer = client.get_authorized(url, &headers, provider).await?;
    let status = answer.status();
    match status {
        _ if weather(status) => {
            let wait = answer.stated_wait();
            return Ok(Err((format!("GET {}: {status} {}", shown(url), answer.said().await), wait)));
        }
        200 if through.is_none() => *at = 0,
        206 if answer.content_range().is_some_and(|(first, _)| first == *at) => {}
        _ => return Ok(Ok(Err(status))),
    }
    loop {
        match answer.frame().await {
            Ok(Some(frame)) => {
                sink(*at, &frame).map_err(GetError::Caller)?;
                *at += frame.len() as u64;
            }
            Ok(None) => return Ok(Ok(Ok(answer.response.headers().clone()))),
            Err(refusal) => return Ok(Err((format!("GET {}: {}", shown(url), refusal.detail), None))),
        }
    }
}
