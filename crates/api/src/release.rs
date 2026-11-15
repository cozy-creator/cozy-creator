//! Which release a name resolves to: one rule for Calcifer and TensorD, for packages and models (xs-183, cl-952, se-128).
//! A prerelease (`0.10.0rc1`) is built for the open RC's Runtime (cl-922), so it counts only on a machine on the RC (`rc`), or
//! after a prerelease named (a model's `1.0.0-rc.8`); on the RC the newest by PEP 440 alone, elsewhere finals only.
//!
//! A package release runs only on a Runtime its `cozy-runtime` requirement admits (`runs`). The open RC's Runtime is a dev
//! release of the minor it makes (`0.47.0.dev2260+g1a2b3c4d`), so a floor can name it. `choose` decides a run's release with its machine:
//! the newest it takes, moving the machine to the newest Runtime that runs it; a machine held by hand refuses instead, and one
//! that isn't this computer's to move takes the newest its Runtime runs. A Runtime only a newer cozy's line runs (cl-978) is
//! reached by cozy updating itself to that release, a patch or minor; never across a major.

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

/// Whether a Runtime `version` meets `requirement`. Its floor holds as written. A build of the open RC (`0.47.0.dev2260`, a
/// dev release of the minor it makes) also meets any cap (`<0.46`, `<0.47`): the RC's machines run released packages to try
/// them on what's coming, and one that breaks there is a finding. An unparsable version holds nothing back.
pub fn runs(requirement: &VersionSpecifiers, version: &str) -> bool {
    let Ok(version) = Version::from_str(version) else { return true };
    let rc = version.is_dev() && !version.is_pre() && !version.is_post();
    let cap = |specifier: &VersionSpecifier| matches!(specifier.operator(), Operator::LessThan | Operator::LessThanEqual);
    requirement.iter().all(|specifier| specifier.contains(&version) || (rc && cap(specifier)))
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
    /// The Runtimes it can move to on this cozy's line: the released ones this cozy runs, and on the RC (cl-922) the RC's build.
    pub runtimes: &'a [String],
    pub moves: Moves,
    /// The newer cozy whose line runs more Runtimes, when there is one.
    pub newer: Option<&'a Newer>,
}

/// A newer cozy release (the Hub's), and the Runtimes its line runs that this one's doesn't (cl-978).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Newer {
    pub cozy: String,
    pub runtimes: Vec<String>,
    /// Of another major: cozy doesn't update itself to it.
    pub major: bool,
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
    /// Cozy updates itself to `cozy` first, whose line runs `runtime`, the newest that runs it (cl-978).
    Cozy { release: String, cozy: String, runtime: String },
}

/// Why a run takes none of its releases.
#[derive(Debug)]
pub enum Refused<E> {
    /// Its machine is held by hand: `release` needs `needs`, which `runtime` would meet.
    Held { release: String, needs: String, runtime: String },
    /// No Runtime its machine runs or can move to runs any of them: the newest, and what it needs.
    Unmet { release: String, needs: String },
    /// Only a cozy of a new major (`cozy`) runs it, which cozy doesn't update itself to.
    Major { release: String, needs: String, cozy: String },
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
        match takes(&needs, machine) {
            Takes::Runs => return Ok(chosen(release, first)),
            Takes::Moves(runtime) => return Ok(Chosen::Moves { release: (*release).to_owned(), runtime }),
            Takes::Cozy { cozy, runtime } => return Ok(Chosen::Cozy { release: (*release).to_owned(), cozy, runtime }),
            Takes::Held(runtime) => return Err(Refused::Held { release: (*release).to_owned(), needs: needs.to_string(), runtime }),
            Takes::Major(cozy) => return Err(Refused::Major { release: (*release).to_owned(), needs: needs.to_string(), cozy }),
            Takes::No => {}
        }
        first.get_or_insert_with(|| ((*release).to_owned(), needs.to_string()));
    }
    let (release, needs) = first.unwrap_or_default();
    Err(Refused::Unmet { release, needs })
}

/// What `machine` does for one release needing `needs`, as `choose` weighs each.
#[derive(Debug, PartialEq, Eq)]
pub enum Takes {
    /// Its Runtime runs it, or isn't known yet.
    Runs,
    /// It moves up to this Runtime, the newest that runs it.
    Moves(String),
    /// Cozy updates itself to `cozy` first, whose line runs `runtime`, the newest that runs it.
    Cozy { cozy: String, runtime: String },
    /// It would move to this one, but is held by hand.
    Held(String),
    /// Only `cozy`, of a new major, runs it.
    Major(String),
    /// No Runtime it runs or moves to runs it.
    No,
}

/// What `machine` does for a release needing `needs`: its Runtime runs it, else it moves up (never down: an older Runtime would
/// take from what it runs now) to the newest that does on this cozy's line, else on the newer cozy's.
pub fn takes(needs: &VersionSpecifiers, machine: &Machine<'_>) -> Takes {
    if machine.runtime.is_empty() || runs(needs, machine.runtime) {
        return Takes::Runs;
    }
    let running = Version::from_str(machine.runtime).ok();
    let newest = |runtimes: &[String]| {
        (runtimes.iter())
            .filter(|runtime| runs(needs, runtime))
            .filter_map(|runtime| Some((Version::from_str(runtime).ok()?, runtime)))
            .filter(|(version, _)| running.as_ref().is_none_or(|running| version > running))
            .max_by(|(a, _), (b, _)| a.cmp(b))
            .map(|(_, runtime)| runtime.clone())
    };
    match (newest(machine.runtimes), machine.moves) {
        (Some(runtime), Moves::Yes) => return Takes::Moves(runtime),
        (Some(runtime), Moves::Held) => return Takes::Held(runtime),
        (Some(_), Moves::No) => return Takes::No,
        (None, _) => {}
    }
    let Some(newer) = machine.newer else { return Takes::No };
    match (newest(&newer.runtimes), machine.moves) {
        (Some(_), Moves::Yes) if newer.major => Takes::Major(newer.cozy.clone()),
        (Some(runtime), Moves::Yes) => Takes::Cozy { cozy: newer.cozy.clone(), runtime },
        (Some(runtime), Moves::Held) => Takes::Held(runtime),
        _ => Takes::No,
    }
}

/// Whether a `tensorfs` requirement admits a TensorFS of `series`'s minor or older: what a TensorD of the cozy that builds
/// TensorFS `series` speaks (a newer one speaks an older TensorFS's protocol, never a newer one's). None admits any.
pub fn admits_series(requirement: Option<&VersionSpecifiers>, series: &str) -> bool {
    let (Some(requirement), Ok(ours)) = (requirement, Version::from_str(series)) else { return true };
    let minor = |version: &Version| (version.release().first().copied().unwrap_or(0), version.release().get(1).copied().unwrap_or(0));
    let top = minor(&ours);
    let base = Version::new([top.0, top.1, 0]);
    let mut tried = vec![ours.clone(), base];
    for specifier in requirement.iter().filter(|specifier| minor(specifier.version()) <= top) {
        let version = specifier.version().clone();
        let release = version.release().to_vec();
        let mut next = release.clone();
        if let Some(last) = next.last_mut() {
            *last += 1;
        }
        tried.extend([version, Version::new(release), Version::new(next)]);
    }
    tried.iter().filter(|version| minor(version) <= top).any(|version| requirement.contains(version))
}

fn chosen(release: &str, passed: Option<(String, String)>) -> Chosen {
    match passed {
        Some((newest, needs)) => Chosen::Older { release: release.to_owned(), newest, needs },
        None => Chosen::Runs(release.to_owned()),
    }
}
