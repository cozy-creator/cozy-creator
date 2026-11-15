//! The machine lifetime: its boot id and TLS leaf, persisted with the root so a restarted
//! service is the same machine.
use platform::{
    lock::{Kind, LockFile},
    process::{Birth, State},
};
use super::grant::Layout;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};
use platform::durable::Mode;

pub(crate) struct Lifetime {
    pub boot_id: String,
    pub cert_pem: String,
    pub key_pem: String,
    pub cert_der: Vec<u8>,
}

/// How long an earlier machine still leaving may hold the root before this one judges it (cl-977).
const LEAVING: std::time::Duration = std::time::Duration::from_secs((super::stuck::GRACE_S + super::stuck::STALL_S) as u64);

/// The root's lock as this machine's stable parent holds it. The service it forks lets its copy go (`let_go`): a service that
/// can't exit holds no root (cl-977).
static ROOT: std::sync::Mutex<Option<fs::File>> = std::sync::Mutex::new(None);

/// One machine per root. Every machine on a root holds this lock (`agent.lock`) for its life, and the launcher (`cozy
/// machine`) probes it before it starts or replaces one. A live holder refuses the root. A holder recorded stuck (`stuck`, it runs
/// no code again) is taken over at once; one still leaving is waited for on its handle at most `LEAVING`, then the root is
/// refused, naming it: no wait here is unbounded.
pub(crate) fn hold(layout: &Layout, stuck: &[super::stuck::Stuck]) -> io::Result<()> {
    fs::create_dir_all(&layout.state)?;
    let lock = LockFile::new(layout.state.join("agent.lock"));
    let held = match lock.try_lock(Kind::Exclusive)? {
        Some(held) => held,
        None => match waited(&lock, layout, stuck)? {
            Some(held) => held,
            None => return Ok(()),
        },
    };
    held.publish(&serde_json::to_vec(&Birth::own()?)?)?;
    // Held by this parent; never by an executor (close-on-exec), nor the service (`let_go`).
    *ROOT.lock().unwrap() = Some(held.keep());
    Ok(())
}

/// The forked service's copy of the root's lock, closed: only its stable parent holds the root.
#[cfg(unix)]
pub(crate) fn let_go() {
    drop(ROOT.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take());
}

/// The lock once its holder, no live machine, lets go; `None`: taken over unheld.
fn waited(lock: &LockFile, layout: &Layout, stuck: &[super::stuck::Stuck]) -> io::Result<Option<platform::lock::Held>> {
    let root = layout.root.display();
    let holder = lock.holder()?.and_then(|record| serde_json::from_slice::<Birth>(&record).ok());
    if let Some(stuck) = holder.as_ref().and_then(|birth| stuck.iter().find(|stuck| stuck.birth == *birth)) {
        tracing::warn!("this machine's root is held by the TensorD before this one ({}); this machine takes it over", stuck.line());
        return Ok(None);
    }
    // This parent before it exec'd an update (`supervise::exec`): whatever still holds it is of this machine.
    if holder.as_ref() == Some(&Birth::own()?) {
        tracing::warn!("this machine's root is still held from before its update; it serves on");
        return Ok(None);
    }
    let state = holder.as_ref().map(|birth| birth.state()).transpose().map_err(io::Error::other)?;
    if let (Some(live), Some(State::Live | State::Stopped)) = (&holder, state) {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("another machine owns the root {root} (pid {})", live.pid)));
    }
    if let Some(exact) = holder.as_ref().and_then(|birth| platform::process::Exact::open(birth).ok().flatten()) {
        let _ = exact.wait_for(LEAVING);
    }
    if let Some(held) = lock.try_lock(Kind::Exclusive)? {
        return Ok(Some(held));
    }
    let by = match &holder {
        Some(birth) => match platform::process::in_kernel(birth.pid) {
            Some(wchan) => format!(" by pid {}, in the kernel at {wchan}", birth.pid),
            None => format!(" by pid {}", birth.pid),
        },
        None => String::new(),
    };
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("the root {root} is still held{by} {} s after its machine ended: it can't exit, and a reboot frees it", LEAVING.as_secs()),
    ))
}

/// Mints or reopens this root's lifetime.
pub(crate) fn open(layout: &Layout) -> io::Result<Lifetime> {
    for dir in [&layout.bootstrap, &layout.state] {
        fs::create_dir_all(dir)?;
    }
    let path = layout.state.join("boot-id");
    let boot_id = match fs::read_to_string(&path) {
        Ok(text) if !text.trim().is_empty() => text.trim().to_owned(),
        Ok(_) => return Err(invalid("the persisted boot id is empty")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let boot_id = URL_SAFE_NO_PAD.encode(random::<32>()?);
            write_atomic(&path, boot_id.as_bytes(), 0o400)?;
            boot_id
        }
        Err(e) => return Err(e),
    };
    if URL_SAFE_NO_PAD.decode(&boot_id).map(|b| b.len()) != Ok(32) {
        return Err(invalid(
            "the persisted boot id is not unpadded base64url for 32 bytes",
        ));
    }
    let (cert_path, key_path) = (
        layout.bootstrap.join("tls.crt"),
        layout.bootstrap.join("tls.key"),
    );
    if !cert_path.exists() {
        let (cert_pem, key_pem) = mint_leaf()?;
        write_atomic(&key_path, key_pem.as_bytes(), 0o400)?;
        write_atomic(&cert_path, cert_pem.as_bytes(), 0o444)?;
    }
    let cert_pem = fs::read_to_string(&cert_path)?;
    let key_pem = fs::read_to_string(&key_path)?;
    use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
    let certs: Vec<_> = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<_, _>>()
        .map_err(io::Error::other)?;
    let [cert] = certs.as_slice() else {
        return Err(invalid("the machine leaf must be exactly one certificate"));
    };
    PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|_| invalid("the machine leaf has no private key"))?;
    Ok(Lifetime {
        boot_id,
        cert_der: cert.as_ref().to_vec(),
        cert_pem,
        key_pem,
    })
}

/// What signs this machine's worker requests to its Hub: the leaf's own key, which the Hub pinned
/// when it reached the machine at its provider's address. It is never sent anywhere.
pub(crate) fn worker_signer(key_pem: &str) -> io::Result<::api::hub::WorkerSigner> {
    use aws_lc_rs::{rand::SystemRandom, signature::{EcdsaKeyPair, ECDSA_P256_SHA256_ASN1_SIGNING}};
    use rustls::pki_types::{pem::PemObject, PrivatePkcs8KeyDer};
    let der = PrivatePkcs8KeyDer::from_pem_slice(key_pem.as_bytes())
        .map_err(|_| invalid("the machine leaf's key is not PKCS#8"))?;
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, der.secret_pkcs8_der())
        .map_err(|_| invalid("the machine leaf's key is not P-256"))?;
    let random = SystemRandom::new();
    Ok(std::sync::Arc::new(move |message: &[u8]| {
        (key.sign(&random, message).map(|signature| signature.as_ref().to_vec()))
            .map_err(|_| "the machine leaf could not sign".to_owned())
    }))
}

/// The leaf the Hub pins: P-256 (browsers' WebRTC DTLS takes no Ed25519), self-signed, exactly the
/// DNS SAN `cozy-worker`, usable as both TLS server and Tensorhub client, valid until year 9999
/// (clients pin the bytes). Minted once per machine root; its key is this machine's credential.
fn mint_leaf() -> io::Result<(String, String)> {
    use rcgen::{
        CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyPair,
        KeyUsagePurpose,
    };
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(io::Error::other)?;
    let mut params = CertificateParams::new(vec![api::machine::SERVER_NAME.into()]).map_err(io::Error::other)?;
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "tensord");
    params.distinguished_name = name;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    params.not_before = rcgen::date_time_ymd(1970, 1, 1) + now - std::time::Duration::from_secs(300);
    params.not_after = rcgen::date_time_ymd(9999, 12, 31);
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];
    let cert = params.self_signed(&key).map_err(io::Error::other)?;
    Ok((cert.pem(), key.serialize_pem()))
}

pub(crate) fn random<const N: usize>() -> io::Result<[u8; N]> {
    let mut out = [0; N];
    getrandom::fill(&mut out).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(out)
}

pub(crate) fn write_atomic(target: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    let dir = target
        .parent()
        .ok_or_else(|| invalid("a target needs a directory"))?;
    fs::create_dir_all(dir)?;
    let temp = dir.join(format!(".write-{}", URL_SAFE_NO_PAD.encode(random::<9>()?)));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(data)?;
        platform::durable::set_file_mode(&file, mode)?;
        file.sync_all()?;
        // Closed before it is named: an executable written here runs at once, and an open
        // writer (this one, or a fork's copy of it) makes exec fail with ETXTBSY.
        drop(file);
        fs::rename(&temp, target)?;
        Ok(platform::durable::sync_dir(dir)?)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn invalid(detail: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail.to_owned())
}
