//! Local code: a package directory or a script nobody published, run where a published function would be (`cozy run ./dir`, a
//! project's `with: ./dir/fn`). It comes by content (cl-819): a client sends its tree (`api::code`), and a project's own
//! folder is read into one here. Calcifer works from its copy of the tree, kept by the tree's digest, and reads no client's disk.
//! A machine installs it from TensorD's `LocalSource`, a manifest naming each source file by its content; the interface its
//! install answers is kept by the manifest and the Runtime that read it, so the same code on the same Runtime is called at once.
//! A script is a one-file package, its PEP 723 block its pyproject.toml. Local code is locked as `cargo run` locks (`locked`),
//! whatever its sources (a Hub index, a path, git), before it is described or installed. Every machine, this computer's
//! too, is sent the files by content, and Calcifer writes only the bytes it lacks: no machine reads a host path (cl-817). Every client and Calcifer's projects use this one module (cl-716).
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Display,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, SystemTime},
};

use platform::{durable, paths::Home};
use api::{
    calcifer::{Kind as RunKind, On, Run, StagedInput, Status, SubmitRun},
    code::{self as shared, LocalCode},
    config::Origin,
    error::{Error, Kind},
    ids::Digest,
    machine as v1,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) use api::code::source_files;

use crate::{api::Api, rentals::Hubs};

/// The uv index name a project's sources use for cozy packages on the Hub: its account's own, else the root's (`bindings`).
pub(crate) const INDEX: &str = "tensorhub";

/// A distribution name as uv and the Hub's index compare it (PEP 503): `Qwen_Image.2` is `qwen-image-2`.
pub(crate) fn distribution(name: &str) -> String {
    name.split(['-', '_', '.']).filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-").to_ascii_lowercase()
}

pub(crate) fn refuse(code: &str, detail: impl Display) -> Error {
    Error::new(Kind::Validation, code, detail.to_string())
}

fn hub_refusal(error: ::api::hub::Error) -> Error {
    error.refusal(|| "cozy login <email>".into())
}

/// The account `client` is signed in as.
pub(crate) async fn account(client: &::api::hub::Client) -> Result<String, Error> {
    Ok(client.get_current_account().await.map_err(hub_refusal)?.name)
}

/// Local code as a run names it: its source (the manifest's digest), the files Calcifer stages for
/// it (the manifest, kept in Calcifer's home, and the source files a machine elsewhere is sent),
/// and the `local/<name>` package it is.
pub(crate) struct Source {
    pub package: String,
    pub source: v1::LocalSource,
    pub inputs: Vec<StagedInput>,
    /// What the machine's environment is built from, apart from the project's own source: its
    /// metadata and lock, its path dependencies, carried wheels and indexes.
    pub environment: Digest,
}

/// What an install answered: the code's interface, the machine's name for the installation, and
/// the machine it is on.
pub(crate) struct Installed {
    pub interface: Value,
    pub installation: Option<String>,
    pub machine: Option<String>,
}

/// Where manifests are kept for their calls to stage: Calcifer's home, the newest 256.
fn keep_manifest(home: &Home, digest: &Digest, document: &[u8]) -> Result<PathBuf, Error> {
    const KEPT: usize = 256;
    let dir = crate::private_io::cache(crate::private_io::current().as_ref(), home.calcifer().join("local"), "local")?;
    let path = dir.join(format!("{}.json", digest.to_string().trim_start_matches("sha256:")));
    let failed = |e: platform::Error| refuse("stage_unavailable", format!("{}: {e}", dir.display()));
    durable::private_dir(&dir).map_err(failed)?;
    if fs::read(&path).ok().as_deref() != Some(document) {
        durable::replace(&path, document, 0o600).map_err(failed)?;
    }
    let mut kept: Vec<(std::time::SystemTime, PathBuf)> = (fs::read_dir(&dir).into_iter().flatten().flatten())
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    kept.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, old) in kept.into_iter().skip(KEPT) {
        let _ = fs::remove_file(old);
    }
    Ok(path)
}

pub(crate) use api::target::local;

/// Local code a project's text names (`./dir[/fn]`, `./script.py[/fn]` from its folder, `~/…`, an absolute path): a folder of
/// Calcifer's own computer, as the project is, and the function it names. None: `text` is not written as a path (a published
/// name). A client's own code comes by content instead (`copied`).
pub(crate) fn target(text: &str, cwd: Option<&Path>, home: &Path) -> Option<Result<(PathBuf, Option<String>), Error>> {
    let raw = text.trim();
    let path = local(raw, cwd.unwrap_or(Path::new("")), home)?;
    if !path.is_absolute() {
        let said = format!("{raw} is relative, and names no folder of the project's");
        return Some(Err(Error::new(Kind::Usage, "target_relative", said).remedy("name local code by its path from the project's folder")));
    }
    Some(shared::split(&path).ok_or_else(|| {
        Error::new(Kind::NotFound, "target_not_found", format!("{raw} is not a package directory or a script beside the project"))
    }))
}

/// Where copies of local code are kept, by their tree's digest.
fn copies(home: &Home) -> Result<PathBuf, Error> {
    crate::private_io::cache(crate::private_io::current().as_ref(), home.calcifer().join("code"), "code")
}

/// Calcifer's copy of `code`'s tree (`copied` names the package in it): what a lock stages, path dependencies and all.
pub(crate) fn copy_of(home: &Home, code: &LocalCode) -> Result<PathBuf, Error> {
    Ok(copies(home)?.join(hex(&code.tree)))
}

/// How many copies are kept, at least, and for how long one unused stays.
const COPIES: usize = 64;
const UNUSED: Duration = Duration::from_secs(24 * 60 * 60);

/// The package `code` names, in Calcifer's copy of its tree: made once from the files it holds (`PUT /v1/files`), and only read
/// after. A script's copy holds the package it is beside its tree. Its project's folder.
pub(crate) fn copied(home: &Home, code: &LocalCode) -> Result<PathBuf, Error> {
    let dir = copies(home)?.join(hex(&code.tree));
    if !dir.join("tree").is_dir() {
        let scope = crate::private_io::current();
        let held = |digest: &Digest| match &scope {
            Some(scope) => crate::runs::files::held_for(&scope.storage.view_root, digest),
            None => crate::runs::files::held(digest),
        };
        let not_held = |what: String| {
            Error::new(Kind::NotFound, "file_not_held", format!("Calcifer holds no {what}")).remedy("PUT its bytes to /v1/files/sha256:<hex> first")
        };
        let document = held(&code.tree).ok_or_else(|| not_held(format!("tree document {}", code.tree)))?;
        let document = fs::read(&document).map_err(|e| refuse("code_unreadable", e))?;
        if Digest::of(&document) != code.tree {
            return Err(refuse("code_invalid", format!("the file held as {} is not that tree", code.tree)));
        }
        let members = api::tree::parse(&document).map_err(|e| refuse("code_invalid", e))?;
        if let Some((name, member)) = members.iter().find(|(_, member)| held(&digest_of(member)).is_none()) {
            return Err(not_held(format!("{name} of local code (sha256:{})", member.sha256)));
        }
        materialize(home, &code.tree, &members, held)?;
    }
    package_in(&dir, &code.at)
}

/// A folder of Calcifer's own (a project's `with: ./dir`) as local code: read as a client reads its own, and copied as a
/// client's is, so the one path from here on is the copy's.
pub(crate) fn own(home: &Home, path: &Path) -> Result<LocalCode, Error> {
    let staged = shared::stage(path, Some(&crate::private_io::cache(crate::private_io::current().as_ref(), home.root().join("cache").join("digests"), "digests")?))?;
    if !staged.left.is_empty() && crate::private_io::current().is_none() {
        tracing::info!("left out of {}: {}", path.display(), staged.left.join(", "));
    }
    let code = staged.code();
    let members = api::tree::parse(&staged.document).map_err(|e| refuse("code_invalid", e))?;
    let from = |digest: &Digest| staged.files.iter().find(|file| file.digest == *digest).map(|file| file.path.clone());
    if !copies(home)?.join(hex(&code.tree)).join("tree").is_dir() {
        materialize(home, &code.tree, &members, from)?;
    }
    Ok(code)
}

/// The tree's files, each from `from`, copied under `code/<digest>/tree`, made whole and then named; and a copy unused for a day
/// past the newest `COPIES` goes.
pub(crate) fn materialize(home: &Home, tree: &Digest, members: &BTreeMap<String, api::tree::Member>, from: impl Fn(&Digest) -> Option<PathBuf>) -> Result<(), Error> {
    let root = copies(home)?;
    let dir = root.join(hex(tree));
    let failed = |e: std::io::Error| refuse("code_unwritable", format!("{}: {e}", root.display()));
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let part = root.join(format!("{}.part-{}-{nanos}", hex(tree), std::process::id()));
    durable::private_dir(&root).map_err(|e| refuse("code_unwritable", format!("{}: {e}", root.display())))?;
    for (name, member) in members {
        let to = part.join("tree").join(name);
        let source = from(&digest_of(member)).ok_or_else(|| refuse("code_unreadable", format!("{name}: its bytes are gone")))?;
        fs::create_dir_all(to.parent().expect("a member's folder")).map_err(failed)?;
        fs::copy(source, &to).map_err(failed)?;
        mode(&to, member.executable).map_err(failed)?;
    }
    fs::create_dir_all(part.join("tree")).map_err(failed)?;
    match fs::rename(&part, &dir) {
        Ok(()) => {}
        // Another made it first: the same bytes.
        Err(_) if dir.join("tree").is_dir() => {
            let _ = fs::remove_dir_all(&part);
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&part);
            return Err(failed(e));
        }
    }
    sweep(&root);
    Ok(())
}

fn digest_of(member: &api::tree::Member) -> Digest {
    format!("sha256:{}", member.sha256).parse().expect("a tree names sha256 hex")
}

#[cfg(unix)]
fn mode(path: &Path, executable: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(if executable { 0o700 } else { 0o600 }))
}

#[cfg(not(unix))]
fn mode(_path: &Path, _executable: bool) -> std::io::Result<()> {
    Ok(())
}

/// The project's folder `at` names in the copy `dir`, said used now: a script's is the package it is, written beside the tree.
fn package_in(dir: &Path, at: &str) -> Result<PathBuf, Error> {
    let _ = fs::write(dir.join(".used"), b"");
    let package = match at.is_empty() {
        true => dir.join("tree"),
        false => dir.join("tree").join(at),
    };
    match package.is_file() {
        true => script(&package, &dir.join("script")),
        false if package.join("pyproject.toml").is_file() => Ok(package),
        false => Err(Error::new(Kind::NotFound, "project_file_missing", format!("{at}: no pyproject.toml"))
            .remedy("run this in a package's directory (`cozy-runtime new <name>` makes one)")),
    }
}

/// Copies past the newest `COPIES` unused for a day, and parts a day old.
fn sweep(root: &Path) {
    let used = |dir: &Path| fs::metadata(dir.join(".used")).or_else(|_| fs::metadata(dir)).and_then(|m| m.modified()).ok();
    let mut kept: Vec<(SystemTime, PathBuf)> = (fs::read_dir(root).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter_map(|dir| Some((used(&dir)?, dir)))
        .collect();
    kept.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    let old = SystemTime::now() - UNUSED;
    for (at, dir) in kept.into_iter().skip(COPIES).filter(|(at, _)| *at < old) {
        let _ = (at, fs::remove_dir_all(dir));
    }
}

/// A package project, as its pyproject.toml says.
pub(crate) struct Project {
    pub root: PathBuf,
    pub name: String,
    pub version: String,
    /// Whether `version` is written in pyproject.toml, as a release needs.
    pub identified: bool,
    pub requires_python: String,
    /// The distributions its uv sources take from the `tensorhub` index: cozy packages on the Hub.
    pub hub_sources: BTreeSet<String>,
    /// A dependency or uv source fetched with git: locking and publishing need git.
    pub git: bool,
}

impl Project {
    pub(crate) fn read(root: PathBuf) -> Result<Self, Error> {
        let table = |file: &str| -> Result<toml::Table, Error> {
            let text = fs::read_to_string(root.join(file)).map_err(|e| {
                Error::new(
                    Kind::NotFound,
                    "project_file_missing",
                    format!("{file}: {e}"),
                )
                .remedy("run this in a package's directory (`cozy-runtime new <name>` makes one)")
            })?;
            toml::from_str(&text)
                .map_err(|e| refuse("project_file_invalid", format!("{file}: {e}")))
        };
        let pyproject = table("pyproject.toml")?;
        let field = |path: &[&str]| -> Option<&toml::Value> {
            let (first, rest) = path.split_first()?;
            rest.iter()
                .try_fold(pyproject.get(*first)?, |at, key| at.get(*key))
        };
        let text = |path: &[&str]| field(path).and_then(toml::Value::as_str).map(str::to_owned);
        // A dynamic version runs as local code all the same; only a release needs it (`identified`).
        let Some(name) = text(&["project", "name"]) else {
            return Err(refuse(
                "project_identity_missing",
                "pyproject.toml's [project] needs a literal name",
            ));
        };
        let literal = text(&["project", "version"]);
        let identified = literal.is_some();
        let version = literal.unwrap_or_else(|| "0.0.0".to_owned());
        let indexes = field(&["tool", "uv", "index"]).and_then(toml::Value::as_array);
        if indexes
            .into_iter()
            .flatten()
            .any(|i| i.get("name").and_then(|n| n.as_str()) == Some(INDEX))
        {
            return Err(refuse(
                "account_index_declared",
                "pyproject.toml declares the `tensorhub` index; cozy writes it for the Hub and account",
            )
            .remedy("delete that [[tool.uv.index]] table and keep `{ index = \"tensorhub\" }` in [tool.uv.sources]"));
        }
        let sources = field(&["tool", "uv", "sources"]).and_then(toml::Value::as_table);
        let names_index =
            |source: &toml::Value| source.get("index").and_then(|i| i.as_str()) == Some(INDEX);
        let hub_sources = (sources.into_iter().flat_map(|s| s.iter()))
            .filter(|(_, source)| match source {
                toml::Value::Array(alternatives) => alternatives.iter().any(names_index),
                source => names_index(source),
            })
            .map(|(name, _)| distribution(name))
            .collect();
        let requirements = field(&["project", "dependencies"])
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten();
        let git_source = |source: &toml::Value| source.get("git").is_some();
        let git = requirements
            .filter_map(toml::Value::as_str)
            .any(|r| r.contains("git+"))
            || sources
                .into_iter()
                .flat_map(|s| s.values())
                .any(|source| match source {
                    toml::Value::Array(alternatives) => alternatives.iter().any(git_source),
                    source => git_source(source),
                });
        Ok(Self {
            root,
            // The package keeps its name as written (`qwen3.5`); the Hub resolves PEP 503 spellings.
            name: name.trim().to_ascii_lowercase(),
            version,
            identified,
            requires_python: text(&["project", "requires-python"]).unwrap_or_default(),
            hub_sources,
            git,
        })
    }
}

/// The account's index on `hub` at the origin the Hub advertises (RFC 9728 `resource`): the one
/// its publish check holds a uv.lock to, wherever this computer reaches the Hub. A Hub that
/// advertises none takes no publication, and its configured origin stands.
pub(crate) async fn index_url(
    client: &::api::hub::Client,
    hub: &Origin,
    account: &str,
) -> Result<String, Error> {
    let origin = match client.get_protected_resource_metadata().await {
        Ok(metadata) => metadata.resource,
        Err(e) => match hub_refusal(e) {
            e if e.code == "capability.not_served" => hub.to_string(),
            e => return Err(e),
        },
    };
    Ok(format!(
        "{}/v1/index/{account}/simple/",
        origin.trim_end_matches('/')
    ))
}

#[derive(Serialize)]
struct Manifest {
    package: String,
    release: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    python_requires: String,
    files: Vec<Member>,
    #[serde(skip_serializing_if = "String::is_empty")]
    project: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    locals: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    callees: BTreeMap<String, String>,
    /// Wheels a path source names (`{ path = "dist/x.whl" }`), carried: no index the machine
    /// reaches serves them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    wheels: Vec<Member>,
}

#[derive(Serialize)]
struct Member {
    name: String,
    digest: String,
    length: u64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    executable: bool,
}

/// `code` as a machine installs it: its manifest and every file of Calcifer's copy, sent by content, locked for `hub` first
/// (`locked`), so the machine installs what the lock names.
pub(crate) async fn source(home: &Home, code: &LocalCode, hubs: &Hubs, hub: &Origin) -> Result<Source, Error> {
    let (project_dir, _) = locked(home, code, hubs, hub).await?;
    let project = Project::read(project_dir.clone())?;
    let locals = shared::path_dependencies(&project_dir)?;
    let root = shared::common(
        project_dir.as_path(),
        locals.iter().map(|l| {
            if l.is_file() {
                l.parent().unwrap_or(l)
            } else {
                l
            }
        }),
    );
    let relative = |p: &Path| {
        p.strip_prefix(&root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    };
    // The copy holds only what goes: nothing is left out of it.
    let mut left = vec![];
    let mut names: Vec<PathBuf> = source_files(&project_dir, &mut left)?
        .into_iter()
        .map(|f| project_dir.join(f))
        .collect();
    for local in &locals {
        match local.is_file() {
            true => names.push(local.clone()),
            false => names.extend(source_files(local, &mut left)?.into_iter().map(|f| local.join(f))),
        }
    }
    // A path source inside the project (`deps/x.whl`) is one of its files already: listed twice,
    // its wheel was carried twice and the machine's second copy was refused.
    let mut seen = std::collections::HashSet::new();
    names.retain(|name| seen.insert(name.clone()));
    let carried = |file: &Path| {
        locals.iter().any(|l| l == file) && file.extension().is_some_and(|e| e == "whl")
    };
    let (mut files, mut wheels, mut inputs) = (vec![], vec![], vec![]);
    for file in names {
        let bytes = fs::read(&file).map_err(|e| refuse("project_unreadable", e))?;
        let digest = Digest::of(&bytes);
        let length = bytes.len() as u64;
        files.push(Member {
            name: relative(&file),
            digest: digest.to_string(),
            length,
            executable: shared::executable(&file),
        });
        // A carried wheel is an object the machine holds.
        if carried(&file) {
            let name = file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            wheels.push(Member {
                name,
                digest: digest.to_string(),
                length,
                executable: false,
            });
        }
        inputs.push(StagedInput {
            path: file,
            digest,
            length,
        });
    }
    let manifest = Manifest {
        package: format!("local/{}", project.name),
        release: project.version.clone(),
        python_requires: project.requires_python.clone(),
        files,
        project: relative(&project_dir),
        locals: locals.iter().map(|l| relative(l)).collect(),
        callees: callees(&project_dir)?,
        wheels,
    };
    let own = match manifest.project.is_empty() {
        true => String::new(),
        false => format!("{}/", manifest.project),
    };
    let builds = |m: &&Member| {
        let beside = !m.name.starts_with(&own)
            || manifest
                .locals
                .iter()
                .any(|l| m.name == *l || m.name.starts_with(&format!("{l}/")));
        beside || m.name == format!("{own}pyproject.toml") || m.name == format!("{own}uv.lock")
    };
    let built: Vec<(&str, &str)> = (manifest.files.iter().filter(builds))
        .chain(&manifest.wheels)
        .map(|m| (m.name.as_str(), m.digest.as_str()))
        .collect();
    let environment = serde_json::json!({"files": built, "python": manifest.python_requires});
    let environment = Digest::of(environment.to_string().as_bytes());
    let document = serde_json::to_vec(&manifest).map_err(|e| refuse("local_source_invalid", e))?;
    let digest = Digest::of(&document);
    let written = keep_manifest(home, &digest, &document)?;
    inputs.push(StagedInput {
        path: written,
        digest,
        length: document.len() as u64,
    });
    Ok(Source {
        package: manifest.package,
        source: v1::LocalSource {
            manifest: digest.to_string(),
        },
        inputs,
        environment,
    })
}

/// What a lock of `code`'s copy for `hub` leaves beside the copy (`locked`): `lock`, the lock it made or nothing when the tree's
/// own stood, and with one made, `copy`, the tree again with that lock in the package's folder. The copy itself stays the tree
/// as it was sent, which a lock or publication asked of it reads.
fn lock_dir(home: &Home, code: &LocalCode, hub: &Origin) -> Result<PathBuf, Error> {
    let key = Digest::of(format!("{}\n{}", hub.as_str(), code.at).as_bytes());
    Ok(copy_of(home, code)?.join(format!("locked-{}", &hex(&key)[..16])))
}

/// The tree of Calcifer's copy `within` (and a script's package beside it), copied under `to`: not what Calcifer keeps beside.
pub(crate) fn copy_tree(within: &Path, to: &Path) -> Result<(), Error> {
    let files = source_files(within, &mut vec![])?;
    for file in files.into_iter().filter(|file| file.starts_with("tree") || file.starts_with("script")) {
        let at = to.join(&file);
        fs::create_dir_all(at.parent().unwrap_or(to))
            .and_then(|()| fs::copy(within.join(&file), &at))
            .map_err(|e| refuse("code_unwritable", format!("{}: {e}", at.display())))?;
    }
    Ok(())
}

/// One lock of a copy at a time: any other waits and takes what it made.
static LOCKING: LazyLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Mutex::default);

/// Local code locked as `cargo run` locks (cl-871, cl-878): every project gets the uv.lock uv makes, or brings up to date keeping
/// the tree's own pins, for `hub` (`hub::author::locked`), whatever its sources name (a Hub index, a path dependency beside it in
/// the tree, git), before anything describes or installs it. Its cozy dependencies are then read and called at the releases or
/// sources it names (`callees`). Once per copy and Hub, beside the copy (`lock_dir`). A lock uv can't make fails the run in uv's
/// words. The package's folder to describe and install, and the lock when it is not the tree's own: its client writes it into
/// the package's folder (`made`).
pub(crate) async fn locked(home: &Home, code: &LocalCode, hubs: &Hubs, hub: &Origin) -> Result<(PathBuf, Option<String>), Error> {
    let sent = copied(home, code)?;
    let within = copy_of(home, code)?;
    let done = lock_dir(home, code, hub)?;
    let turn = LOCKING.lock().expect("locking lock").entry(done.clone()).or_default().clone();
    let _turn = turn.lock().await;
    if !done.is_dir() {
        let project = Project::read(sent.clone())?;
        let say = |line: &str| tracing::info!("{}: {line}", project.name);
        let made = crate::hub::author::locked(hubs, hub, &project, &within, false, &[], &say).await?;
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let part = done.with_extension(format!("part-{}-{nanos}", std::process::id()));
        let failed = |e: std::io::Error| refuse("code_unwritable", format!("{}: {e}", part.display()));
        let kept = if made.changed { made.lock.as_str() } else { "" };
        fs::create_dir_all(&part).and_then(|()| fs::write(part.join("lock"), kept)).map_err(failed)?;
        if made.changed {
            copy_tree(&within, &part.join("copy"))?;
            let at = sent.strip_prefix(&within).unwrap_or(Path::new(""));
            fs::write(part.join("copy").join(at).join("uv.lock"), &made.lock).map_err(failed)?;
        }
        if let Err(e) = fs::rename(&part, &done) {
            let _ = fs::remove_dir_all(&part);
            return Err(failed(e));
        }
    }
    let made = fs::read_to_string(done.join("lock")).ok().filter(|made| !made.is_empty());
    let dir = match made.is_some() {
        true => done.join("copy").join(sent.strip_prefix(&within).unwrap_or(Path::new(""))),
        false => sent,
    };
    Ok((dir, made))
}

/// The lock `locked` made for `code` at `hub`, when it is not the tree's own: what its client writes into the package's folder.
pub(crate) fn made(home: &Home, code: &LocalCode, hub: &Origin) -> Option<String> {
    fs::read_to_string(lock_dir(home, code, hub).ok()?.join("lock")).ok().filter(|made| !made.is_empty())
}

/// The distributions the lock takes from a Hub index, as the packages they are
/// (`qwen-image-2` → `fidika/qwen-image-2`): their callables are that package's.
fn callees(project: &Path) -> Result<BTreeMap<String, String>, Error> {
    #[derive(Deserialize)]
    struct Lock {
        #[serde(default)]
        package: Vec<Locked>,
    }
    #[derive(Deserialize)]
    struct Locked {
        name: String,
        #[serde(default)]
        source: toml::Table,
    }
    let Ok(text) = fs::read_to_string(project.join("uv.lock")) else {
        return Ok(BTreeMap::new());
    };
    let lock: Lock = toml::from_str(&text).map_err(|e| refuse("lock_invalid", e))?;
    let callee = |p: &Locked| {
        let registry = p.source.get("registry")?.as_str()?;
        let org = registry
            .split_once("/v1/index/")?
            .1
            .strip_suffix("/simple/")?;
        Some((p.name.clone(), format!("{org}/{}", p.name)))
    };
    Ok(lock.package.iter().filter_map(callee).collect())
}

/// A script as the one-file package it is (xs-099): its PEP 723 block is its pyproject.toml (`dependencies` and
/// `requires-python` are [project]'s, [tool] is [tool]), named for the file, its module `cozy_script_<name>`, so `copy.py`
/// isn't Python's `copy`, with its `app`. The project is written at `root`, beside the copy's tree, and from there it is read
/// as any package directory is.
fn script(path: &Path, root: &Path) -> Result<PathBuf, Error> {
    let raw = fs::read(path).map_err(|e| refuse("script_unreadable", e))?;
    let mut block = shared::pep723(&String::from_utf8_lossy(&raw))?;
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let name: String = (stem.chars())
        .map(|c| {
            if c.is_ascii_alphanumeric() || ".-_".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    let module: String = (stem.chars())
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let module = format!("cozy_script_{module}");
    let file = format!("{module}.py");
    let table = |text: String| toml::from_str::<toml::Table>(&text).expect("a constant table");
    let mut project = table(format!(
        "name = \"{name}\"\nversion = \"0.0.0\"\n\
         entry-points.\"cozy.application\".default = \"{module}:app\"\n"
    ));
    for field in ["requires-python", "dependencies"] {
        if let Some(value) = block.remove(field) {
            project.insert(field.into(), value);
        }
    }
    let mut tool = match block.remove("tool") {
        Some(toml::Value::Table(tool)) => tool,
        None => toml::Table::new(),
        Some(_) => {
            return Err(refuse(
                "script_metadata_invalid",
                "the `# /// script` block's tool is not a table",
            ));
        }
    };
    if let Some(other) = block.keys().next().filter(|_| crate::private_io::current().is_none()) {
        tracing::warn!("the `# /// script` block names {other:?}: it takes dependencies, requires-python and [tool]; ignored");
    }
    // A path source names a directory beside the script: relative to the written project
    // instead, so the project and its libraries travel to a machine as one tree.
    let sources = tool
        .get_mut("uv")
        .and_then(|u| u.get_mut("sources"))
        .and_then(|s| s.as_table_mut());
    for (_, source) in sources.into_iter().flat_map(|s| s.iter_mut()) {
        if let Some(beside) = source.get("path").and_then(|p| p.as_str()) {
            let absolute = fs::canonicalize(path.parent().unwrap_or(path).join(beside))
                .map_err(|e| refuse("local_dependency_unreadable", format!("{beside}: {e}")))?;
            source["path"] = from(root, &absolute).into();
        }
    }
    // Hatchling builds the one module, and admits git or URL dependencies as uv locks them.
    let hatch = table(format!(
        "metadata.allow-direct-references = true\n\
         build.targets.wheel.only-include = [\"{file}\"]\n"
    ));
    tool.insert("hatch".into(), hatch.into());
    let mut pyproject = table(
        "build-system = { requires = [\"hatchling>=1.25\"], build-backend = \"hatchling.build\" }"
            .into(),
    );
    pyproject.insert("project".into(), project.into());
    pyproject.insert("tool".into(), tool.into());
    let pyproject =
        toml::to_string(&pyproject).map_err(|e| refuse("script_metadata_invalid", e))?;
    fs::create_dir_all(root).map_err(|e| refuse("script_unwritable", e))?;
    for (name, bytes) in [
        (file.as_str(), raw.as_slice()),
        ("pyproject.toml", pyproject.as_bytes()),
    ] {
        if fs::read(root.join(name)).ok().as_deref() != Some(bytes) {
            fs::write(root.join(name), bytes).map_err(|e| refuse("script_unwritable", e))?;
        }
    }
    Ok(root.to_owned())
}

/// `to` as a slash path relative to the directory `dir`.
fn from(dir: &Path, to: &Path) -> String {
    let shared = dir
        .components()
        .zip(to.components())
        .take_while(|(a, b)| a == b)
        .count();
    let up = dir.components().count() - shared;
    let down = to
        .components()
        .skip(shared)
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    std::iter::repeat_n("..".to_owned(), up)
        .chain(down)
        .collect::<Vec<_>>()
        .join("/")
}

/// Where local code's interfaces are kept, beside release cards: one file per source manifest
/// and Runtime that described it, named for its environment's inputs first, so a changed source
/// with the same environment is seen.
fn dir(home: &Home, filed: &Filed) -> Result<PathBuf, Error> {
    let scope = match &filed.project {
        Some(root) => crate::private_io::of_output(&crate::project::landing(root))?,
        None => crate::private_io::current(),
    };
    let mut dir = crate::private_io::cache(scope.as_ref(), home.root().join("packages"), "packages")?;
    dir.extend(filed.package.split('/'));
    Ok(dir)
}

fn file(home: &Home, filed: &Filed, runtime: &str) -> Result<PathBuf, Error> {
    let manifest = filed.manifest.trim_start_matches("sha256:");
    let runtime = &hex(&Digest::of(runtime.as_bytes()))[..16];
    Ok(dir(home, filed)?.join(format!("{}-{manifest}-{runtime}.json", &hex(&filed.environment)[..16])))
}

/// What a kept interface is filed under: its package, its source's manifest and its environment's inputs. An install's step
/// (`hub::after::After::Keep`) carries it, so whatever asked for the install, a run or `cozy package install`, keeps the same.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Filed {
    pub package: String,
    pub manifest: String,
    pub environment: Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<PathBuf>,
}

impl Source {
    pub(crate) fn filed(&self) -> Filed {
        Filed { package: self.package.clone(), manifest: self.source.manifest.clone(), environment: self.environment,
            project: crate::private_io::current().map(|scope| crate::project::vault::logical(&scope.storage.view_root)) }
    }
}

fn hex(digest: &Digest) -> String {
    digest.to_string().trim_start_matches("sha256:").to_owned()
}

/// The interface kept for this exact source, as `runtime` described it.
pub(crate) fn kept(home: &Home, filed: &Filed, runtime: &str) -> Option<Value> {
    let document: Value = serde_json::from_slice(&fs::read(file(home, filed, runtime).ok()?).ok()?).ok()?;
    interface(&document).then_some(document)
}

/// Whether some kept source of this package has the same environment inputs: a changed source
/// is a sync, not an install.
fn synced(home: &Home, filed: &Filed) -> bool {
    let prefix = format!("{}-", &hex(&filed.environment)[..16]);
    (dir(home, filed).ok().and_then(|dir| fs::read_dir(dir).ok()).into_iter().flatten().flatten())
        .any(|e| e.file_name().to_string_lossy().starts_with(&prefix))
}

/// Keeps `interface` for this source as `runtime` described it, and only the package's newest 16.
pub(crate) fn keep(home: &Home, filed: &Filed, runtime: &str, interface: &Value) {
    const KEPT: usize = 16;
    if !self::interface(interface) {
        return;
    }
    let (Ok(dir), Ok(path)) = (dir(home, filed), file(home, filed, runtime)) else { return };
    let kept = durable::private_dir(&dir)
        .and_then(|()| durable::replace(&path, interface.to_string().as_bytes(), 0o600));
    // An interface that cannot be kept is asked again next time; the run goes on.
    drop(kept);
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = (fs::read_dir(&dir).into_iter().flatten().flatten())
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, old) in files.into_iter().skip(KEPT) {
        let _ = fs::remove_file(old);
    }
}

fn interface(document: &Value) -> bool {
    api::interface::format(document).starts_with("cozy.package.interface/")
}

/// The run that installs local code `on` a machine: a WARM run naming no function. It is named
/// for what the machine does: an install when the environment's inputs (metadata, lock, path
/// dependencies) are new here, else a sync of the changed source.
pub(crate) fn install_run(home: &Home, hub: &str, on: On, source: &Source) -> SubmitRun {
    let target = match synced(home, &source.filed()) {
        true => format!("sync {} (source changed)", source.package),
        false => format!("install {}", source.package),
    };
    let spec = v1::RunSpec {
        kind: v1::RunKind::Warm,
        source: Some(v1::Source::Local(source.source.clone())),
        ..Default::default()
    };
    SubmitRun {
        agreed: None,
        stays: vec![],
        on,
        hub: hub.to_owned(),
        kind: RunKind::Warm,
        target,
        spec,
        inputs: source.inputs.clone(),
        held: vec![],
        out: crate::private_io::current().map(|scope| crate::project::landing(&scope.storage.view_root)),
        warnings: vec![],
    }
}

/// What a settled install answered, or why it installed nothing.
pub(crate) fn installed(run: Run) -> Result<Installed, Error> {
    if run.status != Status::Completed {
        let (code, message, next) = run.reason.map_or_else(
            || ("local_install_unfinished".to_owned(), format!("its install is {:?}", run.status).to_lowercase(), vec![]),
            |r| (r.code, r.message, r.next),
        );
        let failed = Error::new(Kind::Unavailable, "local_install_failed", format!("the machine did not install this code: {message}")).cause(code);
        return Err(next.into_iter().chain([format!("cozy run watch {}", run.number)]).fold(failed, Error::next));
    }
    let mut result = run.result.unwrap_or_default();
    if !interface(&result["interface"]) {
        return Err(Error::new(
            Kind::Unavailable,
            "local_interface_unreadable",
            "the machine's description of this code is not a package interface",
        ));
    }
    let installation = result["installation"].as_str().map(str::to_owned);
    let machine = run
        .machine
        .ok_or_else(|| Error::new(Kind::Internal, "local_install_unplaced", "the install ran on no machine"))?;
    Ok(Installed { interface: result["interface"].take(), installation, machine: Some(machine) })
}

/// Local code made callable `on` a machine: its source and the files a call stages, and its
/// interface (kept for this source and Runtime, or what its install answered). While an install
/// it asked for is going, `install` is that run and `interface` is None: follow it, then ask again.
pub(crate) struct Prepared {
    pub package: String,
    pub source: v1::LocalSource,
    pub files: Vec<StagedInput>,
    pub interface: Option<Value>,
    /// The machine's name for the installation, when an install answered here.
    pub installation: Option<String>,
    /// Where it was installed, when an install answered here.
    pub machine: Option<String>,
    pub install: Option<Run>,
}

/// `code` made callable `on` a machine, never waiting for an install: the install it
/// needs is sent under `{key}/install` and returned, and once it has settled the same `key` finds
/// its answer. `warm` installs again whatever is kept: a warm set names the machine's
/// installation, which only an install answers.
pub(crate) async fn prepare(
    api: &Arc<Api>,
    local: &LocalCode,
    on: &On,
    hub: &str,
    warm: bool,
    key: &str,
    agreed: Option<api::calcifer::Agreed>,
) -> Result<Prepared, Error> {
    let hubs = &api.machines.hubs;
    let home = hubs.home().clone();
    let origin = Origin::parse(&if hub.is_empty() { hubs.default_origin() } else { hub.to_owned() })?;
    let code = source(&home, local, hubs, &origin).await?;
    let runtime = runtime(api, on).await;
    let prepared = |interface, installation, machine, install| Prepared {
        package: code.package.clone(),
        source: code.source.clone(),
        files: code.inputs.clone(),
        interface,
        installation,
        machine,
        install,
    };
    // By its manifest: code that changes during an install is another install, not that one's answer.
    let key = format!("{key}/install/{}", code.source.manifest);
    if sent(api, &key)?.is_none()
        && let Some(interface) = runtime.as_deref().filter(|_| !warm).and_then(|runtime| kept(&home, &code.filed(), runtime))
    {
        return Ok(prepared(Some(interface), None, None, None));
    }
    let submit = || SubmitRun { agreed: agreed.clone(), ..install_run(&home, origin.as_str(), on.clone(), &code) };
    let after = crate::private_io::current().is_none().then(|| crate::hub::after::After::Keep(code.filed()));
    match install_through(api, &key, after.as_ref(), submit).await? {
        Install::Going(run) => Ok(prepared(None, None, None, Some(run))),
        Install::Ended(run) => {
            let installed = installed(run)?;
            if let Some(runtime) = &runtime {
                keep(&home, &code.filed(), runtime, &installed.interface);
            }
            Ok(prepared(Some(installed.interface), installed.installation, installed.machine, None))
        }
    }
}

/// How far an install has got.
pub(crate) enum Install {
    /// It is going: follow it, then ask again.
    Going(Run),
    Ended(Run),
}

/// The install `submit` makes, under `key`, as far as it has got, with `after` its step once its machine completes it.
pub(crate) async fn install_through(api: &Arc<Api>, key: &str, after: Option<&crate::hub::after::After>, submit: impl FnOnce() -> SubmitRun) -> Result<Install, Error> {
    let Some(run) = sent(api, key)? else {
        if let Some(after) = after {
            api.calcifer.plan(key, after)?;
        }
        let run = api.calcifer.submit(key, submit()).await.map_err(|e| crate::api::Refusal::from(e).0)?.run;
        return Ok(Install::Going(run));
    };
    match run.status {
        Status::Completed | Status::Failed | Status::Canceled => Ok(Install::Ended(run)),
        _ => Ok(Install::Going(run)),
    }
}

/// `prepare`, waiting out the install it needs: what Calcifer's own callers (a project's render blocks) use.
pub(crate) async fn ready(api: &Arc<Api>, code: &LocalCode, on: &On, hub: &str, key: &str) -> Result<Prepared, Error> {
    loop {
        let prepared = prepare(api, code, on, hub, false, key, None).await?;
        let Some(run) = &prepared.install else { return Ok(prepared) };
        crate::project::host::Host::new(api).wait(&run.id).await?;
    }
}

/// The run sent under `key`, if one was.
fn sent(api: &Api, key: &str) -> Result<Option<Run>, Error> {
    let calcifer = &api.calcifer;
    let row = calcifer.store().keyed(key).map_err(|e| crate::api::Refusal::from(crate::Error::from(e)).0)?;
    row.map(|row| calcifer.record(&row).map_err(|e| crate::api::Refusal::from(e).0)).transpose()
}

/// The Runtime the machine `on` describes code with, as it last said; none for a machine not
/// chosen yet, or one that has not answered.
async fn runtime(api: &Arc<Api>, on: &On) -> Option<String> {
    let On::Machine(name) = on else { return None };
    let machine = api.machines.show(name).await.ok()?;
    machine.status.map(|status| status.runtime).filter(|runtime| !runtime.is_empty())
}

/// `uv` (cozy's pinned uv, else PATH's: `tools::uv`) in `dir`, quiet and colorless.
pub(crate) fn uv(uv: &Path, dir: &Path) -> Command {
    let mut command = Command::new(uv);
    command.current_dir(dir).env("NO_COLOR", "1");
    match crate::private_io::of_path(dir) {
        Ok(Some(scope)) => {
            let scratch = scope.storage.view_root.join(".vault-calcifer");
            let temp = scratch.join("tmp");
            if fs::create_dir_all(&temp).is_err() {
                command.current_dir(temp.join("unavailable"));
            }
            command.env("UV_CACHE_DIR", scratch.join("uv-cache"))
                .env("UV_TOOL_DIR", scratch.join("uv-tools"))
                .env("XDG_CACHE_HOME", scratch.join("cache"))
                .env("PYTHONPYCACHEPREFIX", scratch.join("bytecode"))
                .env("MPLCONFIGDIR", scratch.join("matplotlib"))
                .env("TMPDIR", &temp).env("TEMP", &temp).env("TMP", &temp)
                .env("PYTHONDONTWRITEBYTECODE", "1");
        }
        Err(_) => { command.current_dir(dir.join("private-view-unavailable")); }
        Ok(None) => {},
    }
    command
}

/// Runs `command`, which a person knows as `what` (`uv lock`): its stdout. A failure is refused as `code` in one line, how
/// it ended and its last words on stderr; the rest of its stderr goes to Calcifer's log.
pub(crate) fn run(command: &mut Command, code: &str, what: &str) -> Result<String, Error> {
    run_said(command, code, what).map_err(|(error, _)| error)
}

/// `run`, a failure with the tool's whole stderr beside its refusal (what `uv_listed` reads).
pub(crate) fn run_said(command: &mut Command, code: &str, what: &str) -> Result<String, (Error, String)> {
    let out = command.output().map_err(|e| {
        let error = match e.kind() {
            std::io::ErrorKind::NotFound => Error::new(
                Kind::Unavailable,
                "uv_unavailable",
                format!("{} is gone", command.get_program().to_string_lossy()),
            )
            .remedy("run it again: cozy fetches uv when it is missing")
            .next("cozy doctor"),
            _ => refuse(code, format!("{what} did not start: {e}")),
        };
        (error, String::new())
    })?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    let (ended, killed) = platform::process::ended(&out.status);
    let refusal = runtime_refusal(&said);
    let words = refusal.as_ref().map(|refusal| refusal.error.message.clone())
        .or_else(|| platform::process::last_line(&said));
    let message = match (words, killed) {
        (Some(words), _) => format!("{what} {ended}: {words}"),
        (None, true) => format!("{what} {ended} before it answered"),
        (None, false) => format!("{what} {ended} and gave no reason on stderr"),
    };
    let kind = if killed { Kind::Unavailable } else { Kind::Validation };
    let mut error = Error::new(kind, code, message);
    if let Some(refusal) = refusal {
        error.remedy = refusal.error.remedy.filter(|remedy| !remedy.trim().is_empty());
        for next in refusal.next.into_iter().filter(|next| !next.trim().is_empty()) {
            error = error.next(next);
        }
    }
    if error.next.is_empty() {
        error = error.next("cozy machine logs local calcifer");
    }
    Err((error, said))
}

#[derive(Deserialize)]
struct RuntimeRefusal {
    error: RuntimeDiagnostic,
    #[serde(default)]
    next: Vec<String>,
}

#[derive(Deserialize)]
struct RuntimeDiagnostic {
    #[serde(rename = "name")]
    _name: String,
    #[serde(rename = "code")]
    _code: i32,
    message: String,
    #[serde(default)]
    remedy: Option<String>,
}

/// Runtime's typed refusal may follow uv's progress output. Keep its first diagnostic
/// line and recovery advice; the full record is already retained in the Calcifer log.
fn runtime_refusal(said: &str) -> Option<RuntimeRefusal> {
    let lines: Vec<&str> = said.lines().collect();
    for at in (0..lines.len()).rev().filter(|&at| lines[at].trim_start().starts_with('{')) {
        let rest = lines[at..].join("\n");
        let Some(Ok(mut refusal)) = serde_json::Deserializer::from_str(&rest).into_iter::<RuntimeRefusal>().next() else { continue };
        let Some(message) = refusal.error.message.lines().map(str::trim).find(|line| !line.is_empty()) else { continue };
        refusal.error.message = message.to_owned();
        return Some(refusal);
    }
    None
}

/// The Python a release of `project` runs on: `.python-version`, else requires-python's floor;
/// its exact version as uv finds it (installing a managed one when none is here).
pub(crate) fn python(project: &Project, uv: &Path) -> Result<Python, Error> {
        let pinned = fs::read_to_string(project.root.join(".python-version")).ok();
        let floor = project.requires_python.split(',').find_map(|clause| {
            let version = clause
                .trim()
                .strip_prefix(">=")
                .or_else(|| clause.trim().strip_prefix("~="))?;
            Some(
                version
                    .trim()
                    .splitn(3, '.')
                    .take(2)
                    .collect::<Vec<_>>()
                    .join("."),
            )
        });
        let request = pinned
            .as_deref()
            .and_then(|text| text.lines().next())
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .or(floor)
            .unwrap_or_else(|| "3.12".into());
        let find = || {
            run(
                self::uv(uv, &project.root).args([
                    "python",
                    "find",
                    "--show-version",
                    request.as_str(),
                ]),
                "python_not_found",
                "uv python find",
            )
        };
        let full = match find() {
            Ok(version) => version,
            Err(_) => {
                run(
                    self::uv(uv, &project.root).args(["python", "install", request.as_str()]),
                    "python_install_failed",
                    "uv python install",
                )?;
                find()?
            }
        };
        Ok(Python {
            request,
            full: full.trim().to_owned(),
        })
    }

/// The Python a release runs on.
pub(crate) struct Python {
    /// What uv is asked for (`3.12`).
    pub request: String,
    /// The exact version a release records (`3.12.12`).
    pub full: String,
}

/// The cozy-runtime release `root`'s uv.lock holds, if it holds one.
pub(crate) fn locked_runtime(root: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct Lock {
        #[serde(default)]
        package: Vec<Locked>,
    }
    #[derive(Deserialize)]
    struct Locked {
        name: String,
        #[serde(default)]
        version: String,
    }
    let lock: Lock = toml::from_str(&fs::read_to_string(root.join("uv.lock")).ok()?).ok()?;
    lock.package.into_iter().find(|p| p.name == "cozy-runtime").map(|p| p.version)
}

/// The SDK is always cozy-runtime: an authoritative lock, else plain unconditional version
/// bounds. URLs, markers and unspecified versions retain the newest-SDK fallback; extras
/// belong to the package, not its static reader. Never forward a project's source as the SDK.
fn describer_requirement(dir: &Path, runtime: Option<&str>) -> Result<String, Error> {
    if let Some(version) = runtime {
        return Ok(format!("cozy-runtime=={version}"));
    }
    let text = fs::read_to_string(dir.join("pyproject.toml"))
        .map_err(|e| refuse("project_file_missing", format!("pyproject.toml: {e}")))?;
    let document: toml::Value = toml::from_str(&text)
        .map_err(|e| refuse("project_file_invalid", format!("pyproject.toml: {e}")))?;
    let dependencies = document.get("project").and_then(|p| p.get("dependencies")).and_then(toml::Value::as_array);
    let mut specifiers = Vec::new();
    for requirement in dependencies.into_iter().flatten().filter_map(toml::Value::as_str) {
        let requirement = requirement.trim();
        let end = requirement.find(|c: char| !c.is_ascii_alphanumeric() && !"-_.".contains(c)).unwrap_or(requirement.len());
        let (name, mut bounds) = requirement.split_at(end);
        // PEP 503: case-insensitive names with runs of '-', '_' and '.' folded to '-'.
        let normalized = name.split(['-', '_', '.']).filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-").to_ascii_lowercase();
        if !name.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
            || !name.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
            || normalized != "cozy-runtime"
        {
            continue;
        }
        bounds = bounds.trim();
        if let Some(extras) = bounds.strip_prefix('[') {
            let Some((_, rest)) = extras.split_once(']') else { continue };
            bounds = rest.trim();
        }
        if bounds.is_empty() || bounds.starts_with('@') || bounds.contains(';') {
            continue;
        }
        let bounds = bounds.strip_prefix('(').and_then(|s| s.strip_suffix(')')).unwrap_or(bounds).trim();
        let parsed: pep440_rs::VersionSpecifiers = bounds.parse()
            .map_err(|e| refuse("runtime_requirement_invalid", format!("cozy-runtime version bounds: {e}")))?;
        specifiers.extend(parsed);
    }
    let bounds: pep440_rs::VersionSpecifiers = specifiers.into_iter().collect();
    Ok(if bounds.is_empty() {
        // A bare name would take whatever cozy-runtime `uv tool install` left here.
        "cozy-runtime@latest".into()
    } else {
        format!("cozy-runtime{bounds}")
    })
}

/// Runtime's source-only description on `python`: the locked SDK, else the newest within
/// declared plain bounds, else PyPI's newest. No package code runs and no project is locked.
pub(crate) fn describe_with(uv: &Path, dir: &Path, python: &str, runtime: Option<&str>) -> Result<(String, Value), Error> {
    let from = describer_requirement(dir, runtime)?;
    let mut command = self::uv(uv, dir);
    command.args(["tool", "run", "--no-progress", "--python", python, "--from", from.as_str()]);
    if runtime.is_none() && from != "cozy-runtime@latest" {
        // Refresh within the bounds, ignoring an older installed tool even when it meets them.
        command.arg("--isolated").args(platform::tools::uv_upgrade(["cozy-runtime"]));
    }
    let said = run(
        command
            .args(["cozy-runtime", "--json", "--dir"])
            .arg(dir)
            .arg("describe"),
        "describe_failed",
        "cozy-runtime describe",
    )?;
    let document: Value = serde_json::from_str(&said).map_err(|e| refuse("describe_failed", format!("cozy-runtime describe answered no interface: {e}")))?;
    if !interface(&document) {
        return Err(refuse("package_interface_invalid", "the describer's answer is not a package interface"));
    }
    Ok((said.trim_end().to_owned(), document))
}

/// What each copy of local code last read as, by the Runtime that read it, and whether it's its machine's own reading. One read
/// at a time of each: any other waits and takes it. A copy never changes, so its reading stands for as long as Calcifer runs, but
/// a static describer's only until the machine's own is kept.
type Described = Arc<tokio::sync::Mutex<Option<((String, Value), bool)>>>;
static DESCRIBED: LazyLock<Mutex<HashMap<(LocalCode, Option<String>), Described>>> = LazyLock::new(Mutex::default);

/// The Runtime the machine `on` last said it runs, asking it nothing: no machine starts for a read.
fn last_runtime(api: &Api, on: &On) -> Option<String> {
    let On::Machine(name) = on else { return None };
    let row = api.machines.named(name).ok()??;
    let status = api.machines.picture(&row).ok()?.status?;
    Some(status.runtime).filter(|runtime| !runtime.is_empty())
}

/// `code` as a run `on` its machine reads it: `local/<name>` and its interface. What that machine's Runtime said installing this
/// very source, else Runtime's static describer, using its lock, plain version bounds or PyPI's newest. That can
/// be older than the machine's, and refuse what the machine runs. No package code runs and no machine is asked.
pub(crate) async fn describe(api: &Arc<Api>, code: &LocalCode, on: &On, hub: &str) -> Result<(String, Value), Error> {
    let runtime = last_runtime(api, on);
    let held = if crate::private_io::current().is_some() {
        copied(api.machines.hubs.home(), code)?;
        Described::default()
    } else {
        DESCRIBED.lock().expect("described lock").entry((code.clone(), runtime.clone())).or_default().clone()
    };
    let mut held = held.lock().await;
    if let Some((known, own)) = held.as_ref() {
        if *own {
            return Ok(known.clone());
        }
        // Read by the static describer: the machine's own reading, once an install keeps one, takes its place.
        let known = known.clone();
        if let Some(found) = own_reading(api, code, hub, runtime.as_deref()).await {
            *held = Some((found.clone(), true));
            return Ok(found);
        }
        return Ok(known);
    }
    if let Some(found) = own_reading(api, code, hub, runtime.as_deref()).await {
        *held = Some((found.clone(), true));
        return Ok(found);
    }
    let known = read_interface(api, code, on, hub, runtime).await?;
    *held = Some((known.clone(), false));
    Ok(known)
}

/// What the machine's Runtime said installing this very source, if it's kept here.
async fn own_reading(api: &Arc<Api>, local: &LocalCode, hub: &str, runtime: Option<&str>) -> Option<(String, Value)> {
    let hubs = &api.machines.hubs;
    let home = hubs.home();
    let origin = Origin::parse(hub).ok()?;
    let code = source(home, local, hubs, &origin).await.ok()?;
    Some((code.package.clone(), kept(home, &code.filed(), runtime?)?))
}

/// Runtime's static reading on the project's Python and locked SDK, declared plain bounds or newest fallback, of the copy locked
/// for `hub` first (`locked`).
async fn read_interface(api: &Arc<Api>, code: &LocalCode, on: &On, hub: &str, runtime: Option<String>) -> Result<(String, Value), Error> {
    let hubs = &api.machines.hubs;
    let home = hubs.home();
    let origin = Origin::parse(&if hub.is_empty() { hubs.default_origin() } else { hub.to_owned() })?;
    let (dir, _) = locked(home, code, hubs, &origin).await?;
    let project = Project::read(dir)?;
    let uv = crate::tools::uv(home, &|line| tracing::info!("{line}")).await?;
    let python = python(&project, &uv)?;
    let locked = locked_runtime(&project.root);
    let access = match crate::private_io::current() {
        Some(scope) => {
            let root = crate::project::vault::logical(&scope.storage.view_root);
            let access = crate::project::vault::admit_root(&root).await?;
            let current = crate::private_io::of_output(&crate::project::landing(&root))?;
            if current.is_none_or(|current| current.storage != scope.storage) {
                return Err(refuse("project_view_changed", "the mounted view changed while describing local code"));
            }
            Some(access)
        }
        None => None,
    };
    let (root, asked, wanted) = (project.root.clone(), python.request.clone(), locked.clone());
    let described = tokio::task::spawn_blocking(move || {
        let _access = access;
        describe_with(&uv, &root, &asked, wanted.as_deref())
    })
        .await
        .map_err(|why| refuse("describe_failed", why))?;
    let described = described.map_err(|error| match (&runtime, &locked, on) {
        // A describer killed (`Kind::Unavailable`) refused nothing.
        (Some(runtime), None, On::Machine(name)) if error.code == "describe_failed" && error.kind != Kind::Unavailable => {
            let mut error = error;
            if error.remedy.is_none() {
                error.remedy = Some(format!("The Runtime describing this source refused it, and {name} runs {runtime}: a run there reads it with that one, and this then answers what it read"));
            }
            error.next(format!("cozy run <its folder> --on {name}"))
        }
        _ => error,
    });
    Ok((format!("local/{}", project.name), described?.1))
}

#[cfg(all(test, unix))]
#[path = "local_code_tests.rs"]
mod tests;
