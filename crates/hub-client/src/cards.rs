//! Release cards this computer keeps under its cozy home (`root`). A release never changes, so its
//! card is read from the Hub once and kept forever, by release; a package's newest release is kept
//! beside them ([`crate::latest`]). Calcifer resolves runs with these, and so do the verbs that name
//! a release.
use std::path::{Path, PathBuf};

use platform::durable;
use protocol::error::{Error, Kind};
use protocol::interface::Interface;
use serde_json::Value;

use crate::Client;

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

    /// The package interface document, as the Hub answered it.
    pub fn interface_document(&self) -> &Value {
        &self.document["package_interface"]
    }

    /// A callable's request struct as its card spells it, with `accelerator: false` for a CPU
    /// function: what `--describe --json` prints.
    pub fn request(&self, name: &str) -> Value {
        request(&self.document["package_interface"], name)
    }
}

/// Where `(hub, package)`'s cards live, beside its newest release.
fn dir(root: &Path, hub: &str, package: &str) -> PathBuf {
    crate::latest::dir(root, hub, package)
}

/// A failed Hub call as a refusal.
pub fn refusal(error: crate::Error) -> Error {
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
    if let Some(card) = kept(root, hub, package, release) {
        return Ok(card);
    }
    let (org, name) = package.split_once('/').expect("a package is org/name");
    let answer = client()?.get_package_release(org, name, release).await;
    let document = answer.map_err(|e| absent(refusal(e), &format!("{package}@{release}")))?;
    let document = serde_json::to_value(&document).expect("a card serializes");
    keep(root, hub, package, &document)
}

/// The newest release of `package` this computer keeps, which Calcifer refreshes in the background:
/// a run makes no Hub read for a package it knows. One it never knew is read once ([`fresh`]).
pub async fn latest(
    client: impl FnOnce() -> Result<Client, Error>,
    root: &Path,
    hub: &str,
    package: &str,
) -> Result<String, Error> {
    match crate::latest::kept(&dir(root, hub, package)) {
        Some(release) => Ok(release),
        None => fresh(&client()?, root, hub, package).await,
    }
}

/// The Hub's newest published release of `package` now, and kept: what an install names.
pub async fn fresh(client: &Client, root: &Path, hub: &str, package: &str) -> Result<String, Error> {
    let (org, name) = package.split_once('/').expect("a package is org/name");
    let release = crate::latest::read(client, &dir(root, hub, package), org, name)
        .await
        .map_err(|e| absent(refusal(e), package))?;
    release.ok_or_else(|| {
        Error::new(
            Kind::NotFound,
            "package_unreleased",
            format!("{package} has no release to run"),
        )
        .remedy("its owner publishes one with `cozy package publish`")
    })
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
        format!("the Hub has no {what}"),
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

/// Keeps a release card as the Hub answered it.
pub fn keep(root: &Path, hub: &str, package: &str, document: &Value) -> Result<Card, Error> {
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
pub fn read(document: &Value) -> Result<Card, Error> {
    let release = document["release"]["release"].as_str().unwrap_or_default();
    if release.is_empty() {
        return Err(Error::new(
            Kind::Unavailable,
            "hub.release_card_invalid",
            "the Hub's release card names no release",
        ));
    }
    let interface = Interface::parse(&document["package_interface"]).map_err(|why| {
        Error::new(
            Kind::Unavailable,
            "hub.package_interface_invalid",
            format!("the Hub's package interface is unreadable: {why}"),
        )
    })?;
    Ok(Card {
        release: release.to_owned(),
        interface,
        document: document.clone(),
    })
}


/// `name`'s request contract in a package interface document (`cozy run … --describe --json`): a
/// release's card and local code's kept interface answer the same.
pub fn request(interface: &Value, name: &str) -> Value {
    let Some(entry) = protocol::interface::entrypoint(interface, name) else {
        return Value::Null;
    };
    let mut request = entry["request"].clone();
    if entry["accelerator"] == Value::Bool(false) {
        request["accelerator"] = Value::Bool(false);
    }
    request
}
