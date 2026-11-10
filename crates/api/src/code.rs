//! Local code as it travels (cl-819): a package folder or a PEP 723 script, with the path dependencies its sources name,
//! under their common folder, as one tree (`api::tree`). Whoever holds the folder reads it here, the CLI its own and
//! Calcifer a project's `with: ./dir`, and Calcifer works from its copy of the tree, by its digest: it reads no client's disk.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    calcifer::StagedInput,
    error::{Error, Kind},
    ids::Digest,
    tree::{self, Member},
};

/// Local code by content: its tree document's digest, each file and then the document sent first (`PUT /v1/files`), and
/// where the package is in the tree.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LocalCode {
    pub tree: Digest,
    /// The package's folder or script in the tree, `/`-separated; empty: the tree itself.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub at: String,
}

/// Local code read from its folder: its tree's document and files, the package's place in it, and what never goes.
pub struct Staged {
    pub document: Vec<u8>,
    pub files: Vec<StagedInput>,
    pub at: String,
    /// What never goes, as people read it (`source_files`).
    pub left: Vec<String>,
}

impl Staged {
    pub fn code(&self) -> LocalCode {
        LocalCode { tree: Digest::of(&self.document), at: self.at.clone() }
    }
}

fn refuse(code: &str, detail: impl std::fmt::Display) -> Error {
    Error::new(Kind::Validation, code, detail.to_string())
}

/// The package (a folder, or a script) and the function `path` names: `./dir`, `./dir/fn`, `./script.py`, `./script.py/fn`.
/// None: it names neither here.
pub fn split(path: &Path) -> Option<(PathBuf, Option<String>)> {
    let package = |path: &Path| path.is_dir() || (path.is_file() && path.extension().is_some_and(|e| e == "py"));
    if package(path) {
        return Some((path.to_owned(), None));
    }
    let function = path.file_name()?.to_string_lossy().into_owned();
    path.parent().filter(|parent| package(parent)).map(|parent| (parent.to_owned(), Some(function)))
}

/// The package at `path` (a folder or a script) as one tree: it and its path dependencies under their common folder, each
/// file's digest remembered in `memo` as an input file's is.
pub fn stage(path: &Path, memo: Option<&Path>) -> Result<Staged, Error> {
    let unreadable = |e: std::io::Error| refuse("project_unreadable", format!("{}: {e}", path.display()));
    if !path.exists() {
        return Err(Error::new(Kind::NotFound, "target_not_found", format!("{} does not exist", path.display())));
    }
    let package = fs::canonicalize(path).map_err(unreadable)?;
    let script = package.is_file();
    let beside = if script { package.parent().unwrap_or(&package).to_owned() } else { package.clone() };
    let locals = match script {
        true => script_dependencies(&package)?,
        false => path_dependencies(&package)?,
    };
    let root = common(&beside, locals.iter().map(|l| if l.is_file() { l.parent().unwrap_or(l) } else { l.as_path() }));
    let mut left = vec![];
    let mut names: Vec<PathBuf> = match script {
        true => vec![package.clone()],
        false => source_files(&package, &mut left)?.into_iter().map(|f| package.join(f)).collect(),
    };
    for local in &locals {
        match local.is_file() {
            true => names.push(local.clone()),
            false => names.extend(source_files(local, &mut left)?.into_iter().map(|f| local.join(f))),
        }
    }
    names.sort();
    names.dedup();
    let named = |path: &Path| -> Result<String, Error> {
        let parts: Option<Vec<&str>> = path.strip_prefix(&root).unwrap_or(path).iter().map(|p| p.to_str()).collect();
        parts.map(|parts| parts.join("/")).ok_or_else(|| refuse("project_unreadable", format!("{} is not a name every OS can hold", path.display())))
    };
    let (mut files, mut members) = (vec![], BTreeMap::new());
    for file in names {
        let length = fs::metadata(&file).map_err(|e| refuse("project_unreadable", format!("{}: {e}", file.display())))?.len();
        let digest = crate::digests::of(memo, &file)?;
        members.insert(named(&file)?, Member { length, sha256: hex::encode(digest.bytes()), executable: executable(&file) });
        files.push(StagedInput { path: file, digest, length });
    }
    let at = match package == root {
        true => String::new(),
        false => named(&package)?,
    };
    let document = tree::document(&members);
    tree::parse(&document).map_err(|e| refuse("project_unreadable", e))?;
    Ok(Staged { document, files, at, left })
}

/// The files a package's source is, relative to `root`, sorted: what publication and local code send.
/// Common credential names, keys, weights, bytecode and symlinks stay out and are said in `left`.
/// This is a filename filter, not secret detection; other runtime resources remain included.
pub fn source_files(root: &Path, left: &mut Vec<String>) -> Result<Vec<PathBuf>, Error> {
    let mut files = vec![];
    walk(root, Path::new(""), &mut files, left)?;
    files.sort();
    Ok(files)
}

fn walk(root: &Path, rel: &Path, files: &mut Vec<PathBuf>, left: &mut Vec<String>) -> Result<(), Error> {
    const SKIP: &str = ".git .hg .svn .jj __pycache__ .mypy_cache .ruff_cache .pytest_cache .tox";
    const SKIP_AT_ROOT: &str = ".venv venv node_modules .idea .vscode build dist";
    const SECRET_DIRS: &str = ".aws .ssh .direnv credentials secrets";
    const SECRET_FILES: &str = ".netrc .npmrc .pypirc .envrc credentials.json service-account.json id_ed25519";
    const REFUSED: &str = ".pyc .pyo .pem .key .p12 .pfx .jks .pickle .pkl .pt .ckpt .safetensors .onnx .gguf";
    let listed = |list: &str, name: &str| list.split(' ').any(|item| item == name);
    let unreadable = |e: std::io::Error| refuse("project_unreadable", e);
    for entry in fs::read_dir(root.join(rel)).map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let path = rel.join(entry.file_name());
        let kind = entry.file_type().map_err(unreadable)?;
        if kind.is_symlink() {
            left.push(format!("{} (a symlink)", path.display()));
            continue;
        }
        if kind.is_dir() {
            if listed(SECRET_DIRS, &name) {
                left.push(format!("{} (holds credentials)", path.display()));
                continue;
            }
            let at_root = rel.as_os_str().is_empty() && listed(SKIP_AT_ROOT, &name);
            if !listed(SKIP, &name) && !name.ends_with(".egg-info") && !at_root {
                walk(root, &path, files, left)?;
            }
            continue;
        }
        let secret = name == ".env" || name.starts_with(".env.") || name.starts_with("id_rsa") || listed(SECRET_FILES, &name);
        if secret || REFUSED.split(' ').any(|suffix| name.ends_with(suffix)) {
            // Bytecode is a build's litter, not worth a word.
            if !(name.ends_with(".pyc") || name.ends_with(".pyo")) {
                left.push(path.display().to_string());
            }
            continue;
        }
        let scratch = name.ends_with(".swp") || name.ends_with('~') || name.starts_with(".#") || name == ".ds_store";
        if !scratch && name != "package-interface.json" {
            files.push(path);
        }
    }
    Ok(())
}

/// The local path dependencies a package's pyproject.toml names (`{ path = "../lib" }`), each a folder or a file.
pub fn path_dependencies(project: &Path) -> Result<Vec<PathBuf>, Error> {
    let text = fs::read_to_string(project.join("pyproject.toml")).map_err(|e| {
        Error::new(Kind::NotFound, "project_file_missing", format!("pyproject.toml: {e}"))
            .remedy("run this in a package's directory (`cozy-runtime new <name>` makes one)")
    })?;
    let pyproject: toml::Table = toml::from_str(&text).map_err(|e| refuse("project_file_invalid", format!("pyproject.toml: {e}")))?;
    sourced(project, pyproject.get("tool"))
}

/// The local path dependencies a script's `# /// script` block names, beside it.
pub(crate) fn script_dependencies(script: &Path) -> Result<Vec<PathBuf>, Error> {
    let raw = fs::read(script).map_err(|e| refuse("script_unreadable", e))?;
    let block = pep723(&String::from_utf8_lossy(&raw))?;
    sourced(script.parent().unwrap_or(script), block.get("tool"))
}

/// The `path` sources of a `[tool]` table, from `dir`.
fn sourced(dir: &Path, tool: Option<&toml::Value>) -> Result<Vec<PathBuf>, Error> {
    let sources = tool.and_then(|t| t.get("uv")).and_then(|u| u.get("sources")).and_then(|s| s.as_table());
    let mut out = vec![];
    for source in sources.into_iter().flat_map(|s| s.values()) {
        let Some(path) = source.get("path").and_then(|p| p.as_str()) else { continue };
        let dependency = fs::canonicalize(dir.join(path)).map_err(|e| refuse("local_dependency_unreadable", format!("{path}: {e}")))?;
        out.push(dependency);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// The deepest folder holding `first` and every one of `rest`.
pub fn common<'a>(first: &'a Path, rest: impl Iterator<Item = &'a Path>) -> PathBuf {
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

/// A script's `# /// script` metadata block (PEP 723) as TOML; none is an empty table.
pub fn pep723(text: &str) -> Result<toml::Table, Error> {
    let mut lines = text.lines().skip_while(|line| line.trim_end() != "# /// script");
    if lines.next().is_none() {
        return Ok(toml::Table::new());
    }
    let mut block = String::new();
    for line in lines {
        match line.trim_end() {
            "# ///" => return toml::from_str(&block).map_err(|e| refuse("script_metadata_invalid", e)),
            "#" => block.push('\n'),
            line => match line.strip_prefix("# ") {
                Some(content) => block.push_str(&format!("{content}\n")),
                None => break,
            },
        }
    }
    Err(refuse("script_metadata_invalid", "the `# /// script` block has no closing `# ///`"))
}

/// Whether `path` exists with any execute bit set.
#[cfg(unix)]
pub fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
pub fn executable(_path: &Path) -> bool {
    false
}
