//! Releases this computer holds (cl-1127; Paul, 11-17: "if tensorhub is down, or unable to be reached, ideally the host machine
//! (here) should be able to, on a remote machine, give it a package from our local machine"). A release a machine installs through
//! Calcifer has its install card kept here behind that install, as a machine reads it from the Hub's install plan (its interface,
//! Python, exact pyproject.toml and uv.lock, and the wheels its Hub hosts), with every such wheel, its lock's cozy packages' too, held
//! in this computer's store by content. Another machine's install of it then takes it from here by content, as local code goes
//! (`v1::Release::card`): it reads nothing of it at the Hub, which may be down. The rest of its lock is PyPI's, read by the machine.
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
};

use api::{
    calcifer::StagedInput,
    config::Origin,
    error::{Error, Kind},
    hub::types::release_digest,
    ids::Digest,
};
use base64::Engine as _;
use platform::{durable, paths::Home};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::api::Api;

/// A machine's install card for a release (TensorD's `published::Card`, its `release.json`).
#[derive(Serialize, Deserialize)]
struct Card {
    interface: Value,
    python: String,
    pyproject: String,
    lock: String,
    wheels: Vec<Wheel>,
    #[serde(default)]
    digest: String,
}

/// A wheel its Hub hosts, by its digest (`published::HubFile`).
#[derive(Clone, Serialize, Deserialize)]
struct Wheel {
    url: String,
    sha256: String,
    name: String,
    length: u64,
}

/// What is kept of a release: its card, and the wheels its lock takes from its Hub's indexes (other cozy packages).
#[derive(Serialize, Deserialize)]
struct Kept {
    card: Card,
    #[serde(default)]
    indexed: Vec<Wheel>,
}

/// The most a release's wheels may weigh to be kept here: one heavier is its Hub's to serve.
const MOST: u64 = 512 << 20;

/// `<calcifer>/held/<hub>`.
fn hub_dir(home: &Home, hub: &str) -> PathBuf {
    let host: String = hub.split_once("://").map_or(hub, |(_, rest)| rest).trim_end_matches('/').chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    home.calcifer().join("held").join(host)
}

/// `<calcifer>/held/<hub>/<org>/<name>`.
fn dir(api: &Api, hub: &str, package: &str) -> PathBuf {
    let mut dir = hub_dir(api.machines.hubs.home(), hub);
    dir.extend(package.split('/'));
    dir
}

/// Each namespace's index at `hub` as a lock last bound it here (`author::locked`): what a lock binds to with the Hub unreachable,
/// so uv finds the pages it holds of it.
const INDEXES: &str = "indexes.json";

pub(crate) fn keep_indexes(home: &Home, hub: &str, bound: &BTreeMap<String, (String, String)>) {
    if bound.is_empty() || crate::private_io::current().is_some_and(|scope| !scope.storage.ordinary) {
        return;
    }
    let path = hub_dir(home, hub).join(INDEXES);
    let mut kept: BTreeMap<String, String> = fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    let before = kept.len();
    let changed = bound.values().fold(false, |changed, (namespace, url)| kept.insert(namespace.clone(), url.clone()).as_ref() != Some(url) || changed);
    if changed || kept.len() != before {
        let bytes = serde_json::to_vec(&kept).expect("indexes serialize");
        let _ = durable::private_dir(path.parent().expect("a Hub's folder")).and_then(|()| durable::replace(&path, &bytes, 0o600));
    }
}

/// The software `hub` names for machines (`GET /v1/software`), as this computer last read it: an update's target with the Hub
/// unreachable.
const SOFTWARE: &str = "software.json";

pub(crate) fn keep_software(home: &Home, hub: &str, software: &api::hub::types::Software) {
    let path = hub_dir(home, hub).join(SOFTWARE);
    let bytes = serde_json::to_vec(software).expect("software serializes");
    if fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
        let _ = durable::private_dir(path.parent().expect("a Hub's folder")).and_then(|()| durable::replace(&path, &bytes, 0o600));
    }
}

pub(crate) fn software(home: &Home, hub: &str) -> Option<api::hub::types::Software> {
    serde_json::from_slice(&fs::read(hub_dir(home, hub).join(SOFTWARE)).ok()?).ok()
}

/// `namespace`'s index at `hub` as last bound here, else where its Hub serves it by default.
pub(crate) fn index(home: &Home, hub: &str, namespace: &str) -> String {
    let kept: BTreeMap<String, String> = fs::read(hub_dir(home, hub).join(INDEXES)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    kept.get(namespace).cloned().unwrap_or_else(|| format!("{}/v1/index/{namespace}/simple/", hub.trim_end_matches('/')))
}

fn file(api: &Api, hub: &str, package: &str, release: &str) -> PathBuf {
    dir(api, hub, package).join(format!("{}.json", release.replace(['/', '\\'], "_")))
}

/// The releases of `package` this computer holds.
pub(crate) fn releases(api: &Api, hub: &str, package: &str) -> Vec<String> {
    let kept = fs::read_dir(dir(api, hub, package)).into_iter().flatten().flatten();
    kept.filter_map(|entry| Some(std::path::Path::new(&entry.file_name()).file_stem()?.to_str()?.to_owned())).collect()
}

/// Keeps `release` of `package` behind whatever asked, once a Calcifer: what a later install of it anywhere takes from here.
pub(crate) fn keep_behind(api: &Arc<Api>, hub: &str, package: &str, release: &str) {
    static ASKED: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Mutex::default);
    if crate::private_io::current().is_some_and(|scope| !scope.storage.ordinary) {
        return;
    }
    let path = file(api, hub, package, release);
    if path.is_file() || !ASKED.lock().expect("asked lock").insert(path) {
        return;
    }
    let (api, hub, package, release) = (api.clone(), hub.to_owned(), package.to_owned(), release.to_owned());
    tokio::spawn(async move {
        if let Err(e) = keep(&api, &hub, &package, &release).await {
            tracing::info!("{package} {release} isn't held here: {e}");
        }
    });
}

async fn keep(api: &Arc<Api>, hub: &str, package: &str, release: &str) -> Result<(), Error> {
    let (org, name) = package.split_once('/').ok_or_else(|| Error::new(Kind::Usage, "package_ref_invalid", package.to_owned()))?;
    let hubs = &api.machines.hubs;
    let plan = hubs.reader(hub)?.get_package_download_plan(org, name, release, None, Some(std::env::consts::ARCH)).await
        .map_err(|e| e.refusal(|| "cozy login <email>".into()))?;
    let unread = |what: &str| Error::new(Kind::Unavailable, "release_plan_invalid", format!("its install plan's {what} doesn't read"));
    if plan.release != release {
        return Err(unread("release"));
    }
    let text = |what: &str, document: &api::hub::types::ExactDocument| {
        base64::engine::general_purpose::STANDARD.decode(&document.canonical_bytes).ok().and_then(|bytes| String::from_utf8(bytes).ok()).ok_or_else(|| unread(what))
    };
    let interface: Value = serde_json::from_str(&text("interface", &plan.package_interface)?).map_err(|_| unread("interface"))?;
    let wheels = plan.downloads.iter().filter(|file| file.path.ends_with(".whl")).map(|file| Wheel {
        url: file.url.clone(),
        sha256: file.digest.trim_start_matches("sha256:").to_ascii_lowercase(),
        name: file.path.rsplit('/').next().unwrap_or(&file.path).to_owned(),
        length: file.length.try_into().unwrap_or(0),
    });
    let card = Card {
        interface,
        python: plan.python_version.clone().unwrap_or_default(),
        pyproject: text("pyproject.toml", &plan.pyproject)?,
        lock: text("uv.lock", &plan.uv_lock)?,
        wheels: wheels.collect(),
        digest: match plan.release_files.is_empty() {
            true => String::new(),
            false => release_digest(&plan.release_files),
        },
    };
    let kept = Kept { indexed: indexed(&card.lock), card };
    let all: Vec<&Wheel> = kept.card.wheels.iter().chain(&kept.indexed).collect();
    if all.iter().map(|wheel| wheel.length).sum::<u64>() > MOST {
        return Ok(());
    }
    // The Hub's own files under a private CA (a test Hub's), as its machines trust it.
    let roots = hubs.facts(&Origin::parse(hub)?).await.ok().map(|facts| facts.ca().to_vec()).filter(|der| !der.is_empty()).map(|der| pem(&der));
    for wheel in all {
        let digest: Digest = format!("sha256:{}", wheel.sha256).parse().map_err(|_| unread("wheel digest"))?;
        if crate::runs::files::held(&digest).is_some() {
            continue;
        }
        let mut bytes = Vec::with_capacity(wheel.length as usize);
        let mut sink = |at: u64, frame: &[u8]| {
            bytes.truncate(at as usize);
            bytes.extend_from_slice(frame);
            Ok(())
        };
        tensorfs::fetch::get_pinned_trusting(&wheel.url, roots.as_deref().map(str::as_bytes), &mut sink).await
            .map_err(|e| Error::new(Kind::Unavailable, "release_wheel_unfetched", format!("{}: {e}", wheel.name)))?;
        if hex::encode(Sha256::digest(&bytes)) != wheel.sha256 {
            return Err(Error::new(Kind::Unavailable, "release_wheel_unfetched", format!("{} isn't the wheel its plan names", wheel.name)));
        }
        tokio::task::spawn_blocking(move || crate::runs::files::hold(&bytes)).await.map_err(|e| Error::internal("release_wheel_unheld", e.to_string()))??;
    }
    let path = file(api, hub, package, release);
    let bytes = serde_json::to_vec(&kept).expect("a kept release serializes");
    durable::private_dir(path.parent().expect("a release's folder")).and_then(|()| durable::replace(&path, &bytes, 0o600))
        .map_err(|e| Error::internal("release_unheld", e.to_string()))?;
    tracing::info!("{package} {release} is held here for any machine");
    Ok(())
}

/// The wheels `lock` takes from a Hub index, one each (`published::read_lock`'s Hub rows): other cozy packages.
fn indexed(lock: &str) -> Vec<Wheel> {
    #[derive(Deserialize)]
    struct Lock {
        #[serde(default)]
        package: Vec<Row>,
    }
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        source: toml::Table,
        #[serde(default)]
        wheels: Vec<LockedWheel>,
    }
    #[derive(Deserialize)]
    struct LockedWheel {
        url: Option<String>,
        hash: Option<String>,
        size: Option<u64>,
    }
    let Ok(lock) = toml::from_str::<Lock>(lock) else { return vec![] };
    let indexed = |row: &Row| row.source.get("registry").and_then(toml::Value::as_str).is_some_and(|registry| registry.contains("/v1/index/"));
    (lock.package.iter().filter(|row| indexed(row)))
        .filter_map(|row| match row.wheels.as_slice() {
            [LockedWheel { url: Some(url), hash: Some(hash), size }] => Some(Wheel {
                url: url.clone(),
                sha256: hash.trim_start_matches("sha256:").to_ascii_lowercase(),
                name: url.rsplit('/').next().unwrap_or_default().to_owned(),
                length: size.unwrap_or(0),
            }),
            _ => None,
        })
        .collect()
}

/// The wheels `lock` takes from a Hub index that this computer holds: each file's name, its staged input and length.
pub(crate) fn indexed_held(lock: &str) -> Vec<(String, StagedInput, u64)> {
    (indexed(lock).into_iter())
        .filter(|wheel| wheel.name.ends_with(".whl") && !wheel.name.contains(['/', '\\']))
        .filter_map(|wheel| {
            let digest: Digest = format!("sha256:{}", wheel.sha256).parse().ok()?;
            let path = crate::runs::files::held(&digest)?;
            let length = fs::metadata(&path).ok()?.len();
            Some((wheel.name, StagedInput { path, digest, length }, length))
        })
        .collect()
}

fn pem(der: &[u8]) -> String {
    let body = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = body.as_bytes().chunks(64).map(|chunk| std::str::from_utf8(chunk).expect("base64 is ASCII")).collect();
    format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", lines.join("\n"))
}

/// `release` of `package` as a machine takes it from here: its card's digest and the files to stage, the card and every wheel; none
/// when this computer doesn't hold it whole, or holds a wheel of one platform only (kept for this computer's).
pub(crate) fn sent(api: &Api, hub: &str, package: &str, release: &str) -> Option<(String, Vec<StagedInput>)> {
    let bytes = fs::read(file(api, hub, package, release)).ok()?;
    let kept: Kept = serde_json::from_slice(&bytes).ok()?;
    if kept.card.wheels.iter().any(|wheel| !wheel.name.ends_with("-none-any.whl")) {
        return None;
    }
    let mut inputs = vec![];
    for wheel in kept.card.wheels.iter().chain(&kept.indexed) {
        let digest: Digest = format!("sha256:{}", wheel.sha256).parse().ok()?;
        let path = crate::runs::files::held(&digest)?;
        inputs.push(StagedInput { length: fs::metadata(&path).ok()?.len(), path, digest });
    }
    let card = serde_json::to_vec(&kept.card).ok()?;
    let digest = crate::runs::files::hold(&card).ok()?;
    let path = crate::runs::files::held(&digest)?;
    inputs.push(StagedInput { path, digest, length: card.len() as u64 });
    Some((digest.to_string(), inputs))
}
