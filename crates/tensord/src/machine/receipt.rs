//! Readiness: what this machine measured about itself, kept once per boot and carried by Status
//! without a capability. The reader is Tensorhub `internal/podreadiness`, over the TLS connection
//! it opens to the address the provider reports: it pins the leaf that answers there, so these
//! facts carry no key of their own.
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{
    collections::BTreeMap,
    fs, io,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};

const MAX_FACTS_BYTES: usize = 64 << 10;

/// This boot's readiness facts. A rental's are measured once and served byte for byte across
/// restarts, so its Hub reads one boot; this computer's may refresh GPU discovery, never its
/// identity.
pub(crate) struct Readiness {
    path: Option<PathBuf>,
    rental: bool,
    state: Mutex<State>,
    proved: Condvar,
}
#[derive(Default)]
struct State {
    facts: Option<Vec<u8>>,
    proved: bool,
}

impl Readiness {
    /// `path` None keeps the facts in memory (development front doors).
    pub(crate) fn open(path: Option<PathBuf>, rental: bool) -> io::Result<Arc<Self>> {
        let facts = match &path {
            Some(path) => match fs::read(path) {
                Ok(raw) if !raw.is_empty() && raw.len() <= MAX_FACTS_BYTES => Some(raw),
                Ok(_) => return Err(invalid("the retained readiness facts are unreadable")),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            },
            None => None,
        };
        Ok(Arc::new(Self { path, rental, state: Mutex::new(State { facts, proved: false }), proved: Condvar::new() }))
    }

    /// Persisted facts belong to a real machine, which measures its GPUs. Development front doors
    /// report none rather than query the driver of the computer they run on.
    pub(crate) fn measures_gpus(&self) -> bool {
        self.path.is_some()
    }

    /// This boot's facts (retained, or measured by this process), or None before readiness.
    pub(crate) fn facts(&self) -> Option<Vec<u8>> {
        self.state.lock().unwrap().facts.clone()
    }

    /// True when this call publishes new facts. A rental keeps the same boot, leaf and GPU set;
    /// this computer's restart may update discovery, never its identity.
    pub(crate) fn seal(&self, payload: Vec<u8>) -> io::Result<bool> {
        let mut state = self.state.lock().unwrap();
        if let Some(kept) = &state.facts {
            if same_attestation(kept, &payload, !self.rental)? {
                state.proved = true;
                self.proved.notify_all();
                return Ok(false);
            }
        }
        if payload.len() > MAX_FACTS_BYTES {
            return Err(invalid("the readiness facts exceed 64 KiB"));
        }
        if let Some(path) = &self.path {
            super::identity::write_atomic(path, &payload, 0o444)?;
        }
        state.facts = Some(payload);
        state.proved = true;
        self.proved.notify_all();
        Ok(true)
    }

    pub(crate) fn proved(&self) -> bool {
        self.state.lock().unwrap().proved
    }

    /// Blocks until this process has proved readiness (sealed, or matched the retained seal).
    pub(crate) fn wait_proved(&self) {
        let state = self.state.lock().unwrap();
        drop(self.proved.wait_while(state, |s| !s.proved).unwrap());
    }
}

/// One accelerator as the Runtime and Hub name it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Gpu {
    pub device_index: u32,
    pub device_name: String,
    pub device_uuid: String,
    pub driver_version: String,
    pub memory_bytes: u64,
    pub pci_bus_id: String,
    /// `"9.0"`: the lane rule's row (`precision`).
    #[serde(default)]
    pub compute_cap: String,
    /// How it reaches host memory (`discrete`, `coherent`, `unified`); empty when unread. Status
    /// reports it; the readiness facts do not carry it.
    #[serde(skip)]
    pub memory_class: String,
}

/// How long the process that opens the GPUs (`probe_gpus`) may take to answer, and with contexts to exit too.
const OPENING_S: u64 = 60;

/// The driver's inventory (NVML) and each GPU's memory class (CUDA's device attributes), which a process of its own reads
/// (`probe_gpus`): TensorD never initializes CUDA itself, so its own exit never waits in the driver (cl-977). With `contexts`
/// that process also opens each GPU as an executor does and must exit, within `OPENING_S`: whether a fresh process can still
/// use them, the last answer an error saying why not. No NVIDIA driver is a CPU machine.
pub(crate) fn gpus(visible: Option<&str>, contexts: bool) -> (Vec<Gpu>, Vec<String>, Result<(), String>) {
    let (mut probed, mut problems) = platform::gpu::probe(visible);
    let opened = match probed.is_empty() {
        true => Ok(()),
        false => opened(&probed, contexts).map(|classes| probed.iter_mut().zip(classes).for_each(|(gpu, class)| gpu.memory_class = class)),
    };
    match &opened {
        Err(why) if !contexts => tracing::warn!("the GPUs' memory classes are unread: {why}"),
        _ => {}
    }
    let gpus = (probed.into_iter())
        .filter_map(|gpu| {
            let row = row(gpu);
            if let Err(why) = &row {
                problems.push(why.clone());
            }
            row.ok()
        })
        .collect();
    (gpus, problems, opened)
}

/// `<this binary> probe-gpus`'s answer for `gpus`, in their order; an error when it refused, or didn't answer (and with
/// `contexts` exit) within `OPENING_S`.
fn opened(gpus: &[platform::gpu::Gpu], contexts: bool) -> Result<Vec<Option<platform::gpu::MemoryClass>>, String> {
    use std::io::BufRead;
    let ids: Vec<&str> = gpus.iter().map(|gpu| gpu.pci_bus_id.as_str()).collect();
    let program = match cfg!(target_os = "linux") {
        true => PathBuf::from("/proc/self/exe"),
        false => std::env::current_exe().map_err(|e| format!("this binary's path: {e}"))?,
    };
    let mut command = std::process::Command::new(program);
    // `cozy machine probe-gpus`, or the `tensord` binary's `probe-gpus`.
    if std::env::args_os().nth(1).is_some_and(|word| word == "machine") {
        command.arg("machine");
    }
    command.arg("probe-gpus").arg(ids.join(","));
    if contexts {
        command.arg("--contexts");
    }
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    let mut child = command.spawn().map_err(|e| format!("a process to open the GPUs: {e}"))?;
    let stdout = child.stdout.take().ok_or("no answer from the process opening the GPUs")?;
    let pid = child.id();
    let (said, heard) = std::sync::mpsc::channel();
    // A process stuck in the driver answers nothing: this thread waits on it for as long as it lives, unwaited on.
    std::thread::Builder::new()
        .name("probe-gpus".into())
        .spawn(move || {
            let mut line = String::new();
            let _ = std::io::BufReader::new(stdout).read_line(&mut line);
            let _ = said.send(Some(line));
            let _ = said.send(child.wait().ok().map(|status| status.to_string()));
        })
        .map_err(|e| format!("a thread to wait on the GPUs' process: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(OPENING_S);
    let wait = || heard.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()));
    let unanswered = |what: &str| format!("a new process opening them (pid {pid}) didn't {what} in {OPENING_S} s");
    let line = wait().ok().flatten().ok_or_else(|| unanswered("answer"))?;
    let answer: serde_json::Value = serde_json::from_str(&line).map_err(|_| format!("the process opening them answered {:?}", line.trim()))?;
    if let Some(why) = answer["error"].as_str() {
        return Err(why.to_owned());
    }
    if contexts {
        wait().map_err(|_| unanswered("exit"))?;
    }
    let class = |class: &serde_json::Value| match class.as_str() {
        Some("discrete") => Some(platform::gpu::MemoryClass::Discrete),
        Some("coherent") => Some(platform::gpu::MemoryClass::Coherent),
        Some("unified") => Some(platform::gpu::MemoryClass::Unified),
        _ => None,
    };
    Ok(answer["classes"].as_array().map(|classes| classes.iter().map(class).collect()).unwrap_or_default())
}

/// `probe-gpus <bus ids> [--contexts]`: opens those GPUs through CUDA (`platform::gpu::open_cuda`) and answers one JSON line,
/// `{"classes": [...]}` or `{"error": "..."}`, then exits.
pub(crate) fn probe_gpus(args: impl IntoIterator<Item = std::ffi::OsString>) -> std::process::ExitCode {
    let args: Vec<String> = args.into_iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
    let contexts = args.iter().any(|arg| arg == "--contexts");
    let ids: Vec<String> = (args.iter().find(|arg| !arg.starts_with("--")))
        .map(|ids| ids.split(',').filter(|id| !id.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default();
    let answer = match platform::gpu::open_cuda(&ids, contexts) {
        Ok(classes) => serde_json::json!({"classes": classes.iter().map(|class| match class {
            Some(platform::gpu::MemoryClass::Discrete) => "discrete",
            Some(platform::gpu::MemoryClass::Coherent) => "coherent",
            Some(platform::gpu::MemoryClass::Unified) => "unified",
            None => "",
        }).collect::<Vec<_>>()}),
        Err(why) => serde_json::json!({"error": why}),
    };
    println!("{answer}");
    std::process::ExitCode::SUCCESS
}

/// One probed GPU as the receipt names it. An integrated GPU's (GB10) memory is the host's, as
/// the probe reads it.
fn row(g: platform::gpu::Gpu) -> Result<Gpu, String> {
    use platform::gpu::MemoryClass;
    let memory_bytes = match g.memory_bytes {
        0 => return Err(format!("the driver reports no memory for GPU {} ({}): it is not used", g.index, g.name)),
        // Whole MiB, as the readiness facts have always named it.
        bytes => bytes >> 20 << 20,
    };
    Ok(Gpu {
        device_index: g.index,
        device_name: g.name,
        device_uuid: g.uuid,
        driver_version: g.driver,
        memory_bytes,
        pci_bus_id: g.pci_bus_id,
        compute_cap: g.capability,
        memory_class: match g.memory_class {
            Some(MemoryClass::Discrete) => "discrete",
            Some(MemoryClass::Coherent) => "coherent",
            Some(MemoryClass::Unified) => "unified",
            None => "",
        }
        .into(),
    })
}

/// The facts the Hub's readiness reader requires of a machine image, as measured here on
/// `cozy.machine.v1`, the one protocol this listener serves.
pub(crate) struct Measured<'a> {
    pub boot_id: &'a str,
    pub worker_port: u16,
    pub cert_der: &'a [u8],
    pub gpus: Vec<Gpu>,
    pub listener_bound: bool,
    pub foreign_credential_refused: bool,
    /// The port `cozy/1` is served on; the Hub derives its pin from the leaf.
    pub webrtc_port: Option<u16>,
}
impl Measured<'_> {
    pub(crate) fn payload(&self) -> Vec<u8> {
        let mut value = serde_json::json!({
            "pod_boot_id": self.boot_id,
            "worker_internal_port": self.worker_port,
            "machine_protocol": "cozy.machine.v1",
            "worker_listener_bound": self.listener_bound,
            "worker_foreign_credential_refused": self.foreign_credential_refused,
            "tls_certificate_der_base64": STANDARD.encode(self.cert_der),
            "runtime_gpus": self.gpus,
            "machine_version": ::api::VERSION,
        });
        if let Some(port) = self.webrtc_port {
            value["webrtc"] = serde_json::json!({ "port": port });
        }
        serde_json::to_vec(&value).expect("a JSON value serializes")
    }
}

/// The attested facts, as compared across a same-boot restart.
fn same_attestation(before: &[u8], after: &[u8], refresh_local_gpu: bool) -> io::Result<bool> {
    fn facts(raw: &[u8]) -> io::Result<BTreeMap<&'static str, String>> {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Auth {
            control_public_key_ed25519_b64url: String,
            media_token_sha256: Vec<String>,
        }
        #[derive(serde::Deserialize)]
        struct Uuid {
            device_uuid: String,
        }
        #[derive(serde::Deserialize)]
        struct Attested {
            pod_boot_id: String,
            tls_certificate_der_base64: String,
            #[serde(default)]
            observed_record_owner_auth: Auth,
            #[serde(default)]
            runtime_gpus: Vec<Uuid>,
        }
        let a: Attested = serde_json::from_slice(raw)?;
        let mut tokens = a.observed_record_owner_auth.media_token_sha256;
        tokens.sort();
        let mut gpus: Vec<_> = a.runtime_gpus.into_iter().map(|g| g.device_uuid).collect();
        gpus.sort();
        Ok(BTreeMap::from([
            ("boot", a.pod_boot_id),
            ("leaf", a.tls_certificate_der_base64),
            (
                "owner",
                a.observed_record_owner_auth
                    .control_public_key_ed25519_b64url,
            ),
            ("tokens", tokens.join(",")),
            ("gpus", gpus.join(",")),
        ]))
    }
    let before = facts(before).map_err(|_| invalid("retained readiness facts are unreadable"))?;
    let after = facts(after).map_err(|_| invalid("current readiness facts are unreadable"))?;
    let same_gpus = before.get("gpus") == after.get("gpus");
    // A rental's facts are served as measured: a GPU lost since costs that GPU, and Status says so.
    if !same_gpus && !refresh_local_gpu {
        tracing::warn!("this machine's GPUs differ from what this boot attested");
    }
    let different: Vec<_> = before.iter().filter_map(|(key, value)|
        (after.get(key) != Some(value) && *key != "gpus").then_some(*key)).collect();
    if !different.is_empty() {
        // Names only: the leaf, owner key and token hashes are never included in diagnostics.
        return Err(invalid(&format!(
            "this boot attested different facts before restarting: {}", different.join(", ")
        )));
    }
    Ok(same_gpus || !refresh_local_gpu)
}

/// The boot id the payload names.
pub(crate) fn boot_id(payload: &[u8]) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Boot {
        pod_boot_id: String,
    }
    serde_json::from_slice::<Boot>(payload)
        .ok()
        .map(|b| b.pod_boot_id)
}

fn invalid(detail: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail.to_owned())
}
