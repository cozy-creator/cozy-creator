//! Which release a name resolves to: one rule for Calcifer and TensorD, for packages and models (xs-183, cl-952, se-128).
//! A prerelease (`0.10.0rc1`) is built for the open RC's Runtime (cl-922), so it counts only on a machine on the RC (`rc`), or
//! after a prerelease named (a model's `1.0.0-rc.8`); on the RC the newest by PEP 440 alone, elsewhere finals only.

use std::str::FromStr;

use pep440_rs::Version;

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

fn newest_where<'a>(releases: impl IntoIterator<Item = &'a str>, keep: impl Fn(&Version) -> bool, pre: bool) -> Option<&'a str> {
    releases
        .into_iter()
        .filter_map(|text| Some((Version::from_str(text).ok()?, text)))
        .filter(|(version, _)| keep(version) && (pre || !version.any_prerelease()))
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, text)| text)
}
