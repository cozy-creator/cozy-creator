//! Code at a public git repository (cl-1127; Paul, 11-17: "ideally users should be able to install code from arbitrary public
//! github links"): `cozy package install <URL>[@ref]`, `cozy run <URL>[@ref]/<fn>` and a project's `with:` (`api::target::Git`).
//! Calcifer fetches the repository at an exact commit with git, on this computer only, into a folder of its own that never changes,
//! and from there it is local code (`local_code`): locked as `cargo run` locks, and sent to every machine by content, so no machine
//! needs git, the repository's host or the Hub. A branch or tag is read at the repository each time, and, with it unreachable, at the
//! commit it last named here. Over https with no credential (none configured is offered): public repositories only.
//!
//! What a machine installed from a repository is named by its manifest (`manifests/<hex>.json`, its `Origin`): a machine's listing
//! says it (`Environment::origin`, `git+<url>@<commit>`).
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{LazyLock, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use api::{
    error::{Error, Kind},
    ids::Digest,
    target::{Git, git_origin},
};
use platform::{durable, paths::Home};
use serde::{Deserialize, Serialize};

/// Calcifer's folder of repositories, `<calcifer>/git`.
static ROOT: OnceLock<PathBuf> = OnceLock::new();

pub(crate) fn start(home: &Home) {
    let _ = ROOT.set(home.calcifer().join("git"));
}

/// Where repositories are fetched: Calcifer's folder, or inside a project's own (`private_io::cache`): what an encrypted project
/// reads stays in it.
fn root() -> Result<PathBuf, Error> {
    let ordinary = ROOT.get().cloned().ok_or_else(|| Error::new(Kind::Internal, "git_unavailable", "Calcifer hasn't started"))?;
    crate::private_io::cache(crate::private_io::current().as_ref(), ordinary, "git")
}

/// Code a repository holds at one commit: the package's folder in it (empty: its root), and the ref it was asked by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Origin {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    pub commit: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
}

impl Origin {
    /// `git+<url>@<commit>[#subdirectory=<folder>]`, as pip and uv name it.
    pub(crate) fn spelled(&self) -> String {
        git_origin(&self.url, &self.commit, &self.folder)
    }

    /// In a line: `<url>@<ref> is at <commit>`, or `<url> at <commit>`.
    pub(crate) fn said(&self) -> String {
        let at = match self.folder.as_str() {
            "" => String::new(),
            folder => format!(" ({folder})"),
        };
        match &self.reference {
            Some(reference) if *reference != self.commit => format!("{}@{reference}{at} is at commit {}", self.url, self.commit),
            _ => format!("{}{at} at commit {}", self.url, self.commit),
        }
    }
}

/// What a repository's folder keeps of it: its URL, and the commit each ref last named here.
#[derive(Default, Serialize, Deserialize)]
struct Repository {
    url: String,
    #[serde(default)]
    refs: BTreeMap<String, String>,
}

fn refuse(code: &str, message: impl Into<String>) -> Error {
    Error::new(Kind::Unavailable, code, message.into())
}

/// The package folder (or script) and function `text` names when it's a repository's URL, the repository fetched at its commit;
/// none when it's no URL.
pub(crate) fn target(text: &str) -> Option<Result<(PathBuf, Option<String>), Error>> {
    let git = match Git::parse(text)? {
        Ok(git) => git,
        Err(e) => return Some(Err(e)),
    };
    Some(blocking(|| fetched(&git)))
}

/// `f`, which waits on git and the network, off the runtime's async workers.
fn blocking<T>(f: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => tokio::task::block_in_place(f),
        _ => f(),
    }
}

fn hex(text: &str) -> String {
    Digest::of(text.as_bytes()).to_string().trim_start_matches("sha256:")[..32].to_owned()
}

fn fetched(git: &Git) -> Result<(PathBuf, Option<String>), Error> {
    let dir = root()?.join(hex(git.repository()));
    durable::private_dir(&dir).map_err(|e| refuse("git_unwritable", format!("{}: {e}", dir.display())))?;
    let commit = commit_of(git, &dir)?;
    let tree = dir.join(&commit);
    if !tree.is_dir() {
        checkout(git, &commit, &dir, &tree)?;
    }
    let at = git.path.split('/').filter(|part| !part.is_empty()).fold(tree.clone(), |at, part| at.join(part));
    let shown = || format!("{}@{}", git.url, &commit[..12]);
    let (package, function) = api::code::split(&at).ok_or_else(|| {
        let what = if git.path.is_empty() { "its root".to_owned() } else { git.path.clone() };
        Error::new(Kind::NotFound, "target_not_found", format!("{what} isn't a package folder or a script at {}.", shown()))
    })?;
    let inside = fs::canonicalize(&package).ok().zip(fs::canonicalize(&tree).ok()).is_some_and(|(package, tree)| package.starts_with(tree));
    if !inside {
        return Err(Error::new(Kind::Validation, "git_path_outside", format!("{} leaves the repository at {}.", git.path, shown())));
    }
    let folder = package.is_dir().then(|| package.join("pyproject.toml")).filter(|pyproject| !pyproject.is_file());
    if let Some(missing) = folder {
        return Err(Error::new(Kind::NotFound, "project_file_missing", format!("{} has no pyproject.toml at {}.", missing.parent().unwrap_or(&package).strip_prefix(&tree).unwrap_or(Path::new("")).display(), shown())));
    }
    Ok((package, function))
}

/// The commit `git` names: one written whole, else the one its ref (or the default branch) names at the repository now; that
/// unreachable, the one it last named here.
fn commit_of(git: &Git, dir: &Path) -> Result<String, Error> {
    let full = |text: &str| text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit());
    if let Some(commit) = git.reference.as_deref().filter(|reference| full(reference)) {
        return Ok(commit.to_ascii_lowercase());
    }
    let kept_at = dir.join("repository.json");
    let mut kept: Repository = fs::read(&kept_at).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    let name = git.reference.clone().unwrap_or_else(|| "HEAD".into());
    // A ref read at its repository this last minute stands: a project names its code at each read.
    static READ: LazyLock<Mutex<HashMap<(String, String), (Instant, String)>>> = LazyLock::new(Mutex::default);
    let key = (git.repository().to_owned(), name.clone());
    if let Some((_, commit)) = READ.lock().expect("refs lock").get(&key).filter(|(at, _)| at.elapsed() < Duration::from_secs(60)) {
        return Ok(commit.clone());
    }
    let read = remote(git, &name, dir);
    if let Ok(commit) = &read {
        READ.lock().expect("refs lock").insert(key, (Instant::now(), commit.clone()));
    }
    match read {
        Ok(commit) => {
            kept.url = git.repository().to_owned();
            kept.refs.insert(name, commit.clone());
            let bytes = serde_json::to_vec(&kept).expect("a repository record serializes");
            durable::replace(&kept_at, &bytes, 0o600).map_err(|e| refuse("git_unwritable", format!("{}: {e}", kept_at.display())))?;
            Ok(commit)
        }
        Err(e) if e.code == "git_unreachable" => match kept.refs.get(&name) {
            Some(commit) => {
                if crate::private_io::current().is_none() {
                    tracing::info!("{} can't be reached: {name} is taken at {commit}, as it last named here", git.url);
                }
                Ok(commit.clone())
            }
            None => Err(e),
        },
        Err(e) => Err(e),
    }
}

/// git with no configuration of this computer's: no credential helper, no URL rewritten to ssh, no prompt. What a public
/// repository answers anyone.
fn git(dir: &Path, args: &[&str]) -> Result<String, Error> {
    let mut command = Command::new("git");
    command.current_dir(dir).args(["-c", "credential.helper=", "-c", "core.askPass=", "-c", "core.symlinks=false", "-c", "protocol.allow=never", "-c", "protocol.https.allow=always"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", dir.join(".no-git-config"))
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
    let out = crate::local_code::project_tool_output(&mut command).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            let tool = platform::tools::tool("git");
            Error::new(Kind::Unavailable, "git_unavailable", format!("git not found: {}", tool.cost))
                .next(platform::tools::install_hint(tool, &crate::tools::path()))
        }
        _ => refuse("git_failed", e.to_string()),
    })?;
    match out.status.success() {
        true => Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned()),
        false => Err(refuse("git_failed", last_words(&out.stderr))),
    }
}

/// git's last line on stderr, its reason.
fn last_words(stderr: &[u8]) -> String {
    let said = String::from_utf8_lossy(stderr);
    let line = said.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with("hint:")).last().unwrap_or("it said nothing");
    line.trim_start_matches("fatal: ").chars().take(300).collect()
}

/// The commit `name` (a branch, a tag, `HEAD`) names at the repository now: a tag's commit, not its annotation. Asked from `dir`.
fn remote(repository: &Git, name: &str, dir: &Path) -> Result<String, Error> {
    let listed = git(dir, &["ls-remote", &repository.url, name]).map_err(|e| match e.code.as_str() {
        "git_failed" => Error::new(Kind::Unavailable, "git_unreachable", format!("{} can't be read: {}", repository.url, e.message))
            .remedy("Cozy fetches public repositories over https: check the URL, and that it's public."),
        _ => e,
    })?;
    let rows: Vec<(&str, &str)> = listed.lines().filter_map(|line| line.split_once('\t')).collect();
    let named = |reference: &str| rows.iter().find(|(_, row)| *row == reference).map(|(commit, _)| commit.to_string());
    let found = match name {
        "HEAD" => named("HEAD"),
        name => named(&format!("refs/tags/{name}^{{}}"))
            .or_else(|| named(&format!("refs/heads/{name}")))
            .or_else(|| named(&format!("refs/tags/{name}")))
            .or_else(|| named(name)),
    };
    found.ok_or_else(|| {
        Error::new(Kind::NotFound, "git_ref_not_found", format!("{} has no branch or tag {name}.", repository.url))
            .remedy("Name a branch, a tag, or a commit by its 40 hex characters.")
    })
}

/// The repository at `commit`, its files alone, made whole beside `tree` and then named it: it never changes after.
fn checkout(repository: &Git, commit: &str, dir: &Path, tree: &Path) -> Result<(), Error> {
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let part = dir.join(format!(".part-{}-{nanos}", std::process::id()));
    let made = (|| {
        fs::create_dir_all(&part).map_err(|e| refuse("git_unwritable", format!("{}: {e}", part.display())))?;
        if crate::private_io::current().is_none() {
            tracing::info!("fetching {} at {commit}", repository.url);
        }
        git(&part, &["init", "-q"])?;
        // A commit by its hash, as GitHub and GitLab serve it; else its ref's tip, which must be it.
        let by_hash = git(&part, &["fetch", "-q", "--depth", "1", &repository.url, commit]);
        if let Err(e) = by_hash {
            let Some(reference) = repository.reference.as_deref().filter(|reference| *reference != commit) else { return Err(e) };
            git(&part, &["fetch", "-q", "--depth", "1", &repository.url, reference])?;
        }
        let fetched = git(&part, &["rev-parse", "FETCH_HEAD"])?;
        if fetched != commit {
            return Err(refuse("git_moved", format!("{} moved while it was fetched ({fetched}, not {commit}): ask again", repository.url)));
        }
        git(&part, &["checkout", "-q", "FETCH_HEAD"])?;
        fs::remove_dir_all(part.join(".git")).map_err(|e| refuse("git_unwritable", e.to_string()))
    })();
    let named = made.and_then(|()| match fs::rename(&part, tree) {
        Ok(()) => Ok(()),
        // Another fetch named it first: the same commit's files.
        Err(_) if tree.is_dir() => Ok(()),
        Err(e) => Err(refuse("git_unwritable", format!("{}: {e}", tree.display()))),
    });
    if part.exists() {
        let _ = fs::remove_dir_all(&part);
    }
    named
}

/// The repository, commit and folder of `path` when it's in a commit's checkout here.
pub(crate) fn origin_of(path: &Path) -> Option<Origin> {
    let root = fs::canonicalize(root().ok()?).ok()?;
    let path = fs::canonicalize(path).ok()?;
    let mut inside = path.strip_prefix(&root).ok()?.components().map(|part| part.as_os_str().to_string_lossy().into_owned());
    let (repository, commit) = (inside.next()?, inside.next()?);
    let folder = inside.collect::<Vec<_>>().join("/");
    let kept: Repository = serde_json::from_slice(&fs::read(root.join(&repository).join("repository.json")).ok()?).ok()?;
    let reference = kept.refs.iter().find(|(_, at)| **at == commit).map(|(reference, _)| reference.clone()).filter(|reference| reference != "HEAD");
    (commit.len() == 40).then_some(Origin { url: kept.url, reference, commit, folder })
}

/// Records that the tree `tree` was read from `path`, when that's a repository's commit here.
pub(crate) fn noted(path: &Path, tree: &Digest) {
    if let Some(origin) = origin_of(path) {
        keep("trees", tree, &origin);
    }
}

/// Records that the manifest `manifest` installs the tree `tree`, when that came from a repository.
pub(crate) fn link(tree: &Digest, manifest: &Digest) {
    if let Some(origin) = kept("trees", &tree.to_string()) {
        keep("manifests", manifest, &origin);
    }
}

/// The repository a machine's installation of the manifest `manifest` (`sha256:<hex>`) came from, as `Origin::spelled`.
pub(crate) fn installed_from(manifest: &str) -> Option<String> {
    if manifest.is_empty() {
        return None;
    }
    kept("manifests", manifest).map(|origin| origin.spelled())
}

fn keep(kind: &str, digest: &Digest, origin: &Origin) {
    let Ok(dir) = root().map(|root| root.join(kind)) else { return };
    let path = dir.join(format!("{}.json", digest.to_string().trim_start_matches("sha256:")));
    let bytes = serde_json::to_vec(origin).expect("an origin serializes");
    // A record not kept is a listing that says `local`: the code runs all the same.
    if fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
        let _ = durable::private_dir(&dir).and_then(|()| durable::replace(&path, &bytes, 0o600));
    }
}

fn kept(kind: &str, digest: &str) -> Option<Origin> {
    let path = root().ok()?.join(kind).join(format!("{}.json", digest.trim_start_matches("sha256:")));
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}
