//! Release cards this computer keeps under its cozy home (`root`). A release never changes, so its
//! card is read from the Hub once and kept forever, by release. Calcifer binds runs with these, and
//! so do the verbs that name a release.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use platform::durable;
use crate::error::{Error, Kind};
use crate::interface::Interface;
use serde_json::Value;

use crate::hub::Client;

/// A release's card: its release and package interface, read from the Hub's answer.
#[derive(Debug)]
pub struct Card {
    pub release: String,
    pub interface: Interface,
    /// The card as the Hub answered it.
    document: Value,
}

impl Card {
    /// What the package does in a line: its pyproject description, as the Hub read it from the
    /// release's wheel ("" for a release committed before the Hub kept it).
    pub fn summary(&self) -> &str {
        self.document["summary"].as_str().unwrap_or_default()
    }

    /// The release's digest (`types::release_digest`), as its Hub served it: none from a Hub before it, or a card kept before.
    pub fn digest(&self) -> Option<&str> {
        self.document["release"]["digest"].as_str().filter(|digest| !digest.is_empty())
    }

    /// The package interface document, as the Hub answered it.
    pub fn interface_document(&self) -> &Value {
        &self.document["package_interface"]
    }

    /// What the release's wheel requires (`Requires-Dist`). None: a card kept before Calcifer kept them.
    pub fn requirements(&self) -> Option<Vec<&str>> {
        Some(self.document.get("requirements")?.as_array()?.iter().filter_map(Value::as_str).collect())
    }

    /// Its publisher's benchmarks (cl-1024). None for a release published before the Hub kept them: a card read from a Hub without
    /// them is of such a release, so it is never read again for them.
    pub fn benchmarks(&self) -> Vec<crate::estimate::Benchmark> {
        serde_json::from_value(self.document["benchmarks"].clone()).unwrap_or_default()
    }

}

/// Where `hub`'s cards live under `root`, the cozy home.
fn hub_dir(root: &Path, hub: &str) -> PathBuf {
    let host = hub.split_once("://").map_or(hub, |(_, rest)| rest);
    let host: String = host
        .trim_end_matches('/')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    root.join("packages").join(host)
}

/// Where `(hub, package)`'s cards live under `root`.
fn dir(root: &Path, hub: &str, package: &str) -> PathBuf {
    let mut dir = hub_dir(root, hub);
    dir.extend(package.split('/'));
    dir
}

/// One page of a Hub's catalog as it answered, by its ETag (cl-1174): what it listed, each package with its newest release if it has
/// one, and the catalog's size. A page the Hub says is unchanged (304) is read from here.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Page {
    pub offset: u32,
    pub etag: String,
    pub rows: Vec<(String, Option<String>)>,
    pub total: i64,
}

/// Keeps what `hub` listed at its last whole read, each package with its newest release, beside their cards: a list asked before
/// the Hub answers again, after a restart too, stands on it (cl-975). With the pages it was read from that carried an ETag.
pub fn keep_listed(root: &Path, hub: &str, listed: &[(String, String)], pages: &[Page]) {
    let dir = hub_dir(root, hub);
    let read_at_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    let document = serde_json::json!({ "read_at_ms": read_at_ms, "packages": listed, "pages": pages });
    // One that cannot be kept is the next restart's to read again.
    drop(durable::private_dir(&dir).and_then(|()| durable::replace(&dir.join(LISTED), document.to_string().as_bytes(), 0o600)));
}

/// What `hub` listed at its last whole read (`keep_listed`), and when.
pub fn kept_listed(root: &Path, hub: &str) -> Option<(SystemTime, Vec<(String, String)>)> {
    let document: Value = serde_json::from_slice(&std::fs::read(hub_dir(root, hub).join(LISTED)).ok()?).ok()?;
    let at = UNIX_EPOCH + Duration::from_millis(document["read_at_ms"].as_u64()?);
    Some((at, serde_json::from_value(document["packages"].clone()).ok()?))
}

/// The pages of `hub`'s catalog kept with its last whole read (`keep_listed`): none from a Hub that sent no ETag, or a Calcifer before.
pub fn kept_pages(root: &Path, hub: &str) -> Vec<Page> {
    let read = || -> Option<Vec<Page>> {
        let document: Value = serde_json::from_slice(&std::fs::read(hub_dir(root, hub).join(LISTED)).ok()?).ok()?;
        serde_json::from_value(document.get("pages")?.clone()).ok()
    };
    read().unwrap_or_default()
}

/// The file `keep_listed` writes: not a card, nor an org's folder.
const LISTED: &str = "listed.json";

/// A failed Hub call as a refusal.
pub fn refusal(error: crate::hub::Error) -> Error {
    error.refusal(|| "cozy login <email>".into())
}

/// The card of `release`: the kept one, else the Hub's, which is then kept.
/// `client` is made only when the Hub is read.
pub async fn card(
    client: impl FnOnce() -> Result<Client, Error>,
    root: &Path,
    hub: &str,
    package: &str,
    release: &str,
) -> Result<Card, Error> {
    match kept(root, hub, package, release) {
        Some(card) if card.digest().is_some() => Ok(card),
        // Kept before its Hub served digests (xs-196): read once more for it, else as it stands.
        Some(card) => Ok(from_hub(client, root, hub, package, release).await.ok().filter(|read| read.digest().is_some()).unwrap_or(card)),
        None => from_hub(client, root, hub, package, release).await,
    }
}

/// The card of `release` with its requirements: one kept without them is read again and kept, or, with no Hub, stands as it is.
pub async fn card_requiring(
    client: impl FnOnce() -> Result<Client, Error>,
    root: &Path,
    hub: &str,
    package: &str,
    release: &str,
) -> Result<Card, Error> {
    match kept(root, hub, package, release) {
        Some(card) if card.requirements().is_some() && card.digest().is_some() => Ok(card),
        Some(card) => Ok(from_hub(client, root, hub, package, release).await.unwrap_or(card)),
        None => from_hub(client, root, hub, package, release).await,
    }
}

async fn from_hub(client: impl FnOnce() -> Result<Client, Error>, root: &Path, hub: &str, package: &str, release: &str) -> Result<Card, Error> {
    let (org, name) = package.split_once('/').expect("a package is org/name");
    let answer = client()?.get_package_release(org, name, release).await;
    let document = answer.map_err(|e| absent(refusal(e), &format!("{package}@{release}")))?;
    let document = serde_json::to_value(&document).expect("a card serializes");
    keep(root, hub, package, &document)
}

/// A package's releases as its Hub answered.
#[derive(Debug, Default)]
pub struct Releases {
    /// The release a run naming none takes (`release::newest` of those not yanked).
    pub newest: Option<String>,
    /// Every release it has, yanked or not.
    pub released: HashSet<String>,
    pub yanked: HashSet<String>,
}

impl Releases {
    /// Every release that isn't yanked.
    pub fn open(&self) -> impl Iterator<Item = &str> {
        self.released.iter().map(String::as_str).filter(|release| !self.yanked.contains(*release))
    }

    /// The newest release, or why `package` has none to run.
    pub fn newest(&self, package: &str) -> Result<String, Error> {
        self.newest.clone().ok_or_else(|| {
            Error::new(Kind::NotFound, "package_unreleased", format!("{package} has no release you can run yet."))
                .remedy("Its owner needs to publish one first.")
        })
    }
}

/// `package`'s releases at its Hub now.
pub async fn releases(client: &Client, package: &str) -> Result<Releases, Error> {
    let (org, name) = package.split_once('/').expect("a package is org/name");
    let card = client.get_package(org, name).await.map_err(|e| absent(refusal(e), package))?;
    let yanked = |row: &&crate::hub::types::CatalogRelease| row.yanked == Some(true) || row.yanked_at.as_deref().is_some_and(|at| !at.is_empty());
    let mut read = Releases {
        newest: None,
        released: card.releases.iter().map(|row| row.release.clone()).collect(),
        yanked: card.releases.iter().filter(yanked).map(|row| row.release.clone()).collect(),
    };
    read.newest = crate::release::newest(read.open()).map(str::to_owned);
    Ok(read)
}

/// The Hub's 404 names what it lacks, and where to look.
fn absent(error: Error, what: &str) -> Error {
    if error.kind != Kind::NotFound {
        return error;
    }
    let name = what.split('@').next().unwrap_or(what);
    Error::new(
        Kind::NotFound,
        "package_not_found",
        format!("The Hub has no package {what}."),
    )
    .cause(error.code)
    .next(format!(
        "cozy package ls {}",
        name.rsplit('/').next().unwrap_or(name)
    ))
}

/// The kept card of `release`.
pub fn kept(root: &Path, hub: &str, package: &str, release: &str) -> Option<Card> {
    let path = file(&dir(root, hub, package), release);
    let document: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    read(&document).ok()
}

/// The packages (`org/name`) with a card kept from `hub`.
pub fn kept_packages(root: &Path, hub: &str) -> Vec<String> {
    let entries = |dir: &Path| std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| e.path().is_dir()).collect::<Vec<_>>();
    (entries(&hub_dir(root, hub)).into_iter())
        .flat_map(|org| entries(&org.path()).into_iter().map(move |name| format!("{}/{}", org.file_name().to_string_lossy(), name.file_name().to_string_lossy())))
        .collect()
}

/// The releases of `package` whose cards are kept.
pub fn kept_releases(root: &Path, hub: &str, package: &str) -> Vec<String> {
    let cards = std::fs::read_dir(dir(root, hub, package)).into_iter().flatten().flatten();
    cards.filter_map(|card| Some(Path::new(&card.file_name()).file_stem()?.to_str()?.to_owned())).collect()
}

/// Keeps a release card as the Hub answered it.
pub(crate) fn keep(root: &Path, hub: &str, package: &str, document: &Value) -> Result<Card, Error> {
    let card = read(document)?;
    let dir = dir(root, hub, package);
    let kept = durable::private_dir(&dir).and_then(|()| {
        durable::replace(
            &file(&dir, &card.release),
            document.to_string().as_bytes(),
            0o600,
        )
    });
    // A card that cannot be kept is read again next time; the run goes on.
    drop(kept);
    Ok(card)
}

fn file(dir: &Path, release: &str) -> PathBuf {
    dir.join(format!("{}.json", release.replace(['/', '\\'], "_")))
}

/// A release card: `{release: {release, …}, package_interface, …}`.
pub(crate) fn read(document: &Value) -> Result<Card, Error> {
    let release = document["release"]["release"].as_str().unwrap_or_default();
    if release.is_empty() {
        return Err(Error::new(
            Kind::Unavailable,
            "hub.release_card_invalid",
            "The Hub sent a release card with no release in it.",
        ));
    }
    let interface = Interface::parse(&document["package_interface"]).map_err(|why| {
        Error::new(
            Kind::Unavailable,
            "hub.package_interface_invalid",
            format!("Cozy couldn't read the package interface the Hub sent: {why}"),
        )
    })?;
    Ok(Card {
        release: release.to_owned(),
        interface,
        document: document.clone(),
    })
}
