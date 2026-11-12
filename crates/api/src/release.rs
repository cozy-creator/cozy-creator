//! Which release a name resolves to: one rule for Calcifer and TensorD, for packages and models (xs-183, cl-952, se-128).
//! A prerelease (`0.10.0rc1`) is built for the open RC's Runtime (cl-922), so it counts only on a machine on the RC (`rc`), or
//! after a prerelease named (a model's `1.0.0-rc.8`); on the RC the newest by PEP 440 alone, elsewhere finals only.
//!
//! A package release runs only on a Runtime its `cozy-runtime` requirement admits (`runs`). The open RC's Runtime is the next
//! minor's dev release (`0.46.0.dev312+g1a2b3c4d`), so a floor can name it. `choose` decides a run's release with its machine:
//! the newest it takes, moving the machine to the newest Runtime that runs it; a machine held by hand refuses instead, and one
//! that isn't this computer's to move takes the newest its Runtime runs.

use std::{future::Future, str::FromStr};

use pep440_rs::{Operator, Version, VersionSpecifier, VersionSpecifiers};

/// A PEP 440 pre- or dev release. An unparsable one is no prerelease.
pub fn prerelease(release: &str) -> bool {
    Version::from_str(release).is_ok_and(|version| version.any_prerelease())
}

/// The newest of `releases` a machine runs: on the RC (`rc`) by PEP 440 alone, elsewhere finals only. Unparsable ones don't count.
pub fn newest<'a>(releases: impl IntoIterator<Item = &'a str>, rc: bool) -> Option<&'a str> {
    newest_where(releases, |_| true, rc)
}

/// The release `release` resolves to forward (xs-183): the newest of `releases` in its series and after it, when one is. A
/// named prerelease moves on to a newer one; a named final to finals, or prereleases too on the RC (`rc`).
pub fn newest_fix<'a>(release: &str, releases: impl IntoIterator<Item = &'a str>, rc: bool) -> Option<&'a str> {
    let named = Version::from_str(release).ok()?;
    let series = &named.release()[..named.release().len().saturating_sub(1).max(1)];
    newest_where(releases, |version| version.release().starts_with(series) && *version > named, rc || named.any_prerelease())
}

/// The releases a machine takes, newest first: on the RC (`rc`) every one, elsewhere finals. Unparsable ones don't count.
pub fn candidates<'a>(releases: impl IntoIterator<Item = &'a str>, rc: bool) -> Vec<&'a str> {
    let mut kept: Vec<(Version, &str)> = (releases.into_iter())
        .filter_map(|text| Some((Version::from_str(text).ok()?, text)))
        .filter(|(version, _)| rc || !version.any_prerelease())
        .collect();
    kept.sort_by(|(a, _), (b, _)| b.cmp(a));
    kept.into_iter().map(|(_, text)| text).collect()
}

fn newest_where<'a>(releases: impl IntoIterator<Item = &'a str>, keep: impl Fn(&Version) -> bool, pre: bool) -> Option<&'a str> {
    releases
        .into_iter()
        .filter_map(|text| Some((Version::from_str(text).ok()?, text)))
        .filter(|(version, _)| keep(version) && (pre || !version.any_prerelease()))
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, text)| text)
}

/// What `requires` (a wheel's `Requires-Dist`) asks of `distribution` outside any extra (`cozy-runtime[media]<0.46,>=0.45.0`):
/// the versions it admits. None: it names none with a version, or one that doesn't parse.
pub fn required<'a>(requires: impl IntoIterator<Item = &'a str>, distribution: &str) -> Option<VersionSpecifiers> {
    let wanted = normalized(distribution);
    let mut found: Vec<VersionSpecifier> = vec![];
    for line in requires {
        let (line, marker) = line.split_once(';').unwrap_or((line, ""));
        if marker.contains("extra") {
            continue;
        }
        let end = line.find(|c: char| " <>=!~[(".contains(c)).unwrap_or(line.len());
        if normalized(&line[..end]) != wanted {
            continue;
        }
        let rest = line[end..].trim_start();
        let rest = match rest.strip_prefix('[') {
            Some(extras) => &extras[extras.find(']')? + 1..],
            None => rest,
        };
        let rest = rest.trim().trim_start_matches('(').trim_end_matches(')').trim();
        if rest.is_empty() || rest.starts_with('@') {
            continue;
        }
        found.extend(VersionSpecifiers::from_str(rest).ok()?.iter().cloned());
    }
    (!found.is_empty()).then(|| found.into_iter().collect())
}

/// A distribution's name as PEP 503 compares it.
fn normalized(name: &str) -> String {
    name.trim().split(['-', '_', '.']).filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-").to_ascii_lowercase()
}

/// Whether a Runtime `version` meets `requirement`. Its floor holds as written. A build of the open RC (`0.46.0.dev312`, the
/// next minor's dev release) also meets a cap at that minor (`<0.46`): the RC's machines run released packages to try it, and
/// one that breaks there is a finding. An unparsable version holds nothing back.
pub fn runs(requirement: &VersionSpecifiers, version: &str) -> bool {
    let Ok(version) = Version::from_str(version) else { return true };
    requirement.iter().all(|specifier| specifier.contains(&version) || rc_cap(specifier, &version))
}

/// Whether `specifier` caps at the minor `version` is the open RC's build of: `<0.46` for `0.46.0.dev312`.
fn rc_cap(specifier: &VersionSpecifier, version: &Version) -> bool {
    let minor = |release: &[u64]| match release {
        [major, minor, rest @ ..] if rest.iter().all(|n| *n == 0) => Some((*major, *minor)),
        _ => None,
    };
    let built = version.is_dev() && !version.is_pre() && !version.is_post();
    let cap = specifier.version();
    built && *specifier.operator() == Operator::LessThan && cap.is_stable() && minor(cap.release()).is_some() && minor(cap.release()) == minor(version.release())
}

/// Whether a machine moves its Runtime for a release its own doesn't run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moves {
    Yes,
    /// Held by hand on what it runs: it refuses, and its update by hand names the way on.
    Held,
    /// Not this computer's to move (another's, added here): it takes an older release.
    No,
}

/// A machine, as `choose` reads it.
pub struct Machine<'a> {
    /// Its Runtime; empty while it isn't known (a rental to come), which holds nothing back.
    pub runtime: &'a str,
    /// The Runtimes it can move to: the released ones, and on the RC (cl-922) the RC's build.
    pub runtimes: &'a [String],
    pub moves: Moves,
}

/// The release a run takes, and what its machine does for it.
#[derive(Debug, PartialEq, Eq)]
pub enum Chosen {
    /// Its machine's Runtime runs it.
    Runs(String),
    /// Its machine moves to `runtime`, the newest Runtime that runs it, first.
    Moves { release: String, runtime: String },
    /// An older one than `newest`, which needs `needs` of a Runtime its machine doesn't move to.
    Older { release: String, newest: String, needs: String },
}

/// Why a run takes none of its releases.
#[derive(Debug)]
pub enum Refused<E> {
    /// Its machine is held by hand: `release` needs `needs`, which `runtime` would meet.
    Held { release: String, needs: String, runtime: String },
    /// No Runtime its machine runs or can move to runs any of them: the newest, and what it needs.
    Unmet { release: String, needs: String },
    /// A release's requirement couldn't be read.
    Unread(E),
}

/// The release of `candidates` (newest first: `candidates`, or the one asked) a run on `machine` takes, reading each one's
/// `cozy-runtime` requirement (`requirement`) only as far as it looks: the newest its Runtime runs, else the newest it moves up
/// to a Runtime for (the newest that runs it). A machine held by hand refuses that move; one that doesn't move takes the newest
/// its Runtime runs. A release no newer Runtime it could take runs is passed over.
pub async fn choose<E, F, Fut>(candidates: &[&str], machine: &Machine<'_>, mut requirement: F) -> Result<Chosen, Refused<E>>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Option<VersionSpecifiers>, E>>,
{
    let mut first: Option<(String, String)> = None;
    for release in candidates {
        let Some(needs) = requirement((*release).to_owned()).await.map_err(Refused::Unread)? else {
            return Ok(chosen(release, first));
        };
        if machine.runtime.is_empty() || runs(&needs, machine.runtime) {
            return Ok(chosen(release, first));
        }
        // Only up: an older Runtime would take from what the machine runs now.
        let running = Version::from_str(machine.runtime).ok();
        let moved = (machine.runtimes.iter())
            .filter(|runtime| runs(&needs, runtime))
            .filter_map(|runtime| Some((Version::from_str(runtime).ok()?, runtime)))
            .filter(|(version, _)| running.as_ref().is_none_or(|running| version > running))
            .max_by(|(a, _), (b, _)| a.cmp(b))
            .map(|(_, runtime)| runtime.clone());
        match (moved, machine.moves) {
            (Some(runtime), Moves::Yes) => return Ok(Chosen::Moves { release: (*release).to_owned(), runtime }),
            (Some(runtime), Moves::Held) => return Err(Refused::Held { release: (*release).to_owned(), needs: needs.to_string(), runtime }),
            _ => {}
        }
        first.get_or_insert_with(|| ((*release).to_owned(), needs.to_string()));
    }
    let (release, needs) = first.unwrap_or_default();
    Err(Refused::Unmet { release, needs })
}

fn chosen(release: &str, passed: Option<(String, String)>) -> Chosen {
    match passed {
        Some((newest, needs)) => Chosen::Older { release: release.to_owned(), newest, needs },
        None => Chosen::Runs(release.to_owned()),
    }
}
