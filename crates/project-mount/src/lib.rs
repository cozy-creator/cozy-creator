//! One pinned encrypted filesystem and the owned process that serves it.
//!
//! Call [`run_supervisor`] before parsing the executable's ordinary CLI. A mount starts that
//! private role in the same executable; it watches its exact parent's lifetime and stops the
//! foreground securefs child when that parent ends. Keys are passed through pipes only.
#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::fs::File;
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

pub const PROVIDER_VERSION: &str = "2.0.0";
const ROLE: &str = "__cozy_vault_provider";
const ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/securefs.zip"));
const LICENSE: &str = include_str!("../helper/LICENSE.securefs.md");
const NOTICES: &[(&str, &str)] = &[
    ("NOTICE.md", include_str!("../helper/NOTICE.md")),
    (
        "licenses/argon2.txt",
        include_str!("../helper/licenses/argon2.txt"),
    ),
    (
        "licenses/cryptopp.txt",
        include_str!("../helper/licenses/cryptopp.txt"),
    ),
    (
        "licenses/abseil.txt",
        include_str!("../helper/licenses/abseil.txt"),
    ),
    (
        "licenses/uni-algo.txt",
        include_str!("../helper/licenses/uni-algo.txt"),
    ),
    (
        "licenses/protobuf.txt",
        include_str!("../helper/licenses/protobuf.txt"),
    ),
    (
        "licenses/mimalloc.txt",
        include_str!("../helper/licenses/mimalloc.txt"),
    ),
    (
        "licenses/doctest.txt",
        include_str!("../helper/licenses/doctest.txt"),
    ),
    (
        "licenses/tclap.txt",
        include_str!("../helper/licenses/tclap.txt"),
    ),
    (
        "licenses/sources.json",
        include_str!("../helper/licenses/sources.json"),
    ),
];
const STARTUP: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "encrypted projects need securefs {PROVIDER_VERSION}; install it or use a cozy release containing its filesystem provider"
    )]
    HelperMissing,
    #[error("encrypted projects need {0}")]
    DriverMissing(&'static str),
    #[error("{operation}: {source}")]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("{0}")]
    Invalid(&'static str),
    #[error("encrypted project is still in use; close programs using {} and try locking again", .0.display())]
    Busy(PathBuf),
    #[error(
        "securefs {operation} failed ({code:?}); check that {requirements} is installed and available"
    )]
    Provider {
        operation: &'static str,
        code: Option<i32>,
        requirements: &'static str,
    },
    #[error("the encrypted filesystem did not become ready within the startup deadline")]
    NotReady,
    #[error("the encrypted filesystem provider has not exited; locking is not complete")]
    StillRunning,
    #[error(transparent)]
    Platform(#[from] platform::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

fn io_error(operation: &'static str) -> impl FnOnce(io::Error) -> Error {
    move |source| Error::Io { operation, source }
}

/// The driver is an explicit prerequisite only for encrypted projects.
pub fn requirements() -> &'static str {
    match std::env::consts::OS {
        "linux" => {
            "FUSE2 (/dev/fuse and fusermount); the bundled provider also requires glibc 2.34 or newer"
        }
        "macos" => {
            "macOS 15.4 or newer and macFUSE 5.5 or newer with its FSKit extension enabled (https://macfuse.github.io/)"
        }
        "windows" => "WinFsp and the Microsoft Visual C++ x64 runtime",
        _ => "a supported encrypted filesystem driver",
    }
}

/// A release must embed its pinned provider; ordinary development builds need not.
pub fn is_bundled() -> bool {
    !ARCHIVE.is_empty()
}

/// A verified executable, with no project key stored in it.
#[derive(Clone, Debug)]
pub struct Provider {
    executable: PathBuf,
}

impl Provider {
    /// Installs the bundled helper without networking, else looks for an explicitly installed
    /// securefs. Nothing calls this for an ordinary project.
    pub fn discover(tools: &Path) -> Result<Self> {
        // An explicit local override permits rebuilding the independently licensed provider,
        // including relinking modified LGPL components. It is never supplied by the Hub.
        if let Some(path) = std::env::var_os("COZY_SECUREFS_PROVIDER") {
            return Self::from_path(PathBuf::from(path));
        }
        if is_bundled() {
            let directory = tools.join("securefs").join(PROVIDER_VERSION);
            platform::durable::private_dir(&directory)?;
            let name = if cfg!(windows) {
                "securefs.exe"
            } else {
                "securefs"
            };
            let mut archive = zip::ZipArchive::new(io::Cursor::new(ARCHIVE))
                .map_err(|_| Error::Invalid("the bundled securefs archive is invalid"))?;
            let mut entry = archive
                .by_name(name)
                .map_err(|_| Error::Invalid("the bundled securefs executable is missing"))?;
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(io_error("read bundled securefs"))?;
            let executable = directory.join(name);
            let matches = fs::read(&executable)
                .ok()
                .is_some_and(|existing| Sha256::digest(&existing) == Sha256::digest(&bytes));
            if !matches {
                install(&executable, &bytes, true)?;
            }
            install(
                &directory.join("LICENSE.securefs.md"),
                LICENSE.as_bytes(),
                false,
            )?;
            platform::durable::private_dir(&directory.join("licenses"))?;
            for (name, text) in NOTICES {
                install(&directory.join(name), text.as_bytes(), false)?;
            }
            return Self::from_path(executable);
        }
        let name = if cfg!(windows) {
            "securefs.exe"
        } else {
            "securefs"
        };
        let cached = tools.join("securefs").join(PROVIDER_VERSION).join(name);
        if cached.is_file() {
            return Self::from_path(cached);
        }
        if let Some(paths) = std::env::var_os("PATH") {
            for directory in std::env::split_paths(&paths) {
                let candidate = directory.join(name);
                if candidate.is_file() {
                    return Self::from_path(candidate);
                }
            }
        }
        Err(Error::HelperMissing)
    }

    /// An explicit development/helper path. Its version must match the supported format tool.
    pub fn from_path(executable: impl Into<PathBuf>) -> Result<Self> {
        let executable = fs::canonicalize(executable.into()).map_err(io_error("find securefs"))?;
        let provider = Self { executable };
        let output = provider
            .command("version")
            .output()
            .map_err(io_error("run securefs"))?;
        let first = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() {
            return Err(provider.failure("version", output.status));
        }
        if first.lines().next() != Some("securefs v2.0.0") {
            return Err(Error::Invalid(
                "the installed securefs version is not supported; use securefs 2.0.0",
            ));
        }
        Ok(provider)
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Creates only a new encrypted backing directory, never an existing plaintext project.
    /// Lite format encrypts file names and contents while preserving encrypted hierarchy.
    pub fn create(&self, backing: &Path, secret: &[u8]) -> Result<()> {
        validate_secret(secret)?;
        if backing.exists() {
            if fs::symlink_metadata(backing)
                .map_err(io_error("read backing directory"))?
                .file_type()
                .is_symlink()
                || !backing.is_dir()
                || fs::read_dir(backing)
                    .map_err(io_error("read backing directory"))?
                    .next()
                    .is_some()
            {
                return Err(Error::Invalid(
                    "encrypted backing storage must be an empty real directory",
                ));
            }
        } else {
            platform::durable::private_dir(backing)?;
        }
        let mut command = self.command("create");
        command.args(["--format", "lite"]).arg(backing);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(io_error("create encrypted filesystem"))?;
        let sent = child
            .stdin
            .take()
            .ok_or(Error::Invalid("securefs stdin is unavailable"))
            .and_then(|mut input| write_secret(&mut input, secret, 2));
        if let Err(error) = sent {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let status = child
            .wait()
            .map_err(io_error("wait for encrypted filesystem creation"))?;
        if status.success() {
            Ok(())
        } else {
            Err(self.failure("create", status))
        }
    }

    /// Mounts through the owned supervisor role in this same executable. The caller must have
    /// installed [`run_supervisor`] at its entry point before using this method.
    pub fn mount(
        &self,
        backing: &Path,
        view: &Path,
        secret: &[u8],
        options: MountOptions,
    ) -> Result<Mounted> {
        // A selected free Windows drive can be claimed by another concurrent unlock. Keep
        // preflight through readiness atomic so a second caller refuses, never aliases the
        // first project's mounted drive. The same rule prevents duplicate Unix view mounts.
        static STARTING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _starting = STARTING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        validate_secret(secret)?;
        self.check_driver()?;
        #[cfg(target_os = "macos")]
        if options.read_only && macos_version() < (26, 4) {
            return Err(Error::Invalid(
                "read-only encrypted views with FSKit require macOS 26.4 or newer",
            ));
        }
        if !backing.join(".config.pb").is_file() {
            return Err(Error::Invalid(
                "the encrypted backing storage has no securefs configuration",
            ));
        }
        if self.is_mounted(view)? {
            return Err(Error::Busy(view.to_path_buf()));
        }
        let guard = ViewGuard::prepare(view)?;
        let parent = platform::process::Birth::own()?;
        let executable = std::env::current_exe().map_err(io_error("find cozy executable"))?;
        let mut command = clean_command(&executable);
        command
            .arg(ROLE)
            .arg(&self.executable)
            .arg(backing)
            .arg(view)
            .arg(parent.pid.to_string())
            .arg(parent.start_ticks.to_string())
            .arg(&parent.boot_id)
            .arg(
                options
                    .idle_timeout
                    .map_or(0, |duration| duration.as_secs())
                    .to_string(),
            )
            .arg(if options.read_only { "1" } else { "0" });
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(io_error("start encrypted filesystem supervisor"))?;
        let mut input = child
            .stdin
            .take()
            .ok_or(Error::Invalid("provider stdin is unavailable"))?;
        if let Err(error) = write_secret(&mut input, secret, 1) {
            drop(input);
            let _ = child.wait();
            return Err(error);
        }
        let mut mounted = Mounted {
            provider: self.clone(),
            view: view.to_path_buf(),
            child,
            input: Some(input),
            guard,
        };
        let deadline = Instant::now() + STARTUP;
        loop {
            if let Some(status) = mounted
                .child
                .try_wait()
                .map_err(io_error("read filesystem supervisor status"))?
            {
                return Err(self.failure("mount", status));
            }
            if self.is_mounted(view)? {
                mounted.guard.seal()?;
                return Ok(mounted);
            }
            if Instant::now() >= deadline {
                return Err(Error::NotReady);
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn is_mounted(&self, view: &Path) -> Result<bool> {
        #[cfg(target_os = "linux")]
        return linux_mounted(view);
        #[cfg(not(target_os = "linux"))]
        {
            let status = self
                .command("ismount")
                .arg(helper_view(view))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(io_error("read encrypted filesystem status"))?;
            match status.code() {
                Some(0) => Ok(true),
                Some(1) => Ok(false),
                _ => Err(self.failure("ismount", status)),
            }
        }
    }

    fn command(&self, operation: &str) -> Command {
        let mut command = clean_command(&self.executable);
        command.arg(operation);
        command
    }

    fn failure(&self, operation: &'static str, status: ExitStatus) -> Error {
        Error::Provider {
            operation,
            code: status.code(),
            requirements: requirements(),
        }
    }

    fn unmount(&self, view: &Path) -> Result<()> {
        // securefs's unmount may detach a live Unix mount. A normal lock must instead let the
        // kernel refuse open handles, leaving the provider owned and its writes intact.
        #[cfg(unix)]
        let status = unix_unmount(view, false)?;
        #[cfg(not(unix))]
        let status = self
            .command("unmount")
            .arg(helper_view(view))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(io_error("unmount encrypted filesystem"))?;
        if self.is_mounted(view)? {
            return Err(Error::Busy(view.to_path_buf()));
        }
        if !status.success() {
            return Err(self.failure("unmount", status));
        }
        Ok(())
    }

    fn cleanup_dead(&self, view: &Path) {
        #[cfg(unix)]
        if self.is_mounted(view).unwrap_or(false) {
            // Only the supervisor calls this after waiting for the provider process to exit.
            // No key-holding service remains; detach an inaccessible stale kernel mount.
            let _ = unix_unmount(view, true);
        }
        #[cfg(not(unix))]
        let _ = self.unmount(view);
    }

    fn check_driver(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        if OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/fuse")
            .is_err()
        {
            return Err(Error::DriverMissing(requirements()));
        }
        #[cfg(target_os = "macos")]
        if macos_version() < (15, 4)
            || (!Path::new("/Library/Filesystems/macfuse.fs").exists()
                && !Path::new("/Library/Filesystems/osxfuse.fs").exists())
            || macfuse_version() < (5, 5)
        {
            return Err(Error::DriverMissing(requirements()));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MountOptions {
    /// Normally None: Calcifer owns viewing-session expiration.
    pub idle_timeout: Option<Duration>,
    pub read_only: bool,
}

/// A live owned provider. Keep this value after a Busy result; manual locking must not forcibly
/// discard open writes. Dropping it is shutdown/abort: EOF tells the supervisor to stop serving.
pub struct Mounted {
    provider: Provider,
    view: PathBuf,
    child: Child,
    input: Option<ChildStdin>,
    guard: ViewGuard,
}

impl Mounted {
    pub fn view(&self) -> &Path {
        &self.view
    }
    pub fn provider_pid(&self) -> u32 {
        self.child.id()
    }
    pub fn is_mounted(&self) -> Result<bool> {
        self.provider.is_mounted(&self.view)
    }

    /// Normal unmount only. An open player/editor can return Busy; its handle and provider stay
    /// owned here. A successful result means the readable view and key-holding helper are gone.
    pub fn try_unmount(&mut self) -> Result<()> {
        if self.provider.is_mounted(&self.view)? {
            self.provider.unmount(&self.view)?;
        }
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(10);
        while self
            .child
            .try_wait()
            .map_err(io_error("wait for filesystem shutdown"))?
            .is_none()
        {
            if Instant::now() >= deadline {
                return Err(Error::StillRunning);
            }
            thread::sleep(Duration::from_millis(20));
        }
        self.guard.seal()?;
        Ok(())
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.guard.seal();
    }
}

/// A Unix private directory view, or an unused Windows drive root. Drive mounts disappear on
/// lock and can never fall through into an ordinary writable directory.
pub fn view_path(runtime: &Path) -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let _ = runtime;
        for letter in (b'D'..=b'Z').rev() {
            let path = PathBuf::from(format!("{}:\\", letter as char));
            if !path.exists() {
                return Ok(path);
            }
        }
        Err(Error::Invalid(
            "no unused Windows drive letter is available for the encrypted project",
        ))
    }
    #[cfg(not(windows))]
    {
        Ok(runtime.join("view"))
    }
}

/// Called before ordinary command parsing. None means this is a normal CLI invocation. This
/// role never starts a daemon, reads project account credentials or logs its stdin secret.
pub fn run_supervisor() -> Option<i32> {
    let arguments: Vec<OsString> = std::env::args_os().collect();
    if arguments.get(1).is_none_or(|argument| argument != ROLE) {
        return None;
    }
    Some(match supervise(&arguments[2..]) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("encrypted filesystem provider: {error}");
            1
        }
    })
}

fn supervise(arguments: &[OsString]) -> Result<()> {
    if arguments.len() != 8 {
        return Err(Error::Invalid("invalid filesystem supervisor arguments"));
    }
    let number = |index: usize| {
        arguments[index]
            .to_str()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(Error::Invalid("invalid filesystem supervisor parent"))
    };
    let pid = u32::try_from(number(3)?)
        .map_err(|_| Error::Invalid("invalid filesystem supervisor parent"))?;
    let birth = platform::process::Birth {
        pid,
        start_ticks: number(4)?,
        boot_id: arguments[5].to_string_lossy().into_owned(),
    };
    let parent = platform::process::Exact::open(&birth)?.ok_or(Error::Invalid(
        "the filesystem controller has already exited",
    ))?;
    let provider = Provider::from_path(PathBuf::from(&arguments[0]))?;
    let backing = PathBuf::from(&arguments[1]);
    let view = PathBuf::from(&arguments[2]);
    let guard = ViewGuard::prepare(&view)?;
    let mut input = io::stdin();
    let mut secret = Zeroizing::new(vec![0; 44]);
    input
        .read_exact(&mut secret)
        .map_err(io_error("read filesystem unlock material"))?;
    if secret[43] != b'\n' {
        return Err(Error::Invalid("invalid filesystem unlock material"));
    }
    validate_secret(&secret[..43])?;
    let mut command = provider.command("mount");
    // FSKit avoids requiring Apple Silicon users to weaken boot policy for a kernel extension.
    #[cfg(target_os = "macos")]
    command.args(["-o", "backend=fskit"]);
    let idle = number(6)?;
    if idle > 0 {
        command.arg("--max-idle-seconds").arg(idle.to_string());
    }
    if arguments[7] == "1" {
        command.args(["-o", "ro"]);
    }
    command.arg(&backing).arg(helper_view(&view));
    let mut child = OwnedChild(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(io_error("start securefs"))?,
    );
    let mut provider_input = child
        .0
        .stdin
        .take()
        .ok_or(Error::Invalid("securefs stdin is unavailable"))?;
    provider_input
        .write_all(&secret)
        .map_err(io_error("unlock securefs"))?;
    drop(provider_input);
    drop(secret);
    let (ended, stop) = mpsc::channel();
    thread::spawn(move || {
        let mut byte = [0];
        let _ = input.read(&mut byte);
        let _ = ended.send(());
    });
    let mut sealed = false;
    loop {
        if let Some(status) = child
            .0
            .try_wait()
            .map_err(io_error("read securefs status"))?
        {
            if status.success() {
                return Ok(());
            }
            return Err(provider.failure("mount", status));
        }
        if !sealed && provider.is_mounted(&view).unwrap_or(false) {
            guard.seal()?;
            sealed = true;
        }
        if stop.try_recv().is_ok() || parent.wait_for(Duration::from_millis(100))? {
            let _ = provider.unmount(&view);
            // Shutdown is not a normal lock: if open handles prevent unmount, terminating the
            // provider makes further reads fail and its key disappear. The underlay is sealed.
            let _ = child.0.kill();
            let _ = child.0.wait();
            provider.cleanup_dead(&view);
            return Ok(());
        }
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn clean_command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command.env_clear();
    for name in [
        "PATH",
        "HOME",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "USERPROFILE",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.env("LC_ALL", "C");
    command
}

#[cfg(target_os = "linux")]
fn linux_mounted(view: &Path) -> Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    // Canonicalize only the parent: probing the mounted root can issue FUSE operations and
    // accidentally renew its idle timeout. The parent remains the private ordinary directory.
    let view = view
        .parent()
        .and_then(|parent| fs::canonicalize(parent).ok())
        .zip(view.file_name())
        .map_or_else(|| view.to_path_buf(), |(parent, name)| parent.join(name));
    let table = fs::read("/proc/self/mountinfo").map_err(io_error("read kernel mount state"))?;
    for line in table.split(|byte| *byte == b'\n') {
        let fields: Vec<_> = line.split(|byte| *byte == b' ').collect();
        let Some(separator) = fields.iter().position(|field| *field == b"-") else {
            continue;
        };
        if fields.get(separator + 1) != Some(&b"fuse.securefs".as_slice()) {
            continue;
        }
        let Some(encoded) = fields.get(4) else {
            continue;
        };
        let mut decoded = Vec::new();
        let mut at = 0;
        while at < encoded.len() {
            if encoded[at] == b'\\'
                && at + 3 < encoded.len()
                && encoded[at + 1..at + 4]
                    .iter()
                    .all(|byte| (b'0'..=b'7').contains(byte))
            {
                decoded.push(
                    (encoded[at + 1] - b'0') * 64 + (encoded[at + 2] - b'0') * 8 + encoded[at + 3]
                        - b'0',
                );
                at += 4;
            } else {
                decoded.push(encoded[at]);
                at += 1;
            }
        }
        if decoded == view.as_os_str().as_bytes() {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(unix)]
fn unix_unmount(view: &Path, dead_provider: bool) -> Result<ExitStatus> {
    #[cfg(target_os = "linux")]
    {
        for program in ["fusermount3", "fusermount"] {
            let mut command = clean_command(Path::new(program));
            command.arg("-u");
            if dead_provider {
                command.arg("-z");
            }
            match command
                .arg(view)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                Ok(status) => return Ok(status),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error("unmount encrypted filesystem")(error)),
            }
        }
        Err(Error::DriverMissing(requirements()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let mut command = clean_command(Path::new("/sbin/umount"));
        if dead_provider {
            command.arg("-f");
        }
        command
            .arg(view)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(io_error("unmount encrypted filesystem"))
    }
}

fn validate_secret(secret: &[u8]) -> Result<()> {
    if secret.len() != 43
        || !secret
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(Error::Invalid("invalid project unlock material"));
    }
    Ok(())
}

fn write_secret(output: &mut impl Write, secret: &[u8], copies: usize) -> Result<()> {
    for _ in 0..copies {
        output
            .write_all(secret)
            .and_then(|_| output.write_all(b"\n"))
            .map_err(io_error("send project unlock material"))?;
    }
    output
        .flush()
        .map_err(io_error("send project unlock material"))
}

fn helper_view(view: &Path) -> OsString {
    #[cfg(windows)]
    {
        return view
            .as_os_str()
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .into();
    }
    #[cfg(not(windows))]
    {
        view.as_os_str().to_owned()
    }
}

#[cfg(target_os = "macos")]
fn macos_version() -> (u64, u64) {
    static VERSION: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();
    *VERSION.get_or_init(|| {
        let Ok(output) = Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
        else {
            return (0, 0);
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let mut parts = text.trim().split('.').filter_map(|part| part.parse().ok());
        (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
    })
}

#[cfg(target_os = "macos")]
fn macfuse_version() -> (u64, u64) {
    let plist = "/Library/Filesystems/macfuse.fs/Contents/Info.plist";
    for key in ["CFBundleShortVersionString", "CFBundleVersion"] {
        let Ok(output) = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", &format!("Print :{key}"), plist])
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut parts = text.trim().split('.').filter_map(|part| part.parse().ok());
        if let Some(major) = parts.next() {
            return (major, parts.next().unwrap_or(0));
        }
    }
    (0, 0)
}

struct ViewGuard {
    #[cfg(unix)]
    directory: File,
}

impl ViewGuard {
    fn prepare(view: &Path) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if !view.is_absolute() {
                return Err(Error::Invalid("the private mount path must be absolute"));
            }
            if let Some(parent) = view.parent() {
                platform::durable::private_dir(parent)?;
            }
            match fs::symlink_metadata(view) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    return Err(Error::Invalid(
                        "the private mount path must be a real empty directory",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(view).map_err(io_error("create private mount path"))?
                }
                Err(error) => return Err(io_error("read private mount path")(error)),
            }
            fs::set_permissions(view, fs::Permissions::from_mode(0o700))
                .map_err(io_error("prepare private mount path"))?;
            if fs::read_dir(view)
                .map_err(io_error("read private mount path"))?
                .next()
                .is_some()
            {
                return Err(Error::Invalid("the private mount path must be empty"));
            }
            let directory = File::open(view).map_err(io_error("hold private mount path"))?;
            Ok(Self { directory })
        }
        #[cfg(windows)]
        {
            let text = view.as_os_str().to_string_lossy();
            let bytes = text.as_bytes();
            if bytes.len() != 3
                || !bytes[0].is_ascii_alphabetic()
                || bytes[1] != b':'
                || !matches!(bytes[2], b'\\' | b'/')
                || view.exists()
            {
                return Err(Error::Invalid(
                    "an encrypted Windows project needs an unused drive root",
                ));
            }
            Ok(Self {})
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = view;
            Err(Error::Invalid(
                "encrypted project mounts are not supported on this OS",
            ))
        }
    }

    fn seal(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // This descriptor names the original directory inode below the mount, not the
            // mounted filesystem root. After crash/unmount, normal writes receive EACCES.
            self.directory
                .set_permissions(fs::Permissions::from_mode(0))
                .map_err(io_error("seal private mount underlay"))?;
        }
        Ok(())
    }
}

impl Drop for ViewGuard {
    fn drop(&mut self) {
        let _ = self.seal();
    }
}

fn install(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        ".install-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let temporary = path.with_file_name(name);
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(io_error("stage bundled filesystem provider"))?;
        file.write_all(bytes)
            .map_err(io_error("write bundled filesystem provider"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(if executable {
                0o755
            } else {
                0o600
            }))
            .map_err(io_error("protect bundled filesystem provider"))?;
        }
        #[cfg(not(unix))]
        let _ = executable;
        file.sync_all()
            .map_err(io_error("persist bundled filesystem provider"))?;
        drop(file);
        #[cfg(windows)]
        if path.exists() {
            fs::remove_file(path).map_err(io_error("replace bundled filesystem provider"))?;
        }
        fs::rename(&temporary, path).map_err(io_error("install bundled filesystem provider"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
