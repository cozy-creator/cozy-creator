//! Local, project-scoped key custody for an optional encrypted project.
//!
//! The data filesystem belongs to the mount provider. This crate stores only a random
//! printable filesystem secret, independently wrapped by a human passphrase and by a
//! high-entropy recovery secret using the standard age file format. Copying the encrypted
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

use age::secrecy::ExposeSecret;
pub use age::secrecy::SecretString;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Filename kept beside an encrypted project's backing directory, not inside its mount.
pub const KEYBAG_FILE: &str = "project.vault.keys.json";
/// Portable keybag format version.
pub const FORMAT_VERSION: u32 = 1;
/// The named cipher/encoding suite for the small key envelopes, not the filesystem data.
pub const ENVELOPE_SUITE: &str = "age-v1-scrypt";
/// Maximum size of a portable keybag accepted from disk.
pub const MAX_KEYBAG_BYTES: u64 = 64 * 1024;
/// Maximum plaintext size of one private journal field. Media belongs in the data vault.
pub const MAX_JOURNAL_BYTES: usize = 16 * 1024 * 1024;

// age's scrypt uses r=8, p=1. Fix and cap log2(N) at 18 (256 MiB), so an
// untrusted envelope cannot request unbounded work. It is not a user tunable.
const SCRYPT_LOG_N: u8 = 18;
const MAX_SECRET_PAYLOAD: u64 = 2048;
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
    #[error("private project journal context or payload is invalid")]
    Journal,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Public keybag information suitable for a local status response.
#[derive(Clone, Debug, Serialize)]
pub struct KeybagInfo {
    pub format_version: u32,
    pub project_id: String,
    pub generation: u64,
    pub key_id: String,
    pub journal_recipient: String,
}

/// A portable, encrypted project keybag. All secret-bearing fields are age ciphertext.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keybag {
    format_version: u32,
    envelope_suite: String,
    project_id: String,
    generation: u64,
    key_id: String,
    journal_recipient: String,
    passphrase_envelope: String,
    recovery_envelope: String,
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
    journal_identity: SecretString,
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
            journal_identity: self.journal_identity.clone(),
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

    /// Open an encrypted local run-journal field only while its project is unlocked.
    /// The context must identify the run and field, preventing ciphertext substitution.
    pub fn open(&self, context: &str, ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        validate_context(context)?;
        if ciphertext.len() > MAX_JOURNAL_BYTES + 64 * 1024 {
            return Err(Error::Journal);
        }
        let identity: age::x25519::Identity = self
            .journal_identity
            .expose_secret()
            .parse()
            .map_err(|_| Error::Journal)?;
        let decryptor = age::Decryptor::new(ciphertext).map_err(|_| Error::Journal)?;
        let stream = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|_| Error::Journal)?;
        let mut bytes = Zeroizing::new(Vec::new());
        stream
            .take((MAX_JOURNAL_BYTES + 8193) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Journal)?;
        if bytes.len() > MAX_JOURNAL_BYTES + 8192 || bytes.len() < 4 {
            return Err(Error::Journal);
        }
        let header_size =
            u32::from_be_bytes(bytes[..4].try_into().map_err(|_| Error::Journal)?) as usize;
        if header_size > 8192 || 4 + header_size > bytes.len() {
            return Err(Error::Journal);
        }
        let header: JournalContext =
            serde_json::from_slice(&bytes[4..4 + header_size]).map_err(|_| Error::Journal)?;
        if header.format_version != FORMAT_VERSION
            || header.project_id != self.project_id
            || header.generation != self.generation
            || header.context != context
            || bytes.len() - 4 - header_size > MAX_JOURNAL_BYTES
        {
            return Err(Error::Journal);
        }
        Ok(Zeroizing::new(bytes.split_off(4 + header_size)))
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
    journal_identity: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalContext {
    format_version: u32,
    project_id: String,
    generation: u64,
    context: String,
}

impl Keybag {
    pub fn create(project_id: &str, passphrase: SecretString) -> Result<CreatedKeybag> {
        Self::create_generation(project_id, 1, passphrase)
    }

    fn create_generation(
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
        let journal_identity = age::x25519::Identity::generate();
        let journal_recipient = journal_identity.to_public().to_string();
        let secret = ProjectSecret {
            project_id: project_id.to_owned(),
            generation,
            key_id: key_id.clone(),
            value,
            journal_identity: journal_identity.to_string(),
        };
        let recovery_secret = RecoverySecret::generate()?;
        let payload = encode_secret(&secret)?;
        let keybag = Self {
            format_version: FORMAT_VERSION,
            envelope_suite: ENVELOPE_SUITE.to_owned(),
            project_id: project_id.to_owned(),
            generation,
            key_id,
            journal_recipient,
            passphrase_envelope: wrap(&payload, passphrase)?,
            recovery_envelope: wrap(
                &payload,
                SecretString::from(recovery_secret.expose().to_owned()),
            )?,
        };
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
            journal_recipient: self.journal_recipient.clone(),
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
        let keybag: Self =
            serde_json::from_slice(&bytes).map_err(|_| Error::Format("invalid keybag JSON"))?;
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
                unwrap(&self.passphrase_envelope, passphrase)?
            }
            Credential::Recovery(recovery) => {
                let recovery = RecoverySecret::parse(recovery)?;
                unwrap(
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
        let journal_identity: age::x25519::Identity = decoded
            .journal_identity
            .parse()
            .map_err(|_| Error::Unlock)?;
        if journal_identity.to_public().to_string() != self.journal_recipient {
            return Err(Error::Unlock);
        }
        Ok(ProjectSecret {
            project_id: self.project_id.clone(),
            generation: self.generation,
            key_id: self.key_id.clone(),
            value: SecretString::from(std::mem::take(&mut decoded.secret)),
            journal_identity: SecretString::from(std::mem::take(&mut decoded.journal_identity)),
        })
    }

    /// Seal a private local journal field using only the public project recipient. This
    /// supports terminal remote output/events arriving while the viewing session is locked.
    /// Callers pin the verified keybag/recipient when admitting the run rather than trusting
    /// a mutable recipient supplied by an untrusted peer.
    pub fn seal(&self, context: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
        self.validate()?;
        validate_context(context)?;
        if plaintext.len() > MAX_JOURNAL_BYTES {
            return Err(Error::Journal);
        }
        let header = JournalContext {
            format_version: FORMAT_VERSION,
            project_id: self.project_id.clone(),
            generation: self.generation,
            context: context.to_owned(),
        };
        let encoded = serde_json::to_vec(&header).map_err(|_| Error::Journal)?;
        if encoded.len() > 8192 {
            return Err(Error::Journal);
        }
        let mut bytes = Zeroizing::new(Vec::with_capacity(4 + encoded.len() + plaintext.len()));
        bytes.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&encoded);
        bytes.extend_from_slice(plaintext);
        let recipient: age::x25519::Recipient =
            self.journal_recipient.parse().map_err(|_| Error::Journal)?;
        age::encrypt(&recipient, &bytes).map_err(|_| Error::Encrypt)
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
        let replacement = wrap(&payload, new_passphrase)?;
        self.passphrase_envelope = replacement;
        Ok(())
    }

    /// Replace the saved recovery credential while preserving the project secret.
    /// An older copied keybag and its older code remain usable for that old generation.
    pub fn replace_recovery(&mut self, secret: &ProjectSecret) -> Result<RecoverySecret> {
        self.verify_secret(secret)?;
        let recovery = RecoverySecret::generate()?;
        let payload = encode_secret(secret)?;
        let replacement = wrap(&payload, SecretString::from(recovery.expose().to_owned()))?;
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
        for envelope in [&self.passphrase_envelope, &self.recovery_envelope] {
            if envelope.is_empty() || envelope.len() > 16 * 1024 {
                return Err(Error::Format("invalid key envelope length"));
            }
            let decoded = STANDARD
                .decode(envelope)
                .map_err(|_| Error::Format("invalid key envelope encoding"))?;
            let decryptor = age::Decryptor::new(decoded.as_slice())
                .map_err(|_| Error::Format("invalid age envelope"))?;
            if !decryptor.is_scrypt() {
                return Err(Error::Format("key envelope is not passphrase-protected"));
            }
        }
        self.journal_recipient
            .parse::<age::x25519::Recipient>()
            .map_err(|_| Error::Format("invalid journal recipient"))?;
        Ok(())
    }

    fn verify_secret(&self, secret: &ProjectSecret) -> Result<()> {
        if secret.project_id != self.project_id
            || secret.generation != self.generation
            || secret.key_id != self.key_id
            || secret
                .journal_identity
                .expose_secret()
                .parse::<age::x25519::Identity>()
                .map_err(|_| Error::WrongProject)?
                .to_public()
                .to_string()
                != self.journal_recipient
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
    if value.expose_secret().is_empty() {
        Err(Error::EmptyPassphrase)
    } else {
        Ok(())
    }
}

fn key_fingerprint(secret: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"cozy-project-vault-key-v1\0");
    hash.update(secret);
    hex::encode(hash.finalize())
}

fn encode_secret(secret: &ProjectSecret) -> Result<Zeroizing<Vec<u8>>> {
    let payload = SecretPayload {
        format_version: FORMAT_VERSION,
        project_id: secret.project_id.clone(),
        generation: secret.generation,
        secret: secret.value.expose_secret().to_owned(),
        journal_identity: secret.journal_identity.expose_secret().to_owned(),
    };
    serde_json::to_vec(&payload)
        .map(Zeroizing::new)
        .map_err(|_| Error::Format("cannot serialize secret payload"))
}

fn wrap(payload: &[u8], passphrase: SecretString) -> Result<String> {
    let mut recipient = age::scrypt::Recipient::new(passphrase);
    recipient.set_work_factor(SCRYPT_LOG_N);
    let ciphertext = age::encrypt(&recipient, payload).map_err(|_| Error::Encrypt)?;
    Ok(STANDARD.encode(ciphertext))
}

fn validate_context(context: &str) -> Result<()> {
    if context.is_empty() || context.len() > 4096 || context.chars().any(char::is_control) {
        Err(Error::Journal)
    } else {
        Ok(())
    }
}

fn unwrap(envelope: &str, passphrase: SecretString) -> Result<Zeroizing<Vec<u8>>> {
    let bytes = STANDARD.decode(envelope).map_err(|_| Error::Unlock)?;
    let decryptor = age::Decryptor::new(bytes.as_slice()).map_err(|_| Error::Unlock)?;
    let mut identity = age::scrypt::Identity::new(passphrase);
    identity.set_max_work_factor(SCRYPT_LOG_N);
    let stream = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|_| Error::Unlock)?;
    let mut payload = Zeroizing::new(Vec::new());
    stream
        .take(MAX_SECRET_PAYLOAD + 1)
        .read_to_end(&mut payload)
        .map_err(|_| Error::Unlock)?;
    if payload.len() as u64 > MAX_SECRET_PAYLOAD {
        return Err(Error::Unlock);
    }
    Ok(payload)
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
        }
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
        let expired = self.policy.idle_seconds.is_some_and(|seconds| {
            let limit = Duration::from_secs(seconds);
            self.activity.elapsed() >= limit || activity_wall >= limit
        }) || self.policy.maximum_seconds.is_some_and(|seconds| {
            let limit = Duration::from_secs(seconds);
            self.opened.elapsed() >= limit || open_wall >= limit
        });
        if expired {
            self.expired.store(true, Ordering::Relaxed);
        }
        expired
    }

    pub fn policy(&self) -> SessionPolicy {
        self.policy
    }
}
