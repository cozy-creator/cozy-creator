//! Real Chrome startup stderr, anonymous RAM only. Seal before any credential injection so
//! later page/project output cannot enter it. Only closed observations leave this module.
use std::{fs::File, io, os::unix::fs::FileExt as _, process::Stdio};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

pub(super) struct Capture(File);

impl Capture {
    pub(super) fn create() -> io::Result<Self> {
        rustix::fs::memfd_create("cozy-browser-startup", rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING)
            .map(|fd| Self(File::from(fd))).map_err(Into::into)
    }

    pub(super) fn stdio(&self) -> io::Result<Stdio> { self.0.try_clone().map(Stdio::from) }

    pub(super) fn seal(&self) -> io::Result<()> {
        use rustix::fs::SealFlags;
        rustix::fs::fcntl_add_seals(&self.0, SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL).map_err(Into::into)
    }

    pub(super) fn facts(&self) -> Map<String, Value> {
        let mut facts = Map::new();
        let Ok(metadata) = self.0.metadata() else { facts.insert("browser_startup_unreadable".into(), json!(true)); return facts };
        let mut bytes = vec![0u8; 64 << 10];
        // Positional reads never move the writer's shared open-file offset.
        let Ok(length) = self.0.read_at(&mut bytes, 0) else { facts.insert("browser_startup_unreadable".into(), json!(true)); return facts };
        bytes.truncate(length);
        facts.insert("browser_startup_bytes".into(), json!(metadata.len()));
        facts.insert("browser_startup_bounded".into(), json!(metadata.len() > bytes.len() as u64));
        facts.insert("browser_startup_sha256".into(), json!(hex::encode(Sha256::digest(&bytes))));
        for (name, literals) in [
            ("browser_startup_no_sandbox", &["No usable sandbox", "Running as root without --no-sandbox"] as &[&str]),
            ("browser_startup_no_display", &["Missing X server", "cannot open display", "Failed to open display"] as &[&str]),
            ("browser_startup_permission_denied", &["Permission denied", "Operation not permitted"] as &[&str]),
            ("browser_startup_invalid_descriptor", &["Invalid file descriptor", "Bad file descriptor", "Remote debugging pipe file descriptors are not open"] as &[&str]),
            ("browser_startup_no_space", &["No space left on device"] as &[&str]),
            ("browser_startup_namespace", &["Failed to move to new namespace", "Failed to unshare"] as &[&str]),
            ("browser_startup_crashpad", &["crashpad", "Crashpad", "crash_reporter"] as &[&str]),
            ("browser_startup_fatal", &["FATAL:", "Check failed:", "CHECK failed"] as &[&str]),
            ("browser_startup_devtools_pipe", &["devtools_pipe_handler", "remote_debugging_pipe", "Remote debugging pipe"] as &[&str]),
            ("browser_startup_zygote", &["zygote", "Zygote"] as &[&str]),
            ("browser_startup_logging", &["logging.cc", "logging.h"] as &[&str]),
            ("browser_startup_profile", &["ProcessSingleton", "process_singleton", "SingletonLock", "profile_manager"] as &[&str]),
            ("browser_startup_allocator", &["malloc()", "tcmalloc", "PartitionAlloc", "double free", "corrupted"] as &[&str]),
            ("browser_startup_ozone_display", &["ozone_platform", "OzonePlatform", "x11", "xcb", "Xlib:"] as &[&str]),
            ("browser_startup_dbus", &["dbus", "Failed to connect to the bus"] as &[&str]),
            ("browser_startup_user_namespace", &["user namespace", "User namespace", "setuid_sandbox", "sandbox_linux.cc"] as &[&str]),
            ("browser_startup_icu", &["ICU", "icu_util"] as &[&str]),
            ("browser_startup_ptrace", &["ptrace", "PR_SET_DUMPABLE"] as &[&str]),
            ("browser_startup_shared_memory", &["shared_memory", "/dev/shm"] as &[&str]),
            ("browser_startup_invalid_flag", &["unknown flag", "unrecognized flag", "Invalid command line"] as &[&str]),
        ] {
            facts.insert(name.into(), json!(literals.iter().any(|literal| bytes.windows(literal.len()).any(|window| window == literal.as_bytes()))));
        }
        facts
    }
}
