//! A dev home's packages checkout (xs-195; Paul 11-13: nothing untested goes to the Hub, so RC testing runs packages from
//! source). `[calcifer] packages` names it, read only with `dev = true` on an RC build (`0.26.0-dev+g…`). Each first-party
//! package it holds, a top-level folder whose pyproject.toml declares a `cozy.application`, is local code wherever its
//! unversioned name is asked for (`qwen-image-2/generate`): `local_code::target` answers its folder, so runs, descriptions, a
//! project's renders and the page's tools take `cozy run ./dir`'s one path. A release named (`qwen-image-2@0.10.0`) and what
//! it lacks come from the Hub. Its packages call each other from it too: a `{ index = "tensorhub" }` source it holds goes in the
//! tree and is locked as a path to its folder, as cargo's `[patch]` does (`api::code::siblings`, `local_code::locked`), wherever
//! a package of a checkout runs.
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{LazyLock, Mutex, OnceLock},
    time::{Duration, Instant},
};

use api::{
    code::{self, LocalCode, Staged},
    config::Config,
    error::{Error, Kind},
    ids::Digest,
    names::{ROOT, shown},
    target::Published,
};
use tracing::info;

use crate::local_code::distribution;

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

pub(crate) fn root() -> Option<&'static Path> {
    CHECKOUT.get()?.as_deref()
}

/// The checkout changed: which packages it holds and its commit are read again.
pub(crate) fn moved() {
    *LISTED.lock().expect("listed lock") = None;
    *HEAD_READ.lock().expect("head lock") = None;
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
pub(crate) fn application(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join("pyproject.toml")).ok()?;
    let table: toml::Table = toml::from_str(&text)
        .inspect_err(|e| tracing::warn!("{}: its pyproject.toml doesn't read, so its package comes from the Hub: {e}", dir.display()))
        .ok()?;
    let project = table.get("project")?;
    project.get("entry-points")?.get("cozy.application")?;
    Some(project.get("name")?.as_str()?.trim().to_ascii_lowercase())
}

/// What the package in `dir` does in a line, its pyproject's description, as the Hub keeps a release's (`Card::summary`): "" for none.
pub(crate) fn description(dir: &Path) -> String {
    let table = fs::read_to_string(dir.join("pyproject.toml")).ok().and_then(|text| toml::from_str::<toml::Table>(&text).ok());
    let said = table.as_ref().and_then(|table| table.get("project")?.get("description")?.as_str().map(str::to_owned));
    said.unwrap_or_default()
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

/// Where code staged from the checkout came from: its package and the commit.
#[derive(Clone, Debug)]
pub(crate) struct From {
    /// `_/qwen-image-2`.
    pub package: String,
    pub head: Option<Head>,
    /// What it does in a line (`description`), as its release's card would say.
    pub summary: String,
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

/// The checkout's package at `dir` as one tree (`cozy run ./dir`'s, its first-party siblings with it: `api::code::siblings`), at
/// one commit: read again when the checkout moved while it was read (rc-follow's fast-forward), so a tree is never half one
/// commit. `memo` remembers file digests.
pub(crate) fn stage(dir: &Path, memo: Option<&Path>) -> Result<Staged, Error> {
    let root = root().ok_or_else(|| Error::internal("checkout_unset", "no packages checkout is named"))?;
    let package = package_at(dir).ok_or_else(|| Error::new(Kind::NotFound, "checkout_package_unknown", format!("{} is no package of {}", dir.display(), root.display())))?;
    for _ in 0..3 {
        let before = head(root);
        let staged = code::stage(dir, memo)?;
        if head(root) == before {
            let from = From { package, head: before, summary: description(dir) };
            STAGED.lock().expect("staged lock").insert((Digest::of(&staged.document), staged.at.clone()), from);
            return Ok(staged);
        }
    }
    Err(Error::new(Kind::Unavailable, "checkout_moving", format!("{} moved each time {} was read", root.display(), dir.display())).remedy("run it again"))
}
