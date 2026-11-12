//! A Linux tmpfs profile is volatile application storage, not a disk temporary-directory claim.
use std::path::{Path, PathBuf};
use ::api::error::{Error, Kind};
use crate::api::Refusal;
use platform::{durable, process::Birth};
use serde::{Deserialize, Serialize};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Identity { device: u64, inode: u64 }

pub(crate) struct VolatileProfileLease { path: PathBuf, identity: Identity, receipt: Receipt }

const RECEIPT: &str = ".cozy-browser-owner-v1.json";
const LIMIT: u64 = 256 << 10;
static HOME_IDENTITY: std::sync::OnceLock<Identity> = std::sync::OnceLock::new();

/// Only ownership facts: no project reference, URL, session credential or private payload.
/// The nonce names this profile generation and cannot authorize any API request.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u8,
    home: Identity,
    profile: Identity,
    caf: Birth,
    chrome: Option<Birth>,
    observed: Vec<Birth>,
    nonce: String,
}

/// Only the owned-browser retirement path can mint this one-use proof.
pub(super) struct BrowserRetired {
    pub(super) profile: Identity,
    pub(super) birth: platform::process::Birth,
    pub(super) session: String,
}

fn refused() -> Refusal {
    Refusal(Error::new(Kind::Unavailable, "private_browser_profile", "Cozy needs a verified volatile private-browser profile on this platform"))
}

fn pending() -> Refusal {
    Refusal(Error::new(Kind::Unavailable, "private_browser_recovery_pending", "Cozy cannot yet verify retirement of an earlier owned browser profile"))
}

#[cfg(target_os = "linux")]
fn identity(file: &std::fs::File) -> Result<Identity, Refusal> {
    let id = durable::FileId::of(file).map_err(|_| pending())?;
    Ok(Identity { device: id.dev, inode: id.ino })
}

#[cfg(target_os = "linux")]
fn directory(path: &Path) -> Result<std::fs::File, Refusal> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new().read(true)
        .custom_flags((rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC).bits() as i32)
        .open(path).map_err(|_| pending())
}

#[cfg(target_os = "linux")]
fn parents() -> Vec<PathBuf> {
    let candidates = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).into_iter().chain([PathBuf::from("/dev/shm")]);
    let mut parents = Vec::new();
    for parent in candidates {
        if !parent.is_absolute() { continue; }
        let Ok(parent) = parent.canonicalize() else { continue };
        let Ok(stat) = rustix::fs::statfs(&parent) else { continue };
        if stat.f_type as u64 == 0x0102_1994 && !parents.contains(&parent) { parents.push(parent); }
    }
    parents.sort_by_key(|path| path.as_os_str().as_encoded_bytes().len());
    parents
}

#[cfg(target_os = "linux")]
fn fd_path(file: &std::fs::File) -> PathBuf {
    use std::os::fd::AsRawFd as _;
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

#[cfg(target_os = "linux")]
fn read_receipt(dir: &std::fs::File) -> Result<Receipt, Refusal> {
    use std::{io::Read as _, os::unix::fs::MetadataExt as _};
    let fd = rustix::fs::openat(dir, RECEIPT, rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC, rustix::fs::Mode::empty()).map_err(|_| pending())?;
    let file = std::fs::File::from(fd);
    let meta = file.metadata().map_err(|_| pending())?;
    if !meta.is_file() || meta.uid() != rustix::process::geteuid().as_raw() || meta.mode() & 0o777 != 0o600 || meta.nlink() != 1 || meta.len() > LIMIT { return Err(pending()); }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes).map_err(|_| pending())?;
    if bytes.len() as u64 > LIMIT { return Err(pending()); }
    serde_json::from_slice(&bytes).map_err(|_| pending())
}

/// Called with the exclusive default-home Caf lock, before serving or launching any browser.
/// A prefix only finds candidates: native identity + receipt establish ownership before action.
#[cfg(target_os = "linux")]
pub(super) fn recover(home: &Path) -> Result<(), Refusal> {
    use std::os::unix::fs::MetadataExt as _;
    let file = directory(home)?;
    let meta = file.metadata().map_err(|_| pending())?;
    if meta.uid() != rustix::process::geteuid().as_raw() || meta.mode() & 0o077 != 0 { return Err(pending()); }
    let home = identity(&file)?;
    if HOME_IDENTITY.get().is_some_and(|held| *held != home) { return Err(pending()); }
    HOME_IDENTITY.get_or_init(|| home);
    for parent in parents() {
        let parent = directory(&parent)?;
        let entries = std::fs::read_dir(fd_path(&parent)).map_err(|_| pending())?;
        for (index, entry) in entries.enumerate() {
            if index >= 16_384 { return Err(pending()); }
            let entry = entry.map_err(|_| pending())?;
            let name = entry.file_name();
            let Some(name_text) = name.to_str().filter(|name| name.starts_with("cozy-private-")) else { continue };
            // Never follow an apparent profile link or inspect another user's directory.
            let meta = std::fs::symlink_metadata(entry.path()).map_err(|_| pending())?;
            if meta.uid() != rustix::process::geteuid().as_raw() { continue; }
            if !meta.is_dir() || meta.mode() & 0o777 != 0o700 { return Err(pending()); }
            let dir = directory(&entry.path())?;
            let profile = identity(&dir)?;
            if profile.device != identity(&parent)?.device || profile.device != meta.dev() || profile.inode != meta.ino() { return Err(pending()); }
            let record = read_receipt(&dir)?;
            let valid_nonce = record.nonce.len() == 48 && record.nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
                || record.nonce.len() == 32 && record.nonce.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
            if record.version != 1 || record.profile != profile || !valid_nonce || record.observed.len() > 2048
                || name_text.strip_prefix("cozy-private-") != Some(record.nonce.as_str()) { return Err(pending()); }
            if record.home != home { continue; } // A different Caf home is never ours to retire.
            if !record.caf.ended().map_err(|_| pending())? { return Err(pending()); }
            let chrome = record.chrome.as_ref().ok_or_else(pending)?;
            if chrome.boot_id != record.caf.boot_id || chrome.start_ticks < record.caf.start_ticks
                || record.observed.iter().any(|birth| birth.boot_id != chrome.boot_id || birth.start_ticks < chrome.start_ticks) { return Err(pending()); }
            retire_processes(chrome, &record.observed)?;
            // Recheck pinned native identity after retirement, then remove descriptor-relative.
            remove_profile(&parent, name_text, &dir, profile)?;
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(super) fn recover(_home: &Path) -> Result<(), Refusal> { Ok(()) }

#[cfg(target_os = "linux")]
fn retire_processes(chrome: &Birth, observed: &[Birth]) -> Result<(), Refusal> {
    use platform::process::{self, Exact};
    let mut owned = Vec::new();
    for birth in std::iter::once(chrome).chain(observed) {
        if let Some(exact) = Exact::open(birth).map_err(|_| pending())? { owned.push(exact); }
    }
    let group = process::group_members(chrome);
    // A live exact leader or an observed member anchors this still-owned process group.
    // An unanchored/reused number is never permission to kill an unrelated process.
    let anchored = owned.iter().any(|exact| !exact.ended() && (exact.birth() == chrome || group.contains(exact.birth())));
    if !group.is_empty() && !anchored { return Err(pending()); }
    if anchored {
        for member in group {
            if !owned.iter().any(|exact| exact.birth() == &member) {
                if let Some(exact) = Exact::open(&member).map_err(|_| pending())? { owned.push(exact); }
            }
        }
        if !chrome.ended().map_err(|_| pending())? {
            for exact in process::tree(chrome.pid) {
                if !owned.iter().any(|held| held.birth() == exact.birth()) { owned.push(exact); }
            }
        }
    }
    process::end_tree(&owned).map_err(|_| pending())?;
    if !chrome.ended().map_err(|_| pending())? || observed.iter().any(|birth| !birth.ended().unwrap_or(false)) { return Err(pending()); }
    // A recycled group with no observed anchor is left alone. Refuse cleanup instead of guessing.
    if !process::group_members(chrome).is_empty() { return Err(pending()); }
    Ok(())
}

#[cfg(target_os = "linux")]
fn remove_profile(parent: &std::fs::File, name: &str, dir: &std::fs::File, expected: Identity) -> Result<(), Refusal> {
    let named = directory(&fd_path(parent).join(name))?;
    if identity(&named)? != expected || identity(dir)? != expected { return Err(pending()); }
    let mut budget = 200_000usize;
    remove_contents(dir, expected.device, &mut budget, 0)?;
    let named = directory(&fd_path(parent).join(name))?;
    if identity(&named)? != expected { return Err(pending()); }
    // Keep the recovery receipt through partial cleanup, so a later holder can resume it.
    rustix::fs::unlinkat(dir, RECEIPT, rustix::fs::AtFlags::empty()).map_err(|_| pending())?;
    rustix::fs::unlinkat(parent, name, rustix::fs::AtFlags::REMOVEDIR).map_err(|_| pending())?;
    parent.sync_all().map_err(|_| pending())
}

#[cfg(target_os = "linux")]
fn remove_contents(dir: &std::fs::File, device: u64, budget: &mut usize, depth: u32) -> Result<(), Refusal> {
    if depth > 128 { return Err(pending()); }
    for entry in std::fs::read_dir(fd_path(dir)).map_err(|_| pending())? {
        if *budget == 0 { return Err(pending()); }
        *budget -= 1;
        let entry = entry.map_err(|_| pending())?;
        let name = entry.file_name();
        if depth == 0 && name == RECEIPT { continue; }
        let meta = std::fs::symlink_metadata(entry.path()).map_err(|_| pending())?;
        if meta.is_dir() {
            let child = directory(&entry.path())?;
            let id = identity(&child)?;
            if id.device != device { return Err(pending()); }
            remove_contents(&child, device, budget, depth + 1)?;
            if identity(&directory(&entry.path())?)? != id { return Err(pending()); }
            rustix::fs::unlinkat(dir, &name, rustix::fs::AtFlags::REMOVEDIR).map_err(|_| pending())?;
        } else {
            // Chrome's singleton symlinks/sockets are unlinked themselves, never followed.
            rustix::fs::unlinkat(dir, &name, rustix::fs::AtFlags::empty()).map_err(|_| pending())?;
        }
    }
    dir.sync_all().map_err(|_| pending())
}

impl VolatileProfileLease {
    #[cfg(target_os = "linux")]
    pub(crate) fn create() -> Result<Self, Refusal> {
        use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
        let home = *HOME_IDENTITY.get().ok_or_else(refused)?;
        for parent in parents() {
            let mut nonce = [0; 24];
            getrandom::fill(&mut nonce).map_err(|_| refused())?;
            // Preserve all192 random bits while leaving room for Chromium's Unix
            // SingletonSocket below this profile's private temporary directory.
            let nonce = URL_SAFE_NO_PAD.encode(nonce);
            let path = parent.join(format!("cozy-private-{nonce}"));
            if std::fs::DirBuilder::new().mode(0o700).create(&path).is_err() { continue; }
            let meta = std::fs::symlink_metadata(&path).map_err(|_| refused())?;
            let stat = rustix::fs::statfs(&path).map_err(|_| refused())?;
            if !meta.is_dir() || meta.uid() != rustix::process::geteuid().as_raw() || meta.mode() & 0o777 != 0o700 || stat.f_type as u64 != 0x0102_1994 {
                return Err(refused());
            }
            let identity = identity(&directory(&path)?)?;
            let receipt = Receipt { version: 1, home, profile: identity, caf: Birth::own().map_err(|_| refused())?, chrome: None, observed: Vec::new(), nonce };
            let lease = Self { path, identity, receipt };
            lease.save()?;
            return Ok(lease);
        }
        Err(refused())
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn create() -> Result<Self, Refusal> { Err(refused()) }

    pub(crate) fn path(&self) -> &Path { &self.path }
    pub(super) fn identity(&self) -> Identity { self.identity }
    pub(super) fn nonce(&self) -> &str { &self.receipt.nonce }

    #[cfg(target_os = "linux")]
    fn save(&self) -> Result<(), Refusal> {
        let dir = directory(&self.path)?;
        if identity(&dir)? != self.identity { return Err(pending()); }
        let bytes = serde_json::to_vec(&self.receipt).map_err(|_| pending())?;
        if bytes.len() as u64 > LIMIT { return Err(pending()); }
        durable::replace(&fd_path(&dir).join(RECEIPT), &bytes, 0o600).map_err(|_| pending())
    }

    /// Refresh before navigation and during the existing watcher; only exact native births enter.
    #[cfg(target_os = "linux")]
    pub(super) fn observe(&mut self, chrome: &Birth) -> Result<(), Refusal> {
        if self.receipt.chrome.as_ref().is_some_and(|held| held != chrome) { return Err(pending()); }
        self.receipt.chrome = Some(chrome.clone());
        for exact in super::OwnedBrowser::processes(chrome) {
            if !self.receipt.observed.contains(exact.birth()) { self.receipt.observed.push(exact.birth().clone()); }
        }
        if self.receipt.observed.len() > 2048 { return Err(pending()); }
        self.save()
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn observe(&mut self, _chrome: &Birth) -> Result<(), Refusal> { Err(refused()) }

    /// No Drop deletion: a profile can be retired only after its exact process/session is gone.
    #[cfg(target_os = "linux")]
    pub(super) fn retire(self, retired: BrowserRetired) -> Result<(), Refusal> {
        if retired.profile != self.identity || retired.session != self.receipt.nonce || self.receipt.chrome.as_ref() != Some(&retired.birth)
            || !retired.birth.ended().map_err(|_| refused())? || self.receipt.observed.iter().any(|birth| !birth.ended().unwrap_or(false)) { return Err(refused()); }
        let parent = directory(self.path.parent().ok_or_else(refused)?)?;
        let name = self.path.file_name().and_then(|name| name.to_str()).ok_or_else(refused)?;
        remove_profile(&parent, name, &directory(&self.path)?, self.identity)
    }
    #[cfg(not(target_os = "linux"))]
    pub(super) fn retire(self, _retired: BrowserRetired) -> Result<(), Refusal> { Err(refused()) }
}
