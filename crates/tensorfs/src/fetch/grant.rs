//! A fetch grant (xs-210): one Hub call (`POST /v1/models/grants`) says "I need this": a commit, or a model the Hub
//! resolves first as `resolve` would. Its answer is the commit it resolved and every object the download may read, in
//! the order to fetch them, each with a presigned storage URL good until the grant expires. An [`super::Endpoint`] bound
//! to a want asks it once, asks again (for the commit it resolved) near the end or when storage refuses a URL, and
//! locates what it lists there, with no credential. A Hub without the route is asked per object (`GET /blobs/…`).

use std::collections::HashMap;
use std::io::Read as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};

use super::endpoint::{Hardware, Resolved};
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

/// What a download asks one grant for: `components` (none: all) of a commit, or of the checkpoint the Hub resolves model
/// `name` to at `release` and `lane` (none: the one `hardware` runs best, its components the ones read), as `resolve`
/// would; with `structure`, only its tree, repo files and header.
#[derive(Debug, Clone)]
pub enum Wanted {
    Commit { tree: ObjectRef, components: Vec<String>, structure: bool },
    Model { name: String, release: Option<String>, lane: Option<String>, hardware: Hardware, structure: bool },
}

impl Wanted {
    fn wire(&self) -> Value {
        let (mut want, components, structure) = match self {
            Wanted::Commit { tree, components, structure } => (json!({"commit": tree.id()}), components, *structure),
            Wanted::Model { name, release, lane, hardware, structure } => {
                let mut want = json!({"model": name.to_ascii_lowercase()});
                release.iter().for_each(|release| want["release"] = json!(release));
                lane.iter().for_each(|lane| want["lane"] = json!(lane));
                if !hardware.gpus.is_empty() {
                    want["gpus"] = json!(hardware.gpus);
                    hardware.free_vram.iter().for_each(|free| want["free_vram"] = json!(free));
                }
                if hardware.plain {
                    want["plain"] = json!(true);
                }
                (want, &hardware.components, *structure)
            }
        };
        if !components.is_empty() {
            want["components"] = json!(components);
        }
        if structure {
            want["structure"] = json!(true);
        }
        want
    }
}

/// What one grant answers, as a download reads it: the commit a model want resolved to, and every object it may read, in
/// the Hub's order.
#[derive(Debug)]
pub struct Listing {
    pub resolved: Option<Resolved>,
    pub objects: Vec<Listed>,
}

/// One object a grant lists, and its kind (`tree`, `header`, `model_asset` or `part`).
#[derive(Debug, Clone)]
pub struct Listed {
    pub object: ObjectRef,
    pub kind: String,
}

/// An endpoint's want and its grant as it stands (one ask at a time; the rest wait for it). Once a model want resolves,
/// it is asked again as the commit it resolved to: a renewal never moves to a newer release.
pub(crate) struct Grant {
    pub(crate) held: tokio::sync::Mutex<(Wanted, Option<Arc<Granted>>)>,
}

impl Grant {
    pub(crate) fn new(want: Wanted) -> Grant {
        Grant { held: tokio::sync::Mutex::new((want, None)) }
    }
}

/// What a grant lists, each object's URL by digest, and why the Hub refused any of the want.
pub(crate) struct Granted {
    urls: HashMap<String, String>,
    pub(crate) listing: Arc<Listing>,
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
    pub(crate) fn absent(&self, object: &ObjectRef) -> Refusal {
        let why = match self.refused.is_empty() {
            true => String::new(),
            false => format!(" (refused: {})", self.refused.join("; ")),
        };
        Refusal { code: Code::REF_NOT_FOUND, detail: format!("{} is not in the Hub's grant{why}", object.id()) }
    }

    /// One grant of `want` from its answer's pages, each with the Hub's `Date`, read when it was `asked`. A model want the
    /// Hub resolved nothing for is refused with its reason.
    pub(crate) fn of(want: &Wanted, pages: Vec<(Answer, Option<String>)>, asked: Instant) -> Result<Granted> {
        let mut granted = Granted {
            urls: HashMap::new(),
            listing: Arc::new(Listing { resolved: None, objects: vec![] }),
            refused: vec![],
            asked,
            until: asked + Duration::from_secs(3 * 60 * 60),
            unknown: false,
        };
        let (mut resolved, mut objects) = (None, vec![]);
        for (answer, date) in pages {
            let expires = parse(&answer.expires_at)?;
            // Its life on the Hub's clock, from its `Date`, not this machine's.
            let now = date.and_then(|date| jiff::fmt::rfc2822::DateTimeParser::new().parse_timestamp(&date).ok());
            let now = now.map_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64, |t| t.as_second());
            granted.until = granted.until.min(asked + Duration::from_secs(expires.saturating_sub(now).max(0) as u64));
            for listed in answer.objects.unwrap_or_default() {
                let sha256 = listed.digest.trim_start_matches("sha256:").to_owned();
                granted.urls.insert(sha256.clone(), listed.url);
                objects.push(Listed { object: ObjectRef { sha256, length: listed.size }, kind: listed.kind });
            }
            let refused = answer.refused.unwrap_or_default();
            granted.refused.extend(refused.iter().map(|r| format!("{}: {}", r.reason, r.detail)));
            granted.unknown |= refused.iter().any(|r| r.reason == "not_found");
            resolved = resolved.or(answer.resolved.unwrap_or_default().into_iter().next());
        }
        if matches!(want, Wanted::Model { .. }) && resolved.is_none() {
            let why = match granted.refused.is_empty() {
                true => "the Hub resolved nothing for it".to_owned(),
                false => granted.refused.join("; "),
            };
            return Err(Refusal { code: Code::REF_NOT_FOUND, detail: why });
        }
        granted.unknown &= granted.urls.is_empty();
        granted.listing = Arc::new(Listing { resolved, objects });
        Ok(granted)
    }

    /// The want to renew it with: the commit a model want resolved to.
    pub(crate) fn renewal(&self, want: &Wanted) -> Option<Wanted> {
        let Wanted::Model { hardware, structure, .. } = want else { return None };
        let resolved = self.listing.resolved.as_ref()?;
        Some(Wanted::Commit { tree: resolved.checkpoint.clone(), components: hardware.components.clone(), structure: *structure })
    }
}

/// The body of one ask of `want`: none said held; `cursor` for a page after the first.
pub(crate) fn body(want: &Wanted, cursor: Option<&str>) -> Value {
    let mut body = json!({"want": [want.wire()]});
    cursor.iter().for_each(|cursor| body["cursor"] = json!(cursor));
    body
}

fn parse(at: &str) -> Result<i64> {
    let at: jiff::Timestamp = at.parse().map_err(|e| Refusal { code: Code::UNKNOWN_FORMAT, detail: format!("a grant's expires_at {at:?}: {e}") })?;
    Ok(at.as_second())
}

/// One page of a grant (the fields read). A Go Hub writes an empty list as null.
#[derive(Deserialize)]
pub(crate) struct Answer {
    expires_at: String,
    pub(crate) next: Option<String>,
    #[serde(default)]
    objects: Option<Vec<WireListed>>,
    #[serde(default)]
    refused: Option<Vec<Refused>>,
    #[serde(default)]
    resolved: Option<Vec<Resolved>>,
}

#[derive(Deserialize)]
struct WireListed {
    digest: String,
    size: u64,
    kind: String,
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
