//! The newest release of each package this computer knows, as last read from its Hub with the
//! read's ETag (`latest.json` beside the package's kept cards). A run names it with no Hub read;
//! an install reads it again, and Calcifer refreshes every kept one in the background
//! ([`refresh_all`]), so a new release reaches runs one refresh late at most.
use std::path::{Path, PathBuf};

use platform::durable;
use serde::{Deserialize, Serialize};

use crate::{Client, Error};

const FILE: &str = "latest.json";

#[derive(Deserialize, Serialize)]
struct Latest {
    release: String,
    etag: String,
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

/// The kept newest release in `dir`.
pub fn kept(dir: &Path) -> Option<String> {
    let latest: Latest = serde_json::from_slice(&std::fs::read(dir.join(FILE)).ok()?).ok()?;
    Some(latest.release)
}

/// The Hub's newest release of `org/name` now (None: it has none), read conditionally on the kept
/// ETag and kept again. A Hub that sends no ETag leaves nothing kept.
pub async fn read(client: &Client, dir: &Path, org: &str, name: &str) -> Result<Option<String>, Error> {
    let path = dir.join(FILE);
    let held: Option<Latest> = std::fs::read(&path).ok().and_then(|raw| serde_json::from_slice(&raw).ok());
    let answer = client.get_package_if_changed(org, name, held.as_ref().map(|held| held.etag.as_str())).await?;
    let Some((card, etag)) = answer else {
        return held.map(|held| Some(held.release)).ok_or_else(|| Error::Answer("304 to a read that kept no ETag".into()));
    };
    let release = card.package.latest_release;
    match (&release, etag) {
        (Some(release), Some(etag)) => {
            let latest = serde_json::to_vec(&Latest { release: release.clone(), etag }).expect("serializes");
            // One that cannot be kept is read again next time.
            let _ = durable::private_dir(dir).and_then(|()| durable::replace(&path, &latest, 0o600));
        }
        _ => drop(std::fs::remove_file(&path)),
    }
    Ok(release)
}

/// Every package `root` keeps a newest release of at `hub`, read again: the packages this computer
/// installed or ran. Answers how many were read; a failed read keeps what was kept.
pub async fn refresh_all(client: &Client, root: &Path, hub: &str) -> usize {
    let mut read_again = 0;
    let host = dir(root, hub, "");
    for org in std::fs::read_dir(&host).into_iter().flatten().flatten() {
        for package in std::fs::read_dir(org.path()).into_iter().flatten().flatten() {
            let (org, name) = (org.file_name(), package.file_name());
            let (Some(org), Some(name)) = (org.to_str(), name.to_str()) else { continue };
            if !package.path().join(FILE).is_file() {
                continue;
            }
            match read(client, &package.path(), org, name).await {
                Ok(_) => read_again += 1,
                Err(error) => tracing::debug!("{org}/{name}'s newest release: {error}"),
            }
        }
    }
    read_again
}
