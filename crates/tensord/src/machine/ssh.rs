//! A development rental's SSH maintenance: the granted key for root and the image's sshd
//! config. Started by the stable parent, so it outlives service restarts.
//!
//! The key is kept outside `/root/.ssh`: a provider may write its own key there after the
//! container starts (vast.ai replaces `authorized_keys` about a second in). SSH is optional: a
//! machine without sshd, or not root, serves without it and says so once.
use std::io;
#[cfg(target_os = "linux")]
use std::{fs, io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf, process::Command};

#[cfg(target_os = "linux")]
const KEY_DIR: &str = "/run/cozy-dev";
#[cfg(target_os = "linux")]
const KEYS: &str = "/run/cozy-dev/authorized_keys";

/// SSH into a dev machine is a Linux rental's.
#[cfg(not(target_os = "linux"))]
pub(crate) fn start(_public_key: &str) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "no SSH into a machine on this OS; see `cozy doctor`"))
}

/// Starts sshd for `public_key`; the error is the one warning a machine without SSH logs.
#[cfg(target_os = "linux")]
pub(crate) fn start(public_key: &str) -> io::Result<()> {
    let (Some(keygen), Some(sshd)) = (tool("ssh-keygen"), tool("sshd")) else {
        return Err(io::Error::other("sshd not found: no SSH into this dev machine; see `cozy doctor`"));
    };
    if !rustix::process::geteuid().is_root() {
        return Err(io::Error::other("sshd needs root: no SSH into this dev machine; see `cozy doctor`"));
    }
    launch(public_key, keygen, sshd).map_err(|error| {
        io::Error::other(format!("sshd did not start ({error}): no SSH into this dev machine; see `cozy doctor`"))
    })
}

#[cfg(target_os = "linux")]
fn launch(public_key: &str, keygen: PathBuf, sshd: PathBuf) -> io::Result<()> {
    if public_key.len() > 8192 || public_key.contains(['\0', '\r', '\n']) {
        return Err(io::Error::other(
            "the SSH key must be one public key of at most 8192 bytes",
        ));
    }
    fs::create_dir_all(KEY_DIR)?;
    platform::durable::set_mode(std::path::Path::new(KEY_DIR), 0o700).map_err(std::io::Error::from)?;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(KEYS)?
        .write_all(format!("{public_key}\n").as_bytes())?;
    fs::create_dir_all("/run/sshd")?;
    let keys = Command::new(keygen).arg("-A").output()?;
    if !keys.status.success() {
        return Err(io::Error::other(format!(
            "ssh host keys: {}",
            String::from_utf8_lossy(&keys.stderr)
        )));
    }
    Command::new(sshd)
        .args(["-D", "-e", "-f", "/opt/cozy/dev/sshd_config", "-o"])
        .arg(format!("AuthorizedKeysFile={KEYS}"))
        .spawn()?;
    Ok(())
}

/// `name` on this machine's PATH.
#[cfg(target_os = "linux")]
fn tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|file| file.is_file())
}

/// SSH sessions open now (cl-1193): sshd's process for each login, titled `sshd: <user>@<tty>` (`sshd-session:` from OpenSSH 9.8),
/// never its listener or a login's `[priv]` monitor. Read at most every 10 s: Status looks every second while a run goes.
#[cfg(target_os = "linux")]
pub(crate) fn sessions() -> u32 {
    use std::{
        sync::Mutex,
        time::{Duration, Instant},
    };
    static READ: Mutex<Option<(Instant, u32)>> = Mutex::new(None);
    let mut read = READ.lock().unwrap();
    if let Some((at, n)) = *read {
        if at.elapsed() < Duration::from_secs(10) {
            return n;
        }
    }
    let numbered = |entry: &fs::DirEntry| entry.file_name().to_str().is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()));
    let n = fs::read_dir("/proc").map_or(0, |entries| {
        (entries.filter_map(Result::ok))
            .filter(numbered)
            .filter(|entry| fs::read(entry.path().join("cmdline")).is_ok_and(|cmdline| login(&cmdline)))
            .count() as u32
    });
    *read = Some((Instant::now(), n));
    n
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn sessions() -> u32 {
    0
}

/// A login's sshd: its title's first word after `sshd:` is `<user>@<tty>`.
#[cfg(target_os = "linux")]
fn login(cmdline: &[u8]) -> bool {
    let title = cmdline.split(|b| *b == 0).next().map(String::from_utf8_lossy).unwrap_or_default();
    let Some(rest) = title.strip_prefix("sshd: ").or_else(|| title.strip_prefix("sshd-session: ")) else { return false };
    rest.split_whitespace().next().is_some_and(|word| word.contains('@'))
}
