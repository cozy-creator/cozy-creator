//! Real provider fixture: FUSE/WinFsp and ordinary file/media I/O, without a mock filesystem.
//!   vault-provider normal|crash-host <securefs> <artifacts>
//!   vault-provider throughput <securefs> <artifacts> <bytes>   write, lock, unlock, read back: {write_MBps, read_MBps}
//!   vault-provider controls <securefs> <artifacts>   an exited child's birth, then a mounted view's status read many times at once
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use project_mount::{MountOptions, Provider};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

fn main() {
    if let Some(code) = project_mount::run_supervisor() {
        std::process::exit(code);
    }
    if let Err(error) = run() {
        eprintln!("provider fixture: {error}");
        std::process::exit(1);
    }
}

type Failure = Box<dyn std::error::Error>;

fn run() -> Result<(), Failure> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let mode = arguments.first().map(|mode| mode.to_string_lossy().into_owned()).unwrap_or_default();
    // `controls`' child: exits at once.
    if mode == "exit" {
        return Ok(());
    }
    let bytes = match (mode.as_str(), arguments.len()) {
        ("normal" | "crash-host" | "mount" | "controls" | "dropped", 3) => None,
        ("throughput", 4) => Some(
            arguments[3]
                .to_str()
                .and_then(|bytes| bytes.parse::<u64>().ok())
                .filter(|bytes| *bytes > 0)
                .ok_or("throughput takes a positive byte count")?,
        ),
        _ => {
            return Err("usage: vault-provider normal|crash-host|mount|controls|dropped <securefs> <artifacts> | throughput <securefs> <artifacts> <bytes>".into());
        }
    };
    let expected_helper = PathBuf::from(&arguments[1]);
    let artifacts = PathBuf::from(&arguments[2]);
    fs::create_dir_all(&artifacts)?;
    let provider = provider(&expected_helper, &artifacts)?;
    if let Some(bytes) = bytes {
        return throughput(&provider, &artifacts, bytes);
    }
    if mode == "mount" {
        return mount_only(&provider, &artifacts);
    }
    if mode == "controls" {
        return controls(&provider, &artifacts);
    }
    if mode == "dropped" {
        return dropped(&provider, &artifacts);
    }
    let backing = artifacts.join("encrypted");
    let view = project_mount::view_path(&artifacts.join("runtime"))?;
    let secret = new_secret()?;
    eprintln!("provider fixture: create and mount");
    provider.create(&backing, secret.as_bytes())?;
    #[cfg(target_os = "macos")]
    {
        eprintln!("provider fixture: immediate private SQLite stage detach");
        let mut stage =
            provider.mount(&backing, &view, secret.as_bytes(), MountOptions::default())?;
        {
            let connection = rusqlite::Connection::open(view.join("initial-stage.sqlite"))?;
            connection.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE snapshot (id INTEGER PRIMARY KEY); INSERT INTO snapshot VALUES (1)")?;
        }
        stage.try_unmount()?;
    }
    let mut mounted =
        provider.mount(&backing, &view, secret.as_bytes(), MountOptions::default())?;
    #[cfg(windows)]
    {
        eprintln!("provider fixture: native canonical path on a WinFsp volume");
        let canonical = platform::durable::canonical(&view)?;
        assert!(canonical.is_dir());
        let probe = canonical.join("canonical-probe.bin");
        fs::write(&probe, b"CANONICAL_VIRTUAL_VOLUME")?;
        assert_eq!(
            fs::read(view.join("canonical-probe.bin"))?,
            b"CANONICAL_VIRTUAL_VOLUME"
        );
        assert_eq!(platform::durable::canonical(&probe)?, probe);
        let directory = canonical.join("native-directory-lock");
        fs::create_dir(&directory)?;
        let held_directory = platform::lock::dir(&directory)?;
        drop(held_directory);
        let file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(canonical.join("native-file-lock.bin"))?;
        file.lock()?;
        file.unlock()?;
        file.lock_shared()?;
        file.unlock()?;
        drop(file);
        let database = canonical.join("canonical-state.sqlite");
        {
            let connection = rusqlite::Connection::open(&database)?;
            connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE payload (value TEXT); INSERT INTO payload VALUES ('NATIVE_CANONICAL_SQLITE'); PRAGMA wal_checkpoint(TRUNCATE)")?;
        }
        let connection = rusqlite::Connection::open(&database)?;
        let saved: String =
            connection.query_row("SELECT value FROM payload", [], |row| row.get(0))?;
        assert_eq!(saved, "NATIVE_CANONICAL_SQLITE");
        drop(connection);
    }
    let file = view.join("private-media-input.bin");
    let marker = b"COZY_XS160_PRIVATE_FILE_CONTENT";
    let mut content = marker.repeat(36_000);
    eprintln!("provider fixture: initial file create");
    let mut output = fs::File::create(&file)?;
    eprintln!("provider fixture: initial file write");
    output.write_all(&content)?;
    eprintln!("provider fixture: initial file fsync");
    output.sync_all()?;
    drop(output);
    eprintln!("provider fixture: initial file read");
    assert_eq!(fs::read(&file)?, content);
    let mut seek = fs::File::open(&file)?;
    seek.seek(SeekFrom::Start(123_456))?;
    let mut part = vec![0; 9876];
    seek.read_exact(&mut part)?;
    assert_eq!(part, content[123_456..123_456 + part.len()]);
    drop(seek);

    #[allow(unused_mut)]
    let mut supervisor_environ = "unread";
    #[cfg(target_os = "linux")]
    {
        // The supervisor holds the key and is non-dumpable (`disable_project_parent_dumps`): its environ reads only as root, which a CI
        // runner is through passwordless sudo. Where there's none (a developer's computer) that check is SKIPPED, said as such.
        let pid = mounted.provider_pid();
        for path in [
            format!("/proc/{pid}/cmdline"),
            format!("/proc/{pid}/environ"),
        ] {
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if path.ends_with("environ") && error.kind() == std::io::ErrorKind::PermissionDenied => {
                    match Command::new("sudo").args(["-n", "cat", &path]).output() {
                        Ok(output) if output.status.success() => {
                            supervisor_environ = "checked as root";
                            output.stdout
                        }
                        _ => {
                            eprintln!("provider fixture: SKIPPED: the supervisor's environ is root's to read and there's no passwordless sudo");
                            supervisor_environ = "SKIPPED: no passwordless sudo";
                            continue;
                        }
                    }
                }
                Err(error) => return Err(format!("{path}: {error}").into()),
            };
            if path.ends_with("environ") && supervisor_environ == "unread" {
                supervisor_environ = "checked";
            }
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|part| part == secret.as_bytes())
            );
        }
    }

    if mode == "crash-host" {
        fs::write(
            artifacts.join("ready.json"),
            serde_json::to_vec(&json!({"view":view,"supervisor_pid":mounted.provider_pid()}))?,
        )?;
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    if mode != "normal" {
        return Err("unknown provider fixture mode".into());
    }

    eprintln!("provider fixture: overwrite and rename");
    let mut update = fs::OpenOptions::new().write(true).open(&file)?;
    update.seek(SeekFrom::Start(1024))?;
    update.write_all(b"UPDATED")?;
    update.sync_all()?;
    drop(update);
    content[1024..1031].copy_from_slice(b"UPDATED");
    let renamed = view.join("renamed-input.bin");
    fs::rename(&file, &renamed)?;
    assert_eq!(fs::read(&renamed)?, content);

    eprintln!("provider fixture: media write and seek");
    let media = view.join("private-video.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=24",
            "-t",
            "2",
            "-c:v",
            "mpeg4",
        ])
        .arg(&media)
        .status()?;
    assert!(
        status.success(),
        "ffmpeg must write through the mounted view"
    );
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-ss", "1", "-i"])
        .arg(&media)
        .args(["-frames:v", "1", "-f", "null", "-"])
        .status()?;
    assert!(
        status.success(),
        "ffmpeg must seek and decode through the mounted view"
    );
    eprintln!("provider fixture: normal lock with open handle");
    eprintln!("provider fixture: durable SQLite transaction");
    let database = view.join("project-state.sqlite");
    {
        let mut conn = rusqlite::Connection::open(&database)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE state (id INTEGER PRIMARY KEY, payload BLOB NOT NULL)")?;
        let transaction = conn.transaction()?;
        transaction.execute("INSERT INTO state (id,payload) VALUES (1,?1)", [&content])?;
        transaction.commit()?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    }
    {
        let conn = rusqlite::Connection::open_with_flags(
            &database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let saved: Vec<u8> =
            conn.query_row("SELECT payload FROM state WHERE id=1", [], |row| row.get(0))?;
        assert!(
            saved == content,
            "the committed SQLite payload must survive reopen"
        );
    }
    let mut held = fs::File::open(&renamed)?;
    let busy = match mounted.try_unmount() {
        Err(project_mount::Error::Busy(_)) => {
            assert!(mounted.is_mounted()?);
            "busy-preserved"
        }
        Ok(()) => {
            assert!(!mounted.is_mounted()?);
            assert!(
                held.read_exact(&mut [0]).is_err(),
                "a successful lock must refuse fresh reads through a previously opened handle"
            );
            "unmounted"
        }
        Err(error) => return Err(error.into()),
    };
    drop(held);
    mounted.try_unmount()?;
    assert!(!mounted.is_mounted()?);
    assert!(
        fs::write(view.join("must-not-leak.bin"), marker).is_err(),
        "an absent mount must refuse plaintext fallback writes"
    );

    eprintln!("provider fixture: remount parity");
    let mut remount =
        provider.mount(&backing, &view, secret.as_bytes(), MountOptions::default())?;
    assert_eq!(fs::read(&renamed)?, content);
    {
        let conn = rusqlite::Connection::open_with_flags(
            &database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let saved: Vec<u8> =
            conn.query_row("SELECT payload FROM state WHERE id=1", [], |row| row.get(0))?;
        assert!(
            saved == content,
            "the SQLite commit must survive a full encrypted unmount/remount"
        );
    }
    remount.try_unmount()?;
    #[cfg(target_os = "macos")]
    if provider.storage_format() == "apple-apfs-encrypted-sparsebundle-v1" {
        eprintln!("provider fixture: native supervisor crash reconciliation");
        let mut crashed =
            provider.mount(&backing, &view, secret.as_bytes(), MountOptions::default())?;
        let birth = platform::process::Birth::of(crashed.provider_pid())?;
        platform::process::Exact::open(&birth)?
            .ok_or("owned supervisor already exited")?
            .kill()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            crashed.reconcile()?;
            if !crashed.is_mounted()? {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the controller must reconcile a killed native supervisor"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = Command::new("/usr/bin/hdiutil")
            .args(["info", "-plist"])
            .output()?;
        assert!(output.status.success());
        let info = plist::Value::from_reader(std::io::Cursor::new(output.stdout))?;
        let image = fs::canonicalize(backing.join("project.sparsebundle"))?;
        for entry in info
            .as_dictionary()
            .and_then(|dictionary| dictionary.get("images"))
            .and_then(plist::Value::as_array)
            .ok_or("native image registry missing")?
        {
            if let Some(path) = entry
                .as_dictionary()
                .and_then(|dictionary| dictionary.get("image-path"))
                .and_then(plist::Value::as_string)
            {
                assert!(
                    fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path)) != image,
                    "reconciliation must release the kernel image/key attachment"
                );
            }
        }
        assert!(fs::write(view.join("crash-leak.bin"), marker).is_err());
        crashed.try_unmount()?;
        eprintln!("provider fixture: native stale volume owner cannot adopt a reused view");
        let replacement_backing = artifacts.join("replacement-encrypted");
        let mut replacement_random = Zeroizing::new([0u8; 32]);
        getrandom::fill(&mut *replacement_random)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let replacement_secret = Zeroizing::new(URL_SAFE_NO_PAD.encode(*replacement_random));
        provider.create(&replacement_backing, replacement_secret.as_bytes())?;
        let mut previous =
            provider.mount(&backing, &view, secret.as_bytes(), MountOptions::default())?;
        struct Resume(u32);
        impl Drop for Resume {
            fn drop(&mut self) {
                let _ = Command::new("/bin/kill")
                    .args(["-CONT", &self.0.to_string()])
                    .status();
            }
        }
        assert!(
            Command::new("/bin/kill")
                .args(["-STOP", &previous.provider_pid().to_string()])
                .status()?
                .success()
        );
        let paused = Resume(previous.provider_pid());
        // Unmount only the volume while its unlocked image and stopped supervisor remain.
        assert!(
            Command::new("/usr/bin/hdiutil")
                .arg("unmount")
                .arg(&view)
                .status()?
                .success()
        );
        let mut replacement = provider.mount(
            &replacement_backing,
            &view,
            replacement_secret.as_bytes(),
            MountOptions::default(),
        )?;
        let independent = view.join("independent-project.bin");
        fs::write(&independent, b"SECOND_PROJECT_INDEPENDENT_CONTENT")?;
        assert!(
            !previous.is_mounted()?,
            "an attached old image must not adopt another image's mounted volume"
        );
        drop(paused);
        previous.try_unmount()?;
        drop(previous);
        assert!(replacement.is_mounted()?);
        assert_eq!(
            fs::read(&independent)?,
            b"SECOND_PROJECT_INDEPENDENT_CONTENT"
        );
        replacement.try_unmount()?;
    }
    #[cfg(target_os = "linux")]
    {
        eprintln!("provider fixture: helper crash cleanup");
        let mut crashed =
            provider.mount(&backing, &view, secret.as_bytes(), MountOptions::default())?;
        let supervisor = crashed.provider_pid();
        let mut victim = None;
        for pid in fs::read_to_string(format!("/proc/{supervisor}/task/{supervisor}/children"))?
            .split_whitespace()
        {
            let pid: u32 = pid.parse()?;
            let command = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            if command.split(|byte| *byte == 0).nth(1) == Some(b"mount".as_slice()) {
                victim = Some(pid);
                break;
            }
        }
        let birth =
            platform::process::Birth::of(victim.ok_or("owned securefs child was not found")?)?;
        platform::process::Exact::open(&birth)?
            .ok_or("owned provider already exited")?
            .kill()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while provider.is_mounted(&view)? && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !provider.is_mounted(&view)?,
            "a crashed provider must not leave a stale kernel mount"
        );
        assert!(fs::write(view.join("crash-leak.bin"), marker).is_err());
        crashed.try_unmount()?;
    }
    #[cfg(windows)]
    {
        eprintln!("provider fixture: Windows helper crash cleanup");
        let replacement_backing = artifacts.join("replacement-encrypted");
        let mut replacement_random = Zeroizing::new([0u8; 32]);
        getrandom::fill(&mut *replacement_random)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let replacement_secret = Zeroizing::new(URL_SAFE_NO_PAD.encode(*replacement_random));
        provider.create(&replacement_backing, replacement_secret.as_bytes())?;
        let mut crashed =
            provider.mount(&backing, &view, secret.as_bytes(), MountOptions::default())?;
        let supervisor = crashed.provider_pid();
        let mut helpers = Vec::new();
        for process in platform::process::tree(supervisor) {
            if process.birth().pid == supervisor {
                continue;
            }
            // A console host can also descend from the supervisor. Query the executable
            // through the exact process handle; no command-line or credential is read.
            if process
                .executable()
                .ok()
                .and_then(|path| fs::canonicalize(path).ok())
                .as_deref()
                == Some(provider.executable())
            {
                helpers.push(process);
            }
        }
        assert!(!helpers.is_empty(), "owned securefs helper must be present");
        for helper in helpers {
            helper.kill()?;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while provider.is_mounted(&view)? && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !crashed.is_mounted()?,
            "a crashed helper must release its WinFsp volume"
        );
        assert!(fs::read(&renamed).is_err());
        assert!(fs::write(view.join("crash-leak.bin"), marker).is_err());
        eprintln!("provider fixture: stale Windows owner cannot lock a reused drive");
        let mut replacement = provider.mount(
            &replacement_backing,
            &view,
            replacement_secret.as_bytes(),
            MountOptions::default(),
        )?;
        let independent = view.join("independent-project.bin");
        fs::write(&independent, b"SECOND_PROJECT_INDEPENDENT_CONTENT")?;
        assert!(
            !crashed.is_mounted()?,
            "the dead owner must not adopt a replacement volume"
        );
        crashed.try_unmount()?;
        drop(crashed);
        assert!(
            replacement.is_mounted()?,
            "locking/dropping the dead owner must preserve its replacement"
        );
        assert_eq!(
            fs::read(&independent)?,
            b"SECOND_PROJECT_INDEPENDENT_CONTENT"
        );
        replacement.try_unmount()?;
    }
    eprintln!("provider fixture: read-only view");
    let read_only = match provider.mount(
        &backing,
        &view,
        secret.as_bytes(),
        MountOptions {
            read_only: true,
            ..MountOptions::default()
        },
    ) {
        Ok(mut read_only) => {
            assert_eq!(fs::read(&renamed)?, content);
            assert!(fs::write(view.join("read-only.bin"), marker).is_err());
            read_only.try_unmount()?;
            "enforced"
        }
        Err(project_mount::Error::Unsupported(_)) => "unsupported-refused",
        Err(error) => return Err(error.into()),
    };
    eprintln!("provider fixture: idle expiration");
    let idle = match provider.mount(
        &backing,
        &view,
        secret.as_bytes(),
        MountOptions {
            idle_timeout: Some(Duration::from_secs(2)),
            ..MountOptions::default()
        },
    ) {
        Ok(mut idle) => {
            std::thread::sleep(Duration::from_secs(4));
            assert!(!idle.is_mounted()?, "provider idle timeout must unmount");
            idle.try_unmount()?;
            "provider-enforced"
        }
        Err(project_mount::Error::Unsupported(_)) => "controller-managed",
        Err(error) => return Err(error.into()),
    };

    fn inspect(directory: &std::path::Path, marker: &[u8]) -> std::io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                inspect(&path, marker)?;
            } else if entry.file_type()?.is_file() {
                let bytes = fs::read(path)?;
                assert!(!bytes.windows(marker.len()).any(|part| part == marker));
            }
            assert!(
                !entry
                    .file_name()
                    .to_string_lossy()
                    .contains("private-video")
            );
        }
        Ok(())
    }
    inspect(&backing, marker)?;
    println!(
        "{}",
        json!({"result":"pass","provider":provider.storage_format(),"bundled_provider":project_mount::is_bundled(),"bytes":content.len(),"normal_io":true,"media_seek":true,"open_handle_result":busy,"manual_lock":true,"no_plaintext_fallback":true,"remount_parity":true,"read_only":read_only,"idle_lock":idle,"sqlite_durable_commit":true,"backing_marker_absent":true,"supervisor_environ":supervisor_environ,"view":view})
    );
    Ok(())
}

/// The provider every mode runs: the embedded one, installed as a release installs it; else, on Linux, the given helper where a
/// release installs its own, which `discover` finds as it does that one (with any `fusermount` link); elsewhere the given executable.
fn provider(helper: &Path, artifacts: &Path) -> Result<Provider, Failure> {
    let tools = artifacts.join("tools");
    if project_mount::is_bundled() {
        let installed = Provider::discover(&tools)?;
        assert_eq!(
            Sha256::digest(fs::read(installed.executable())?),
            Sha256::digest(fs::read(helper)?),
            "the embedded provider must match its pinned executable"
        );
        assert!(tools.join("securefs/2.0.0/LICENSE.securefs.md").is_file());
        return Ok(installed);
    }
    if cfg!(target_os = "linux") {
        let installed = tools.join("securefs").join(project_mount::PROVIDER_VERSION);
        fs::create_dir_all(&installed)?;
        fs::copy(helper, installed.join("securefs"))?;
        return Ok(Provider::discover(&tools)?);
    }
    Ok(Provider::from_path(helper)?)
}

fn new_secret() -> Result<Zeroizing<String>, Failure> {
    let mut randomness = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut *randomness).map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(Zeroizing::new(URL_SAFE_NO_PAD.encode(*randomness)))
}

/// Create, mount, write a file, fsync it, read it back through a new descriptor, and lock: what a distro's container proves (cl-1000).
fn mount_only(provider: &Provider, artifacts: &Path) -> Result<(), Failure> {
    let backing = artifacts.join("encrypted");
    let view = project_mount::view_path(&artifacts.join("runtime"))?;
    let secret = new_secret()?;
    eprintln!("provider fixture: create and mount");
    provider.create(&backing, secret.as_bytes())?;
    let mut mounted = provider.mount(&backing, &view, secret.as_bytes(), MountOptions::default())?;
    let content = b"COZY_CL1000_MOUNTED".repeat(4096);
    eprintln!("provider fixture: write and fsync");
    let mut output = fs::File::create(view.join("mounted.bin"))?;
    output.write_all(&content)?;
    output.sync_all()?;
    drop(output);
    eprintln!("provider fixture: read back");
    if fs::read(view.join("mounted.bin"))? != content {
        return Err("the file read back differs from what was written".into());
    }
    eprintln!("provider fixture: lock");
    mounted.try_unmount()?;
    println!("{}", json!({"result":"mounted","wrote":content.len(),"read_back":true,"provider":provider.storage_format()}));
    Ok(())
}

/// A child that exited before its waiter read its birth, as a control command that finishes at once (cl-1050): macOS answered
/// `read a process's birth: No such process`, e2e 43789546310's 503 on an encrypted import. First one such child, made exited on
/// purpose: its birth must read and its exact handle say it ended. Then the real path under load: a mounted view's status (on
/// macOS `/sbin/mount` and `hdiutil info`, each a control command) read by 4 threads per core 250 times each, a spinner per core
/// keeping every core busy. Every read must say mounted; the view then locks.
fn controls(provider: &Provider, artifacts: &Path) -> Result<(), Failure> {
    use std::sync::atomic::{AtomicBool, Ordering};
    const READS: usize = 250;
    eprintln!("provider fixture: an exited child's birth");
    let mut child = Command::new(std::env::current_exe()?)
        .arg("exit")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    // Its output ends as it exits; it's then exited, unreaped, well before the read.
    child.stdout.take().ok_or("no child output")?.read_to_end(&mut Vec::new())?;
    std::thread::sleep(Duration::from_millis(200));
    let birth = platform::process::Birth::of(child.id())
        .map_err(|error| format!("an exited child's birth: {error}"))?;
    let exact = platform::process::Exact::open(&birth)?;
    if exact.as_ref().is_some_and(|exact| !exact.ended()) {
        return Err("an exited child's handle says it's running".into());
    }
    if !child.wait()?.success() {
        return Err("the exited child failed".into());
    }
    let backing = artifacts.join("encrypted");
    let view = project_mount::view_path(&artifacts.join("runtime"))?;
    let secret = new_secret()?;
    eprintln!("provider fixture: create and mount");
    provider.create(&backing, secret.as_bytes())?;
    let mut mounted = provider.mount(&backing, &view, secret.as_bytes(), MountOptions::default())?;
    let cores = std::thread::available_parallelism().map_or(2, |cores| cores.get());
    let calls = 4 * cores * READS;
    eprintln!("provider fixture: {calls} status reads on {} threads", 4 * cores);
    let busy = AtomicBool::new(true);
    let started = Instant::now();
    let failures: Vec<String> = std::thread::scope(|scope| {
        for _ in 0..cores {
            scope.spawn(|| {
                while busy.load(Ordering::Relaxed) {
                    std::hint::spin_loop();
                }
            });
        }
        let readers: Vec<_> = (0..4 * cores)
            .map(|_| {
                scope.spawn(|| {
                    (0..READS)
                        .filter_map(|_| match mounted.is_mounted() {
                            Ok(true) => None,
                            Ok(false) => Some("a mounted view read as unmounted".to_owned()),
                            Err(error) => Some(error.to_string()),
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let joined: Vec<_> = readers.into_iter().map(|reader| reader.join()).collect();
        busy.store(false, Ordering::Relaxed);
        joined.into_iter().flat_map(|reader| reader.unwrap_or_else(|_| vec!["a reader panicked".to_owned()])).collect()
    });
    let seconds = started.elapsed().as_secs_f64();
    println!(
        "{}",
        json!({"result": if failures.is_empty() {"pass"} else {"failed"}, "exited_child": "birth read", "calls": calls, "seconds": seconds, "failures": failures.len(), "first": failures.first()})
    );
    eprintln!("provider fixture: lock");
    mounted.try_unmount()?;
    match failures.first() {
        None => Ok(()),
        Some(first) => Err(format!("{} of {calls} status reads failed; first: {first}", failures.len()).into()),
    }
}

/// Linux: a session dropped without a lock while another process of this user holds the supervisor's stdin open for writing, as
/// any can now that the supervisor is dumpable. The drop must still end the provider and leave no view mounted, within its deadline.
fn dropped(provider: &Provider, artifacts: &Path) -> Result<(), Failure> {
    let backing = artifacts.join("encrypted");
    let view = project_mount::view_path(&artifacts.join("runtime"))?;
    let secret = new_secret()?;
    eprintln!("provider fixture: create and mount");
    provider.create(&backing, secret.as_bytes())?;
    let mounted = provider.mount(&backing, &view, secret.as_bytes(), MountOptions::default())?;
    let stdin = format!("/proc/{}/fd/0", mounted.provider_pid());
    let held = fs::OpenOptions::new()
        .write(true)
        .open(&stdin)
        .map_err(|error| format!("reopen the supervisor's stdin ({stdin}) for writing: {error}"))?;
    eprintln!("provider fixture: drop the session while {stdin} has a second writer");
    let started = Instant::now();
    drop(mounted);
    let seconds = started.elapsed().as_secs_f64();
    let mounted_after = provider.is_mounted(&view)?;
    drop(held);
    let pass = !mounted_after && seconds < 15.0;
    println!("{}", json!({"result": if pass {"pass"} else {"failed"}, "drop_seconds": seconds, "view_mounted": mounted_after}));
    if !pass {
        return Err(format!("a dropped session with a second stdin writer took {seconds:.1} s, view mounted after: {mounted_after}").into());
    }
    Ok(())
}

/// Writes `bytes` through a new encrypted view in 4 MiB chunks of a deterministic pattern, fsyncs, locks, unlocks and reads them
/// back, checking every byte. Prints MB/s (10^6 bytes) of the I/O alone: making and checking the pattern is off the clock.
fn throughput(provider: &Provider, artifacts: &Path, bytes: u64) -> Result<(), Failure> {
    const CHUNK: u64 = 4 << 20;
    // splitmix64 of each 8-byte word's index in the file: no two chunks alike.
    fn pattern(chunk: u64, buffer: &mut [u8]) {
        for (word, out) in buffer.chunks_mut(8).enumerate() {
            let mut z = (chunk * (CHUNK / 8) + word as u64).wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            out.copy_from_slice(&z.to_le_bytes()[..out.len()]);
        }
    }
    let backing = artifacts.join("encrypted");
    let view = project_mount::view_path(&artifacts.join("runtime"))?;
    let secret = new_secret()?;
    let file = view.join("throughput.bin");
    let chunks = bytes.div_ceil(CHUNK);
    let length = |index: u64| (bytes - index * CHUNK).min(CHUNK) as usize;
    let mut buffer = vec![0u8; CHUNK as usize];
    let mut expected = vec![0u8; CHUNK as usize];
    eprintln!("provider fixture: throughput: create, mount, write {bytes} bytes");
    provider.create(&backing, secret.as_bytes())?;
    let mut mounted = provider.mount(&backing, &view, secret.as_bytes(), MountOptions::default())?;
    let mut output = fs::File::create(&file)?;
    let mut writing = Duration::ZERO;
    for index in 0..chunks {
        let part = &mut buffer[..length(index)];
        pattern(index, part);
        let started = Instant::now();
        output.write_all(part)?;
        writing += started.elapsed();
    }
    let started = Instant::now();
    output.sync_all()?;
    drop(output);
    writing += started.elapsed();
    eprintln!("provider fixture: throughput: lock, unlock, read back");
    mounted.try_unmount()?;
    let mut mounted = provider.mount(&backing, &view, secret.as_bytes(), MountOptions::default())?;
    let mut input = fs::File::open(&file)?;
    let mut reading = Duration::ZERO;
    for index in 0..chunks {
        let part = &mut buffer[..length(index)];
        let started = Instant::now();
        input.read_exact(part)?;
        reading += started.elapsed();
        let want = &mut expected[..part.len()];
        pattern(index, want);
        if part != want {
            return Err(format!("throughput: chunk {index} read back differs from what was written").into());
        }
    }
    assert_eq!(input.read(&mut [0])?, 0, "the file must end where the writes did");
    drop(input);
    mounted.try_unmount()?;
    let rate = |time: Duration| (bytes as f64 / 1e6 / time.as_secs_f64() * 10.0).round() / 10.0;
    println!(
        "{}",
        json!({"bytes":bytes,"chunk_bytes":CHUNK,"write_MBps":rate(writing),"read_MBps":rate(reading),"provider":provider.storage_format(),"verified":true})
    );
    Ok(())
}
