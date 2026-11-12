//! The Hub's worker API as this machine (`api::hub`), HTTPS to the granted origin (public roots
//! plus the granted CA): rental authority, the speed report and release. Each request is signed by
//! the machine's leaf key, which the Hub pinned when it reached the machine at its provider's address.
use super::grant::HubGrant;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ed25519_dalek::VerifyingKey;
use ::api::hub::{types, Credential, WorkerSigner};
use std::{
    io,
    sync::atomic::{AtomicI64, Ordering},
    time::Duration,
};

pub(crate) struct Hub {
    grant: HubGrant,
    client: ::api::hub::Client,
    /// When the Hub last answered anything (0: never in this process).
    contact_ms: AtomicI64,
}

#[derive(Debug)]
pub(crate) enum Refusal {
    /// 401 `worker.ended`: the rental behind this pod is over, for good.
    Ended,
    /// Any other 401/403: the Hub denies this attempt's authority now, saying why.
    Denied(String),
    /// 425 `worker.not_ready`: the Hub has not reached this machine yet.
    NotReady,
    /// Unreachable, slow or any other answer: proves nothing either way. With how long the answer
    /// said to wait.
    Transport(String, Option<Duration>),
}

impl Hub {
    pub(crate) fn new(grant: HubGrant, sign: WorkerSigner) -> io::Result<Self> {
        let http = crate::hub::http(grant.ca_der.as_deref()).map_err(|e| io::Error::other(e.0))?;
        let credential = Credential::Worker { id: grant.worker_id.clone(), sign };
        Ok(Self {
            client: ::api::hub::Client::connect(&grant.origin, http, credential),
            grant,
            contact_ms: AtomicI64::new(0),
        })
    }

    /// When the Hub itself last answered this machine, whatever it said; 0: never in this process.
    pub(crate) fn contact_ms(&self) -> i64 {
        self.contact_ms.load(Ordering::Acquire)
    }

    /// An answer, or a refusal the Hub made, is contact. An unreachable Hub is not, nor a proxy's
    /// page in its place (`http_<status>`, no Hub envelope: `no available server` when it is down),
    /// so a pod still ends itself through the outage that page stands for.
    fn heard<T>(&self, answer: Result<T, ::api::hub::Error>) -> Result<T, Refusal> {
        let contact = match &answer {
            Ok(_) => true,
            Err(::api::hub::Error::Hub { code, .. }) => !code.starts_with("http_"),
            Err(_) => false,
        };
        if contact {
            self.contact_ms
                .store(super::lifecycle::now_ms(), Ordering::Release);
        }
        answer.map_err(|error| match error {
            ::api::hub::Error::Hub { status: 401, code, .. } if code == "worker.ended" => Refusal::Ended,
            error @ ::api::hub::Error::Hub { status: 401 | 403, .. } => Refusal::Denied(error.to_string()),
            ::api::hub::Error::Hub { status: 425, .. } => Refusal::NotReady,
            ::api::hub::Error::Call(why) => Refusal::Transport(format!("reach Tensorhub: {why}"), None),
            other => {
                let wait = other.stated_wait();
                Refusal::Transport(other.to_string(), wait)
            }
        })
    }

    /// The rental's current authorized keys and their lease.
    pub(crate) async fn authorized_keys(&self) -> Result<(Vec<VerifyingKey>, Duration), Refusal> {
        let answer = self.client.get_worker_authorized_keys().await;
        let document = self.heard(answer)?;
        let invalid = || Refusal::Transport("rental authority metadata is invalid".into(), None);
        if document.worker_id != self.grant.worker_id
            || !(1..=3600).contains(&document.lease_seconds)
        {
            return Err(invalid());
        }
        // The lease is the only source of whom this machine admits: a key it cannot read is skipped
        // and said, never a reason to admit nobody.
        let keys = (document.authorized_keys.iter())
            .filter_map(|spelled| {
                let key = (URL_SAFE_NO_PAD.decode(spelled).ok())
                    .and_then(|raw| <[u8; 32]>::try_from(raw).ok())
                    .and_then(|raw| VerifyingKey::from_bytes(&raw).ok());
                if key.is_none() {
                    tracing::warn!("the Hub's lease names a key that is not an Ed25519 public key; it is not admitted: {spelled}");
                }
                key
            })
            .collect();
        Ok((keys, Duration::from_secs(document.lease_seconds as u64)))
    }

    /// What this machine measured of its host (`machine::speed`). A Hub from before the route
    /// answers 404, which is no refusal.
    pub(crate) async fn report_speed(
        &self,
        download: Option<types::WorkerDownload>,
        cpu: Option<types::WorkerProbe>,
        gpu_heat: Option<types::WorkerGpuHeat>,
    ) -> Result<(), Refusal> {
        let report = types::WorkerSpeedReport { worker_id: self.grant.worker_id.clone(), download, cpu, gpu_heat };
        match self.client.report_worker_speed(&report).await {
            Err(::api::hub::Error::Hub { status: 404, .. }) => Ok(()),
            answer => self.heard(answer),
        }
    }

    /// Ends this rental's allocation; acceptance is the Hub's 204.
    pub(crate) async fn release(&self) -> Result<(), Refusal> {
        let answer = self.client.release_worker_rental(&self.grant.worker_id).await;
        self.heard(answer)
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ended => write!(f, "the Hub says this rental has ended"),
            Self::Denied(why) => write!(f, "the Hub denied this rental's authority: {why}"),
            Self::NotReady => write!(f, "the Hub says this rental is not ready yet"),
            Self::Transport(detail, _) => write!(f, "{detail}"),
        }
    }
}
