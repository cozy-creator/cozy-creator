//! Authorized signing keys and the lifetimes of capability-authenticated streams.
use ed25519_dalek::VerifyingKey;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, RwLock},
    task::{Context, Poll},
    time::Duration,
};
use protocol::error::Error;
use tokio_stream::Stream;

#[derive(Clone)]
pub struct Authority {
    pub worker_id: String,
    pub boot_id: String,
    pub keys: Keys,
}

/// The signing keys this machine admits, shared by every clone of its Authority: the granted
/// ones and its home's. A rental's granted set is the Hub's: the boot keys hold until the Hub
/// answers, and each answer holds until the next. Only the Hub's answer changes it: a Hub this
/// machine cannot reach is weather and changes nothing, a denial revokes at once. The home's are
/// this computer's own controllers (its Calcifer's owner key, `cozy machine authorize`'s keys), on
/// a laptop and on a pod alike, read again when a signer is unknown. Losing authority never
/// cancels accepted work.
#[derive(Clone)]
pub struct Keys {
    granted: Arc<RwLock<Vec<VerifyingKey>>>,
    home: Arc<RwLock<Vec<VerifyingKey>>>,
    changes: Arc<tokio::sync::watch::Sender<u64>>,
    source: Option<Arc<dyn Fn() -> std::io::Result<Vec<VerifyingKey>> + Send + Sync>>,
}
impl Keys {
    pub fn fixed(keys: Vec<VerifyingKey>) -> Self {
        Self {
            granted: Arc::new(RwLock::new(keys)),
            home: Arc::default(),
            changes: Arc::new(tokio::sync::watch::channel(0).0),
            source: None,
        }
    }
    /// `granted`, and the home's keys `source` reads, read again whenever a signer is unknown
    /// (`cozy machine authorize` takes effect at the next call).
    pub fn read(
        granted: Vec<VerifyingKey>,
        source: impl Fn() -> std::io::Result<Vec<VerifyingKey>> + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let home = source()?;
        let keys = Self { source: Some(Arc::new(source)), ..Self::fixed(granted) };
        *keys.home.write().unwrap() = home;
        Ok(keys)
    }
    /// `check` over the admitted keys; when it refuses and the home has a source, once more with
    /// the home's current keys. A concurrent check may have read them meanwhile (a controller's
    /// first calls, as its key is written): the set read decides, not the one refused.
    pub fn check<T, E>(&self, check: impl Fn(&[VerifyingKey]) -> Result<T, E>) -> Result<T, E> {
        let before = self.home.read().unwrap().clone();
        let first = check(&self.admitted());
        let (Err(_), Some(source)) = (&first, &self.source) else { return first };
        match source() {
            Ok(home) if home != before => {
                if home != *self.home.read().unwrap() {
                    *self.home.write().unwrap() = home;
                    self.changed();
                }
                check(&self.admitted())
            }
            Ok(_) => first,
            Err(error) => {
                tracing::warn!("authorized keys unreadable: {error}");
                first
            }
        }
    }
    fn changed(&self) {
        self.changes.send_modify(|generation| *generation += 1);
    }
    /// The Hub's current answer: these keys.
    pub fn renew(&self, keys: Vec<VerifyingKey>) {
        *self.granted.write().unwrap() = keys;
        self.changed();
    }
    /// The Hub denied this rental: its keys go; the home's stay.
    pub fn revoke(&self) {
        self.renew(vec![]);
    }
    /// The keys that may authorize a new control now.
    pub fn admitted(&self) -> Vec<VerifyingKey> {
        [self.granted.read().unwrap().as_slice(), self.home.read().unwrap().as_slice()].concat()
    }
    /// Resolves once `key` left the set. Ends only that key's open transports; accepted work
    /// is never cancelled.
    pub async fn revoked(&self, key: [u8; 32]) {
        let mut changes = self.changes.subscribe();
        while self.admitted().iter().any(|k| k.to_bytes() == key) {
            if changes.changed().await.is_err() {
                return std::future::pending().await;
            }
        }
    }
}
impl From<Vec<VerifyingKey>> for Keys {
    fn from(keys: Vec<VerifyingKey>) -> Self {
        Self::fixed(keys)
    }
}

/// Why an open transport lost its authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lapse {
    /// The key that opened it left the admitted set.
    Revoked,
    /// The expiry its signer chose passed (a play link's `--expires`).
    Expired,
}
impl Lapse {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Lapse::Revoked => "revoked",
            Lapse::Expired => "expired",
        }
    }
    pub(crate) fn message(self) -> &'static str {
        match self {
            Lapse::Revoked => "the key that opened this stream no longer authorizes it",
            Lapse::Expired => "the capability that opened this stream has expired",
        }
    }
}
impl From<Lapse> for Error {
    fn from(lapse: Lapse) -> Self {
        Error::credential(format!("capability_{}", lapse.code()), lapse.message())
    }
}

type Transport<T> = Pin<Box<dyn Stream<Item = Result<T, Error>> + Send + 'static>>;

/// One open transport's authority. A machine-scope stream ends only when its key stops
/// authorizing: its capability's expiry was checked when it opened, and a long run, upload or
/// read outlives it. A play link's stream, and a media session, also end at the signed expiry
/// their signer chose. Losing authority detaches the transport; it never controls accepted work.
#[derive(Clone)]
pub(crate) struct StreamAuthority {
    keys: Keys,
    key: [u8; 32],
    expires: Option<i64>,
}
impl StreamAuthority {
    pub(crate) fn new(keys: Keys, actor: VerifiedActor, expires: Option<i64>) -> Self {
        Self { keys, key: actor.public_key, expires }
    }
    pub(crate) fn check(&self) -> Result<(), Lapse> {
        if self.expires.is_some_and(|expires| unix_now() >= expires) {
            return Err(Lapse::Expired);
        }
        match self.keys.admitted().iter().any(|key| key.to_bytes() == self.key) {
            true => Ok(()),
            false => Err(Lapse::Revoked),
        }
    }
    pub(crate) async fn ended(self) -> Lapse {
        let expiry = async {
            let Some(expires) = self.expires else {
                return std::future::pending().await;
            };
            // A wall-clock expiry, re-read at most daily so a distant one stays in range.
            while unix_now() < expires {
                let left = (expires - unix_now()).clamp(1, 86_400) as u64;
                tokio::time::sleep(Duration::from_secs(left)).await;
            }
        };
        tokio::select! {
            _ = self.keys.revoked(self.key) => Lapse::Revoked,
            _ = expiry => Lapse::Expired,
        }
    }
    /// Ends `stream` with a credential error once authority lapses, dropping it (and the
    /// observer and files behind it) even while its peer applies backpressure.
    pub(crate) fn wrap<T: Send + 'static>(&self, stream: Transport<T>) -> Transport<T> {
        Box::pin(Authorized { stream: Some(stream), ended: Box::pin(self.clone().ended()) })
    }
}
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}
struct Authorized<T> {
    stream: Option<Transport<T>>,
    ended: Pin<Box<dyn Future<Output = Lapse> + Send>>,
}
impl<T> Stream for Authorized<T> {
    type Item = Result<T, Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.stream.is_none() {
            return Poll::Ready(None);
        }
        if let Poll::Ready(lapse) = self.ended.as_mut().poll(cx) {
            self.stream = None;
            return Poll::Ready(Some(Err(lapse.into())));
        }
        self.stream.as_mut().unwrap().as_mut().poll_next(cx)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedActor {
    pub public_key: [u8; 32],
}
