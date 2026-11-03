//! The newest release of each package this computer knows, as last read from its Hub with the
//! read's ETag (`latest.json` beside the package's kept cards). A run names it with no Hub read
//! while the read is recent, and reads it first when it is not. An install reads it again, and
//! Calcifer refreshes those run within the last hour in the background ([`refresh_all`]): a
//! computer at rest reads nothing, and a new release reaches runs one refresh late at most.
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use platform::durable;
use serde::{Deserialize, Serialize};

use crate::{Client, Error};

const FILE: &str = "latest.json";
/// A package run this recently is kept fresh in the background.
const ACTIVE: Duration = Duration::from_secs(3600);
/// A read older than this is read again before a run names it.
const RECENT: Duration = Duration::from_secs(600);

#[derive(Deserialize, Serialize)]
struct Latest {
    release: String,
    etag: String,
    /// When it was last read from the Hub (Unix ms).
    #[serde(default)]
    read_at_ms: u64,
    /// When a run or an install last named it (Unix ms).
    #[serde(default)]
    used_at_ms: u64,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn held(dir: &Path) -> Option<Latest> {
    serde_json::from_slice(&std::fs::read(dir.join(FILE)).ok()?).ok()
}

/// Keeps `latest`; one that cannot be kept is read again next time.
fn keep(dir: &Path, latest: &Latest) {
    let bytes = serde_json::to_vec(latest).expect("serializes");
    let _ = durable::private_dir(dir).and_then(|()| durable::replace(&dir.join(FILE), &bytes, 0o600));
}

/// Where `(hub, package)`'s cards and newest release live under `root`, the cozy home.
pub fn dir(root: &Path, hub: &str, package: &str) -> PathBuf {
    let host = hub.split_once("://").map_or(hub, |(_, rest)| rest);
    let host: String = host
        .trim_end_matches('/')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    let mut dir = root.join("packages").join(host);
    dir.extend(package.split('/'));
    dir
}

/// The kept newest release in `dir`, and whether its read is recent enough to name unread.
pub fn kept(dir: &Path) -> Option<(String, bool)> {
    let latest = held(dir)?;
    let recent = now_ms().saturating_sub(latest.read_at_ms) < RECENT.as_millis() as u64;
    Some((latest.release, recent))
}

/// A run or an install named the release kept in `dir`: it stays fresh for the next hour.
pub fn used(dir: &Path) {
    if let Some(mut latest) = held(dir) {
        // A burst of runs writes this once a minute at most.
        if now_ms().saturating_sub(latest.used_at_ms) >= 60_000 {
            latest.used_at_ms = now_ms();
            keep(dir, &latest);
        }
    }
}

/// The Hub's newest release of `org/name` now (None: it has none), read conditionally on the kept
/// ETag and kept again. A Hub that sends no ETag leaves nothing kept, nor does one that no longer
/// has the package.
pub async fn read(client: &Client, dir: &Path, org: &str, name: &str) -> Result<Option<String>, Error> {
    let path = dir.join(FILE);
    let was = held(dir);
    let answer = client.get_package_if_changed(org, name, was.as_ref().map(|held| held.etag.as_str())).await;
    if matches!(answer, Err(Error::Hub { status: 404, .. })) {
        let _ = std::fs::remove_file(&path);
    }
    let used_at_ms = was.as_ref().map_or_else(now_ms, |held| held.used_at_ms);
    let Some((card, etag)) = answer? else {
        let Some(was) = was else {
            return Err(Error::Answer("304 to a read that kept no ETag".into()));
        };
        let release = was.release.clone();
        keep(dir, &Latest { read_at_ms: now_ms(), ..was });
        return Ok(Some(release));
    };
    let release = card.package.latest_release;
    match (&release, etag) {
        (Some(release), Some(etag)) => keep(dir, &Latest { release: release.clone(), etag, read_at_ms: now_ms(), used_at_ms }),
        _ => drop(std::fs::remove_file(&path)),
    }
    Ok(release)
}

/// The packages `root` keeps a newest release of at `hub` that this computer ran or installed
/// within [`ACTIVE`], read again. Answers how many were read; a failed read keeps what was kept,
/// and a Hub that cannot answer now (no answer, a 429, a 5xx) is asked nothing more this pass.
pub async fn refresh_all(client: &Client, root: &Path, hub: &str) -> usize {
    let mut read_again = 0;
    let host = dir(root, hub, "");
    for org in std::fs::read_dir(&host).into_iter().flatten().flatten() {
        for package in std::fs::read_dir(org.path()).into_iter().flatten().flatten() {
            let (org, name) = (org.file_name(), package.file_name());
            let (Some(org), Some(name)) = (org.to_str(), name.to_str()) else { continue };
            let active = held(&package.path()).is_some_and(|latest| now_ms().saturating_sub(latest.used_at_ms) < ACTIVE.as_millis() as u64);
            if !active {
                continue;
            }
            match read(client, &package.path(), org, name).await {
                Ok(_) => read_again += 1,
                Err(error) if error.retry() => {
                    tracing::debug!("{org}/{name}'s newest release: {error}; asking nothing more this pass");
                    return read_again;
                }
                Err(error) => tracing::debug!("{org}/{name}'s newest release: {error}"),
            }
        }
    }
    read_again
}
