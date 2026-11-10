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
    let mut mounted =
        provider.mount(&backing, &view, secret.as_bytes(), MountOptions::default())?;
    let file = view.join("private-media-input.bin");
    let marker = b"COZY_XS160_PRIVATE_FILE_CONTENT";
    let mut content = marker.repeat(36_000);
    let mut output = fs::File::create(&file)?;
    output.write_all(&content)?;
    output.sync_all()?;
    drop(output);
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
    let held = fs::File::open(&renamed)?;
    let busy = match mounted.try_unmount() {
        Err(project_mount::Error::Busy(_)) => {
            assert!(mounted.is_mounted()?);
            "busy-preserved"
        }
        Ok(()) => {
            assert!(!mounted.is_mounted()?);
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
    remount.try_unmount()?;
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
        while crashed.is_mounted()? && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !crashed.is_mounted()?,
            "a crashed provider must not leave a stale kernel mount"
        );
        assert!(fs::write(view.join("crash-leak.bin"), marker).is_err());
        crashed.try_unmount()?;
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
    let mut idle = provider.mount(
        &backing,
        &view,
        secret.as_bytes(),
        MountOptions {
            idle_timeout: Some(Duration::from_secs(2)),
            ..MountOptions::default()
        },
    )?;
    // Do not issue filesystem requests while the provider's own idle timer runs.
    std::thread::sleep(Duration::from_secs(4));
    assert!(!idle.is_mounted()?, "provider idle timeout must unmount");
    idle.try_unmount()?;

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
        json!({"result":"pass","provider":"securefs 2.0.0 lite","bundled_provider":project_mount::is_bundled(),"bytes":content.len(),"normal_io":true,"media_seek":true,"open_handle_result":busy,"manual_lock":true,"no_plaintext_fallback":true,"remount_parity":true,"read_only":read_only,"idle_lock":true,"backing_marker_absent":true,"view":view})
    );
    Ok(())
}
