//! What a run's send carries for its Hub: where its machine reads the Hub (the Hub's own word,
//! kept per Hub: in a dev or CI Hub it is not the origin this computer dials) and, signed in,
//! this computer's user Bearer as its TensorFS token, for every model it resolves or reads: the
//! Hub decides by org membership what a commit is to that user. The Bearer is added at the send,
//! never to the stored submission; each one the session mints after goes to every machine sent
//! one, at the session's own refresh point (`PUT /v1/bearer`). Signed out, a run reads public
//! releases at its Hub's origin; a run that publishes needs the sign-in.
use std::{
    sync::Weak,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hub_client::{Bearer, Credential};
use protocol::{
    calcifer::Reason,
    config::Origin,
    error::{Error, Kind},
    ids::RunId,
    machine as v1,
};

use crate::{
    machines::{Machines, internal, lock},
    rentals::{Hubs, hub_refusal, signed_out},
    store::RunRow,
};

/// What a send carries besides the spec the run was submitted with.
#[derive(Default)]
pub(crate) struct Access {
    pub hub: Option<v1::HubAccess>,
    pub providers: Option<v1::ProviderAccess>,
    pub tensorfs: Option<v1::TensorFSOptions>,
    /// The account local code's org-relative references name: this computer's at the run's Hub.
    pub owner: Option<String>,
}

/// Why a run cannot be sent now.
pub(crate) enum Denied {
    /// For good: the run fails with this.
    Refused(Reason),
    /// The Hub did not answer: try again.
    Unreachable(String),
}

/// What a Hub says of this computer's account there, read once per device key: a login as
/// another account (a new key) reads it again.
#[derive(Clone)]
pub(crate) struct Facts {
    key: String,
    account: String,
    /// Where machines read the Hub.
    reads: String,
    /// Both where machines read it and its public origin: the configured Hub's other names.
    names: Vec<Origin>,
    /// The CA a machine trusts for the Hub when a private one signs it (a test Hub).
    ca_der: Vec<u8>,
}

impl Machines {
    /// The Hub access and provider tokens the send of `run` as `spec` carries.
    pub(crate) async fn access(&self, run: &RunRow, spec: &v1::RunSpec) -> Result<Access, Denied> {
        let providers = self.providers.clone();
        let origin = match &run.hub {
            Some(hub) => Some(self.hubs.resolve(hub).await),
            None => None,
        };
        let Some(origin) = origin.filter(|origin| {
            // Signed out, a run reads public releases at its Hub's origin (lib's finish_spec).
            self.hubs.client(origin).is_ok() || !destination(spec).is_empty()
        }) else {
            return Ok(Access {
                providers,
                ..Default::default()
            });
        };
        let facts = self.hubs.facts(&origin).await?;
        // An explicit endpoint's own TensorFS options get none.
        let bearer = match spec.tensorfs.is_none() {
            true => Some(self.hubs.bearer(&origin).await?),
            false => None,
        };
        if let (Some(machine), Some(bearer)) = (&run.machine, &bearer) {
            self.holds(machine, &origin, bearer);
        }
        let tensorfs = bearer.map(|bearer| tensorfs(&facts, &bearer));
        Ok(Access {
            hub: Some(v1::HubAccess {
                origin: facts.reads,
                ca_der: facts.ca_der,
            }),
            providers,
            tensorfs,
            owner: Some(facts.account),
        })
    }

    /// What a send of the stored `run` would carry to its Hub now; none when this computer is
    /// not signed in there and the run is public.
    pub async fn hub_access(&self, run: &RunId) -> Result<Option<v1::HubAccess>, Error> {
        let row = self.store().run(run.as_str()).map_err(internal)?;
        let row =
            row.ok_or_else(|| Error::new(Kind::NotFound, "run_not_found", run.to_string()))?;
        match self.access(&row, &row.spec).await {
            Ok(access) => Ok(access.hub),
            Err(Denied::Refused(reason)) => {
                Err(Error::new(Kind::Conflict, reason.code, reason.message))
            }
            Err(Denied::Unreachable(message)) => Err(Error::new(
                Kind::Unavailable,
                "hub_access_unavailable",
                message,
            )),
        }
    }
}

impl Machines {
    /// `machine` was sent `bearer`, this computer's at `origin`: it is given each one minted after.
    fn holds(&self, machine: &str, origin: &Origin, bearer: &Bearer) {
        let mut holders = lock(&self.bearers);
        let armed = holders.contains_key(origin);
        holders
            .entry(origin.clone())
            .or_default()
            .insert(machine.to_owned());
        drop(holders);
        if !armed {
            let granting = self.calcifer.granting.get().cloned().unwrap_or_default();
            self.spawn(refresh(granting, origin.clone(), bearer.clone()));
        }
    }

    /// Mints this computer's bearer at `origin` and gives it to each machine sent one that is
    /// still driven. `Ok(None)` once no machine is left to give it to; `Err` when none could be
    /// minted: the Hub's refusal, or its silence, which the refresh asks again.
    async fn push(&self, origin: &Origin) -> Result<Option<Bearer>, Denied> {
        let minted = match self.hubs.facts(origin).await {
            Ok(facts) => self.hubs.bearer(origin).await.map(|bearer| (tensorfs(&facts, &bearer), bearer)),
            Err(e) => Err(e),
        };
        if let Ok((options, _)) = &minted {
            let holders: Vec<String> = lock(&self.bearers)
                .get(origin)
                .into_iter()
                .flatten()
                .cloned()
                .collect();
            // An offline machine is not dialed; it gets the next one once it is back.
            for name in holders.into_iter().filter(|name| self.offline(name).is_none()) {
                match self.linked(&name) {
                    Ok((_, link)) => {
                        if let Err(error) = link.bearer(options).await {
                            tracing::warn!(machine = %name, %error, "giving the machine its Hub bearer");
                        }
                    }
                    Err(_) => {
                        lock(&self.bearers)
                            .entry(origin.clone())
                            .or_default()
                            .remove(&name);
                    }
                }
            }
        }
        let mut held = lock(&self.bearers);
        if held.get(origin).is_none_or(|machines| machines.is_empty()) {
            held.remove(origin);
            return Ok(None);
        }
        minted.map(|(_, bearer)| Some(bearer))
    }
}

/// From the session's own refresh point of `held`, the bearer the machines were sent, each one
/// it mints at `origin` goes to them (`push`), until no machine is left to give it to. A mint
/// that fails is asked again, backing off to a minute, for as long as machines hold one.
async fn refresh(machines: Weak<Machines>, origin: Origin, held: Bearer) {
    let mut due = held.refresh_at();
    let mut failing = Duration::ZERO;
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if failing.is_zero() && due <= now {
            tracing::warn!(%origin, "the Hub's bearers expire within the session's refresh margin");
            if let Some(machines) = machines.upgrade() {
                lock(&machines.bearers).remove(&origin);
            }
            return;
        }
        tokio::time::sleep(Duration::from_secs(due.saturating_sub(now)).max(failing)).await;
        let Some(machines) = machines.upgrade() else {
            return;
        };
        match machines.push(&origin).await {
            Ok(Some(bearer)) => (due, failing) = (bearer.refresh_at(), Duration::ZERO),
            Ok(None) => return,
            Err(Denied::Unreachable(error)) => {
                failing = (failing * 2).clamp(Duration::from_secs(1), Duration::from_secs(60));
                tracing::warn!(%origin, %error, retry_s = failing.as_secs(), "minting the machines' next Hub bearer");
            }
            Err(Denied::Refused(reason)) => {
                tracing::warn!(%origin, code = %reason.code, "the Hub refused the machines' next bearer: {}", reason.message);
                lock(&machines.bearers).remove(&origin);
                return;
            }
        }
    }
}

/// What a machine reads `facts`' Hub with under `bearer`.
fn tensorfs(facts: &Facts, bearer: &Bearer) -> v1::TensorFSOptions {
    v1::TensorFSOptions {
        endpoint: facts.reads.clone(),
        token: bearer.token.clone(),
        expires_unix_ms: i64::try_from(bearer.expires)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000),
        ca_der: facts.ca_der.clone(),
    }
}

impl Hubs {
    /// This computer's live Bearer at `origin`, from the normal cached user session: held, kept,
    /// or minted again past its refresh point. The key stays on this computer.
    async fn bearer(&self, origin: &Origin) -> Result<Bearer, Denied> {
        let client = self.client(origin).map_err(|_| needs_sign_in(origin))?;
        let Credential::User(session) = client.credential() else {
            return Err(needs_sign_in(origin));
        };
        session
            .bearer(&client.anonymous())
            .await
            .map_err(|error| hub_denied(origin, error))
    }

    /// This computer's account at `origin` and where machines read it: read once per device
    /// key, so a login, logout or account switch is seen at the next run with no Hub read.
    pub(crate) async fn facts(&self, origin: &Origin) -> Result<Facts, Denied> {
        let client = self.client(origin).map_err(|_| needs_sign_in(origin))?;
        let Credential::User(session) = client.credential() else {
            return Err(needs_sign_in(origin));
        };
        let key = session.key().public_key();
        if let Some(facts) = lock(&self.facts)
            .get(origin)
            .filter(|facts| facts.key == key)
        {
            return Ok(facts.clone());
        }
        let hub = |error| hub_denied(origin, error);
        let account = client
            .get_current_account()
            .await
            .map_err(|e| hub(e.into()))?;
        let environment = client.get_execution_environment().await;
        let environment = environment
            .map_err(|e| hub(e.into()))?
            .into_inner()
            .environment;
        let names = [
            &environment.tensorhub_origin,
            &environment.tensorhub_public_origin,
        ];
        let facts = Facts {
            key,
            account: account.into_inner().name,
            names: names.iter().filter_map(|o| Origin::parse(o).ok()).collect(),
            reads: environment.tensorhub_origin,
            ca_der: environment
                .tensorhub_ca_der_b64url
                .and_then(|ca| URL_SAFE_NO_PAD.decode(ca.trim()).ok())
                .unwrap_or_default(),
        };
        lock(&self.facts).insert(origin.clone(), facts.clone());
        Ok(facts)
    }

    /// The configured Hub `origin` names: itself, or the one a machine reads or the public
    /// knows at `origin` (a dev Hub this computer dials on loopback is read elsewhere), as the
    /// facts read so far say.
    pub(crate) fn configured(&self, origin: &Origin) -> Option<Origin> {
        if self.configured.iter().any(|hub| &hub.origin == origin) {
            return Some(origin.clone());
        }
        let facts = lock(&self.facts);
        let named = self
            .configured
            .iter()
            .find(|hub| (facts.get(&hub.origin)).is_some_and(|facts| facts.names.contains(origin)));
        named.map(|hub| hub.origin.clone())
    }

    /// `origin`'s configured Hub, reading the facts of each configured Hub not read yet when the
    /// ones read so far do not name it; `origin` itself when none does.
    pub(crate) async fn resolve(&self, origin: &Origin) -> Origin {
        if let Some(hub) = self.configured(origin) {
            return hub;
        }
        self.learn().await;
        self.configured(origin).unwrap_or_else(|| origin.clone())
    }

    /// Reads the facts of each configured Hub this computer is signed in to and has not read.
    pub(crate) async fn learn(&self) {
        for hub in &self.configured {
            if !lock(&self.facts).contains_key(&hub.origin) {
                let _ = self.facts(&hub.origin).await;
            }
        }
    }

    /// An installation's Hub as a machine records it, as this computer names that Hub: the
    /// configured Hub's origin, else its one spelling; local code's stays empty.
    pub(crate) fn named(&self, hub: &str) -> String {
        match Origin::parse(hub) {
            Ok(origin) => self.configured(&origin).unwrap_or(origin).into(),
            Err(_) => hub.to_owned(),
        }
    }
}

/// The existing outbound Hub context also names public model defaults resolved later by
/// TensorD. Copying its transport acquires no credential and makes no network call.
pub(crate) fn anonymous_tensorfs(spec: &v1::RunSpec) -> Option<v1::TensorFSOptions> {
    let hub = spec.hub.as_ref().filter(|hub| !hub.origin.is_empty())?;
    Some(v1::TensorFSOptions {
        endpoint: hub.origin.clone(),
        ca_der: hub.ca_der.clone(),
        ..Default::default()
    })
}

fn hub_denied(origin: &Origin, error: hub_client::Error) -> Denied {
    let refused = error.refused();
    let error = hub_refusal(origin, error);
    match refused {
        true => Denied::Refused(Reason {
            code: error.cause.unwrap_or(error.code),
            message: error.message,
            ..Default::default()
        }),
        false => unreachable(error.message),
    }
}

/// The Hub model a run publishes into; empty for none or a `local/` one.
fn destination(spec: &v1::RunSpec) -> &str {
    let destination = spec.weights_destination.trim_start_matches("model://");
    match destination.starts_with("local/") {
        true => "",
        false => destination,
    }
}

fn needs_sign_in(origin: &str) -> Denied {
    let error: Error = signed_out(origin);
    refused(
        "hub_sign_in_required",
        format!(
            "this run publishes to {origin}, which needs this computer's sign-in there: {}",
            error.message
        ),
    )
}

fn refused(code: &str, message: String) -> Denied {
    Denied::Refused(Reason {
        code: code.into(),
        message,
        ..Default::default()
    })
}

fn unreachable(message: String) -> Denied {
    Denied::Unreachable(message)
}
