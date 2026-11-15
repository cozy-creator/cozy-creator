//! cozy's own update, one path for `cozy update` and Calcifer's update for a package that needs a newer cozy (cl-978): a release's
//! archive for this target fetched, held to its sha256 and its GitHub attestation (`platform::release`), checked to run, and put in
//! place of the installed binary, the one it replaces kept beside it as `.cozy-previous` (design §5). Homebrew's cozy moves by
//! `brew upgrade`. `dir` serves one release's files: GitHub's (`releases/latest/download`, `download/v<version>`), or
//! `download_url`'s (`COZY_DOWNLOAD_URL`, which dist's installers read too).
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

use platform::release::{self, Release, TARGET};

use crate::error::{Error, Kind};

/// A release put in place.
#[derive(Debug)]
pub struct Installed {
    pub to: String,
    pub binary: PathBuf,
    /// The archive and its sha256, and who signed it: none for Homebrew's, which holds its own.
    pub archive: String,
    pub sha256: String,
    pub signer: String,
}

/// Where release `wanted` (none: the Latest) is served: `download_url`'s, else GitHub's.
pub fn dir(download_url: Option<&str>, wanted: Option<&str>) -> Result<String, Error> {
    match download_url {
        Some(url) => Ok(url.trim_end_matches('/').to_owned()),
        None => release::dir(wanted).map_err(refused),
    }
}

/// The release `dir` serves, for this target; `wanted` when named.
pub async fn release(dir: &str, wanted: Option<&str>) -> Result<Release, Error> {
    let manifest = text(&format!("{dir}/dist-manifest.json")).await?;
    Release::read(dir, &manifest, wanted).map_err(refused)
}

/// Puts `release` (served by `dir`; `download_url`: `dir` is that) in place of `binary`, the installed cozy: Homebrew's by `brew
/// upgrade`, any other fetched, verified and swapped in. `moved`: the archive's bytes so far.
pub async fn install(dir: &str, download_url: bool, release: &Release, binary: &Path, moved: &mut (dyn FnMut(u64) + Send)) -> Result<Installed, Error> {
    if let Some(stable) = homebrew(binary) {
        return brew_upgrade(&stable, &release.version).await;
    }
    // Its folder takes the new binary beside it before anything is fetched: one that isn't this user's says so at once.
    let folder = binary.parent().unwrap_or(Path::new("."));
    tempfile::Builder::new().prefix(".cozy-writable-").tempfile_in(folder).map_err(|e| unwritable(binary, "write beside", e))?;
    let url = match download_url {
        true => format!("{dir}/{}", release.archive),
        false => release.url(),
    };
    let bundles = format!("{url}.sigstore.jsonl");
    let signer = release.signer(&bundles, &text(&bundles).await?).map_err(refused)?;
    let part = binary.with_file_name(format!(".cozy-{}.part", release.sha256.get(..12).unwrap_or_default()));
    let (archive, sha256) = tensorfs::fetch::get_pinned_into(&url, &part, moved).await.map_err(|e| unavailable(&url, e))?;
    if sha256 != release.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(Error::new(Kind::Validation, "checksum_mismatch", format!("{url} has sha256 {sha256}, not the one its release names")));
    }
    let new = unpack(release, archive, binary).map_err(|e| unwritable(binary, "unpack it beside", e))?;
    let _ = std::fs::remove_file(&part);
    let version = &release.version;
    let runs = std::process::Command::new(&new).arg("--version").output().map_err(|e| local("run the new binary", e))?;
    if String::from_utf8_lossy(&runs.stdout).trim() != format!("cozy {version}") {
        return Err(Error::new(
            Kind::Validation,
            "new_binary_refused",
            format!("cozy {version} for {TARGET} does not run here: {}", String::from_utf8_lossy(&runs.stderr).trim()),
        ));
    }
    keep_previous(binary).map_err(|e| unwritable(binary, "keep the binary it replaces beside", e))?;
    platform::durable::replace_program(binary, &new).map_err(|e| unwritable(binary, "replace", e))?;
    Ok(Installed { to: version.clone(), binary: binary.to_owned(), archive: release.archive.clone(), sha256, signer })
}

/// Homebrew's `<prefix>/bin/cozy` for a cozy it installed at `<prefix>/Cellar/<formula>/<version>/bin/cozy`, a path every upgrade
/// moves.
pub fn homebrew(exe: &Path) -> Option<PathBuf> {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_owned());
    let bin = exe.parent()?;
    let cellar = bin.parent()?.parent()?.parent()?;
    if bin.file_name()? != "bin" || cellar.file_name()? != "Cellar" {
        return None;
    }
    Some(cellar.parent()?.join("bin").join(exe.file_name()?))
}

/// Homebrew's cozy at `stable` (`<prefix>/bin/cozy`) moved to `wanted` or newer by `brew upgrade`, as Homebrew updates what it
/// installed. One it holds (`brew pin`) stays, refused naming the commands that move it.
async fn brew_upgrade(stable: &Path, wanted: &str) -> Result<Installed, Error> {
    let formula = stable.canonicalize().ok().and_then(|exe| Some(exe.parent()?.parent()?.parent()?.file_name()?.to_str()?.to_owned())).unwrap_or_else(|| "cozy".into());
    let brew = stable.with_file_name("brew");
    let run = move |args: Vec<String>| {
        let brew = brew.clone();
        async move {
            tokio::task::spawn_blocking(move || std::process::Command::new(&brew).args(&args).env("HOMEBREW_NO_INSTALL_CLEANUP", "1").output())
                .await
                .map_err(|e| io::Error::other(e.to_string()))
                .and_then(|out| out)
        }
    };
    let upgrade = format!("brew upgrade {formula}");
    let pinned = run(vec!["list".into(), "--pinned".into()]).await.map_err(|e| brew_failed(&upgrade, e.to_string()))?;
    if String::from_utf8_lossy(&pinned.stdout).lines().any(|line| line.trim() == formula) {
        return Err(Error::new(Kind::Conflict, "homebrew_pinned", format!("Homebrew holds {formula} where it is (brew pin): cozy {wanted} isn't installed"))
            .next(format!("brew unpin {formula} && {upgrade}")));
    }
    let out = run(vec!["upgrade".into(), formula.clone()]).await.map_err(|e| brew_failed(&upgrade, e.to_string()))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        return Err(brew_failed(&upgrade, said.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("it failed").trim().to_owned()));
    }
    let runs = std::process::Command::new(stable).arg("--version").output().map_err(|e| local("run Homebrew's cozy", e))?;
    let said = String::from_utf8_lossy(&runs.stdout).trim().trim_start_matches("cozy ").to_owned();
    if !newer(&said, wanted) && said != wanted {
        return Err(brew_failed(&upgrade, format!("it left cozy {said}, not {wanted}: its tap may not have {wanted} yet")));
    }
    Ok(Installed { to: said, binary: stable.to_owned(), archive: String::new(), sha256: String::new(), signer: "Homebrew".into() })
}

fn brew_failed(upgrade: &str, why: String) -> Error {
    Error::new(Kind::Unavailable, "homebrew_upgrade_failed", format!("`{upgrade}` didn't update cozy: {why}")).next(upgrade.to_owned())
}

/// Whether release `a` is newer than `b` (`0.26.0` over `0.25.3`), build metadata aside. Unparsable: not newer.
pub fn newer(a: &str, b: &str) -> bool {
    match (semver::Version::parse(a.trim_start_matches('v')), semver::Version::parse(b.trim_start_matches('v'))) {
        (Ok(a), Ok(b)) => a.cmp_precedence(&b).is_gt(),
        _ => false,
    }
}

/// A release's major: across one cozy doesn't update itself (Paul, 11-13). Pre-1.0 a minor is a minor, so `0.x` is major 0.
pub fn major(version: &str) -> u64 {
    semver::Version::parse(version.trim_start_matches('v')).map_or(u64::MAX, |version| version.major)
}

/// Keeps the binary being replaced beside it as `.cozy-previous`, a machine's rollback when the new one never serves.
fn keep_previous(binary: &Path) -> io::Result<()> {
    let previous = binary.with_file_name(".cozy-previous");
    match std::fs::remove_file(&previous) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    std::fs::hard_link(binary, &previous).or_else(|_| std::fs::copy(binary, &previous).map(drop))
}

/// Writes the archive's `cozy` beside `binary` (a file there can run, and the swap stays on one disk).
fn unpack(release: &Release, archive: File, binary: &Path) -> io::Result<tempfile::TempPath> {
    let mut file = tempfile::Builder::new()
        .prefix(".cozy-update-")
        .suffix(std::env::consts::EXE_SUFFIX)
        .tempfile_in(binary.parent().unwrap_or(Path::new(".")))?;
    release.unpack(archive, &mut file)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file().set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }
    // Closed: neither OS runs a file another handle still writes.
    Ok(file.into_temp_path())
}

/// A release document, read whole by TensorFS's one GET rule; the archive it names is held to its digest.
async fn text(url: &str) -> Result<String, Error> {
    let mut body = Vec::new();
    let mut sink = |at: u64, frame: &[u8]| -> tensorfs::err::Result<()> {
        body.truncate(at as usize);
        body.extend_from_slice(frame);
        Ok(())
    };
    match tensorfs::fetch::get_pinned(url, 0, &mut sink).await {
        Ok(()) => String::from_utf8(body).map_err(|e| unavailable(url, e)),
        Err(e) if e.code == tensorfs::err::Code::REF_NOT_FOUND => Err(Error::new(Kind::NotFound, "release_not_found", format!("{url}: not found"))),
        Err(e) => Err(unavailable(url, e)),
    }
}

fn refused(e: release::Refused) -> Error {
    let kind = match e.code {
        "release_not_found" => Kind::NotFound,
        "unattested" => Kind::Validation,
        _ => Kind::Unavailable,
    };
    Error::new(kind, e.code, e.message)
}

fn unavailable(url: &str, error: impl std::fmt::Display) -> Error {
    Error::new(Kind::Unavailable, "release_unavailable", format!("{url}: {error}"))
}

/// The installed binary's folder isn't this user's to write (a system-wide install): its owner updates it.
fn unwritable(binary: &Path, op: &str, error: io::Error) -> Error {
    match error.kind() {
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
            Error::new(Kind::Conflict, "cozy_unwritable", format!("cozy can't {op} {}: {error}", binary.display())).next("sudo cozy update")
        }
        _ => local(&format!("{op} {}", binary.display()), error),
    }
}

fn local(op: &str, error: io::Error) -> Error {
    Error::new(Kind::Internal, "update_failed", format!("{op}: {error}"))
}
