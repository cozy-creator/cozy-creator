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

/// The driver's inventory (NVML) and each GPU's memory class (CUDA's device attributes). It
/// initializes CUDA here: call it once, in the service, which forks nothing without exec after.
/// No NVIDIA driver is a CPU machine.
pub(crate) fn gpus(visible: Option<&str>) -> (Vec<Gpu>, Vec<String>) {
    let (mut probed, mut problems) = platform::gpu::probe(visible);
    platform::gpu::memory_classes(&mut probed);
    let gpus = (probed.into_iter())
        .filter_map(|gpu| {
            let row = row(gpu);
            if let Err(why) = &row {
                problems.push(why.clone());
            }
            row.ok()
        })
        .collect();
    (gpus, problems)
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
            "machine_version": env!("CARGO_PKG_VERSION"),
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
