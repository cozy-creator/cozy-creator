//! A dev home's packages checkout (xs-195; Paul 11-13: nothing untested goes to the Hub, so RC testing runs packages from
//! source). `[calcifer] packages` names it, read only with `dev = true` on an RC build (`0.26.0-dev+g…`). Each first-party
//! package it holds, a top-level folder whose pyproject.toml declares a `cozy.application`, is local code wherever its
//! unversioned name is asked for (`qwen-image-2/generate`): `local_code::target` answers its folder, so runs, descriptions, a
//! project's renders and the page's tools take `cozy run ./dir`'s one path. A release named (`qwen-image-2@0.10.0`) and what
//! it lacks come from the Hub. Its packages call each other from it too: a `{ index = "tensorhub" }` source it holds is
//! staged as a path to its folder, as cargo's `[patch]` does (`stage`).
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{LazyLock, Mutex, OnceLock},
    time::{Duration, Instant},
};

use api::{
    calcifer::StagedInput,
    code::{self, LocalCode, Staged},
    config::Config,
    error::{Error, Kind},
    ids::Digest,
    names::{ROOT, shown},
    target::Published,
    tree,
};
use tracing::info;

use crate::local_code::{INDEX, distribution};

static CHECKOUT: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Reads the checkout `config` names, once, as Calcifer starts.
pub(crate) fn start(config: &Config) {
    let named = config.calcifer.packages.as_ref();
    let root = named.filter(|_| config.dev && crate::update::rc_version()).map(|root| fs::canonicalize(root).unwrap_or_else(|_| root.clone()));
    match (&root, named) {
        (Some(root), _) => info!("first-party packages run from source: {}", root.display()),
        (None, Some(named)) => info!("[calcifer] packages = {} is read only with dev = true on an RC build: packages come from the Hub", named.display()),
        (None, None) => {}
    }
    let _ = CHECKOUT.set(root);
}

fn root() -> Option<&'static Path> {
    CHECKOUT.get()?.as_deref()
}

/// Whether this dev home runs first-party packages from a checkout: then `local` follows the RC's Runtime too.
pub(crate) fn configured() -> bool {
    root().is_some()
}

/// How long a read of which packages the checkout holds stands: a project's read asks of each of its boxes.
const LISTED_FOR: Duration = Duration::from_secs(5);
type Listed = Option<(Instant, Vec<(String, PathBuf)>)>;
static LISTED: Mutex<Listed> = Mutex::new(None);

/// Each first-party package the checkout holds: its name as written (`flux.3-klein`) and its folder.
pub(crate) fn packages() -> Vec<(String, PathBuf)> {
    let Some(root) = root() else { return vec![] };
    let mut listed = LISTED.lock().expect("listed lock");
    if let Some((_, found)) = listed.as_ref().filter(|(at, _)| at.elapsed() < LISTED_FOR) {
        return found.clone();
    }
    let mut found: Vec<(String, PathBuf)> =
        fs::read_dir(root).into_iter().flatten().flatten().filter_map(|entry| Some((application(&entry.path())?, entry.path()))).collect();
    found.sort();
    *listed = Some((Instant::now(), found.clone()));
    found
}

/// Whether the checkout holds the first-party package `package` (`_/qwen-image-2`, `qwen-image-2`): what then isn't the Hub's.
pub(crate) fn holds(package: &str) -> bool {
    let name = match package.split_once('/') {
        None => package,
        Some((org, name)) if org == ROOT => name,
        Some(_) => return false,
    };
    let wanted = distribution(name);
    packages().iter().any(|(held, _)| distribution(held) == wanted)
}

/// The name a folder's pyproject.toml gives the package it is, lowercase, when it declares a `cozy.application`. One it can't
/// read is said: its package would be the Hub's.
fn application(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join("pyproject.toml")).ok()?;
    let table: toml::Table = toml::from_str(&text)
        .inspect_err(|e| tracing::warn!("{}: its pyproject.toml doesn't read, so its package comes from the Hub: {e}", dir.display()))
        .ok()?;
    let project = table.get("project")?;
    project.get("entry-points")?.get("cozy.application")?;
    Some(project.get("name")?.as_str()?.trim().to_ascii_lowercase())
}

/// The checkout's folder of the package `text` names, unversioned and first-party, and the function it names. None: the Hub's.
pub(crate) fn named(text: &str) -> Option<(PathBuf, Option<String>)> {
    root()?;
    let text = text.trim();
    if api::target::written_as_path(text) || text.ends_with(".py") {
        return None;
    }
    let Published { package, function, release, .. } = Published::parse(text).ok()?;
    let wanted = distribution(package.strip_prefix(&format!("{ROOT}/"))?);
    if release.is_some() {
        return None;
    }
    packages().into_iter().find(|(name, _)| distribution(name) == wanted).map(|(_, dir)| (dir, function))
}

/// The first-party name (`_/qwen-image-2`) of the checkout's package at `path`, one of its top-level folders.
pub(crate) fn package_at(path: &Path) -> Option<String> {
    let root = root()?;
    let path = fs::canonicalize(path).ok()?;
    (path.parent()? == root).then_some(())?;
    Some(format!("{ROOT}/{}", application(&path)?))
}

/// The checkout's commit, as git says it: its branch (`rc/0.26.0`), the commit and whether any file changed since or is new.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Head {
    pub branch: String,
    pub commit: String,
    pub dirty: bool,
}

fn head(root: &Path) -> Option<Head> {
    // No optional locks: a read here never holds the index while rc-follow fast-forwards.
    let out = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain=v2", "--branch", "--untracked-files=normal"])
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut said = Head { branch: String::new(), commit: String::new(), dirty: false };
    for line in text.lines() {
        match (line.strip_prefix("# branch.oid "), line.strip_prefix("# branch.head ")) {
            (Some(commit), _) => said.commit = commit.trim().to_owned(),
            (_, Some(branch)) => said.branch = branch.trim().to_owned(),
            _ if !line.starts_with('#') => said.dirty = true,
            _ => {}
        }
    }
    (!said.commit.is_empty()).then_some(said)
}

/// How long one read of the checkout's commit stands for the packages a list reads at once.
const HEAD_FOR: Duration = Duration::from_secs(1);
static HEAD_READ: Mutex<Option<(Instant, Option<Head>)>> = Mutex::new(None);

/// The checkout's commit now, read at most once a [`HEAD_FOR`].
fn head_now(root: &Path) -> Option<Head> {
    let mut read = HEAD_READ.lock().expect("head lock");
    if let Some((_, head)) = read.as_ref().filter(|(at, _)| at.elapsed() < HEAD_FOR) {
        return head.clone();
    }
    let head = head(root);
    *read = Some((Instant::now(), head.clone()));
    head
}

/// Each package folder's tree admitted as independent source, at the clean commit it was read at.
static ADMITTED: LazyLock<Mutex<HashMap<PathBuf, (Head, LocalCode)>>> = LazyLock::new(Mutex::default);

/// The tree admitted for `dir` when the checkout is still at that clean commit: nothing to read or hash again.
pub(crate) fn unchanged(dir: &Path) -> Option<LocalCode> {
    let head = head_now(root()?).filter(|head| !head.dirty)?;
    let admitted = ADMITTED.lock().expect("admitted lock");
    let (at, code) = admitted.get(dir)?;
    (*at == head).then(|| code.clone())
}

/// Keeps `code` as `dir`'s admitted tree at the commit it was staged at, when that was clean.
pub(crate) fn admitted(dir: &Path, code: &LocalCode) {
    if let Some(head) = of(code).and_then(|from| from.head).filter(|head| !head.dirty) {
        ADMITTED.lock().expect("admitted lock").insert(dir.to_owned(), (head, code.clone()));
    }
}

/// Where code staged from the checkout came from: its package, the commit, and the checkout's packages it calls, by distribution.
#[derive(Clone, Debug)]
pub(crate) struct From {
    /// `_/qwen-image-2`.
    pub package: String,
    pub head: Option<Head>,
    /// Each `{ index = "tensorhub" }` source staged as a path to its folder here, as the package it is: `minimax-h3` → `_/minimax-h3`.
    pub callees: BTreeMap<String, String>,
}

impl From {
    /// `rc/0.26.0 @ 1a2b3c4d`: what a run and the page say it ran.
    pub(crate) fn said(&self) -> String {
        match &self.head {
            Some(head) => {
                let changed = if head.dirty { " with uncommitted changes" } else { "" };
                format!("{} @ {}{changed}", head.branch, &head.commit[..head.commit.len().min(8)])
            }
            None => "a folder git doesn't read".into(),
        }
    }

    /// `qwen-image-2 from source: rc/0.26.0 @ 1a2b3c4d`.
    pub(crate) fn line(&self) -> String {
        format!("{} from source: {}", shown(&self.package), self.said())
    }
}

/// What each tree staged from the checkout came from, by its digest and place: a run of it says so.
static STAGED: LazyLock<Mutex<HashMap<(Digest, String), From>>> = LazyLock::new(Mutex::default);

/// Where `code` came from, when it was staged from the checkout.
pub(crate) fn of(code: &LocalCode) -> Option<From> {
    STAGED.lock().expect("staged lock").get(&(code.tree, code.at.clone())).cloned()
}

/// The checkout's package at `dir` as one tree, at one commit: read again when the checkout moved while it was read (rc-follow's
/// fast-forward), so a tree is never half one commit. `memo` remembers file digests; `patched` holds a pyproject.toml whose
/// `{ index = "tensorhub" }` sources became paths (`patch`).
pub(crate) fn stage(dir: &Path, memo: Option<&Path>, patched: &Path) -> Result<Staged, Error> {
    let root = root().ok_or_else(|| Error::internal("checkout_unset", "no packages checkout is named"))?;
    let package = package_at(dir).ok_or_else(|| Error::new(Kind::NotFound, "checkout_package_unknown", format!("{} is no package of {}", dir.display(), root.display())))?;
    for _ in 0..3 {
        let before = head(root);
        let (staged, callees) = staged(root, dir, memo, patched)?;
        if head(root) == before {
            let from = From { package, head: before, callees };
            STAGED.lock().expect("staged lock").insert((Digest::of(&staged.document), staged.at.clone()), from);
            return Ok(staged);
        }
    }
    Err(Error::new(Kind::Unavailable, "checkout_moving", format!("{} moved each time {} was read", root.display(), dir.display())).remedy("run it again"))
}

/// `dir`'s tree, and the checkout's packages it calls by distribution. With none, the tree `cozy run ./dir` stages; else the
/// checkout's folders it calls (and they call) beside it, each pyproject.toml naming them by path.
fn staged(root: &Path, dir: &Path, memo: Option<&Path>, patched: &Path) -> Result<(Staged, BTreeMap<String, String>), Error> {
    let folders: BTreeMap<String, PathBuf> = packages().into_iter().map(|(name, folder)| (distribution(&name), folder)).collect();
    let name_of = |folder: &Path| folder.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    // Each folder of the closure, with the sources of it that become paths: (source key, the folder it names).
    let mut closure: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut callees = BTreeMap::new();
    let mut next = vec![dir.to_path_buf()];
    while let Some(folder) = next.pop() {
        let at = name_of(&folder);
        if closure.contains_key(&at) {
            continue;
        }
        let mut patches = vec![];
        for key in hub_sources(&folder) {
            let Some(callee) = folders.get(&distribution(&key)) else { continue };
            if let Some(package) = application(callee) {
                callees.insert(distribution(&key), format!("{ROOT}/{package}"));
            }
            patches.push((key, name_of(callee)));
            next.push(callee.clone());
        }
        closure.insert(at, patches);
    }
    if callees.is_empty() {
        return Ok((code::stage(dir, memo)?, callees));
    }
    let unreadable = |path: &Path, e: std::io::Error| Error::new(Kind::Validation, "project_unreadable", format!("{}: {e}", path.display()));
    let (mut members, mut files, mut left) = (BTreeMap::new(), vec![], vec![]);
    for (at, patches) in &closure {
        let folder = root.join(at);
        for relative in code::source_files(&folder, &mut left)? {
            let mut path = folder.join(&relative);
            let named = format!("{at}/{}", relative.iter().map(|part| part.to_string_lossy()).collect::<Vec<_>>().join("/"));
            let (digest, length) = match relative.as_os_str() == "pyproject.toml" && !patches.is_empty() {
                true => {
                    let text = fs::read_to_string(&path).map_err(|e| unreadable(&path, e))?;
                    let bytes = patch(&text, patches)?.into_bytes();
                    let digest = Digest::of(&bytes);
                    path = patched.join(format!("{}.toml", digest.to_string().trim_start_matches("sha256:")));
                    if !path.is_file() {
                        fs::create_dir_all(patched).and_then(|()| fs::write(&path, &bytes)).map_err(|e| unreadable(&path, e))?;
                    }
                    (digest, bytes.len() as u64)
                }
                false => (api::digests::of(memo, &path)?, fs::metadata(&path).map_err(|e| unreadable(&path, e))?.len()),
            };
            members.insert(named, tree::Member { length, sha256: hex::encode(digest.bytes()), executable: code::executable(&path) });
            files.push(StagedInput { path, digest, length });
        }
    }
    let document = tree::document(&members);
    tree::parse(&document).map_err(|e| Error::new(Kind::Validation, "project_unreadable", e.to_string()))?;
    Ok((Staged { document, files, at: name_of(dir), left }, callees))
}

/// The keys of `folder`'s `[tool.uv.sources]` that take a cozy package from the Hub (`{ index = "tensorhub" }`).
fn hub_sources(folder: &Path) -> Vec<String> {
    let Some(table) = fs::read_to_string(folder.join("pyproject.toml")).ok().and_then(|text| toml::from_str::<toml::Table>(&text).ok()) else {
        return vec![];
    };
    let sources = table.get("tool").and_then(|tool| tool.get("uv")).and_then(|uv| uv.get("sources")).and_then(toml::Value::as_table);
    let hub = |source: &toml::Value| source.get("index").and_then(toml::Value::as_str) == Some(INDEX);
    (sources.into_iter().flatten())
        .filter(|(_, source)| match source {
            toml::Value::Array(alternatives) => alternatives.iter().any(hub),
            source => hub(source),
        })
        .map(|(key, _)| key.clone())
        .collect()
}

/// `pyproject` with each `(key, folder)` source a path to that folder beside it: `minimax-h3 = { path = "../minimax-h3" }`.
fn patch(pyproject: &str, patches: &[(String, String)]) -> Result<String, Error> {
    let mut doc: toml_edit::DocumentMut = pyproject.parse().map_err(|e| Error::new(Kind::Validation, "project_file_invalid", format!("pyproject.toml: {e}")))?;
    let sources = (doc.get_mut("tool").and_then(|tool| tool.get_mut("uv")))
        .and_then(|uv| uv.get_mut("sources"))
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or_else(|| Error::internal("checkout_patch", "pyproject.toml lost its [tool.uv.sources]"))?;
    for (key, folder) in patches {
        let mut path = toml_edit::InlineTable::new();
        path.insert("path", format!("../{folder}").into());
        sources.insert(key, toml_edit::value(path));
    }
    Ok(doc.to_string())
}
