//! Native encrypted macOS storage. Logical project bytes remain the same; the physical image
//! is deliberately Mac-specific and never represented as a portable securefs archive.
use super::*;
use plist::Value;

pub(super) const MARKER: &str = ".cozy-apfs-v1";
const MAGIC: &[u8] = b"cozy-encrypted-apfs-sparsebundle-v1\n";
const IMAGE: &str = "project.sparsebundle";
const NORMAL_DETACH_ACK: Duration = Duration::from_secs(2);
const MONITOR_POLL: Duration = Duration::from_millis(100);
// EOF can arrive during a control probe. A detach pass can then use its initial
// probe, normal detach, confirmation probe, forced detach and final probe.
pub(super) const SHUTDOWN_ACK: Duration = CONTROL_ACK
    .saturating_mul(5)
    .saturating_add(NORMAL_DETACH_ACK)
    .saturating_add(MONITOR_POLL);

pub(super) fn validate(backing: &Path) -> Result<PathBuf> {
    let marker = backing.join(MARKER);
    if !marker.exists() && backing.join(".config.pb").exists() {
        return Err(Error::Unsupported(
            "this raw securefs vault needs a compatible securefs provider; export decoded project data before changing local storage formats",
        ));
    }

    if fs::metadata(&marker)
        .map_err(io_error("read native vault format"))?
        .len()
        != MAGIC.len() as u64
        || fs::read(marker).map_err(io_error("read native vault format"))? != MAGIC
    {
        return Err(Error::Unsupported(
            "this vault is not a supported native macOS APFS format",
        ));
    }
    let image = backing.join(IMAGE);
    let metadata = fs::symlink_metadata(&image).map_err(io_error("read native encrypted image"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Error::Invalid(
            "the native encrypted image must be a real sparsebundle directory",
        ));
    }
    fs::canonicalize(image).map_err(io_error("find native encrypted image"))
}

pub(super) fn create(provider: &Provider, backing: &Path, secret: &[u8]) -> Result<()> {
    let image = backing.join(IMAGE);
    let mut command = provider.command("create");
    command
        .args([
            "-size",
            "16t",
            "-type",
            "SPARSEBUNDLE",
            "-fs",
            "Case-sensitive APFS",
            "-nospotlight",
            "-volname",
            "Cozy Project",
            "-encryption",
            "AES-256",
            "-stdinpass",
        ])
        .arg(&image);
    run(
        provider,
        command,
        "create native encrypted image",
        Some(secret),
        Duration::from_secs(300),
    )?;
    encrypted(provider, &image)?;
    let mut marker = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(backing.join(MARKER))
        .map_err(io_error("write native vault format"))?;
    marker
        .write_all(MAGIC)
        .map_err(io_error("write native vault format"))?;
    marker
        .sync_all()
        .map_err(io_error("persist native vault format"))
}

fn encrypted(provider: &Provider, image: &Path) -> Result<()> {
    let mut command = provider.command("isencrypted");
    command.arg("-plist").arg(image);
    let value = decode(run(
        provider,
        command,
        "verify image encryption",
        None,
        CONTROL_ACK,
    )?)?;
    let encrypted = value.as_boolean().unwrap_or(false)
        || value.as_dictionary().is_some_and(|dictionary| {
            ["encrypted", "Encrypted", "image-encrypted"]
                .iter()
                .any(|key| dictionary.get(*key).and_then(Value::as_boolean) == Some(true))
        });
    if !encrypted {
        return Err(Error::Invalid(
            "the native image is not encrypted; refusing to expose it as a vault",
        ));
    }
    Ok(())
}

fn decode(bytes: Vec<u8>) -> Result<Value> {
    Value::from_reader(io::Cursor::new(bytes))
        .map_err(|_| Error::Invalid("the native disk-image tool returned an invalid property list"))
}

fn run(
    provider: &Provider,
    mut command: Command,
    operation: &'static str,
    secret: Option<&[u8]>,
    budget: Duration,
) -> Result<Vec<u8>> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("COZY_VAULT_PROVIDER_DIAGNOSTICS") {
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(io_error("open native provider diagnostics"))?;
        // Only executable/arguments: unlock material is supplied separately over stdin.
        writeln!(log, "native operation {operation}: {command:?}")
            .map_err(io_error("write native provider diagnostics"))?;
    }
    let mut child = command
        .stdin(if secret.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(provider_stderr()?)
        .spawn()
        .map_err(io_error("start native disk-image tool"))?;
    if let Some(secret) = secret {
        let mut input = child
            .stdin
            .take()
            .ok_or(Error::Invalid("native image stdin is unavailable"))?;
        // hdiutil's -stdinpass consumes a NUL-terminated single password. In particular,
        // securefs's newline/confirmation framing must never enter this password.
        if let Err(error) = input.write_all(secret).and_then(|_| input.write_all(&[0])) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io_error("send native image unlock material")(error));
        }
        drop(input);
    }
    let mut output = child
        .stdout
        .take()
        .ok_or(Error::Invalid("native image output is unavailable"))?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        output.read_to_end(&mut bytes).map(|_| bytes)
    });
    let status = if secret.is_some() {
        // Creating/attaching sends an OS mutation. Killing its client on a timer can leave a
        // late attachment behind; retain ownership until it actually completes, then cleanup.
        child
            .wait()
            .map_err(io_error("wait for native image mutation"))?
    } else {
        control_process(child, operation, budget)?.0
    };
    if !status.success() {
        return Err(provider.failure(operation, status));
    }
    reader
        .join()
        .map_err(|_| Error::Invalid("native image output reader failed"))?
        .map_err(io_error("read native image output"))
}

fn base_device(value: &Value) -> Option<String> {
    value
        .as_dictionary()?
        .get("system-entities")?
        .as_array()?
        .iter()
        .filter_map(Value::as_dictionary)
        .filter(|entity| {
            entity.get("content-hint").and_then(Value::as_string) == Some("GUID_partition_scheme")
        })
        .filter_map(|entity| entity.get("dev-entry")?.as_string())
        .find(|device| {
            device.strip_prefix("/dev/disk").is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
        .map(str::to_owned)
}

fn current_device(provider: &Provider, image: &Path) -> Result<Option<String>> {
    let mut command = provider.command("info");
    command.arg("-plist");
    let info = decode(run(
        provider,
        command,
        "read attached native images",
        None,
        CONTROL_ACK,
    )?)?;
    let images = info
        .as_dictionary()
        .and_then(|dictionary| dictionary.get("images"))
        .and_then(Value::as_array)
        .ok_or(Error::Invalid("the native image registry is unavailable"))?;
    for candidate in images {
        let Some(path) = candidate
            .as_dictionary()
            .and_then(|dictionary| dictionary.get("image-path"))
            .and_then(Value::as_string)
        else {
            continue;
        };
        let path = fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
        if path == image {
            return base_device(candidate).map(Some).ok_or(Error::Invalid(
                "an attached native image has no valid owned device identity",
            ));
        }
    }
    Ok(None)
}

pub(super) fn unmount(provider: &Provider, view: &Path, budget: Duration) -> Result<()> {
    // Disk Arbitration can reject an image eject solely because mds monitors its root.
    // A normal kernel unmount releases the volume without forcing an editor's open files.
    // The owned supervisor then detaches the exact image and its key before lock completes.
    let status = unix_unmount(view, false, budget)?;
    if provider.is_mounted(view)? {
        return Err(Error::Busy(view.to_path_buf()));
    }
    if !status.success() {
        return Err(provider.failure("unmount encrypted volume", status));
    }
    Ok(())
}

pub(super) struct Lease {
    provider: Provider,
    image: PathBuf,
    device: Option<String>,
    view: PathBuf,
    volume: Option<String>,
    attached: bool,
}
impl Lease {
    pub(super) fn is_current(&self) -> Result<bool> {
        Ok(self.attached
            && self.device.is_some()
            && self.volume.is_some()
            && macos_source(&self.view)? == self.volume
            && current_device(&self.provider, &self.image)? == self.device)
    }

    pub(super) fn capture(provider: &Provider, backing: &Path, view: &Path) -> Result<Self> {
        let image = validate(backing)?;
        let device = current_device(provider, &image)?.ok_or(Error::Invalid(
            "the mounted image has no owned backing-device identity",
        ))?;
        Ok(Self {
            provider: provider.clone(),
            image,
            device: Some(device),
            view: view.to_owned(),
            volume: Some(macos_source(view)?.ok_or(Error::Invalid(
                "the native image has no mounted volume identity",
            ))?),
            attached: true,
        })
    }

    pub(super) fn finish(&mut self) -> Result<bool> {
        if !self.attached {
            return Ok(true);
        }
        let Some(current) = current_device(&self.provider, &self.image)? else {
            self.attached = false;
            return Ok(true);
        };
        if self
            .device
            .as_ref()
            .is_some_and(|expected| *expected != current)
        {
            self.attached = false;
            return Ok(true);
        }
        let mut normal = self.provider.command("detach");
        normal.arg(&current);
        if run(
            &self.provider,
            normal,
            "close owned encrypted image",
            None,
            NORMAL_DETACH_ACK,
        )
        .is_err()
            && current_device(&self.provider, &self.image)?.as_deref() == Some(current.as_str())
        {
            let mut forced = self.provider.command("detach");
            forced.arg("-force").arg(&current);
            let _ = run(
                &self.provider,
                forced,
                "abort owned encrypted image",
                None,
                CONTROL_ACK,
            );
        }
        let remains =
            current_device(&self.provider, &self.image)?.as_deref() == Some(current.as_str());
        if !remains {
            self.attached = false;
        }
        Ok(!remains)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        // Device/key release, not elapsed time, completes shutdown. A live native attachment
        // must not survive a successful supervisor exit after its owner has disappeared.
        while !matches!(self.finish(), Ok(true)) {
            thread::sleep(Duration::from_secs(1));
        }
    }
}

pub(super) struct Request {
    pub(super) provider: Provider,
    pub(super) backing: PathBuf,
    pub(super) view: PathBuf,
    pub(super) secret: Zeroizing<Vec<u8>>,
    pub(super) parent: platform::process::Exact,
    pub(super) input: io::Stdin,
    pub(super) guard: ViewGuard,
    pub(super) read_only: bool,
}

pub(super) fn serve(request: Request) -> Result<()> {
    let Request {
        provider,
        backing,
        view,
        secret,
        parent,
        mut input,
        guard,
        read_only,
    } = request;
    let image = validate(&backing)?;
    encrypted(&provider, &image)?;
    if current_device(&provider, &image)?.is_some() {
        return Err(Error::Invalid(
            "the encrypted image is already attached; close that attachment before unlocking it here",
        ));
    }
    let mut lease = Lease {
        provider: provider.clone(),
        image: image.clone(),
        device: None,
        view: view.clone(),
        volume: None,
        attached: true,
    };
    let mut command = provider.command("attach");
    command
        .args(["-stdinpass", "-nobrowse", "-noautoopen", "-mountpoint"])
        .arg(&view)
        .arg("-plist");
    if read_only {
        command.arg("-readonly");
    }
    command.arg(&image);
    run(
        &provider,
        command,
        "attach encrypted image",
        Some(&secret[..43]),
        STARTUP,
    )?;
    drop(secret);
    // APFS attach output may name its synthesized container rather than the backing disk.
    // Record identity from the same image registry used for every later ownership check;
    // comparing those two different device identities would skip controller-death cleanup.
    let device = current_device(&provider, &image)?.ok_or(Error::Invalid(
        "the native image attached without an owned device identity",
    ))?;
    lease.device = Some(device);
    lease.volume = Some(macos_source(&view)?.ok_or(Error::Invalid(
        "the native image has no mounted volume identity",
    ))?);
    guard.seal()?;
    acknowledge()?;
    let (ended, stop) = mpsc::channel();
    thread::spawn(move || {
        let mut byte = [0];
        let _ = input.read(&mut byte);
        let _ = ended.send(());
    });
    let mut next_status = Instant::now() + Duration::from_secs(1);
    loop {
        if stop.try_recv().is_ok() || parent.wait_for(MONITOR_POLL)? {
            while !lease.finish()? {
                thread::sleep(Duration::from_secs(1));
            }
            return Ok(());
        }
        if Instant::now() >= next_status {
            next_status = Instant::now() + Duration::from_secs(1);
            if !provider.is_mounted(&view)? {
                // An external eject must also release the image's unlocked kernel attachment.
                while !lease.finish()? {
                    thread::sleep(Duration::from_secs(1));
                }
                return Ok(());
            }
        }
    }
}
