//! What a run names: a published package, `[org/]pkg[/fn][@release]` (a package with no org is
//! first-party, the root's, xs-071), or local code written as a path (`./dir[/fn]`,
//! `./script.py[/fn]`; a script is a one-file package), which `calcifer::local_code` reads.
use std::path::{Path, PathBuf};

use crate::error::{Error, Kind};
use crate::names::ROOT;

/// A published target.
#[derive(Debug, PartialEq, Eq)]
pub struct Published {
    /// `org/pkg`, lowercase (names are never case-sensitive); `_/pkg` for a root package.
    pub package: String,
    pub function: Option<String>,
    /// None: the newest.
    pub release: Option<String>,
    /// `x/y`'s other reading, account x's package y, when `package` is root package x with
    /// function y. Accounts and root names never collide, so the Hub holds at most one.
    pub owned: Option<String>,
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

impl Published {
    /// The one package grammar, `[org/]pkg[/fn][@release]`: names and releases are letters,
    /// digits, `-`, `_` and `.`, and names are lowercase (never case-sensitive).
    pub fn parse(raw: &str) -> Result<Self, Error> {
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
            .next("cozy package ls <words>")
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
            [name] => Published { package: format!("{ROOT}/{}", lower(name)), function: None, release, owned: None },
            [first, second] if first == ROOT => Published {
                package: format!("{ROOT}/{}", lower(second)), function: None, release, owned: None,
            },
            [first, second] => Published {
                package: format!("{ROOT}/{}", lower(first)),
                function: Some(second.to_owned()),
                release,
                owned: Some(format!("{}/{}", lower(first), lower(second))),
            },
            _ => Published {
                package: format!("{}/{}", lower(parts[0]), lower(parts[1])),
                function: Some(parts[2].to_owned()),
                release,
                owned: None,
            },
        })
    }
}
