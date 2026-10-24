//! What `cozy run` runs: `[org/]pkg[/fn][@release]` or local code, `./dir[/fn]` or
//! `./script.py[/fn]` (a script is a one-file package). A package with no org is first-party, the
//! root's (xs-071).
use std::path::{Path, PathBuf};

use protocol::error::{Error, Kind};
use protocol::names::ROOT;

#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    Published {
        /// `org/pkg`, lowercase (names are never case-sensitive); `_/pkg` for a root package.
        package: String,
        function: Option<String>,
        /// Empty: the machine runs the release it holds, else the newest.
        release: Option<String>,
        /// `x/y`'s other reading, account x's package y, when `package` is root package x with
        /// function y. Accounts and root names never collide, so the Hub holds at most one.
        owned: Option<String>,
    },
    /// A package directory or a script, on this computer.
    Local {
        path: PathBuf,
        function: Option<String>,
    },
}

/// Whether `text` is written as a path on this computer, never a name: `.`, `..` or `~`, alone or
/// before a separator (`./dir`, `..\dir` on Windows), or a rooted or absolute path (`/dir`,
/// `C:\dir`). The CLI's one test, for code and for input files.
pub(crate) fn written_as_path(text: &str) -> bool {
    let text = text.trim();
    let led = |lead: &str| text.strip_prefix(lead).is_some_and(|rest| rest.is_empty() || rest.starts_with(std::path::is_separator));
    led(".") || led("..") || led("~") || text.starts_with(std::path::is_separator) || Path::new(text).is_absolute()
}

/// The path `text` spells: `~` is `home`, and a relative one is under `cwd`.
pub(crate) fn spelled(text: &str, cwd: &Path, home: &Path) -> PathBuf {
    let path = Path::new(text.trim());
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => cwd.join(path),
    }
}

/// Local code `text` names, when it is written as a path or names a script here (`x.py`).
pub(crate) fn local(text: &str, cwd: &Path, home: &Path) -> Option<PathBuf> {
    let text = text.trim();
    let script = text.ends_with(".py") && cwd.join(text).is_file();
    (written_as_path(text) || script).then(|| spelled(text, cwd, home))
}

impl Target {
    /// Reads a target. `cwd` and `home` place local code.
    pub fn parse(raw: &str, cwd: &Path, home: &Path) -> Result<Self, Error> {
        let raw = raw.trim();
        let Some(path) = local(raw, cwd, home) else {
            return Self::published(raw);
        };
        let package = |path: &Path| path.is_dir() || (path.is_file() && path.extension().is_some_and(|e| e == "py"));
        if package(&path) {
            return Ok(Target::Local { path, function: None });
        }
        let function = path.file_name().map(|n| n.to_string_lossy().into_owned());
        match path.parent().filter(|parent| package(parent)) {
            Some(parent) => Ok(Target::Local { path: parent.to_path_buf(), function }),
            None => Err(Error::new(
                Kind::NotFound,
                "target_not_found",
                format!("{raw} is not a package directory or a script on this computer"),
            )),
        }
    }

    /// The one package grammar, `[org/]pkg[/fn][@release]`: names and releases are letters,
    /// digits, `-`, `_` and `.`, and names are lowercase (never case-sensitive).
    pub(crate) fn published(raw: &str) -> Result<Self, Error> {
        let (path, release) = match raw.split_once('@') {
            Some((path, release)) => (path, Some(release)),
            None => (raw, None),
        };
        let parts: Vec<&str> = path.split('/').collect();
        let malformed = || {
            Error::new(
                Kind::Usage,
                "target_malformed",
                format!("{raw:?} is not [org/]package[/function][@release]"),
            )
            .remedy("omit /function to list the package's functions; ./dir or ./script.py is local code")
            .next("cozy package search <words>")
        };
        if parts.len() == 4 {
            return Err(malformed().remedy(format!(
                "a release goes last: {}/{}/{}@{}",
                parts[0], parts[1], parts[3], parts[2]
            )));
        }
        let valid = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if parts.len() > 3 || !parts.iter().all(|p| valid(p)) || !release.is_none_or(valid) {
            return Err(malformed());
        }
        let release = release.map(str::to_owned);
        let lower = |s: &str| s.to_ascii_lowercase();
        Ok(match parts[..] {
            [name] => Target::Published { package: format!("{ROOT}/{}", lower(name)), function: None, release, owned: None },
            [first, second] if first == ROOT => Target::Published {
                package: format!("{ROOT}/{}", lower(second)), function: None, release, owned: None,
            },
            [first, second] => Target::Published {
                package: format!("{ROOT}/{}", lower(first)),
                function: Some(second.to_owned()),
                release,
                owned: Some(format!("{}/{}", lower(first), lower(second))),
            },
            _ => Target::Published {
                package: format!("{}/{}", lower(parts[0]), lower(parts[1])),
                function: Some(parts[2].to_owned()),
                release,
                owned: None,
            },
        })
    }
}

/// The package and function a published target names, and its release: for `x/y`, root package
/// x's function y when the Hub holds root package x, else account x's package y (one read when it
/// is the root's; an older Hub answers the root's absent).
pub(crate) async fn resolve(
    ctx: &crate::Context,
    hub: &protocol::config::Hub,
    package: String,
    function: Option<String>,
    release: Option<&str>,
    owned: Option<String>,
) -> Result<(String, Option<String>, super::Resolved), Error> {
    let root = super::resolve(ctx, hub, &package, release).await;
    match (root, owned) {
        (Ok(resolved), _) => Ok((package, function, resolved)),
        (Err(e), Some(owned)) if matches!(e.kind, Kind::NotFound | Kind::Usage) => {
            match super::resolve(ctx, hub, &owned, release).await {
                Ok(resolved) => Ok((owned, None, resolved)),
                Err(e) if e.kind == Kind::NotFound => {
                    let first = package.trim_start_matches("_/");
                    Err(Error::new(Kind::NotFound, "package_not_found",
                        format!("the Hub has neither a package {first} nor {owned}"))
                        .next(format!("cozy package search {first}")))
                }
                Err(e) => Err(e),
            }
        }
        (Err(e), _) => Err(e),
    }
}
