//! `/v1/package-locks` and `/v1/package-publications`: authoring a package on this computer, with
//! uv. Package code never runs here: the interface comes from Runtime's static describer.
//!
//! The project names no account: `{ index = "tensorhub" }` sources resolve to the account's index
//! on the Hub, written into a private copy. Torch is the machine's (cl-715): the copy excludes its
//! layer, so no lock holds it. The authored files never change; a lock writes back only
//! `uv.lock`.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use futures_util::{StreamExt as _, TryStreamExt as _};
use hub_client::types::{PackageDeclaredFile, PackageFileGrant, PresignedUpload};
use protocol::{
    config::Origin,
    error::{Error, Kind},
    hub::{LockAsk, Locked, PublishAsk, Published},
    interface::Interface,
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::{origin, reader, refusal, signed_in, work::Work};
use crate::{
    api::Api,
    local_code::{INDEX, Project, account, describe_with, index_url, python, refuse, run, source_files, uv},
    tools::Say,
};

/// What a machine provides every package (cl-715): its PyTorch layer. A lock leaves these, and
/// what only they bring (the CUDA wheels), to the machine.
const TORCH_LAYER: [&str; 4] = ["torch", "torchvision", "torchaudio", "triton"];

/// `f`, which waits on uv or the disk, off the runtime's async workers.
fn blocking<T>(f: impl FnOnce() -> T) -> T {
    tokio::task::block_in_place(f)
}

/// The tools locking and publishing `project` need: git when it depends on a git source (refused
/// with its install named, before uv runs and fails on it), and uv (fetched, pinned).
async fn tools(api: &Api, project: &Project, say: Say<'_>) -> Result<PathBuf, Error> {
    if project.git && platform::tools::on_path("git", &crate::tools::path()).is_none() {
        return Err(git_missing());
    }
    crate::tools::uv(api.machines.hubs.home(), say).await
}

fn git_missing() -> Error {
    let tool = platform::tools::tool("git");
    Error::new(Kind::Unavailable, "git_unavailable", format!("git not found: {}", tool.cost))
        .next(crate::tools::install_hint(tool, &crate::tools::path()))
        .next("cozy doctor")
}

/// A private copy of the project to lock and build in.
struct Stage {
    _dir: tempfile::TempDir,
    root: PathBuf,
    src: PathBuf,
    /// The uv it is locked and built with.
    uv: PathBuf,
}

impl Stage {
    /// The copy to lock and publish, with Torch left to the machine (`TORCH_LAYER`): its lock
    /// holds no Torch, and an author's cap or exact pin on Torch is said once.
    fn new(project: &Project, uv: PathBuf, say: Say<'_>) -> Result<Self, Error> {
        let dir = tempfile::tempdir().map_err(|e| refuse("stage_unavailable", e))?;
        let root = dir.path().to_path_buf();
        let src = root.join("src");
        for file in source_files(&project.root)? {
            let to = src.join(&file);
            fs::create_dir_all(to.parent().unwrap_or(&src))
                .and_then(|()| fs::copy(project.root.join(&file), &to))
                .map_err(|e| refuse("stage_unavailable", e))?;
        }
        let path = src.join("pyproject.toml");
        let authored = fs::read_to_string(&path).map_err(|e| refuse("stage_unavailable", e))?;
        let mut doc: toml_edit::DocumentMut = authored.parse().map_err(|e| refuse("project_invalid", e))?;
        let on_layer = |spec: &str| {
            TORCH_LAYER.iter().any(|name| {
                (spec.trim_start().strip_prefix(name))
                    .is_some_and(|rest| rest.is_empty() || rest.trim_start().starts_with(['<', '>', '=', '!', '~', ';', '[']))
            })
        };
        let capped: Vec<String> = (doc["project"]["dependencies"].as_array().into_iter().flatten())
            .filter_map(|spec| spec.as_str())
            .filter(|spec| on_layer(spec) && (spec.contains('<') || spec.contains("==") || spec.contains("~=")))
            .map(str::to_owned)
            .collect();
        if !capped.is_empty() {
            say(&format!(
                "{} caps or pins Torch: most machines run a newer one, so it installs its own there",
                capped.join(", ")
            ));
        }
        let excluded = (doc["tool"]["uv"]["exclude-dependencies"].or_insert(toml_edit::value(toml_edit::Array::new())))
            .as_array_mut()
            .ok_or_else(|| refuse("project_invalid", "tool.uv.exclude-dependencies is not a list"))?;
        for name in TORCH_LAYER {
            if !excluded.iter().any(|held| held.as_str() == Some(name)) {
                excluded.push(name);
            }
        }
        fs::write(&path, doc.to_string()).map_err(|e| refuse("stage_unavailable", e))?;
        Ok(Self { _dir: dir, root, src, uv })
    }

    /// Points the copy's `tensorhub` index at `url`.
    fn bind(&self, url: &str) -> Result<(), Error> {
        let path = self.src.join("pyproject.toml");
        let mut text = fs::read_to_string(&path).map_err(|e| refuse("stage_unavailable", e))?;
        text.push_str(&format!("\n[[tool.uv.index]]\nname = \"{INDEX}\"\nurl = \"{url}\"\nexplicit = true\n"));
        fs::write(&path, text).map_err(|e| refuse("stage_unavailable", e))
    }

    /// Relocks the copy against its bound index with `pins` as constraints, then without them:
    /// a lock made for another index moves to this one keeping each exact version (or fails
    /// naming the pin), and the copy publishes with no constraint it did not author.
    fn relock_pinned(&self, python: &str, pins: &BTreeMap<String, String>) -> Result<(), Error> {
        let path = self.src.join("pyproject.toml");
        let authored = fs::read_to_string(&path).map_err(|e| refuse("stage_unavailable", e))?;
        let mut doc: toml_edit::DocumentMut = authored.parse().map_err(|e| refuse("project_invalid", e))?;
        let constraints = (doc["tool"]["uv"]["constraint-dependencies"].or_insert(toml_edit::value(toml_edit::Array::new())))
            .as_array_mut()
            .ok_or_else(|| refuse("project_invalid", "tool.uv.constraint-dependencies is not a list"))?;
        for (name, version) in pins {
            constraints.push(format!("{name}=={version}"));
        }
        fs::write(&path, doc.to_string()).map_err(|e| refuse("stage_unavailable", e))?;
        let locked = run(uv(&self.uv, &self.src).args(["lock", "--no-progress", "--python", python]), "lock_failed");
        fs::write(&path, authored).map_err(|e| refuse("stage_unavailable", e))?;
        locked.map(drop)
    }
}

/// A uv.lock, as far as cozy reads it.
#[derive(Deserialize)]
struct Lock {
    #[serde(default)]
    package: Vec<LockedPackage>,
}

#[derive(Deserialize)]
struct LockedPackage {
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    source: toml::Table,
}

/// The packages a lock takes from a Tensorhub account index, by exact version.
fn index_pins(lock: &str) -> Result<BTreeMap<String, String>, Error> {
    let lock: Lock = toml::from_str(lock).map_err(|e| refuse("lock_invalid", e))?;
    Ok((lock.package.into_iter())
        .filter(|p| p.source.get("registry").and_then(|r| r.as_str()).is_some_and(|r| r.contains("/v1/index/")))
        .map(|p| (p.name, p.version))
        .collect())
}

/// Each locked package's version.
fn versions(lock: &str) -> Result<BTreeMap<String, String>, Error> {
    let lock: Lock = toml::from_str(lock).map_err(|e| refuse("lock_invalid", e))?;
    let mut out = BTreeMap::new();
    for package in lock.package {
        let local = ["directory", "path", "virtual", "editable"].iter().find(|k| package.source.contains_key(**k));
        if let Some(kind) = local
            && package.source.get(*kind).and_then(|v| v.as_str()) != Some(".")
        {
            return Err(refuse(
                "package_local_dependency",
                format!("{} is a local {kind} dependency, which a release cannot carry", package.name),
            )
            .remedy(format!(
                "publish {} to your account and name it with `{{ index = \"{INDEX}\" }}` in [tool.uv.sources]",
                package.name
            )));
        }
        out.insert(package.name, package.version);
    }
    Ok(out)
}

/// Whose name a publication takes: the signed-in account's, or with `root` the root namespace's
/// (first-party; the Hub admits only its admins there).
async fn publisher(client: &hub_client::Client, root: bool) -> Result<String, Error> {
    match root {
        true => Ok(protocol::names::ROOT.to_owned()),
        false => account(client).await,
    }
}

/// Locks the project in `ask.dir` for the Hub and account it publishes to.
pub(super) async fn lock(api: &Arc<Api>, ask: LockAsk, work: &Work) -> Result<Locked, Error> {
    let say = &|message: &str| work.say(message);
    let hub = origin(api, ask.hub.as_deref()).map_err(|r| r.0)?;
    let project = blocking(|| Project::read(ask.dir.clone()))?;
    let uv_path = tools(api, &project, say).await?;
    let stage = blocking(|| Stage::new(&project, uv_path, say))?;
    let account_index = match project.account_index {
        false => None,
        true => {
            let client = signed_in(api, &hub).map_err(|r| r.0)?;
            let account = publisher(&client, ask.root).await?;
            let url = index_url(&client, &hub, &account).await?;
            stage.bind(&url)?;
            Some(url)
        }
    };
    blocking(|| {
        let python = python(&project, &stage.uv)?;
        let mut command = uv(&stage.uv, &stage.src);
        command.args(["lock", "--no-progress", "--python", python.request.as_str()]);
        for package in &ask.upgrade {
            command.args(["--upgrade-package", package.as_str()]);
        }
        run(&mut command, "lock_failed")?;
        let locked = fs::read(stage.src.join("uv.lock")).map_err(|e| refuse("lock_failed", e))?;
        versions(&String::from_utf8_lossy(&locked))?;
        let path = project.root.join("uv.lock");
        let changed = fs::read(&path).ok().as_deref() != Some(&locked[..]);
        if changed {
            let staged = project.root.join(".uv.lock.cozy");
            fs::write(&staged, &locked).and_then(|()| fs::rename(&staged, &path)).map_err(|e| refuse("lock_unwritable", e))?;
        }
        Ok(Locked { lock: path, account_index, changed })
    })
}

/// One file of a release: where it goes and where it is here.
struct File {
    path: String,
    local: PathBuf,
    digest: String,
    length: i64,
}

impl File {
    fn new(path: String, local: PathBuf) -> Result<Self, Error> {
        let bytes = fs::read(&local).map_err(|e| refuse("release_file_unreadable", e))?;
        Ok(Self {
            path,
            local,
            digest: format!("sha256:{}", hex::encode(Sha256::digest(&bytes))),
            length: bytes.len() as i64,
        })
    }
}

/// Publishes the project in `ask.dir`: staged, relocked for this Hub and account, built,
/// described, uploaded and finalized. A version already published is answered as it is.
pub(super) async fn publish(api: &Arc<Api>, ask: PublishAsk, work: &Work) -> Result<Published, Error> {
    let say = &|message: &str| work.say(message);
    let hub = origin(api, ask.hub.as_deref()).map_err(|r| r.0)?;
    let hub_error = |e| refusal(&hub, e).0;
    let project = blocking(|| Project::read(ask.dir.clone()))?;
    let client = signed_in(api, &hub).map_err(|r| r.0)?;
    let account = publisher(&client, ask.root).await?;
    let mut result = Published {
        package: format!("{account}/{}", project.name),
        release: project.version.clone(),
        status: "already published".into(),
        changed: false,
        uploaded_bytes: 0,
        hub: hub.to_string(),
        warnings: vec![],
    };
    match client.get_package_release(&account, &project.name, &project.version).await {
        Ok(_) => return Ok(result),
        Err(e) => match hub_error(e) {
            e if e.kind == Kind::NotFound => {}
            e => return Err(e),
        },
    }
    let uv_path = tools(api, &project, say).await?;
    let stage = blocking(|| Stage::new(&project, uv_path, say))?;
    let committed = fs::read_to_string(project.root.join("uv.lock")).map_err(|_| {
        Error::new(Kind::Validation, "package_unlocked", "this project has no uv.lock").next("cozy package lock")
    })?;
    // Locking the copy is a no-op for a lock made for this Hub and account. For another, it moves
    // the account's packages to this index pinned to the versions the lock holds: a pin this Hub
    // cannot meet is refused by name, never re-resolved to a newer release.
    let python = blocking(|| python(&project, &stage.uv))?;
    if project.account_index {
        let url = index_url(&client, &hub, &account).await?;
        stage.bind(&url)?;
        let pins = index_pins(&committed)?;
        if !pins.is_empty() && !committed.contains(&url) {
            blocking(|| stage.relock_pinned(python.request.as_str(), &pins)).map_err(|e| unmet(e, &pins, &account, &hub))?;
        }
    }
    let files = blocking(|| built(&stage, &python.request, &committed, &result.package, &hub))?;
    let declared: Vec<PackageDeclaredFile> = (files.iter())
        .map(|f| PackageDeclaredFile { digest: f.digest.clone(), length: f.length, path: f.path.clone() })
        .collect();
    let (org, name, release) = (&account, &project.name, &project.version);
    let grants = client.begin_package_release(org, name, release, &declared).await.map_err(hub_error)?;
    result.uploaded_bytes = upload(&files, &grants.files, say).await?;
    let mut state = (client.finalize_package_release(org, name, release, &grants.publication_id, &python.full).await)
        .map_err(hub_error)?;
    // Finalization re-hashes every file on the Hub; its state ends the wait.
    while state.state != "committed" && state.state != "failed" {
        tokio::time::sleep(Duration::from_secs(2)).await;
        state = client.get_package_finalization(org, name, release).await.map_err(hub_error)?;
    }
    if state.state == "failed" {
        let said = |key: &str| state.error.as_ref().and_then(|e| e.get(key)).and_then(|v| v.as_str()).map(str::to_owned);
        let mut error =
            refuse("package_release_failed", said("message").unwrap_or_else(|| "the Hub refused the release".into()));
        if let Some(code) = said("code") {
            error = error.cause(code);
        }
        if let Some(remedy) = said("remedy") {
            error = error.remedy(remedy);
        }
        return Err(error);
    }
    result.status = "published".into();
    result.changed = true;
    result.warnings = state.warnings;
    // This computer's next unversioned run takes the release just published, not one refresh later.
    if let Ok(reader) = reader(api, &hub) {
        let _ = hub_client::cards::fresh(&reader, api.machines.hubs.home().root(), &hub, &result.package).await;
    }
    Ok(result)
}

/// The staged project locked for this Hub, held to the committed lock, built, its git
/// dependencies' wheels built, and described: the release's files.
fn built(
    stage: &Stage,
    python: &str,
    committed: &str,
    package: &str,
    hub: &Origin,
) -> Result<Vec<File>, Error> {
    run(uv(&stage.uv, &stage.src).args(["lock", "--no-progress", "--python", python]), "lock_failed")?;
    let bound = fs::read_to_string(stage.src.join("uv.lock")).map_err(|e| refuse("lock_failed", e))?;
    let (before, after) = (versions(committed)?, versions(&bound)?);
    if before != after {
        let detail = (before.iter())
            .find(|(name, version)| after.get(*name) != Some(*version))
            .map(|(name, version)| match after.get(name) {
                Some(now) => format!("uv re-resolved {name} from {version} to {now}"),
                None => format!("uv dropped {name} {version}"),
            })
            .unwrap_or_else(|| "uv added a package the lock does not hold".into());
        return Err(refuse(
            "account_index_lock_drift",
            format!("uv.lock does not hold for {package} on {}: {detail}", hub.as_str()),
        )
        .remedy("publish the locked versions of your own dependencies to this Hub first, or relock")
        .next("cozy package lock"));
    }
    let dist = stage.root.join("dist");
    run(
        uv(&stage.uv, &stage.src).args(["build", "--no-progress", "--python", python, "--out-dir"]).arg(&dist),
        "build_failed",
    )?;
    let git_wheels = git_wheels(stage, python)?;
    let runtime = after.get("cozy-runtime").map(String::as_str);
    let (said, document) = describe_with(&stage.uv, &stage.src, python, runtime)?;
    // A model default with no org stays as written: it names a first-party model (a machine reads
    // `sdxl@1.0.0` as `_/sdxl@1.0.0`), and the Runtime's interface parser refuses an org of `_`.
    let interface = Interface::parse(&document)
        .map_err(|why| refuse("package_interface_invalid", format!("installs would refuse it: {why}")))?;
    if let Some((callable, why)) = interface.unavailable.first() {
        return Err(refuse("package_interface_invalid", format!("installs could not call {callable}: {why}")));
    }
    let interface_path = stage.root.join("package-interface.json");
    fs::write(&interface_path, said.trim_end()).map_err(|e| refuse("stage_unavailable", e))?;
    let wheel = single(&dist, ".whl")?;
    let sdist = single(&dist, ".tar.gz")?;
    // Where each file is declared: the Hub's PackageReleasePath.
    let named = |dir: &str, local: &Path| format!("{dir}{}", local.file_name().unwrap_or_default().to_string_lossy());
    let mut files = vec![
        File::new(named("artifacts/project/", &wheel), wheel.clone())?,
        File::new(named("artifacts/source/", &sdist), sdist.clone())?,
        File::new("metadata/package-interface.json".into(), interface_path)?,
    ];
    for document in ["pyproject.toml", "uv.lock"] {
        files.push(File::new(document.into(), stage.src.join(document))?);
    }
    for local in git_wheels {
        files.push(File::new(named("artifacts/dependencies/", &local), local)?);
    }
    Ok(files)
}

/// A relock that could not keep the lock's account-index pins on `hub`: the pin uv names, by
/// package, version and Hub, when it names one; otherwise uv's own words.
fn unmet(error: Error, pins: &BTreeMap<String, String>, account: &str, hub: &Origin) -> Error {
    let named = pins.iter().find(|(name, version)| error.message.contains(&format!("{name}=={version}")));
    match named {
        Some((name, version)) => refuse(
            "account_index_pin_unpublished",
            format!("uv.lock pins {account}/{name} {version}, which {} does not publish", hub.as_str()),
        )
        .remedy(format!("publish {name} {version} to this Hub as {account} first, or relock against it"))
        .next("cozy package lock"),
        None => error,
    }
}

/// Sends each file the Hub does not hold to its grant, once per digest, sixteen at once; the bytes
/// sent.
async fn upload(files: &[File], grants: &[PackageFileGrant], say: Say<'_>) -> Result<u64, Error> {
    let http = reqwest::Client::new();
    let mut digests = std::collections::BTreeSet::new();
    let mut owed = vec![];
    for grant in grants.iter().filter(|g| !g.present) {
        if !digests.insert(grant.digest.as_str()) {
            continue;
        }
        let (Some(file), Some(target)) = (files.iter().find(|f| f.path == grant.path), &grant.upload) else {
            return Err(refuse("release_grant_invalid", format!("the Hub granted {} without an upload", grant.path)));
        };
        owed.push((file.path.clone(), file.local.clone(), target.clone()));
    }
    let pace = (Duration::from_millis(250), Duration::from_secs(32));
    let sends = owed.into_iter().map(|(path, local, target)| {
        let http = http.clone();
        async move { put(&http, &path, &local, &target, pace, say).await }
    });
    futures_util::stream::iter(sends)
        .buffer_unordered(16)
        .try_fold(0, |sum, sent| async move { Ok::<u64, Error>(sum + sent) })
        .await
}

/// PUTs the file at `local` to a content-addressed `target`, whose signed `If-None-Match: *`
/// makes a re-send safe. A send that ends with no answer, or with the store's 5xx (transient by
/// design, as S3's SlowDown), goes again after a pause that doubles from `first` to `ceiling`
/// and starts over whenever a send carries the body further than any before. It gives up only
/// when a whole such run passes with no send getting further: the store took nothing more.
/// Another 4xx is final, and 412 is the object already there (0 bytes sent).
async fn put(
    http: &reqwest::Client,
    path: &str,
    local: &Path,
    target: &PresignedUpload,
    (first, ceiling): (Duration, Duration),
    say: Say<'_>,
) -> Result<u64, Error> {
    let failed = |message: String| Error::new(Kind::Unavailable, "release_upload_failed", message);
    let (mut furthest, mut pause, mut sends) = (0, first, 0u32);
    loop {
        let file = tokio::fs::File::open(local).await.map_err(|e| refuse("release_file_unreadable", e))?;
        let length = file.metadata().await.map_err(|e| refuse("release_file_unreadable", e))?.len();
        let carried = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut request =
            http.put(&target.url).header(reqwest::header::CONTENT_LENGTH, length).body(counted(file, carried.clone()));
        for (name, value) in &target.required_headers {
            request = request.header(name, value);
        }
        sends += 1;
        let why = match request.send().await {
            Ok(answer) => match answer.status().as_u16() {
                200..=299 => return Ok(length),
                412 => return Ok(0),
                status @ 500..=599 => format!("the store answered HTTP {status}"),
                status => return Err(failed(format!("{path}: the store answered HTTP {status}"))),
            },
            Err(e) => chain(&e),
        };
        let reached = carried.load(std::sync::atomic::Ordering::Relaxed).min(length);
        if reached > furthest {
            (furthest, pause) = (reached, first);
        } else if pause > ceiling {
            return Err(failed(format!(
                "{path}: {why}; {sends} sends, none carrying more than {furthest} of its {length} bytes since a pause of {}s",
                ceiling.as_secs_f64()
            )));
        }
        say(&format!(
            "{path}: {why}, after {reached} of {length} bytes; sending it again in {:.2}s",
            pause.as_secs_f64()
        ));
        tokio::time::sleep(pause).await;
        pause *= 2;
    }
}

/// The file as a body, adding each part's length to `carried` as the transport takes it.
fn counted(file: tokio::fs::File, carried: Arc<std::sync::atomic::AtomicU64>) -> reqwest::Body {
    let parts = tokio_util::io::ReaderStream::with_capacity(file, 1 << 20);
    reqwest::Body::wrap_stream(parts.inspect(move |part| {
        if let Ok(part) = part {
            carried.fetch_add(part.len() as u64, std::sync::atomic::Ordering::Relaxed);
        }
    }))
}

/// An error and each of its sources, as `error sending request: …: connection reset by peer`.
fn chain(error: &dyn std::error::Error) -> String {
    let mut said = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if !said.contains(&text) {
            said.push_str(": ");
            said.push_str(&text);
        }
        source = cause.source();
    }
    said
}

/// A git source as the lock spells it: `https://host/repo?rev=<rev>#<commit>`.
struct Vcs {
    url: String,
    requested: Option<String>,
    commit: String,
}

impl Vcs {
    fn parse(locked: &str) -> Self {
        let (head, commit) = locked.split_once('#').unwrap_or((locked, ""));
        let (url, query) = head.split_once('?').unwrap_or((head, ""));
        let requested = query.split('&').find_map(|pair| pair.strip_prefix("rev=")).map(str::to_owned);
        Self { url: url.to_owned(), requested, commit: commit.to_owned() }
    }
}

/// The wheels a release carries for its git dependencies. Machines install everything else straight
/// from the release's `uv.lock`, by the URLs and hashes locked there; a git source has no wheel to
/// lock, so publish builds it at its pinned commit and the machine installs it by its Hub digest.
fn git_wheels(stage: &Stage, python: &str) -> Result<Vec<PathBuf>, Error> {
    let text = fs::read_to_string(stage.src.join("uv.lock")).map_err(|e| refuse("lock_failed", e))?;
    let lock: Lock = toml::from_str(&text).map_err(|e| refuse("lock_invalid", e))?;
    (lock.package.iter())
        .filter_map(|p| Some((p, Vcs::parse(p.source.get("git")?.as_str()?))))
        .map(|(p, vcs)| git_wheel(stage, python, &p.name.to_ascii_lowercase(), &vcs))
        .collect()
}

/// A git dependency pinned to a full commit, built to a wheel whose build tag names its bytes:
/// `<name>-<version>-0<12 hex>-<tags>.whl`. A branch or tag could move, so it is refused.
fn git_wheel(stage: &Stage, python: &str, name: &str, vcs: &Vcs) -> Result<PathBuf, Error> {
    let commit = vcs.commit.as_str();
    let pinned = commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit());
    if !pinned || vcs.requested.as_deref() != Some(commit) {
        return Err(Error::new(
            Kind::Validation,
            "git_dependency_unpinned",
            format!("{name} is locked to git {} at a revision that can move", vcs.url),
        )
        .remedy(format!("depend on `{name} @ git+{}@<40-hex commit>`", vcs.url)));
    }
    let checkout = stage.root.join("git").join(name);
    fs::create_dir_all(&checkout).map_err(|e| refuse("git_dependency_checkout", e))?;
    let git = |args: &[&str]| -> Result<String, Error> {
        let out = std::process::Command::new("git").arg("-C").arg(&checkout).args(args).output().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => git_missing(),
            _ => refuse("git_dependency_checkout", e),
        })?;
        match out.status.success() {
            true => Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned()),
            false => Err(refuse("git_dependency_fetch", String::from_utf8_lossy(&out.stderr))),
        }
    };
    git(&["init", "-q"])?;
    git(&["fetch", "-q", "--depth", "1", vcs.url.as_str(), commit])?;
    git(&["checkout", "-q", "FETCH_HEAD"])?;
    let epoch = git(&["log", "-1", "--format=%ct"])?;
    let out = stage.root.join("git-wheels").join(name);
    run(
        uv(&stage.uv, &checkout)
            .args(["build", "--wheel", "--no-progress", "--python", python, "--out-dir"])
            .arg(&out)
            .env("SOURCE_DATE_EPOCH", &epoch),
        "git_dependency_build",
    )?;
    let built = single(&out, ".whl")?;
    let bytes = fs::read(&built).map_err(|e| refuse("git_dependency_wheel", e))?;
    let tag = &hex::encode(Sha256::digest(&bytes))[..12];
    let file = built.file_name().and_then(|f| f.to_str()).unwrap_or_default();
    let mut parts: Vec<&str> = file.splitn(3, '-').collect();
    let renamed = match parts.len() {
        3 => {
            let build = format!("0{tag}");
            parts.insert(2, &build);
            parts.join("-")
        }
        _ => return Err(refuse("git_dependency_wheel", format!("{file} is not a wheel name"))),
    };
    let target = out.join(renamed);
    fs::rename(&built, &target).map_err(|e| refuse("git_dependency_wheel", e))?;
    Ok(target)
}

/// The one file with `suffix` in `dir`.
fn single(dir: &Path, suffix: &str) -> Result<PathBuf, Error> {
    let found: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| refuse("build_output_unreadable", e))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.to_string_lossy().ends_with(suffix))
        .collect();
    match <[PathBuf; 1]>::try_from(found) {
        Ok([path]) => Ok(path),
        Err(found) => Err(refuse(
            "build_output_unexpected",
            format!("uv build left {} {suffix} files in {}", found.len(), dir.display()),
        )),
    }
}
