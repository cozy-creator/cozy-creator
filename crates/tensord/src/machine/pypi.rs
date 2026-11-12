//! A Runtime or TensorFS wheel the machine fetches itself: published on uv's index, held to the
//! index's own SHA-256. It lands through the machine's store with the one downloader: `stop` ends
//! it, and what landed stays for the next ask.
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use tensorfs::{
    err::{Code, Refusal, Result},
    fetch::{is_loopback_origin, Boxed, Location, Origin},
    format::ObjectRef,
};

use crate::{
    origin::{priority, Machine, Remote},
    stop::Stop,
};

pub(crate) fn wheel(distribution: &str, version: &str, uv: &super::uv::Uv, python: &str, dir: &Path, machine: &Machine, stop: &Stop) -> io::Result<PathBuf> {
    let project = distribution.replace('_', "-");
    if version.is_empty() || !version.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-+!".contains(&c)) {
        return Err(io::Error::other("a published wheel requires a concrete version"));
    }
    let input = format!("{project}=={version}\n");
    let plan = super::update::compile_sdk(uv, python, &input, stop, true)?
        .map_err(|why| io::Error::other(format!("{project} {version}: {why}")))?;
    let wheel = selected(&plan, &project, version)?;
    wheel_at(&wheel.url, &wheel.hashes.sha256, &wheel.filename()?, dir, machine, priority::NOW, stop)
}

#[derive(serde::Deserialize)]
struct Artifact {
    url: String,
    hashes: Hashes,
}
#[derive(serde::Deserialize)]
struct Hashes { sha256: String }

impl Artifact {
    fn filename(&self) -> io::Result<String> {
        let name = self.url.rsplit('/').next().unwrap_or_default();
        if !self.url.starts_with("https://") || !name.ends_with(".whl")
            || !name.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-+".contains(&c))
            || self.hashes.sha256.len() != 64 || !self.hashes.sha256.bytes().all(|c| c.is_ascii_hexdigit())
        { return Err(io::Error::other("the resolver returned no verified wheel URL")); }
        Ok(name.into())
    }
}

/// uv resolves for the executor's Python and this machine's platform, returning one wheel.
/// Filename order and a matching architecture alone cannot establish Python ABI compatibility.
fn selected(plan: &str, distribution: &str, version: &str) -> io::Result<Artifact> {
    #[derive(serde::Deserialize)]
    struct Package { name: String, version: String, #[serde(default)] wheels: Vec<Artifact> }
    #[derive(serde::Deserialize)]
    struct Plan { packages: Vec<Package> }
    let parsed: Plan = toml::from_str(plan).map_err(io::Error::other)?;
    let mut packages = parsed.packages.into_iter().filter(|p| p.name.replace('_', "-") == distribution);
    let package = packages.next().ok_or_else(|| io::Error::other("the resolver omitted the requested wheel"))?;
    if packages.next().is_some() || package.version != version {
        return Err(io::Error::other("the resolver returned ambiguous or different SDK wheels"));
    }
    // uv lists every wheel this Python and host install (a Runtime's built one and its pure
    // one); a built wheel is the one uv itself installs over a pure one.
    let wheel = package.wheels.into_iter().min_by_key(|w| w.url.ends_with("-none-any.whl"))
        .ok_or_else(|| io::Error::other("no host wheel"))?;
    if !wheel.filename()?.starts_with(&format!("{}-{version}-", distribution.replace('-', "_"))) {
        return Err(io::Error::other("the selected filename differs from its package/version"));
    }
    Ok(wheel)
}

/// The newest GitHub prerelease of `distribution` whose version `meets` admits and that holds a wheel
/// for this machine, landed in `dir`: its version and path. None: no release meets it. Err: the
/// releases couldn't be read, which says nothing of what is published.
pub(crate) fn prerelease(distribution: &str, meets: &dyn Fn(&str) -> bool, dir: &Path, machine: &Machine, order: u64, stop: &Stop) -> io::Result<Option<(String, PathBuf)>> {
    #[derive(serde::Deserialize)]
    struct Release { tag_name: String, #[serde(default)] draft: bool }
    // Nothing yet says what is published: `NotConnected`, asking again may be answered.
    let unread = |why: Refusal| io::Error::new(io::ErrorKind::NotConnected, why.to_string());
    let listed = text(machine, stop, api::prerelease::releases(distribution)).map_err(unread)?;
    let releases: Vec<Release> = serde_json::from_str(&listed).map_err(io::Error::other)?;
    let mut versions: Vec<&str> = (releases.iter().filter(|release| !release.draft))
        .filter_map(|release| api::prerelease::version_of(distribution, &release.tag_name))
        .filter(|version| meets(version))
        .collect();
    let platform = format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH);
    while let Some(version) = tensorfs::format::newest_by(versions.iter().copied(), |version| version) {
        versions.retain(|other| *other != version);
        let base = api::prerelease::base(distribution, version);
        let sums = match text(machine, stop, format!("{base}/SHA256SUMS")) {
            Ok(sums) => sums,
            Err(e) if matches!(e.code, Code::REF_NOT_FOUND | Code::ORIGIN_REFUSED) => continue,
            Err(e) => return Err(unread(e)),
        };
        if let Some((file, sha256)) = api::prerelease::wheel(&sums, distribution, version, &platform) {
            let wheel = wheel_at(&format!("{base}/{file}"), sha256, file, dir, machine, order, stop)?;
            return Ok(Some((version.to_owned(), wheel)));
        }
    }
    Ok(None)
}

/// Wheel `file` at an https `url`, redirects followed: its bytes are held to `sha256`, so no host
/// vouches for anything. Other bytes fail `wheel_digest_mismatch`. At `order` in the download line: an install's, its job's.
pub(crate) fn wheel_at(url: &str, sha256: &str, file: &str, dir: &Path, machine: &Machine, order: u64, stop: &Stop) -> io::Result<PathBuf> {
    // A private server's root as uv trusts it: the bundle SSL_CERT_FILE names. Only this fetch
    // reads it; the Hub's clients trust their own CA alone.
    let bundle = api::config::environment().get("SSL_CERT_FILE").and_then(|path| fs::read(path).ok()).filter(|pem| !pem.is_empty());
    let failed = |e: Refusal| match e.code {
        Code::OBJECT_ID_MISMATCH => io::Error::other(super::update::Coded {
            code: "wheel_digest_mismatch",
            message: format!("{file} from {url}: {}", e.detail),
        }),
        _ => {
            let network = tensorfs::Failure::from(e.clone()).kind == tensorfs::Kind::Network;
            let kind = if network { io::ErrorKind::NotConnected } else { io::ErrorKind::Other };
            io::Error::new(kind, format!("{file} from {url}: {e}"))
        }
    };
    let blob = object_at(machine, url, sha256, None, bundle, order, stop).map_err(failed)?;
    land(&blob, dir, file)
}

/// Object `sha256` at `url` (its length asked of the server when not given), through the store:
/// its path there. `trust` is a PEM bundle of roots beyond the public ones.
pub(crate) fn object_at(machine: &Machine, url: &str, sha256: &str, length: Option<u64>, trust: Option<Vec<u8>>, order: u64, stop: &Stop) -> Result<PathBuf> {
    let length = match length {
        Some(length) => length,
        None => {
            let (probed, roots) = (url.to_string(), trust.clone());
            machine.block(stop, async move { tensorfs::fetch::length_pinned(&probed, roots.as_deref()).await })?
        }
    };
    let object = ObjectRef { sha256: sha256.into(), length };
    fetch(machine, url, &object, trust.map(Arc::new), order, stop)
}

/// One object from one URL, through the store.
fn fetch(machine: &Machine, url: &str, object: &ObjectRef, trust: Option<Arc<Vec<u8>>>, order: u64, stop: &Stop) -> Result<PathBuf> {
    let remote = Remote { origin: Arc::new(Url(url.into())), trust, local: is_loopback_origin(url) };
    machine.objects(&remote, std::slice::from_ref(object), order, stop)?;
    machine.store().path(object).map_err(|e| e.refusal())
}

/// A file at a fixed URL: the one object it serves.
struct Url(String);

impl Origin for Url {
    fn locate<'a>(&'a self, objects: &'a [ObjectRef]) -> Boxed<'a, Result<Vec<Location>>> {
        Box::pin(async move { Ok(objects.iter().map(|_| Location::Url { url: self.0.clone(), headers: vec![] }).collect()) })
    }

    fn public(&self) -> bool {
        false
    }
}

/// A published text file (a release's manifest or signatures, a listing), asked again by the one retry rule
/// (`tensorfs::fetch::get_pinned`): what reads it judges it.
pub(crate) fn text(machine: &Machine, stop: &Stop, url: String) -> Result<String> {
    let asked = url.clone();
    let bytes = machine.block(stop, async move {
        let mut bytes = vec![];
        tensorfs::fetch::get_pinned(&asked, 0, &mut |at, frame| {
            bytes.truncate(at as usize);
            bytes.extend_from_slice(frame);
            Ok(())
        })
        .await
        .map(|()| bytes)
    })?;
    String::from_utf8(bytes).map_err(|e| Refusal { code: Code::ORIGIN_REFUSED, detail: format!("{url}: {e}") })
}

fn land(blob: &Path, dir: &Path, file: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(file);
    let _ = fs::remove_file(&path);
    fs::hard_link(blob, &path).or_else(|_| fs::copy(blob, &path).map(drop))?;
    Ok(path)
}
