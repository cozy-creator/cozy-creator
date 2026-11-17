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
            .remedy("Use cozy run <package> to list functions, or cozy run ./path for local code.")
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
        // `@latest` is what naming no release means.
        let release = release.filter(|release| !release.eq_ignore_ascii_case("latest")).map(str::to_owned);
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

/// Code at a public git repository (cl-1127): `https://<host>/<repository>[@<ref>][/<path>]`, as pip and uv write it too
/// (`git+https://…`, `#subdirectory=<folder>`), and a GitHub page's `/tree/<ref>/<path>`. `path` is in the repository as
/// `./dir/fn` is here: a package's folder or script, then a function. On github.com, gitlab.com, codeberg.org and bitbucket.org
/// a repository is its owner and name; elsewhere it ends at `.git`, else at the URL's end. Over https and with no credential:
/// public repositories only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Git {
    /// `https://<host>/<repository>`, as it is fetched.
    pub url: String,
    /// A branch, tag or commit; none: the repository's default branch.
    pub reference: Option<String>,
    /// Inside the repository, `/`-separated: a package's folder or script and a function after it; empty: its root.
    pub path: String,
}

impl Git {
    /// The repository `text` names when it is a URL; none when it's no URL. One cozy doesn't fetch (ssh, a credential) is refused.
    pub fn parse(text: &str) -> Option<Result<Self, Error>> {
        let text = text.trim();
        let rest = text.strip_prefix("git+").unwrap_or(text);
        let refused = |why: &str| {
            Error::new(Kind::Usage, "git_url_unsupported", format!("{text}: {why}"))
                .remedy("Name a public repository by its https URL: https://github.com/<owner>/<repository>[@<ref>]")
        };
        let https = match rest.split_once("://") {
            Some((scheme, after)) if scheme.eq_ignore_ascii_case("https") => after,
            Some((scheme, _)) if ["http", "ssh", "git"].iter().any(|s| scheme.eq_ignore_ascii_case(s)) => {
                return Some(Err(refused("Cozy fetches repositories over https only")));
            }
            Some(_) => return None,
            None if rest.starts_with("git@") => return Some(Err(refused("Cozy fetches repositories over https only"))),
            None => return None,
        };
        let (https, fragment) = https.split_once('#').unwrap_or((https, ""));
        let https = https.split_once('?').map_or(https, |(before, _)| before);
        let (host, path) = https.split_once('/').unwrap_or((https, ""));
        if host.contains('@') {
            return Some(Err(refused("a URL with a credential isn't taken: Cozy installs public repositories")));
        }
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')) {
            return Some(Err(refused("its host isn't a host name")));
        }
        let host = host.to_ascii_lowercase();
        let mut segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let named = ["github.com", "gitlab.com", "codeberg.org", "bitbucket.org"].contains(&host.as_str());
        // Where the repository ends: a `.git` segment, else its owner and name on a known host, else the whole path; a `@ref`
        // on its last segment.
        let at = |s: &str| s.split_once('@').map_or(s, |(name, _)| name).to_owned();
        let end = (segments.iter().position(|s| at(s).ends_with(".git")))
            .or_else(|| named.then_some(1).filter(|_| segments.len() >= 2))
            .unwrap_or(segments.len().saturating_sub(1));
        if segments.is_empty() {
            return Some(Err(refused("it names no repository")));
        }
        let mut reference = None;
        if let Some((name, written)) = segments[end].split_once('@') {
            reference = Some(written.to_owned());
            segments[end] = name;
        }
        let repository = segments[..=end].join("/");
        let mut inside: Vec<&str> = segments[end + 1..].to_vec();
        // A GitHub page: `/tree/<ref>/<path>` or `/blob/<ref>/<path>`.
        if reference.is_none() && matches!(inside.first(), Some(&"tree" | &"blob")) && inside.len() >= 2 {
            reference = Some(inside[1].to_owned());
            inside.drain(..2);
        }
        let mut path = inside.join("/");
        for pair in fragment.split('&') {
            if let Some(folder) = pair.strip_prefix("subdirectory=") {
                let folder = folder.trim_matches('/');
                path = [folder, path.as_str()].iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join("/");
            }
        }
        let valid = |r: &str| !r.is_empty() && !r.contains("..") && r.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '+'));
        if reference.as_deref().is_some_and(|r| !valid(r)) {
            return Some(Err(refused("its ref isn't a branch, tag or commit")));
        }
        if path.split('/').any(|part| part == ".." || part == ".") {
            return Some(Err(refused("its path leaves the repository")));
        }
        Some(Ok(Self { url: format!("https://{host}/{repository}"), reference, path }))
    }

    /// The repository as it is named whatever its spelling: no `.git`, its host lowercase.
    pub fn repository(&self) -> &str {
        self.url.strip_suffix(".git").unwrap_or(&self.url)
    }
}

/// `git+<url>@<commit>[#subdirectory=<folder>]`: code at a repository's commit, as pip and uv name it (PEP 610's VCS URL).
pub fn git_origin(url: &str, commit: &str, folder: &str) -> String {
    match folder.trim_matches('/') {
        "" => format!("git+{url}@{commit}"),
        folder => format!("git+{url}@{commit}#subdirectory={folder}"),
    }
}

