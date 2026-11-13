//! Local, project-scoped key custody for an optional encrypted project.
//!
//! The data filesystem belongs to the mount provider. This crate stores only a random
//! printable filesystem secret, independently wrapped by a human passphrase and by a
//! high-entropy recovery secret using Argon2id and AES-256-GCM. Copying the encrypted
//! project and its keybag is enough for offline recovery; no account service is involved.
//!
//! Keybags deliberately have no plaintext secret fields. Unlocked values redact Debug,
//! cannot be serialized, and zeroize on drop. Callers must pass filesystem secrets through
//! an anonymous pipe, never process arguments, environment variables or logs.
#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

use std::{
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime},
};

use argon2::{Algorithm, Argon2, Params, Version};
use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Explicitly owned, redacted and zeroizing credential material.
#[derive(Clone)]
pub struct SecretString(Zeroizing<String>);
impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
}
impl SecretString {
    fn expose_secret(&self) -> &str {
        self.0.as_str()
    }
}
impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString([REDACTED])")
    }
}

/// Filename kept beside an encrypted project's backing directory, not inside its mount.
pub const KEYBAG_FILE: &str = "project.vault.keys.json";
/// Portable keybag format version.
pub const FORMAT_VERSION: u32 = 2;
/// The named cipher/encoding suite for the small key envelopes, not the filesystem data.
pub const ENVELOPE_SUITE: &str = "argon2id-v19-aes256gcm-v1";
/// Maximum size of a portable keybag accepted from disk.
pub const MAX_KEYBAG_BYTES: u64 = 64 * 1024;
// Fixed, validated before KDF work. A received keybag cannot request arbitrary resources.
const KDF_MEMORY_KIB: u32 = 256 * 1024;
const KDF_ITERATIONS: u32 = 3;
const KDF_LANES: u32 = 1;
const MAX_SECRET_PAYLOAD: usize = 2048;
const RECOVERY_PREFIX: &str = "cozy1";

/// Errors intentionally never contain credentials, decrypted payloads or secret bytes.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("project vault I/O: {0}")]
    Io(#[from] io::Error),
    #[error("invalid project keybag: {0}")]
    Format(&'static str),
    #[error("the project keybag uses an unsupported format or cipher suite")]
    Unsupported,
    #[error("passphrase must not be empty")]
    EmptyPassphrase,
    #[error("the passphrase or recovery secret did not unlock this project")]
    Unlock,
    #[error("recovery secret has an invalid format or checksum")]
    Recovery,
    #[error("project secret does not belong to this project key generation")]
    WrongProject,
    #[error("could not encrypt the project key envelope")]
    Encrypt,
    #[error("could not obtain operating-system randomness")]
    Random,
    #[error("project key generation is exhausted")]
    GenerationExhausted,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Public keybag information suitable for a local status response.
#[derive(Clone, Debug, Serialize)]
pub struct KeybagInfo {
    pub format_version: u32,
    pub project_id: String,
    pub generation: u64,
    pub key_id: String,
}

/// A portable encrypted keybag. Only authenticated ciphertext contains the filesystem secret.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keybag {
    format_version: u32,
    envelope_suite: String,
    project_id: String,
    generation: u64,
    key_id: String,
    passphrase_envelope: Envelope,
    recovery_envelope: Envelope,
}

impl fmt::Debug for Keybag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Keybag")
            .field("info", &self.info())
            .finish_non_exhaustive()
    }
}

/// A passphrase or the independent saved recovery secret.
pub enum Credential {
    Passphrase(SecretString),
    Recovery(SecretString),
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Passphrase(_) => "Credential::Passphrase([REDACTED])",
            Self::Recovery(_) => "Credential::Recovery([REDACTED])",
        })
    }
}

/// An unlocked secret. The 43 ASCII bytes are suitable for the mount provider's stdin.
pub struct ProjectSecret {
    project_id: String,
    generation: u64,
    key_id: String,
    value: SecretString,
}

impl fmt::Debug for ProjectSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectSecret")
            .field("project_id", &self.project_id)
            .field("generation", &self.generation)
            .field("key_id", &self.key_id)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

impl ProjectSecret {
    /// Deliberately copy an already authorized in-memory secret for one scoped operation
    /// such as rekeying. The independent copy remains redacted, non-serializable and
    /// zeroizing; callers drop it after the operation and never persist it.
    pub fn duplicate(&self) -> Self {
        Self {
            project_id: self.project_id.clone(),
            generation: self.generation,
            key_id: self.key_id.clone(),
            value: self.value.clone(),
        }
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Expose bytes only within an explicit operation such as writing the helper's pipe.
    /// No newline is included; the mount provider owns its password-input framing.
    pub fn with_bytes<T>(&self, operation: impl FnOnce(&[u8]) -> T) -> T {
        operation(self.value.expose_secret().as_bytes())
    }

    /// Write one newline-terminated copy through the helper's stdin, never a command line.
    pub fn write_stdin(&self, output: &mut impl Write) -> io::Result<()> {
        self.with_bytes(|value| output.write_all(value))?;
        output.write_all(b"\n")
    }
}

/// A 256-bit recovery secret with a transcription checksum. Never serialized implicitly.
pub struct RecoverySecret(SecretString);

impl fmt::Debug for RecoverySecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoverySecret([REDACTED])")
    }
}

impl RecoverySecret {
    /// Explicit disclosure for the one-time local copy/download/print/QR flow.
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }

    /// Parse a saved secret, accepting case differences and presentation whitespace.
    pub fn parse(value: SecretString) -> Result<Self> {
        let mut compact = Zeroizing::new(String::new());
        for ch in value.expose_secret().chars() {
            if !ch.is_ascii() {
                return Err(Error::Recovery);
            }
            if !ch.is_ascii_whitespace() && ch != '-' {
                compact.push(ch.to_ascii_lowercase());
            }
        }
        let Some(body) = compact.strip_prefix(RECOVERY_PREFIX) else {
            return Err(Error::Recovery);
        };
        if body.len() != 72 {
            return Err(Error::Recovery);
        }
        let bytes = Zeroizing::new(hex::decode(&body[..64]).map_err(|_| Error::Recovery)?);
        if bytes.len() != 32 || body[64..] != recovery_checksum(&bytes) {
            return Err(Error::Recovery);
        }
        Ok(Self(SecretString::from(format_recovery(&bytes))))
    }

    fn generate() -> Result<Self> {
        let mut bytes = Zeroizing::new([0u8; 32]);
        getrandom::fill(&mut *bytes).map_err(|_| Error::Random)?;
        Ok(Self(SecretString::from(format_recovery(&bytes[..]))))
    }
}

/// Initialization returns the recovery secret exactly once; callers decide its local UI.
pub struct CreatedKeybag {
    pub keybag: Keybag,
    pub secret: ProjectSecret,
    pub recovery_secret: RecoverySecret,
}

impl fmt::Debug for CreatedKeybag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedKeybag")
            .field("keybag", &self.keybag)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
struct SecretPayload {
    #[zeroize(skip)]
    format_version: u32,
    #[zeroize(skip)]
    project_id: String,
    #[zeroize(skip)]
    generation: u64,
    secret: String,
}

impl Keybag {
    pub fn create(project_id: &str, passphrase: SecretString) -> Result<CreatedKeybag> {
        Self::create_generation(project_id, 1, passphrase)
    }

    /// Create fresh material after a plaintext interval while preserving the monotonically
    /// increasing generation used by retained project-local execution records.
    pub fn create_generation(
        project_id: &str,
        generation: u64,
        passphrase: SecretString,
    ) -> Result<CreatedKeybag> {
        validate_project_id(project_id)?;
        if generation == 0 {
            return Err(Error::Format("generation must be positive"));
        }
        check_passphrase(&passphrase)?;
        let mut bytes = Zeroizing::new([0u8; 32]);
        getrandom::fill(&mut *bytes).map_err(|_| Error::Random)?;
        let value =
            SecretString::from(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&*bytes));
        let key_id = key_fingerprint(value.expose_secret().as_bytes());
        let secret = ProjectSecret {
            project_id: project_id.to_owned(),
            generation,
            key_id: key_id.clone(),
            value,
        };
        let recovery_secret = RecoverySecret::generate()?;
        let payload = encode_secret(&secret)?;
        let mut keybag = Self {
            format_version: FORMAT_VERSION,
            envelope_suite: ENVELOPE_SUITE.to_owned(),
            project_id: project_id.to_owned(),
            generation,
            key_id,
            passphrase_envelope: Envelope::empty(),
            recovery_envelope: Envelope::empty(),
        };
        keybag.passphrase_envelope = keybag.wrap("passphrase", &payload, passphrase)?;
        keybag.recovery_envelope = keybag.wrap(
            "recovery",
            &payload,
            SecretString::from(recovery_secret.expose().to_owned()),
        )?;
        Ok(CreatedKeybag {
            keybag,
            secret,
            recovery_secret,
        })
    }

    pub fn info(&self) -> KeybagInfo {
        KeybagInfo {
            format_version: self.format_version,
            project_id: self.project_id.clone(),
            generation: self.generation,
            key_id: self.key_id.clone(),
        }
    }

    /// Load a bounded, fully validated ciphertext-only keybag.
    pub fn load(path: &Path) -> Result<Self> {
        let input = File::open(path)?;
        if input.metadata()?.len() > MAX_KEYBAG_BYTES {
            return Err(Error::Format("keybag is too large"));
        }
        let mut bytes = Vec::new();
        input.take(MAX_KEYBAG_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_KEYBAG_BYTES {
            return Err(Error::Format("keybag is too large"));
        }
        let document: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| Error::Format("invalid keybag JSON"))?;
        if document
            .get("format_version")
            .and_then(serde_json::Value::as_u64)
            != Some(u64::from(FORMAT_VERSION))
        {
            return Err(Error::Unsupported);
        }
        let keybag: Self =
            serde_json::from_value(document).map_err(|_| Error::Format("invalid keybag JSON"))?;
        keybag.validate()?;
        Ok(keybag)
    }

    /// Atomically replace this keybag after syncing its ciphertext bytes.
    /// The caller must serialize project mutation and retain the prior generation until
    /// the matching replacement filesystem has been verified.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|_| Error::Format("cannot serialize keybag"))?;
        if bytes.len() as u64 > MAX_KEYBAG_BYTES {
            return Err(Error::Format("keybag is too large"));
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        platform::durable::replace(path, &bytes, 0o600).map_err(io::Error::from)?;
        Ok(())
    }

    /// Open one of the two independent wrapping slots. Credential contents never appear
    /// in the returned error. Caller can separately compare the public project ID with its
    /// registered project identity before accepting a copied/moved keybag.
    pub fn unlock(&self, credential: Credential) -> Result<ProjectSecret> {
        self.validate()?;
        let payload = match credential {
            Credential::Passphrase(passphrase) => {
                check_passphrase(&passphrase)?;
                self.unwrap("passphrase", &self.passphrase_envelope, passphrase)?
            }
            Credential::Recovery(recovery) => {
                let recovery = RecoverySecret::parse(recovery)?;
                self.unwrap(
                    "recovery",
                    &self.recovery_envelope,
                    SecretString::from(recovery.expose().to_owned()),
                )?
            }
        };
        let mut decoded: SecretPayload =
            serde_json::from_slice(&payload).map_err(|_| Error::Unlock)?;
        if decoded.format_version != FORMAT_VERSION
            || decoded.project_id != self.project_id
            || decoded.generation != self.generation
            || decoded.secret.len() != 43
            || key_fingerprint(decoded.secret.as_bytes()) != self.key_id
        {
            return Err(Error::Unlock);
        }
        // Also require canonical, full-entropy URL-safe encoding, rather than allowing
        // arbitrary decrypted strings to reach a line-oriented helper input.
        let raw = Zeroizing::new(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(decoded.secret.as_bytes())
                .map_err(|_| Error::Unlock)?,
        );
        if raw.len() != 32 {
            return Err(Error::Unlock);
        }
        Ok(ProjectSecret {
            project_id: self.project_id.clone(),
            generation: self.generation,
            key_id: self.key_id.clone(),
            value: SecretString::from(std::mem::take(&mut decoded.secret)),
        })
    }

    /// Replace only the passphrase slot. This does not re-encrypt files or revoke copied
    /// older keybags/credentials; actual key compromise requires a fresh generation.
    pub fn change_passphrase(
        &mut self,
        secret: &ProjectSecret,
        new_passphrase: SecretString,
    ) -> Result<()> {
        self.verify_secret(secret)?;
        check_passphrase(&new_passphrase)?;
        let payload = encode_secret(secret)?;
        let replacement = self.wrap("passphrase", &payload, new_passphrase)?;
        self.passphrase_envelope = replacement;
        Ok(())
    }

    /// Replace the saved recovery credential while preserving the project secret.
    /// An older copied keybag and its older code remain usable for that old generation.
    pub fn replace_recovery(&mut self, secret: &ProjectSecret) -> Result<RecoverySecret> {
        self.verify_secret(secret)?;
        let recovery = RecoverySecret::generate()?;
        let payload = encode_secret(secret)?;
        let replacement = self.wrap(
            "recovery",
            &payload,
            SecretString::from(recovery.expose().to_owned()),
        )?;
        self.recovery_envelope = replacement;
        Ok(recovery)
    }

    /// Create fresh key material for a complete filesystem rewrite. This method never
    /// activates it: the caller stages/copies/verifies and atomically switches the vault.
    pub fn new_generation(&self, passphrase: SecretString) -> Result<CreatedKeybag> {
        self.validate()?;
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(Error::GenerationExhausted)?;
        Self::create_generation(&self.project_id, generation, passphrase)
    }

    fn validate(&self) -> Result<()> {
        if self.format_version != FORMAT_VERSION || self.envelope_suite != ENVELOPE_SUITE {
            return Err(Error::Unsupported);
        }
        validate_project_id(&self.project_id)?;
        if self.generation == 0
            || self.key_id.len() != 64
            || !self.key_id.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::Format("invalid key identity"));
        }
        self.passphrase_envelope.validate()?;
        self.recovery_envelope.validate()?;
        Ok(())
    }

    fn verify_secret(&self, secret: &ProjectSecret) -> Result<()> {
        if secret.project_id != self.project_id
            || secret.generation != self.generation
            || secret.key_id != self.key_id
        {
            return Err(Error::WrongProject);
        }
        Ok(())
    }
}

fn validate_project_id(project_id: &str) -> Result<()> {
    if project_id.is_empty() || project_id.len() > 256 || project_id.chars().any(char::is_control) {
        return Err(Error::Format("invalid project identity"));
    }
    Ok(())
}

fn check_passphrase(value: &SecretString) -> Result<()> {
    if value.expose_secret().len() > 4096 {
        return Err(Error::Format("credential is too long"));
    }
    if value.expose_secret().is_empty() {
        Err(Error::EmptyPassphrase)
    } else {
        Ok(())
    }
}

fn key_fingerprint(secret: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"cozy-project-vault-key-v2\0");
    hash.update(secret);
    hex::encode(hash.finalize())
}

fn encode_secret(secret: &ProjectSecret) -> Result<Zeroizing<Vec<u8>>> {
    let payload = SecretPayload {
        format_version: FORMAT_VERSION,
        project_id: secret.project_id.clone(),
        generation: secret.generation,
        secret: secret.value.expose_secret().to_owned(),
    };
    serde_json::to_vec(&payload)
        .map(Zeroizing::new)
        .map_err(|_| Error::Format("cannot serialize secret payload"))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    memory_kib: u32,
    iterations: u32,
    lanes: u32,
    salt: String,
    nonce: String,
    ciphertext: String,
}

impl Envelope {
    fn empty() -> Self {
        Self {
            memory_kib: KDF_MEMORY_KIB,
            iterations: KDF_ITERATIONS,
            lanes: KDF_LANES,
            salt: String::new(),
            nonce: String::new(),
            ciphertext: String::new(),
        }
    }
    fn validate(&self) -> Result<()> {
        if self.memory_kib != KDF_MEMORY_KIB
            || self.iterations != KDF_ITERATIONS
            || self.lanes != KDF_LANES
        {
            return Err(Error::Format("unsupported password derivation parameters"));
        }
        if self.salt.len() != 64
            || self.nonce.len() != 24
            || !self
                .salt
                .bytes()
                .chain(self.nonce.bytes())
                .all(|byte| byte.is_ascii_hexdigit())
            || self.ciphertext.len() > 4096
        {
            return Err(Error::Format("invalid key envelope"));
        }
        let cipher = STANDARD
            .decode(&self.ciphertext)
            .map_err(|_| Error::Format("invalid envelope encoding"))?;
        if cipher.len() < 16 || cipher.len() > MAX_SECRET_PAYLOAD + 16 {
            return Err(Error::Format("invalid envelope length"));
        }
        Ok(())
    }
}

impl Keybag {
    fn aad(&self, slot: &str) -> Result<Vec<u8>> {
        serde_json::to_vec(&(
            FORMAT_VERSION,
            ENVELOPE_SUITE,
            &self.project_id,
            self.generation,
            &self.key_id,
            slot,
            KDF_MEMORY_KIB,
            KDF_ITERATIONS,
            KDF_LANES,
        ))
        .map_err(|_| Error::Encrypt)
    }
    fn wrap(&self, slot: &str, payload: &[u8], passphrase: SecretString) -> Result<Envelope> {
        check_passphrase(&passphrase)?;
        if payload.len() > MAX_SECRET_PAYLOAD {
            return Err(Error::Encrypt);
        }
        let mut salt = [0u8; 32];
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut salt).map_err(|_| Error::Random)?;
        getrandom::fill(&mut nonce).map_err(|_| Error::Random)?;
        let key = derive_key(&passphrase, &salt)?;
        let cipher =
            LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &*key).map_err(|_| Error::Encrypt)?);
        let mut body = Zeroizing::new(payload.to_vec());
        cipher
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(self.aad(slot)?),
                &mut *body,
            )
            .map_err(|_| Error::Encrypt)?;
        Ok(Envelope {
            salt: hex::encode(salt),
            nonce: hex::encode(nonce),
            ciphertext: STANDARD.encode(&*body),
            ..Envelope::empty()
        })
    }
    fn unwrap(
        &self,
        slot: &str,
        envelope: &Envelope,
        passphrase: SecretString,
    ) -> Result<Zeroizing<Vec<u8>>> {
        envelope.validate()?;
        check_passphrase(&passphrase)?;
        let salt = hex::decode(&envelope.salt).map_err(|_| Error::Unlock)?;
        let nonce: [u8; 12] = hex::decode(&envelope.nonce)
            .map_err(|_| Error::Unlock)?
            .try_into()
            .map_err(|_| Error::Unlock)?;
        let key = derive_key(&passphrase, &salt)?;
        let cipher =
            LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &*key).map_err(|_| Error::Unlock)?);
        let mut body = Zeroizing::new(
            STANDARD
                .decode(&envelope.ciphertext)
                .map_err(|_| Error::Unlock)?,
        );
        let opened = cipher
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(self.aad(slot)?),
                &mut body,
            )
            .map_err(|_| Error::Unlock)?;
        Ok(Zeroizing::new(opened.to_vec()))
    }
}

fn derive_key(passphrase: &SecretString, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(KDF_MEMORY_KIB, KDF_ITERATIONS, KDF_LANES, Some(32))
        .map_err(|_| Error::Encrypt)?;
    // Argon2's allocating convenience API does not wipe its arena. Own every block so
    // intermediate password-derived material is cleared on success and every error path.
    let mut memory = Zeroizing::new(Vec::<argon2::Block>::new());
    memory
        .try_reserve_exact(params.block_count())
        .map_err(|_| Error::Encrypt)?;
    memory.resize(params.block_count(), argon2::Block::new());
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into_with_memory(
            passphrase.expose_secret().as_bytes(),
            salt,
            &mut *key,
            &mut memory[..],
        )
        .map_err(|_| Error::Encrypt)?;
    Ok(key)
}

fn recovery_checksum(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"cozy-project-recovery-v1\0");
    hash.update(bytes);
    hex::encode(&hash.finalize()[..4])
}

fn format_recovery(bytes: &[u8]) -> String {
    let encoded = Zeroizing::new(hex::encode(bytes));
    let mut text = String::with_capacity(87);
    text.push_str(RECOVERY_PREFIX);
    for group in encoded.as_bytes().chunks(8) {
        text.push('-');
        // hex::encode always contains ASCII.
        for byte in group {
            text.push(char::from(*byte));
        }
    }
    text.push('-');
    text.push_str(&recovery_checksum(bytes));
    text
}

/// Viewing timeout configuration; it does not authorize a separate serving session.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionPolicy {
    pub idle_seconds: Option<u64>,
    pub maximum_seconds: Option<u64>,
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            idle_seconds: Some(15 * 60),
            maximum_seconds: None,
        }
    }
}

/// Explicit viewing activity and expiration. Reading status never extends a session.
/// This holds no key and does not claim to lock/unmount a provider; Calcifer owns that.
pub struct SessionTimer {
    policy: SessionPolicy,
    opened: Instant,
    opened_wall: SystemTime,
    activity: Instant,
    activity_wall: SystemTime,
    expired: AtomicBool,
    /// Work in progress holds the session open ([`SessionTimer::hold`]).
    held: bool,
}

impl SessionTimer {
    pub fn new(policy: SessionPolicy) -> Self {
        let now = Instant::now();
        let wall = SystemTime::now();
        Self {
            policy,
            opened: now,
            opened_wall: wall,
            activity: now,
            activity_wall: wall,
            expired: AtomicBool::new(false),
            held: false,
        }
    }

    /// Work in progress (`working`: a run or render of the project) holds the session open: neither its
    /// idle limit nor its maximum ends it meanwhile, a suspend included. The idle clock starts again when
    /// the last of it ends, and a maximum reached meanwhile ends it then. A backwards clock jump still ends it.
    pub fn hold(&mut self, working: bool) {
        if self.held && !working {
            self.activity = Instant::now();
            self.activity_wall = SystemTime::now();
        }
        self.held = working;
    }

    /// An expired timer cannot be revived by delayed interaction or a heartbeat.
    /// Reauthorization creates a new timer instead.
    pub fn activity(&mut self) -> bool {
        if self.expired() {
            return false;
        }
        self.activity = Instant::now();
        self.activity_wall = SystemTime::now();
        true
    }

    /// Wall time also catches suspend on platforms whose monotonic clock pauses.
    /// A backwards wall-clock jump is treated as expiration, not extra unlocked time.
    pub fn expired(&self) -> bool {
        if self.expired.load(Ordering::Relaxed) {
            return true;
        }
        let wall = SystemTime::now();
        let clocks = wall
            .duration_since(self.opened_wall)
            .ok()
            .zip(wall.duration_since(self.activity_wall).ok());
        let Some((open_wall, activity_wall)) = clocks else {
            self.expired.store(true, Ordering::Relaxed);
            return true;
        };
        let expired = !self.held && self.policy.idle_seconds.is_some_and(|seconds| {
            let limit = Duration::from_secs(seconds);
            self.activity.elapsed() >= limit || activity_wall >= limit
        }) || !self.held && self.policy.maximum_seconds.is_some_and(|seconds| {
            let limit = Duration::from_secs(seconds);
            self.opened.elapsed() >= limit || open_wall >= limit
        });
        if expired {
            self.expired.store(true, Ordering::Relaxed);
        }
        expired
    }

    /// When [`SessionTimer::expired`] turns true if nothing moves it: the earlier of each limit's
    /// wall and monotonic ends. None while work holds it, or with no limit.
    pub fn deadline(&self) -> Option<SystemTime> {
        if self.held {
            return None;
        }
        let (wall, now) = (SystemTime::now(), Instant::now());
        let end = |since: Instant, since_wall: SystemTime, seconds: u64| {
            let limit = Duration::from_secs(seconds);
            let monotonic = wall.checked_add((since + limit).saturating_duration_since(now));
            since_wall.checked_add(limit).into_iter().chain(monotonic).min()
        };
        let idle = self.policy.idle_seconds.and_then(|seconds| end(self.activity, self.activity_wall, seconds));
        let maximum = self.policy.maximum_seconds.and_then(|seconds| end(self.opened, self.opened_wall, seconds));
        idle.into_iter().chain(maximum).min()
    }

    pub fn policy(&self) -> SessionPolicy {
        self.policy
    }

    /// Whether work in progress holds the session open.
    pub fn held(&self) -> bool {
        self.held
    }
}
