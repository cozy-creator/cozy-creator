//! A machine's workload identity at a Hub (cl-675): AuthKit's jwt-bearer grant (RFC 7523) with a
//! capability the owner's device key signed for the machine's key, and DPoP (RFC 9449) binding
//! every token to that key. The key is made in memory and never written.
use aws_lc_rs::{
    digest::{SHA256, digest},
    rand::{SecureRandom, SystemRandom},
    signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{
    Algorithm, EncodingKey, Header,
    jwk::{CommonParameters, Jwk, ThumbprintHash},
};
use reqwest::{
    Url,
    header::{AGE, DATE, HeaderMap, HeaderName},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{Client, Error, types::WorkloadToken};

/// The OAuth client every machine redeems capabilities as.
pub(crate) const CLIENT: &str = "tensord";
const JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// A P-256 key: what a machine's capabilities bind (`cnf.jkt`) and its tokens prove.
pub struct WorkloadKey {
    key: EncodingKey,
    jwk: Jwk,
    thumbprint: String,
}

impl WorkloadKey {
    /// A new key, in memory only.
    pub fn generate() -> Self {
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
            .expect("a P-256 key");
        let key = EncodingKey::from_ec_der(pkcs8.as_ref());
        let mut jwk = Jwk::from_encoding_key(&key, Algorithm::ES256).expect("a P-256 key's JWK");
        // kty, crv, x and y only, as a proof's header carries it.
        jwk.common = CommonParameters::default();
        let thumbprint = jwk.thumbprint(ThumbprintHash::SHA256).expect("a P-256 JWK's thumbprint");
        Self { key, jwk, thumbprint }
    }

    /// The RFC 7638 thumbprint, unpadded base64url.
    pub fn thumbprint(&self) -> &str {
        &self.thumbprint
    }

    fn sign(&self, typ: &str, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some(typ.into());
        header.jwk = Some(self.jwk.clone());
        jsonwebtoken::encode(&header, claims, &self.key).expect("ES256 signs")
    }

    /// A proof of this key for `method` on `url` (its query and fragment dropped), naming `token`
    /// when it goes with one.
    pub fn proof(&self, method: &str, url: &str, token: Option<&str>) -> String {
        let htu = url.split(['?', '#']).next().unwrap_or(url);
        let mut claims = json!({"htm": method, "htu": htu, "iat": hub_now(url), "jti": jti()});
        if let Some(token) = token {
            claims["ath"] = URL_SAFE_NO_PAD.encode(digest(&SHA256, token.as_bytes())).into();
        }
        self.sign("dpop+jwt", &claims)
    }

    /// The assertion that presents `capability` at `token_endpoint`, signed by this key.
    fn assertion(&self, token_endpoint: &str, capability: &str) -> String {
        let now = hub_now(token_endpoint);
        let claims = json!({
            "iss": CLIENT, "sub": self.thumbprint, "aud": token_endpoint,
            "iat": now, "exp": now + 60, "jti": jti(), "capability": capability,
        });
        self.sign("JWT", &claims)
    }
}

impl Client {
    /// Where this Hub redeems capabilities: the token endpoint of the authorization server its
    /// protected-resource metadata names (RFC 9728, then AuthKit's metadata beneath the issuer).
    pub async fn token_endpoint(&self) -> Result<String, Error> {
        let resource = self.get_protected_resource_metadata().await?;
        let issuer = resource
            .authorization_servers
            .first()
            .ok_or_else(|| Error::Answer("the Hub names no authorization server".into()))?;
        let url = format!("{}/.well-known/oauth-authorization-server", issuer.trim_end_matches('/'));
        let response = self.http().get(url).send().await.map_err(|e| Error::unanswered(&e))?;
        observe(response.url(), response.headers());
        if !response.status().is_success() {
            return Err(Error::refused(response).await);
        }
        let metadata: Value = response.json().await.map_err(|e| Error::Answer(e.to_string()))?;
        metadata["token_endpoint"]
            .as_str()
            .filter(|endpoint| !endpoint.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| Error::Answer("the authorization server names no token endpoint".into()))
    }

    /// `capability` redeemed at `token_endpoint` for a token bound to `key`. A proof refused on a
    /// clock its answer corrected is made again, once, in the Hub's time.
    pub async fn redeem(&self, key: &WorkloadKey, token_endpoint: &str, capability: &str) -> Result<WorkloadToken, Error> {
        let mut again = true;
        loop {
            let signed_at = hub_now(token_endpoint);
            let form = [
                ("grant_type", JWT_BEARER),
                ("client_id", CLIENT),
                ("assertion", &key.assertion(token_endpoint, capability)),
            ];
            let request = self
                .http()
                .post(token_endpoint)
                .header("dpop", key.proof("POST", token_endpoint, None))
                .form(&form);
            let response = request.send().await.map_err(|e| Error::unanswered(&e))?;
            observe(response.url(), response.headers());
            let status = response.status().as_u16();
            let body: Value = response.json().await.map_err(|e| Error::Answer(e.to_string()))?;
            if status == 200 {
                return serde_json::from_value(body).map_err(|e| Error::Answer(e.to_string()));
            }
            let text = |name: &str| body[name].as_str().unwrap_or_default().to_owned();
            let (error, reason) = (text("error"), text("reason"));
            if std::mem::take(&mut again) && error == "invalid_dpop_proof" && hub_now(token_endpoint).abs_diff(signed_at) > SKEWED_S as u64 {
                continue;
            }
            return Err(Error::Hub {
                status,
                code: if reason.is_empty() { error } else { reason },
                message: text("error_description"),
                remedy: String::new(),
                retry_after_s: None,
                metadata: None,
            });
        }
    }
}

/// How far each Hub's clock reads ahead of this host's, in seconds, by origin. AuthKit takes a
/// proof only within 60 s of its own clock, so a workload signs in the Hub's time: a rental host's
/// clock ran 11 minutes fast, and every redemption there was refused.
static AHEAD: Mutex<BTreeMap<String, i64>> = Mutex::new(BTreeMap::new());

/// A Hub this far off is named in the log.
const SKEWED_S: i64 = 30;

/// Reads the clock of the Hub that answered `url` from the answer's `Date`, plus its `Age` when a
/// cache kept it.
pub(crate) fn observe(url: &Url, headers: &HeaderMap) {
    let text = |name: HeaderName| headers.get(name)?.to_str().ok();
    let Some(date) = text(DATE).and_then(|date| httpdate::parse_http_date(date).ok()) else {
        return;
    };
    let age = text(AGE).and_then(|age| age.trim().parse::<i64>().ok()).unwrap_or(0);
    let hub = date.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64) + age;
    let ahead = hub - local_now() as i64;
    let origin = url.origin().ascii_serialization();
    let before = AHEAD.lock().unwrap().insert(origin.clone(), ahead);
    if ahead.abs() > SKEWED_S && before.is_none_or(|before| before.abs() <= SKEWED_S) {
        let (seconds, way) = (ahead.abs(), if ahead > 0 { "behind" } else { "ahead of" });
        tracing::warn!(%origin, ahead_s = ahead, "this host's clock is {seconds} s {way} the Hub's: its proofs there are signed in the Hub's time");
    }
}

/// Now on the clock of the Hub that serves `url`; this host's until that Hub has answered.
fn hub_now(url: &str) -> u64 {
    let origin = Url::parse(url).map(|url| url.origin().ascii_serialization()).unwrap_or_default();
    let ahead = AHEAD.lock().unwrap().get(&origin).copied().unwrap_or(0);
    local_now().saturating_add_signed(ahead)
}

fn local_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// 128 random bits, unpadded base64url.
pub(crate) fn jti() -> String {
    let mut bytes = [0u8; 16];
    SystemRandom::new().fill(&mut bytes).expect("random bytes");
    URL_SAFE_NO_PAD.encode(bytes)
}
