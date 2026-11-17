//! A fetch grant (xs-210): one Hub call (`POST /v1/models/grants`) answers every object a commit's download may read,
//! each with a presigned storage URL good until the grant expires. An [`super::Endpoint`] bound to a commit asks it when
//! it first locates one of its objects, asks again once near the end or when storage refuses a URL, and locates what it
//! lists there, with no credential. A Hub without the route is asked per object (`GET /blobs/…`), as before.

use std::collections::HashMap;
use std::io::Read as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::err::{Code, Refusal, Result};
use crate::format::ObjectRef;

/// A grant is asked again when this little of its life is left, objects still to fetch (at most a quarter of it).
const RENEW: Duration = Duration::from_secs(10 * 60);
/// A URL storage refuses this soon after its grant was asked did not outlive it: the refusal stands.
const FRESH: Duration = Duration::from_secs(60);
/// A Hub that answered without the route is asked per object this long, then for a grant again (upgraded meanwhile).
const UNROUTED: Duration = Duration::from_secs(10 * 60);
/// The most bytes an answer may inflate to.
const ANSWER_BYTES: u64 = 64 << 20;

/// Hubs found without the route, and when.
static WITHOUT: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

pub(crate) fn unrouted(base: &str) -> bool {
    let mut without = WITHOUT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    without.retain(|(_, at)| at.elapsed() < UNROUTED);
    without.iter().any(|(hub, _)| hub == base)
}

pub(crate) fn without_route(base: &str) {
    tracing::info!(hub = base, "the Hub has no fetch grants: its objects are asked for one at a time");
    WITHOUT.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((base.to_owned(), Instant::now()));
}

/// The commit an endpoint's objects belong to, and its grant as it stands (one ask at a time; the rest wait for it).
pub(crate) struct Grant {
    pub(crate) tree: ObjectRef,
    components: Vec<String>,
    pub(crate) held: tokio::sync::Mutex<Option<Arc<Granted>>>,
}

impl Grant {
    /// `components` of commit `tree`; none: all of them.
    pub(crate) fn new(tree: &ObjectRef, components: &[String]) -> Grant {
        Grant { tree: tree.clone(), components: components.to_vec(), held: tokio::sync::Mutex::new(None) }
    }

    /// The ask: this commit wanted, nothing said held.
    pub(crate) fn body(&self) -> Value {
        let mut want = json!({"commit": self.tree.id()});
        if !self.components.is_empty() {
            want["components"] = json!(self.components);
        }
        json!({"want": [want]})
    }
}

/// What a grant lists, each object's URL by digest, and why the Hub refused any of the want.
pub(crate) struct Granted {
    urls: HashMap<String, String>,
    refused: Vec<String>,
    asked: Instant,
    until: Instant,
    /// The Hub doesn't know the commit (a tree this machine wrote for a header by hash): its objects are asked one at a time.
    pub(crate) unknown: bool,
}

impl Granted {
    pub(crate) fn url(&self, object: &ObjectRef) -> Option<&str> {
        self.urls.get(&object.sha256).map(String::as_str)
    }

    /// Near its end: asked again before more objects start.
    pub(crate) fn ending(&self) -> bool {
        let life = self.until.saturating_duration_since(self.asked);
        self.until.saturating_duration_since(Instant::now()) < RENEW.min(life / 4)
    }

    /// Old enough that storage refusing its URL may mean it ran out.
    pub(crate) fn renewable(&self) -> bool {
        self.asked.elapsed() >= FRESH
    }

    /// `object` is not among what it lists.
    pub(crate) fn absent(&self, object: &ObjectRef, tree: &ObjectRef) -> Refusal {
        let why = match self.refused.is_empty() {
            true => String::new(),
            false => format!(" (refused: {})", self.refused.join("; ")),
        };
        Refusal { code: Code::REF_NOT_FOUND, detail: format!("{} is not in the Hub's grant for commit {}{why}", object.id(), tree.id()) }
    }

    /// One grant of its answer's pages, each with the Hub's `Date`, read when it was `asked`.
    pub(crate) fn of(pages: Vec<(Answer, Option<String>)>, asked: Instant) -> Result<Granted> {
        let mut granted = Granted { urls: HashMap::new(), refused: vec![], asked, until: asked + Duration::from_secs(3 * 60 * 60), unknown: false };
        for (answer, date) in pages {
            let expires = parse(&answer.expires_at)?;
            // Its life on the Hub's clock, from its `Date`, not this machine's.
            let now = date.and_then(|date| jiff::fmt::rfc2822::DateTimeParser::new().parse_timestamp(&date).ok());
            let now = now.map_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64, |t| t.as_second());
            granted.until = granted.until.min(asked + Duration::from_secs(expires.saturating_sub(now).max(0) as u64));
            let (objects, refused) = (answer.objects.unwrap_or_default(), answer.refused.unwrap_or_default());
            granted.urls.extend(objects.into_iter().map(|o| (o.digest.trim_start_matches("sha256:").to_owned(), o.url)));
            granted.refused.extend(refused.iter().map(|r| format!("{}: {}", r.reason, r.detail)));
            granted.unknown |= refused.iter().any(|r| r.reason == "not_found");
        }
        granted.unknown &= granted.urls.is_empty();
        Ok(granted)
    }
}

fn parse(at: &str) -> Result<i64> {
    let at: jiff::Timestamp = at.parse().map_err(|e| Refusal { code: Code::UNKNOWN_FORMAT, detail: format!("a grant's expires_at {at:?}: {e}") })?;
    Ok(at.as_second())
}

/// One page of a grant (the fields read).
#[derive(Deserialize)]
pub(crate) struct Answer {
    expires_at: String,
    pub(crate) next: Option<String>,
    // A Go Hub writes an empty list as null.
    #[serde(default)]
    objects: Option<Vec<Listed>>,
    #[serde(default)]
    refused: Option<Vec<Refused>>,
}

#[derive(Deserialize)]
struct Listed {
    digest: String,
    url: String,
}

#[derive(Deserialize)]
struct Refused {
    reason: String,
    #[serde(default)]
    detail: String,
}

impl Answer {
    /// An answer's body, inflated when the Hub gzipped it.
    pub(crate) fn read(encoding: Option<&str>, body: &[u8]) -> std::result::Result<Answer, String> {
        let inflated;
        let body = match encoding {
            Some(encoding) if encoding.eq_ignore_ascii_case("gzip") => {
                let mut bytes = Vec::new();
                flate2::read::GzDecoder::new(body).take(ANSWER_BYTES).read_to_end(&mut bytes).map_err(|e| format!("gzip: {e}"))?;
                inflated = bytes;
                &inflated[..]
            }
            _ => body,
        };
        serde_json::from_slice(body).map_err(|e| e.to_string())
    }
}
