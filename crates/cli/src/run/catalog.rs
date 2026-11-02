//! Release cards this computer keeps. A release never changes, so its card is read from the Hub
//! once and kept forever, by release. Which release a run binds and runs is resolved before
//! (`super::resolve`): the one asked, else the package's newest as this computer keeps it.
use std::path::{Path, PathBuf};

use platform::{durable, paths::Home};
use protocol::config::Origin;
use protocol::error::{Error, Kind};
use serde_json::Value;

use super::interface::Interface;
use crate::{
    Context,
    context::{hub_answer, hub_error},
};

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

    /// A callable's request struct as its card spells it, with `accelerator: false` for a CPU
    /// function: what `--describe --json` prints.
    pub fn request(&self, name: &str) -> Value {
        request(&self.document["package_interface"], name)
    }
}

/// Where `(hub, package)`'s cards live, beside its newest release (`hub_client::latest`).
fn dir(home: &Home, hub: &str, package: &str) -> PathBuf {
    hub_client::latest::dir(home.root(), hub, package)
}

/// The card of `release`: the kept one, else the Hub's, which is then kept.
pub async fn card(ctx: &Context, hub: &Origin, package: &str, release: &str) -> Result<Card, Error> {
    if let Some(card) = kept(&ctx.home, hub, package, release) {
        return Ok(card);
    }
    let (org, name) = package.split_once('/').expect("a package is org/name");
    let answer = ctx
        .hub_client(hub)?
        .get_package_release(org, name, release)
        .await;
    let document = hub_answer(answer).map_err(|e| absent(e, &format!("{package}@{release}")))?;
    let document = serde_json::to_value(&document).expect("a card serializes");
    keep(&ctx.home, hub, package, &document)
}

/// The newest release of `package` this computer keeps, which Calcifer refreshes in the background:
/// a run makes no Hub read for a package it knows. One it never knew is read once ([`fresh`]).
pub async fn latest(ctx: &Context, hub: &Origin, package: &str) -> Result<String, Error> {
    match hub_client::latest::kept(&dir(&ctx.home, hub, package)) {
        Some(release) => Ok(release),
        None => fresh(ctx, hub, package).await,
    }
}

/// The Hub's newest published release of `package` now, and kept: what an install names.
pub async fn fresh(ctx: &Context, hub: &Origin, package: &str) -> Result<String, Error> {
    let (org, name) = package.split_once('/').expect("a package is org/name");
    let client = ctx.hub_client(hub)?;
    let release = hub_client::latest::read(&client, &dir(&ctx.home, hub, package), org, name)
        .await
        .map_err(|e| absent(hub_error(e), package))?;
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
pub fn kept(home: &Home, hub: &str, package: &str, release: &str) -> Option<Card> {
    let path = file(&dir(home, hub, package), release);
    let document: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    read(&document).ok()
}

/// Keeps a release card as the Hub answered it.
pub fn keep(home: &Home, hub: &str, package: &str, document: &Value) -> Result<Card, Error> {
    let card = read(document)?;
    let dir = dir(home, hub, package);
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
