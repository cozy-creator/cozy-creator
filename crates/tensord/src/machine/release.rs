//! A `cozy` release the machine fetches itself (`cozy machine update --tensord <version>`), verified
//! as `cozy update` verifies one (`platform::release`): the archive lands through the machine's
//! store held to the manifest's sha256, as a published wheel does (`pypi`).
use std::{
    fs, io,
    path::{Path, PathBuf},
};

use platform::release::{self, Release};

use super::update::Coded;
use crate::{origin::Machine, stop::Stop};

/// cozy `version`'s binary for this machine, unpacked into `dir`.
pub(crate) fn cozy(version: &str, dir: &Path, machine: &Machine, stop: &Stop) -> io::Result<PathBuf> {
    let refused = |e: release::Refused| io::Error::other(Coded { code: e.code, message: e.message });
    let base = release::dir(Some(version)).map_err(refused)?;
    let manifest = text(machine, stop, format!("{base}/dist-manifest.json"))?;
    let release = Release::read(&base, &manifest, Some(version)).map_err(refused)?;
    let url = release.url();
    let bundles = format!("{url}.sigstore.jsonl");
    let signer = release.signer(&bundles, &text(machine, stop, bundles.clone())?).map_err(refused)?;
    let archive = super::pypi::object_at(machine, &url, &release.sha256, None, None, crate::origin::priority::NOW, stop).map_err(|e| io::Error::other(format!("{url}: {e}")))?;
    fs::create_dir_all(dir)?;
    let binary = dir.join(format!("cozy-{version}"));
    release.unpack(fs::File::open(archive)?, &mut fs::File::create(&binary)?)?;
    tracing::info!("cozy {version}: {url} sha256 {}, signed by {signer}", release.sha256);
    Ok(binary)
}

/// A release's text file through `pypi::text`'s retry rule; its signature judges it.
fn text(machine: &Machine, stop: &Stop, url: String) -> io::Result<String> {
    super::pypi::text(machine, stop, url).map_err(|e| io::Error::other(e.to_string()))
}
