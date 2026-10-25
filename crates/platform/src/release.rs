//! A `cozy` release, as `cozy update` and a machine's update fetch one: its dist-manifest.json names
//! this target's archive and its sha256, and one of the release's GitHub attestations of that digest
//! (`<archive>.sigstore.jsonl`, one Sigstore bundle per line) must verify as this repo's release
//! workflow, pinned by GitHub repository id so a rename keeps verifying and a repo that takes an old
//! name never does. The caller downloads, holding the archive to the sha256.
use std::{collections::BTreeMap, io};

use sigstore_verify::{
    VerificationPolicy, VerificationResult,
    trust_root::{SigstoreInstance, TrustedRoot},
    types::{Artifact, Bundle, Sha256Hash},
};

pub const RELEASES: &str = "https://github.com/cozy-creator/cozy-creator/releases";
const ISSUER: &str = "https://token.actions.githubusercontent.com";
/// This repo's GitHub id, which a rename keeps.
const REPOSITORY_ID: &str = "1345607976";
/// Its release workflow, on any ref. A hard cut (Paul 10-24): releases signed by release.yml, before
/// it, are refused.
const WORKFLOW: &str = "/.github/workflows/cozy-publish.yaml@";

/// The dist target this build is.
pub const TARGET: &str = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    "x86_64-unknown-linux-gnu"
} else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
    "aarch64-unknown-linux-gnu"
} else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    "aarch64-apple-darwin"
} else if cfg!(all(windows, target_arch = "x86_64")) {
    "x86_64-pc-windows-msvc"
} else if cfg!(all(windows, target_arch = "aarch64")) {
    "aarch64-pc-windows-msvc"
} else {
    "unreleased"
};

/// Why a release is refused: `code` is `release_not_found`, `release_unavailable` or `unattested`.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct Refused {
    pub code: &'static str,
    pub message: String,
}

/// One release's archive for this target.
pub struct Release {
    pub version: String,
    /// Its file name (`cozy-x86_64-unknown-linux-gnu.tar.gz`).
    pub archive: String,
    pub sha256: String,
    tag: String,
}

/// Where release `version`'s files are; `None`: the newest's.
pub fn dir(version: Option<&str>) -> Result<String, Refused> {
    let Some(v) = version else {
        return Ok(format!("{RELEASES}/latest/download"));
    };
    let valid = |c: u8| c.is_ascii_alphanumeric() || b".-+".contains(&c);
    match !v.is_empty() && v.bytes().all(valid) {
        true => Ok(format!("{RELEASES}/download/v{v}")),
        false => Err(not_found(format!("{v:?} is not a cozy release version"))),
    }
}

impl Release {
    /// This target's archive in `manifest`, the dist-manifest.json in `dir`; cozy `wanted` when named.
    pub fn read(dir: &str, manifest: &str, wanted: Option<&str>) -> Result<Self, Refused> {
        #[derive(serde::Deserialize)]
        struct Manifest {
            announcement_tag: String,
            releases: Vec<App>,
            artifacts: BTreeMap<String, Archive>,
        }
        #[derive(serde::Deserialize)]
        struct App {
            app_name: String,
            app_version: String,
        }
        #[derive(serde::Deserialize)]
        struct Archive {
            kind: String,
            #[serde(default)]
            target_triples: Vec<String>,
            #[serde(default)]
            checksums: BTreeMap<String, String>,
        }
        let manifest: Manifest = serde_json::from_str(manifest).map_err(|e| {
            refused(
                "release_unavailable",
                format!("{dir}/dist-manifest.json: {e}"),
            )
        })?;
        let version = (manifest.releases.into_iter())
            .find(|r| r.app_name == "cozy")
            .map(|r| r.app_version)
            .ok_or_else(|| not_found(format!("{dir} holds no cozy release")))?;
        if let Some(wanted) = wanted.filter(|w| *w != version) {
            return Err(not_found(format!(
                "{dir} holds cozy {version}, not {wanted}"
            )));
        }
        let (archive, sha256) = (manifest.artifacts.into_iter())
            .find(|(_, a)| {
                a.kind == "executable-zip" && a.target_triples.iter().any(|t| t == TARGET)
            })
            .and_then(|(name, mut a)| Some((name, a.checksums.remove("sha256")?)))
            .ok_or_else(|| not_found(format!("cozy {version} has no build for {TARGET}")))?;
        Ok(Self {
            version,
            archive,
            sha256,
            tag: manifest.announcement_tag,
        })
    }

    /// The archive on GitHub.
    pub fn url(&self) -> String {
        format!("{RELEASES}/download/{}/{}", self.tag, self.archive)
    }

    /// The release workflow run that signed the archive's sha256: the first of `lines`, the bundles
    /// read from `bundles`, that attests it as this repository's (Sigstore public good).
    pub fn signer(&self, bundles: &str, lines: &str) -> Result<String, Refused> {
        let unattested = |why: String| {
            let message = format!(
                "no bundle in {bundles} verifies as the release workflow of GitHub repository {REPOSITORY_ID}: {why}"
            );
            refused("unattested", message)
        };
        let digest = Sha256Hash::from_hex(&self.sha256).map_err(|e| unattested(e.to_string()))?;
        let root = TrustedRoot::from_embedded(SigstoreInstance::PublicGood)
            .map_err(|e| unattested(e.to_string()))?;
        let policy = VerificationPolicy::any_identity().require_issuer(ISSUER);
        let mut seen = Vec::new();
        for line in lines.lines().filter(|line| !line.trim().is_empty()) {
            let verified = (Bundle::from_json(line).map_err(|e| e.to_string()))
                .and_then(|bundle| {
                    sigstore_verify::verify(Artifact::from_digest(digest), &bundle, &policy, &root)
                        .map_err(|e| e.to_string())
                })
                .and_then(|result| ours(&result));
            match verified {
                Ok(signer) => return Ok(signer),
                Err(e) => seen.push(e),
            }
        }
        Err(unattested(format!("[{}]", seen.join("; "))))
    }

    /// The release's `cozy` out of its archive into `out`: dist's `.tar.gz` holds `<its stem>/cozy`,
    /// its `.zip` `cozy.exe`.
    pub fn unpack(
        &self,
        archive: impl io::Read + io::Seek,
        out: &mut impl io::Write,
    ) -> io::Result<u64> {
        let member = match self.archive.strip_suffix(".tar.gz") {
            Some(stem) => format!("{stem}/cozy"),
            None => "cozy.exe".into(),
        };
        crate::tools::unpack(archive, &member, out)
    }
}

/// The signer, when the certificate is this repository's (by id) and the signer is its own release workflow.
fn ours(result: &VerificationResult) -> Result<String, String> {
    let signer = result.identity().map_or("", |id| id.as_str());
    let claims = result.certificate().map(|cert| &cert.ci_claims);
    let repository = claims
        .and_then(|c| c.source_repository_uri.as_deref())
        .unwrap_or_default();
    let id = claims
        .and_then(|c| c.source_repository_identifier.as_deref())
        .unwrap_or_default();
    let release_workflow = !repository.is_empty()
        && signer
            .strip_prefix(repository)
            .is_some_and(|path| path.starts_with(WORKFLOW));
    if result.certificate_verified() && id == REPOSITORY_ID && release_workflow {
        Ok(signer.to_owned())
    } else {
        Err(format!("signed by {signer} (GitHub repository {id})"))
    }
}

fn refused(code: &'static str, message: String) -> Refused {
    Refused { code, message }
}

fn not_found(message: String) -> Refused {
    refused("release_not_found", message)
}
