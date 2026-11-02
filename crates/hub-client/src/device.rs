//! A user's device key (Ed25519, AuthKit device keys): enrollment by emailed code, and silent
//! login to a short Bearer.
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer as _, SigningKey};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use crate::types::AuthResult;
use crate::{Client, Error};
use platform::paths::Home;
use protocol::{config::Origin, error::Error as Refusal};

const ENROLL_DOMAIN: &str = "authkit.device-key-enrollment/1";
const LOGIN_DOMAIN: &str = "authkit.device-key-login/1";
/// A capability's JWS `typ` (AuthKit's `devicekey.CapabilityType`).
const CAPABILITY_TYPE: &str = "authkit-capability+jwt";
/// A Bearer this close to its expiry is minted again rather than sent.
const MARGIN: u64 = 30;

/// A device key and the account it signs in, as kept in its file (mode 0600).
pub struct DeviceKey {
    pub hub: String,
    pub email: String,
    pub id: String,
    key: SigningKey,
}

#[derive(Deserialize, Serialize)]
struct Kept {
    version: u32,
    hub: String,
    email: String,
    device_key_id: String,
    /// The Ed25519 seed, base64url.
    private_key: String,
}

impl DeviceKey {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let raw = std::fs::read(path)
            .map_err(|e| Error::Credential(format!("{}: {e}", path.display())))?;
        let kept: Kept = serde_json::from_slice(&raw)
            .map_err(|e| Error::Credential(format!("{}: {e}", path.display())))?;
        let seed: [u8; 32] = unb64(&kept.private_key)
            .and_then(|seed| seed.try_into().ok())
            .filter(|_| kept.version == 1 && !kept.device_key_id.is_empty())
            .ok_or_else(|| Error::Credential(format!("{}: not a device key", path.display())))?;
        Ok(Self {
            hub: kept.hub,
            email: kept.email,
            id: kept.device_key_id,
            key: SigningKey::from_bytes(&seed),
        })
    }

    /// Writes the key to `path`, owner-only, replacing any key there.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        let kept = Kept {
            version: 1,
            hub: self.hub.clone(),
            email: self.email.clone(),
            device_key_id: self.id.clone(),
            private_key: b64(self.key.to_bytes()),
        };
        write_private(path, &serde_json::to_vec(&kept).expect("a key serializes"))
    }

    /// The public key, unpadded base64url: what a rental's `creator_public_key` names.
    pub fn public_key(&self) -> String {
        b64(self.key.verifying_key().as_bytes())
    }

    /// `claims` as a capability this key signs for its user (AuthKit's `devicekey.SignCapability`),
    /// with a fresh `jti`: what a workload may do for them, redeemed with the jwt-bearer grant.
    pub fn capability(&self, claims: &serde_json::Value) -> String {
        let mut claims = claims.clone();
        claims["jti"] = crate::workload::jti().into();
        let pair = aws_lc_rs::signature::Ed25519KeyPair::from_seed_unchecked(&self.key.to_bytes())
            .expect("an Ed25519 seed");
        let pkcs8 = pair.to_pkcs8().expect("an Ed25519 key's PKCS#8");
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::EdDSA);
        header.typ = Some(CAPABILITY_TYPE.into());
        header.kid = Some(self.id.clone());
        jsonwebtoken::encode(&header, &claims, &jsonwebtoken::EncodingKey::from_ed_der(pkcs8.as_ref()))
            .expect("EdDSA signs")
    }

    fn sign_challenge(key: &SigningKey, domain: &str, challenge: &str) -> Result<String, Error> {
        let challenge = unb64(challenge)
            .filter(|bytes| bytes.len() == 32)
            .ok_or_else(|| Error::Answer("a malformed device-key challenge".into()))?;
        let message = [domain.as_bytes(), &[0], &challenge].concat();
        Ok(b64(key.sign(&message).to_bytes()))
    }
}

/// A short user Bearer.
#[derive(Clone, Deserialize, Serialize)]
pub struct Bearer {
    pub token: String,
    /// Unix seconds.
    pub expires: u64,
    pub device_key_id: String,
}

impl Bearer {
    fn from_answer(answer: &AuthResult, started: u64) -> Result<Self, Error> {
        let set = answer
            .token_set
            .as_ref()
            .ok_or_else(|| Error::Answer("no access token".into()))?;
        if !set.token_type.eq_ignore_ascii_case("bearer")
            || set.access_token.is_empty()
            || set.expires_in <= 0
        {
            return Err(Error::Answer("an unusable access token".into()));
        }
        let device_key_id = answer
            .device_key
            .as_ref()
            .map(|key| key.id.clone())
            .unwrap_or_default();
        Ok(Self {
            token: set.access_token.clone(),
            expires: started + set.expires_in as u64,
            device_key_id,
        })
    }

    fn live(&self) -> bool {
        self.expires > now() + MARGIN
    }

    /// When the session mints the next one (Unix seconds).
    pub fn refresh_at(&self) -> u64 {
        self.expires.saturating_sub(MARGIN)
    }
}

/// One user signed in by a device key: a Bearer kept in memory and, if given a path, beside
/// the key for the next process, minted again when stale or refused.
pub struct UserSession {
    key: DeviceKey,
    cache: Option<PathBuf>,
    bearer: Mutex<Option<Bearer>>,
}

impl UserSession {
    pub fn new(key: DeviceKey, cache: Option<PathBuf>) -> Self {
        Self {
            key,
            cache,
            bearer: Mutex::new(None),
        }
    }

    /// This computer's session at `hub`: its device key and the Bearer kept beside it. Refused
    /// when it holds no key there, `login` (asked only then) signing it in.
    pub fn open(
        hub: &Origin,
        home: &Home,
        login: impl FnOnce() -> String,
    ) -> Result<Self, Refusal> {
        let path = hub.device_key(home);
        if !path.exists() {
            let message = format!("this computer is not signed in to {hub}");
            return Err(Refusal::credential("not_logged_in", message).next(login()));
        }
        match DeviceKey::load(&path) {
            Ok(key) => Ok(Self::new(key, Some(hub.bearer(home)))),
            Err(e) => Err(e.refusal(login)),
        }
    }

    pub fn key(&self) -> &DeviceKey {
        &self.key
    }

    /// A live Bearer: held, kept, or from a fresh login at `hub`.
    pub async fn bearer(&self, hub: &Client) -> Result<Bearer, Error> {
        let mut held = self.bearer.lock().await;
        let ours = |b: &Bearer| b.live() && b.device_key_id == self.key.id;
        if let Some(bearer) = held.as_ref().filter(|b| ours(b)) {
            return Ok(bearer.clone());
        }
        let kept = self
            .cache
            .as_deref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|raw| serde_json::from_slice::<Bearer>(&raw).ok())
            .filter(ours);
        let bearer = match kept {
            Some(bearer) => bearer,
            None => {
                let bearer = self.login(hub).await?;
                if let Some(path) = &self.cache
                    && let Err(error) = write_private(
                        path,
                        &serde_json::to_vec(&bearer).expect("a bearer serializes"),
                    )
                {
                    tracing::warn!(%error, "keeping the session");
                }
                bearer
            }
        };
        *held = Some(bearer.clone());
        Ok(bearer)
    }

    /// The Hub refused `token`: the next request logs in again.
    pub async fn refused(&self, token: &str) {
        let mut held = self.bearer.lock().await;
        if held.as_ref().is_some_and(|b| b.token == token) {
            *held = None;
            if let Some(path) = &self.cache {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// A login at `hub`. Its refusal is the key's (revoked, unknown): final.
    async fn login(&self, hub: &Client) -> Result<Bearer, Error> {
        let refused = |e: Error| match e.retry() {
            true => e,
            false => Error::Credential(format!("sign-in refused: {e}")),
        };
        let begun = hub
            .begin_device_key_login(&self.key.id)
            .await
            .map_err(refused)?;
        let signature = DeviceKey::sign_challenge(&self.key.key, LOGIN_DOMAIN, &begun.challenge)?;
        let started = now();
        let answer = hub
            .finish_device_key_login(&begun.challenge_id, &signature)
            .await
            .map_err(refused)?;
        Bearer::from_answer(&answer, started)
    }
}

/// A device key's enrollment, waiting for the code AuthKit emailed.
pub struct Enrollment {
    id: String,
    challenge: String,
    key: SigningKey,
    email: String,
    hub: String,
}

impl Enrollment {
    /// Asks `hub` to email `email` a code that enrolls a new device key, or proves the mailbox
    /// for `existing` (to revoke the account's other keys).
    pub async fn begin(
        hub: &Client,
        email: &str,
        label: &str,
        existing: Option<&DeviceKey>,
    ) -> Result<Self, Error> {
        let key = match existing {
            Some(existing) => existing.key.clone(),
            None => SigningKey::from_bytes(&random()),
        };
        let public_key = b64(key.verifying_key().as_bytes());
        let begun = hub
            .begin_device_key_enrollment(email, &public_key, label)
            .await?;
        Ok(Self {
            id: begun.enrollment_id,
            challenge: begun.challenge,
            key,
            email: email.to_string(),
            hub: hub.origin().to_string(),
        })
    }

    /// Completes enrollment with the emailed `code`: the key, and its first Bearer.
    pub async fn finish(self, hub: &Client, code: &str) -> Result<(DeviceKey, Bearer), Error> {
        let signature = DeviceKey::sign_challenge(&self.key, ENROLL_DOMAIN, &self.challenge)?;
        let started = now();
        let answer = hub
            .finish_device_key_enrollment(&self.id, code.trim(), &signature)
            .await?;
        let bearer = Bearer::from_answer(&answer, started)?;
        let key = DeviceKey {
            hub: self.hub,
            email: self.email,
            id: bearer.device_key_id.clone(),
            key: self.key,
        };
        Ok((key, bearer))
    }
}

/// Writes `bytes` to `path` owner-only, through a rename so a reader never sees half.
fn b64(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn unb64(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text).ok()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn random<const N: usize>() -> [u8; N] {
    let mut raw = [0u8; N];
    SystemRandom::new()
        .fill(&mut raw)
        .expect("the system random source");
    raw
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let fail = |e: std::io::Error| Error::Credential(format!("{}: {e}", path.display()));
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(fail)?;
    let staged = dir.join(format!(".{}.tmp", b64(random::<24>())));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options.open(&staged).and_then(|mut file| {
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()
    });
    let renamed = written.and_then(|()| std::fs::rename(&staged, path));
    if renamed.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    renamed.map_err(fail)
}
