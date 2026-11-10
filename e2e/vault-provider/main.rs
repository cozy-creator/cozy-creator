//! Real provider fixture: FUSE/WinFsp and ordinary file/media I/O, without a mock filesystem.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use project_mount::{MountOptions, Provider};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    process::Command,
    time::Duration,
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

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 3 {
        return Err("usage: vault-provider normal|crash-host <securefs> <artifacts>".into());
    }
    let mode = arguments[0].to_string_lossy();
    let expected_helper = PathBuf::from(&arguments[1]);
    let artifacts = PathBuf::from(&arguments[2]);
    fs::create_dir_all(&artifacts)?;
    let provider = if project_mount::is_bundled() {
        let installed = Provider::discover(&artifacts.join("tools"))?;
        assert_eq!(
            Sha256::digest(fs::read(installed.executable())?),
            Sha256::digest(fs::read(&expected_helper)?),
            "the embedded provider must match its pinned executable"
        );
        assert!(
            artifacts
                .join("tools/securefs/2.0.0/LICENSE.securefs.md")
                .is_file()
        );
        installed
    } else {
        Provider::from_path(expected_helper)?
    };
    let backing = artifacts.join("encrypted");
    let view = project_mount::view_path(&artifacts.join("runtime"))?;
    let mut randomness = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut *randomness).map_err(|error| std::io::Error::other(error.to_string()))?;
    let secret = Zeroizing::new(URL_SAFE_NO_PAD.encode(*randomness));
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
        if let Err(error) = stage.try_unmount() {
            // Capture the actual dissenter before Mounted's owned abort cleanup releases it.
            let holders = Command::new("sudo")
                .args(["-n", "/usr/sbin/lsof", "-nP", "-Fpcftn", "+f", "--"])
                .arg(&view)
                .output()?;
            eprintln!(
                "native stage holders: {}{}",
                String::from_utf8_lossy(&holders.stdout),
                String::from_utf8_lossy(&holders.stderr)
            );
            let unmount = Command::new("/sbin/umount").arg(&view).output()?;
            eprintln!(
                "native stage direct normal umount: status={} stdout={} stderr={}",
                unmount.status,
                String::from_utf8_lossy(&unmount.stdout),
                String::from_utf8_lossy(&unmount.stderr)
            );
            return Err(error.into());
        }
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

    #[cfg(target_os = "linux")]
    {
        let pid = mounted.provider_pid();
        for path in [
            format!("/proc/{pid}/cmdline"),
            format!("/proc/{pid}/environ"),
        ] {
            let bytes = fs::read(path)?;
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
        json!({"result":"pass","provider":provider.storage_format(),"bundled_provider":project_mount::is_bundled(),"bytes":content.len(),"normal_io":true,"media_seek":true,"open_handle_result":busy,"manual_lock":true,"no_plaintext_fallback":true,"remount_parity":true,"read_only":read_only,"idle_lock":idle,"sqlite_durable_commit":true,"backing_marker_absent":true,"view":view})
    );
    Ok(())
}
