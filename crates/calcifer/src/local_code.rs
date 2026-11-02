//! Local code: a package directory or a script on this computer that nobody published, run where a
//! published function would be (`cozy run ./dir`, a project's `with: ./dir/fn`). A machine installs
//! it from TensorD's `LocalSource`, a manifest naming each source file by its content; the
//! interface its install answers is kept by the manifest and the Runtime that read it, so the same
//! code on the same Runtime is called at once. A directory travels with the path dependencies its
//! sources name, under their common directory; a script is a one-file package, its PEP 723 block
//! its pyproject.toml. Nothing is locked here: a project without a uv.lock is resolved on the
//! machine. This computer's machine reads the files in place, so only the manifest and carried
//! wheels travel; another receives the files too, and Calcifer writes only the bytes it lacks.
//! Every client and Calcifer's projects use this one module (cl-716).
use std::{
    collections::BTreeMap,
    fmt::Display,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use platform::{durable, paths::Home};
use protocol::{
    calcifer::{Kind as RunKind, On, Run, StagedInput, Status, SubmitRun},
    config::Origin,
    error::{Error, Kind},
    ids::Digest,
    machine as v1,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::api::Api;

/// The uv index name a project's sources use for its own account's packages.
pub const INDEX: &str = "tensorhub";

pub fn refuse(code: &str, detail: impl Display) -> Error {
    Error::new(Kind::Validation, code, detail.to_string())
}

fn hub_refusal(error: hub_client::Error) -> Error {
    error.refusal(|| "cozy login <email>".into())
}

/// The account `client` is signed in as.
pub async fn account(client: &hub_client::Client) -> Result<String, Error> {
    Ok(client.get_current_account().await.map_err(hub_refusal)?.name)
}

/// Local code as a run names it: its source (the manifest's digest), the files Calcifer stages for
/// it (the manifest, kept in Calcifer's home, and the source files a machine elsewhere is sent),
/// and the `local/<name>` package it is.
pub struct Source {
    pub package: String,
    pub source: v1::LocalSource,
    pub inputs: Vec<StagedInput>,
    /// What the machine's environment is built from, apart from the project's own source: its
    /// metadata and lock, its path dependencies, carried wheels and indexes.
    pub environment: Digest,
}

/// What an install answered: the code's interface, the machine's name for the installation, and
/// the machine it is on.
pub struct Installed {
    pub interface: Value,
    pub installation: Option<String>,
    pub machine: Option<String>,
}

/// Where manifests are kept for their calls to stage: Calcifer's home, the newest 256.
fn keep_manifest(home: &Home, digest: &Digest, document: &[u8]) -> Result<PathBuf, Error> {
    const KEPT: usize = 256;
    let dir = home.calcifer().join("local");
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

/// Whether `text` is written as a path on this computer, never a name: `.`, `..` or `~`, alone or
/// before a separator (`./dir`, `..\dir` on Windows), or a rooted or absolute path (`/dir`,
/// `C:\dir`). The CLI's one test, for code and for input files.
pub fn written_as_path(text: &str) -> bool {
    let text = text.trim();
    let led = |lead: &str| text.strip_prefix(lead).is_some_and(|rest| rest.is_empty() || rest.starts_with(std::path::is_separator));
    led(".") || led("..") || led("~") || text.starts_with(std::path::is_separator) || Path::new(text).is_absolute()
}

/// The path `text` spells: `~` is `home`, and a relative one is under `cwd`.
pub fn spelled(text: &str, cwd: &Path, home: &Path) -> PathBuf {
    let path = Path::new(text.trim());
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => cwd.join(path),
    }
}

/// Local code `text` names, when it is written as a path or names a script here (`x.py`).
pub fn local(text: &str, cwd: &Path, home: &Path) -> Option<PathBuf> {
    let text = text.trim();
    let script = text.ends_with(".py") && cwd.join(text).is_file();
    (written_as_path(text) || script).then(|| spelled(text, cwd, home))
}

/// Local code `text` names (`./dir[/fn]`, `./script.py[/fn]`, from `cwd`): its package directory or
/// script and the function it names. None: `text` is not written as a path (a published name).
pub fn target(text: &str, cwd: &Path, home: &Path) -> Option<Result<(PathBuf, Option<String>), Error>> {
    let raw = text.trim();
    let path = local(raw, cwd, home)?;
    let package = |path: &Path| path.is_dir() || (path.is_file() && path.extension().is_some_and(|e| e == "py"));
    if package(&path) {
        return Some(Ok((path, None)));
    }
    let function = path.file_name().map(|n| n.to_string_lossy().into_owned());
    Some(match path.parent().filter(|parent| package(parent)) {
        Some(parent) => Ok((parent.to_path_buf(), function)),
        None => Err(Error::new(
            Kind::NotFound,
            "target_not_found",
            format!("{raw} is not a package directory or a script on this computer"),
        )),
    })
}

/// A package project, as its pyproject.toml says.
pub struct Project {
    pub root: PathBuf,
    pub name: String,
    pub version: String,
    pub requires_python: String,
    pub account_index: bool,
    /// A dependency or uv source fetched with git: locking and publishing need git.
    pub git: bool,
}

impl Project {
    /// The project in the current directory.
    pub fn here() -> Result<Self, Error> {
        Self::read(std::env::current_dir().map_err(|e| refuse("project_unreadable", e))?)
    }

    pub fn read(root: PathBuf) -> Result<Self, Error> {
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
        let (Some(name), Some(version)) =
            (text(&["project", "name"]), text(&["project", "version"]))
        else {
            return Err(refuse(
                "project_identity_missing",
                "pyproject.toml's [project] needs a literal name and version",
            ));
        };
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
        let account_index =
            sources
                .into_iter()
                .flat_map(|s| s.values())
                .any(|source| match source {
                    toml::Value::Array(alternatives) => alternatives.iter().any(names_index),
                    source => names_index(source),
                });
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
            requires_python: text(&["project", "requires-python"]).unwrap_or_default(),
            account_index,
            git,
        })
    }
}

/// The files a package's source is, relative to `root`, sorted: what a publish uploads and local code
/// sends. Credentials, keys, bytecode and weights are refused, not skipped.
pub fn source_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut files = vec![];
    walk(root, Path::new(""), &mut files)?;
    files.sort();
    Ok(files)
}

fn walk(root: &Path, rel: &Path, files: &mut Vec<PathBuf>) -> Result<(), Error> {
    const SKIP: &str = ".git .hg .svn .jj __pycache__ .mypy_cache .ruff_cache .pytest_cache .tox";
    const SKIP_AT_ROOT: &str = ".venv venv node_modules .idea .vscode build dist";
    const SECRET_DIRS: &str = ".aws .ssh credentials secrets";
    const SECRET_FILES: &str =
        ".netrc .npmrc .pypirc credentials.json service-account.json id_ed25519";
    const REFUSED: &str =
        ".pyc .pyo .pem .key .p12 .pfx .jks .pickle .pkl .pt .ckpt .safetensors .onnx .gguf";
    let listed = |list: &str, name: &str| list.split(' ').any(|item| item == name);
    let entries = fs::read_dir(root.join(rel)).map_err(|e| refuse("project_unreadable", e))?;
    for entry in entries {
        let entry = entry.map_err(|e| refuse("project_unreadable", e))?;
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let path = rel.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|e| refuse("project_unreadable", e))?;
        let refused = |why: &str| {
            refuse(
                "package_source_refused",
                format!("{} {why}", path.display()),
            )
            .remedy(
                "move credentials, keys, bytecode and model weights out of the package directory",
            )
        };
        if kind.is_symlink() {
            return Err(refused("is a symlink"));
        }
        if kind.is_dir() {
            if listed(SECRET_DIRS, &name) {
                return Err(refused("holds credentials"));
            }
            let at_root = rel.as_os_str().is_empty() && listed(SKIP_AT_ROOT, &name);
            if !listed(SKIP, &name) && !name.ends_with(".egg-info") && !at_root {
                walk(root, &path, files)?;
            }
            continue;
        }
        let secret = name == ".env"
            || name.starts_with(".env.")
            || name.starts_with("id_rsa")
            || listed(SECRET_FILES, &name);
        if secret || REFUSED.split(' ').any(|suffix| name.ends_with(suffix)) {
            return Err(refused("must never ship in a package"));
        }
        let scratch = name.ends_with(".swp")
            || name.ends_with('~')
            || name.starts_with(".#")
            || name == ".ds_store";
        if !scratch && name != "package-interface.json" {
            files.push(path);
        }
    }
    Ok(())
}

/// The account's index on `hub` at the origin the Hub advertises (RFC 9728 `resource`): the one
/// its publish check holds a uv.lock to, wherever this computer reaches the Hub. A Hub that
/// advertises none takes no publication, and its configured origin stands.
pub async fn index_url(
    client: &hub_client::Client,
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
    #[serde(skip_serializing_if = "String::is_empty")]
    path: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    callees: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    indexes: BTreeMap<String, String>,
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

/// The project `path` names: a package directory, or a script as the one-file package it is.
pub fn project(home: &Home, path: &Path) -> Result<PathBuf, Error> {
    if !path.exists() {
        return Err(Error::new(
            Kind::NotFound,
            "target_not_found",
            format!("{} does not exist", path.display()),
        ));
    }
    let path = fs::canonicalize(path)
        .map_err(|e| refuse("project_unreadable", format!("{}: {e}", path.display())))?;
    match path.is_file() {
        true => script(home, &path),
        false => Ok(path),
    }
}

/// `path` (a package directory or a script) as the machine `on` installs it. This computer's
/// machine reads the files where they are (`in_place`); another is sent them. A project whose
/// sources name its account's index (`{ index = "tensorhub" }`) needs `hub`'s signed-in client.
pub async fn source(
    home: &Home,
    path: &Path,
    on: &On,
    hub: Option<(&hub_client::Client, &Origin)>,
) -> Result<Source, Error> {
    let in_place = matches!(on, On::Machine(name) if name.eq_ignore_ascii_case("local"));
    let project_dir = project(home, path)?;
    let project = Project::read(project_dir.clone())?;
    let locals = path_dependencies(&project_dir)?;
    let root = common(
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
    let mut names: Vec<PathBuf> = source_files(&project_dir)?
        .into_iter()
        .map(|f| project_dir.join(f))
        .collect();
    for local in &locals {
        match local.is_file() {
            true => names.push(local.clone()),
            false => names.extend(source_files(local)?.into_iter().map(|f| local.join(f))),
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
            executable: executable(&file),
        });
        // A carried wheel is an object the machine holds, even one that reads the tree in place.
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
        if !in_place || carried(&file) {
            inputs.push(StagedInput {
                path: file,
                digest,
                length,
            });
        }
    }
    let mut indexes = BTreeMap::new();
    if project.account_index {
        let (client, hub) = hub.ok_or_else(|| {
            Error::new(Kind::Credential, "account_index_unreached", "its `tensorhub` sources need this computer signed in to the Hub")
                .next("cozy login <email>")
        })?;
        let account = account(client).await?;
        indexes.insert(INDEX.to_owned(), index_url(client, hub, &account).await?);
    }
    let manifest = Manifest {
        package: format!("local/{}", project.name),
        release: project.version.clone(),
        python_requires: project.requires_python.clone(),
        files,
        project: relative(&project_dir),
        locals: locals.iter().map(|l| relative(l)).collect(),
        path: if in_place {
            root.to_string_lossy().into_owned()
        } else {
            String::new()
        },
        callees: callees(&project_dir)?,
        indexes,
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
    let environment = serde_json::json!({"files": built, "indexes": manifest.indexes,
        "python": manifest.python_requires});
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

/// The local path dependencies the project's sources name (`{ path = "../lib" }`), each a
/// directory or a file.
fn path_dependencies(project: &Path) -> Result<Vec<PathBuf>, Error> {
    let text = fs::read_to_string(project.join("pyproject.toml"))
        .map_err(|e| refuse("project_unreadable", e))?;
    let pyproject: toml::Table =
        toml::from_str(&text).map_err(|e| refuse("project_file_invalid", e))?;
    let sources = pyproject
        .get("tool")
        .and_then(|t| t.get("uv"))
        .and_then(|u| u.get("sources"))
        .and_then(|s| s.as_table());
    let mut out = vec![];
    for source in sources.into_iter().flat_map(|s| s.values()) {
        let Some(path) = source.get("path").and_then(|p| p.as_str()) else {
            continue;
        };
        let dependency = fs::canonicalize(project.join(path))
            .map_err(|e| refuse("local_dependency_unreadable", format!("{path}: {e}")))?;
        out.push(dependency);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// The deepest directory holding `first` and every one of `rest`.
fn common<'a>(first: &'a Path, rest: impl Iterator<Item = &'a Path>) -> PathBuf {
    let mut root = first.to_path_buf();
    for dir in rest {
        while !dir.starts_with(&root) {
            match root.parent() {
                Some(parent) => root = parent.to_path_buf(),
                None => break,
            }
        }
    }
    root
}

/// The distributions the lock takes from an account index, as the packages they are
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

/// A script as the one-file package it is (xs-099): its PEP 723 block is its pyproject.toml
/// (`dependencies` and `requires-python` are [project]'s, [tool] is [tool]), named for the file,
/// whose module it is, with its `app`. The project is written under `~/.cozy/scripts`, one per
/// script path, and from there it is read as any package directory is.
fn script(home: &Home, path: &Path) -> Result<PathBuf, Error> {
    let raw = fs::read(path).map_err(|e| refuse("script_unreadable", e))?;
    let mut block = pep723(&String::from_utf8_lossy(&raw))?;
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
    let mut module: String = (stem.chars())
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if !module.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        module.insert(0, '_');
    }
    let at = hex::encode(Sha256::digest(path.to_string_lossy().as_bytes()));
    let root = home
        .root()
        .join("scripts")
        .join(format!("{name}-{}", &at[..16]));
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
    if let Some(other) = block.keys().next() {
        return Err(refuse(
            "script_metadata_invalid",
            format!(
                "the `# /// script` block names {other:?}: it takes dependencies, requires-python and [tool]"
            ),
        ));
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
            source["path"] = from(&root, &absolute).into();
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
    fs::create_dir_all(&root).map_err(|e| refuse("script_unwritable", e))?;
    for (name, bytes) in [
        (file.as_str(), raw.as_slice()),
        ("pyproject.toml", pyproject.as_bytes()),
    ] {
        if fs::read(root.join(name)).ok().as_deref() != Some(bytes) {
            fs::write(root.join(name), bytes).map_err(|e| refuse("script_unwritable", e))?;
        }
    }
    Ok(root)
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

/// The script's `# /// script` metadata block (PEP 723) as TOML; none is an empty table.
fn pep723(text: &str) -> Result<toml::Table, Error> {
    let mut lines = text
        .lines()
        .skip_while(|line| line.trim_end() != "# /// script");
    if lines.next().is_none() {
        return Ok(toml::Table::new());
    }
    let mut block = String::new();
    for line in lines {
        match line.trim_end() {
            "# ///" => {
                return toml::from_str(&block).map_err(|e| refuse("script_metadata_invalid", e));
            }
            "#" => block.push('\n'),
            line => match line.strip_prefix("# ") {
                Some(content) => block.push_str(&format!("{content}\n")),
                None => break,
            },
        }
    }
    Err(refuse(
        "script_metadata_invalid",
        "the `# /// script` block has no closing `# ///`",
    ))
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(_path: &Path) -> bool {
    false
}

/// Where local code's interfaces are kept, beside release cards: one file per source manifest
/// and Runtime that described it, named for its environment's inputs first, so a changed source
/// with the same environment is seen.
fn dir(home: &Home, source: &Source) -> PathBuf {
    let mut dir = home.root().join("packages");
    dir.extend(source.package.split('/'));
    dir
}

fn file(home: &Home, source: &Source, runtime: &str) -> PathBuf {
    let manifest = source.source.manifest.trim_start_matches("sha256:");
    let runtime = &hex(&Digest::of(runtime.as_bytes()))[..16];
    dir(home, source).join(format!("{}-{manifest}-{runtime}.json", &hex(&source.environment)[..16]))
}

fn hex(digest: &Digest) -> String {
    digest.to_string().trim_start_matches("sha256:").to_owned()
}

/// The interface kept for this exact source, as `runtime` described it.
pub fn kept(home: &Home, source: &Source, runtime: &str) -> Option<Value> {
    let document: Value = serde_json::from_slice(&fs::read(file(home, source, runtime)).ok()?).ok()?;
    interface(&document).then_some(document)
}

/// Whether some kept source of this package has the same environment inputs: a changed source
/// is a sync, not an install.
fn synced(home: &Home, source: &Source) -> bool {
    let prefix = format!("{}-", &hex(&source.environment)[..16]);
    (fs::read_dir(dir(home, source)).into_iter().flatten().flatten())
        .any(|e| e.file_name().to_string_lossy().starts_with(&prefix))
}

/// Keeps `interface` for this source as `runtime` described it, and only the package's newest 16.
pub fn keep(home: &Home, source: &Source, runtime: &str, interface: &Value) {
    const KEPT: usize = 16;
    let dir = dir(home, source);
    let kept = durable::private_dir(&dir)
        .and_then(|()| durable::replace(&file(home, source, runtime), interface.to_string().as_bytes(), 0o600));
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
    protocol::interface::format(document).starts_with("cozy.package.interface/")
}

/// The run that installs local code `on` a machine: a WARM run naming no function. It is named
/// for what the machine does: an install when the environment's inputs (metadata, lock, path
/// dependencies) are new here, else a sync of the changed source.
pub fn install_run(home: &Home, hub: &str, on: On, source: &Source) -> SubmitRun {
    let target = match synced(home, source) {
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
        on,
        hub: hub.to_owned(),
        kind: RunKind::Warm,
        target,
        spec,
        inputs: source.inputs.clone(),
        out: None,
        warnings: vec![],
    }
}

/// What a settled install answered, or why it installed nothing.
pub fn installed(run: Run) -> Result<Installed, Error> {
    if run.status != Status::Completed {
        let (code, message) = run.reason.map_or_else(
            || ("local_install_unfinished".to_owned(), format!("its install is {:?}", run.status).to_lowercase()),
            |r| (r.code, r.message),
        );
        return Err(Error::new(
            Kind::Unavailable,
            "local_install_failed",
            format!("the machine did not install this code: {message}"),
        )
        .cause(code)
        .next(format!("cozy run show {}", run.number)));
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
pub struct Prepared {
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

/// The code at `path` made callable `on` a machine, never waiting for an install: the install it
/// needs is sent under `{key}/install` and returned, and once it has settled the same `key` finds
/// its answer. `warm` installs again whatever is kept: a warm set names the machine's
/// installation, which only an install answers.
pub async fn prepare(
    api: &Arc<Api>,
    path: &Path,
    on: &On,
    hub: &str,
    warm: bool,
    key: &str,
) -> Result<Prepared, Error> {
    let hubs = &api.machines.hubs;
    let home = hubs.home().clone();
    let origin = Origin::parse(&if hub.is_empty() { hubs.default_origin() } else { hub.to_owned() })?;
    let client = hubs.client(origin.as_str()).ok();
    let code = source(&home, path, on, client.as_ref().map(|client| (client, &origin))).await?;
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
    let key = format!("{key}/install");
    if let Some(run) = sent(api, &key)? {
        if !matches!(run.status, Status::Completed | Status::Failed | Status::Canceled) {
            return Ok(prepared(None, None, None, Some(run)));
        }
        let installed = installed(run)?;
        if let Some(runtime) = &runtime {
            keep(&home, &code, runtime, &installed.interface);
        }
        return Ok(prepared(Some(installed.interface), installed.installation, installed.machine, None));
    }
    if let Some(interface) = runtime.as_deref().filter(|_| !warm).and_then(|runtime| kept(&home, &code, runtime)) {
        return Ok(prepared(Some(interface), None, None, None));
    }
    let install = install_run(&home, origin.as_str(), on.clone(), &code);
    let run = api.calcifer.submit(&key, install).await.map_err(|e| crate::api::Refusal::from(e).0)?.run;
    Ok(prepared(None, None, None, Some(run)))
}

/// `prepare`, waiting out the install it needs: what Calcifer's own callers (a project's render
/// blocks) use.
pub async fn ready(api: &Arc<Api>, path: &Path, on: &On, hub: &str, key: &str) -> Result<Prepared, Error> {
    loop {
        let prepared = prepare(api, path, on, hub, false, key).await?;
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

/// `prepare` without installing: the code's interface when one is kept here for it and the
/// machine's Runtime, else none (a read, such as a project's status, installs nothing).
pub async fn described(api: &Arc<Api>, path: &Path, on: &On, hub: &str) -> Result<Prepared, Error> {
    let hubs = &api.machines.hubs;
    let home = hubs.home().clone();
    let origin = Origin::parse(&if hub.is_empty() { hubs.default_origin() } else { hub.to_owned() })?;
    let client = hubs.client(origin.as_str()).ok();
    let code = source(&home, path, on, client.as_ref().map(|client| (client, &origin))).await?;
    let runtime = runtime(api, on).await;
    Ok(Prepared {
        interface: runtime.as_deref().and_then(|runtime| kept(&home, &code, runtime)),
        package: code.package,
        source: code.source,
        files: code.inputs,
        installation: None,
        machine: None,
        install: None,
    })
}

/// `uv` (cozy's pinned uv, else PATH's: `tools::uv`) in `dir`, quiet and colorless.
pub fn uv(uv: &Path, dir: &Path) -> Command {
    let mut command = Command::new(uv);
    command.current_dir(dir).env("NO_COLOR", "1");
    command
}

/// Runs `command`; a failure is refused as `code` with the tool's own last words.
pub fn run(command: &mut Command, code: &str) -> Result<String, Error> {
    let out = command.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::new(
            Kind::Unavailable,
            "uv_unavailable",
            format!("{} is gone", command.get_program().to_string_lossy()),
        )
        .remedy("run it again: cozy fetches uv when it is missing")
        .next("cozy doctor"),
        _ => refuse(code, e),
    })?;
    if !out.status.success() {
        let said = [&out.stderr[..], &out.stdout[..]].concat();
        let said = String::from_utf8_lossy(&said);
        let tail: Vec<&str> = said.lines().filter(|l| !l.trim().is_empty()).collect();
        return Err(refuse(
            code,
            tail[tail.len().saturating_sub(12)..].join("\n"),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The Python a release of `project` runs on: `.python-version`, else requires-python's floor;
/// its exact version as uv finds it (installing a managed one when none is here).
pub fn python(project: &Project, uv: &Path) -> Result<Python, Error> {
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
            )
        };
        let full = match find() {
            Ok(version) => version,
            Err(_) => {
                run(
                    self::uv(uv, &project.root).args(["python", "install", request.as_str()]),
                    "python_install_failed",
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
pub struct Python {
    /// What uv is asked for (`3.12`).
    pub request: String,
    /// The exact version a release records (`3.12.12`).
    pub full: String,
}

/// The cozy-runtime release `root`'s uv.lock holds, if it holds one.
pub fn locked_runtime(root: &Path) -> Option<String> {
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

/// What Runtime's static describer reads from the source at `dir`, with `runtime`'s release (else
/// the newest) on `python`: its exact words and the document they are. Nothing installs, and none
/// of the code runs.
pub fn describe_with(uv: &Path, dir: &Path, python: &str, runtime: Option<&str>) -> Result<(String, Value), Error> {
    let from = match runtime {
        Some(version) => format!("cozy-runtime=={version}"),
        // `@latest`: a bare name would take whatever cozy-runtime `uv tool install` left here.
        None => "cozy-runtime@latest".into(),
    };
    let said = run(
        self::uv(uv, dir)
            .args(["tool", "run", "--no-progress", "--python", python, "--from", from.as_str()])
            .args(["cozy-runtime", "--json", "--dir"])
            .arg(dir)
            .arg("describe"),
        "describe_failed",
    )?;
    let document: Value = serde_json::from_str(&said).map_err(|e| refuse("describe_failed", e))?;
    if !interface(&document) {
        return Err(refuse("package_interface_invalid", "the describer's answer is not a package interface"));
    }
    Ok((said.trim_end().to_owned(), document))
}

/// The local code at `path` (a package directory or a script) as Runtime's static describer reads
/// it, on the Python and Runtime release its project names: `local/<name>` and its interface.
/// Nothing installs, and no machine is asked.
pub async fn describe(home: &Home, path: &Path) -> Result<(String, Value), Error> {
    let project = Project::read(project(home, path)?)?;
    let uv = crate::tools::uv(home, &|line| tracing::info!("{line}")).await?;
    let python = python(&project, &uv)?;
    let (_, document) = describe_with(&uv, &project.root, &python.request, locked_runtime(&project.root).as_deref())?;
    Ok((format!("local/{}", project.name), document))
}
