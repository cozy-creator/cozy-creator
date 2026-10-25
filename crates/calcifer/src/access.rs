//! What a run's send carries for its Hub: where its machine reads the Hub (the Hub's own word,
//! kept per Hub: in a dev or CI Hub it is not the origin this computer dials) and, signed in, a
//! run capability as its TensorFS token (cl-670), never this computer's sign-in. The Hub signs it
//! for what this computer's live runs on that machine need there: the commits they name by hash
//! and the repositories they publish into. It is minted at the send, never stored with the
//! submission, and reused while it covers the run; each machine holding one gets a newer one at
//! its refresh point (`PUT /v1/capability`) while it has live runs at that Hub, so it dies with
//! them. A run that reads only public releases carries none. Signed out, a run reads public
//! releases at its Hub's origin; a run that publishes needs the sign-in.
use std::{
    collections::BTreeSet,
    sync::Weak,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hub_client::{Credential, types};
use protocol::{
    calcifer::Reason,
    config::Origin,
    error::{Error, Kind},
    machine as v1,
};

use crate::{
    machines::{Machines, lock},
    rentals::{Hubs, hub_refusal, reason},
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
    /// Both where machines read it and its public origin: the Hub's other names.
    names: Vec<Origin>,
    /// The CA a machine trusts for the Hub when a private one signs it (a test Hub).
    ca_der: Vec<u8>,
}

/// What runs need of a Hub's private side: the commits they name by hash, the repositories they
/// publish into.
#[derive(Default)]
pub(crate) struct Needs {
    reads: BTreeSet<String>,
    writes: BTreeSet<String>,
}

impl Needs {
    fn add(&mut self, spec: &v1::RunSpec) {
        let warm = spec
            .set
            .iter()
            .flat_map(|set| &set.items)
            .flat_map(|item| &item.models);
        for choice in spec
            .models
            .iter()
            .chain(warm)
            .filter(|c| !c.repository.starts_with("local/"))
        {
            let adapters = choice.adapters.iter().map(|adapter| &adapter.manifest);
            for manifest in std::iter::once(&choice.manifest).chain(adapters) {
                if is_digest(manifest) {
                    self.reads.insert(manifest.clone());
                }
            }
        }
        if !destination(spec).is_empty() {
            self.writes.insert(destination(spec).to_owned());
        }
    }

    fn covers(&self, other: &Needs) -> bool {
        self.reads.is_superset(&other.reads) && self.writes.is_superset(&other.writes)
    }

    fn is_empty(&self) -> bool {
        self.reads.is_empty() && self.writes.is_empty()
    }
}

/// A machine's capability at a Hub: what it was minted for, and when to mint the next.
pub(crate) struct Held {
    needs: Needs,
    options: v1::TensorFSOptions,
    refresh_at: u64,
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
        let tensorfs = match (spec.tensorfs.is_none(), &run.machine) {
            (true, Some(machine)) => {
                Some(self.capability(&origin, &facts, machine, run, spec).await?)
            }
            _ => None,
        };
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

    /// What `run`, sent to `machine` as `spec`, reads `origin` under: the machine's capability,
    /// minted again unless the one it holds covers the run; none when the run needs none.
    async fn capability(
        &self,
        origin: &Origin,
        facts: &Facts,
        machine: &str,
        run: &RunRow,
        spec: &v1::RunSpec,
    ) -> Result<v1::TensorFSOptions, Denied> {
        let mut own = Needs::default();
        own.add(spec);
        if own.is_empty() {
            return Ok(options(facts, None));
        }
        let now = now_s();
        if let Some(held) = lock(&self.capabilities)
            .get(origin)
            .and_then(|machines| machines.get(machine))
            .filter(|held| held.needs.covers(&own) && now < held.refresh_at)
        {
            return Ok(held.options.clone());
        }
        let mut needs = self.needs(origin, machine, Some(run));
        needs.add(spec);
        let held = self.hubs.mint(origin, facts, machine, needs).await?;
        let options = held.options.clone();
        self.holds(origin, machine, held);
        Ok(options)
    }

    /// What this computer's unsettled runs on `machine` at `origin` need, but `except`'s.
    fn needs(&self, origin: &Origin, machine: &str, except: Option<&RunRow>) -> Needs {
        let mut needs = Needs::default();
        let runs = self.store().unsettled().unwrap_or_default();
        for row in runs
            .iter()
            .filter_map(|id| self.store().run(id.as_str()).ok().flatten())
        {
            let here = row.machine.as_deref() == Some(machine)
                && row
                    .hub
                    .as_ref()
                    .is_some_and(|hub| self.hubs.known(hub).as_ref().unwrap_or(hub) == origin);
            if here && except.is_none_or(|except| except.id != row.id) {
                needs.add(&row.spec);
            }
        }
        needs
    }

    /// `machine` holds `held` at `origin`: it is given each one minted after, while it needs one.
    fn holds(&self, origin: &Origin, machine: &str, held: Held) {
        let mut holders = lock(&self.capabilities);
        let armed = holders.contains_key(origin);
        holders
            .entry(origin.clone())
            .or_default()
            .insert(machine.to_owned(), held);
        drop(holders);
        if !armed {
            let granting = self.calcifer.granting.get().cloned().unwrap_or_default();
            self.spawn(refresh(granting, origin.clone()));
        }
    }

    /// Mints each machine holding a capability at `origin` its next, for what its runs need now,
    /// and gives it the machine. A machine whose runs need none any more, or that is gone, is
    /// given no more. `Ok(None)` once none is left; else when to mint again.
    async fn push(&self, origin: &Origin) -> Result<Option<u64>, Denied> {
        let facts = self.hubs.facts(origin).await?;
        let machines: Vec<String> = lock(&self.capabilities)
            .get(origin)
            .map(|held| held.keys().cloned().collect())
            .unwrap_or_default();
        for machine in machines {
            let needs = self.needs(origin, &machine, None);
            let link = match needs.is_empty() {
                true => None,
                false => self.linked(&machine).ok().map(|(_, link)| link),
            };
            let Some(link) = link else {
                if let Some(held) = lock(&self.capabilities).get_mut(origin) {
                    held.remove(&machine);
                }
                continue;
            };
            let held = self.hubs.mint(origin, &facts, &machine, needs).await?;
            // An offline machine is not dialed; its next send or refresh brings it one.
            if self.offline(&machine).is_none()
                && let Err(error) = link.capability(&held.options).await
            {
                tracing::warn!(%machine, %error, "giving the machine its run capability");
            }
            if let Some(holders) = lock(&self.capabilities).get_mut(origin) {
                holders.insert(machine, held);
            }
        }
        let mut holders = lock(&self.capabilities);
        let due = holders
            .get(origin)
            .and_then(|held| held.values().map(|h| h.refresh_at).min());
        if due.is_none() {
            holders.remove(origin);
        }
        Ok(due)
    }
}

/// Mints the capabilities of the machines holding one at `origin` again at their refresh point
/// (`push`), until none needs one. A mint that fails is asked again, backing off to a minute.
async fn refresh(machines: Weak<Machines>, origin: Origin) {
    let mut failing = Duration::ZERO;
    loop {
        let due = match machines.upgrade() {
            Some(machines) => lock(&machines.capabilities)
                .get(&origin)
                .and_then(|held| held.values().map(|h| h.refresh_at).min()),
            None => return,
        };
        let Some(due) = due else { return };
        tokio::time::sleep(Duration::from_secs(due.saturating_sub(now_s())).max(failing)).await;
        let Some(machines) = machines.upgrade() else {
            return;
        };
        match machines.push(&origin).await {
            Ok(Some(_)) => failing = Duration::ZERO,
            Ok(None) => return,
            Err(Denied::Unreachable(error)) => {
                failing = (failing * 2).clamp(Duration::from_secs(1), Duration::from_secs(60));
                tracing::warn!(%origin, %error, retry_s = failing.as_secs(), "minting the machines' next run capability");
            }
            Err(Denied::Refused(reason)) => {
                tracing::warn!(%origin, code = %reason.code, "the Hub refused the machines' next run capability: {}", reason.message);
                lock(&machines.capabilities).remove(&origin);
                return;
            }
        }
    }
}

/// What a machine reads `facts`' Hub with, under `capability` (`None`: anonymously).
fn options(facts: &Facts, capability: Option<&types::Capability>) -> v1::TensorFSOptions {
    v1::TensorFSOptions {
        endpoint: facts.reads.clone(),
        token: capability.map(|c| c.token.clone()).unwrap_or_default(),
        expires_unix_ms: capability.map_or(0, |c| c.expires_unix_ms),
        ca_der: facts.ca_der.clone(),
    }
}

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn is_digest(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

impl Hubs {
    /// A capability for `needs` on `machine`, minted by `origin` for this computer's account
    /// there; the Hub signs it, this computer's sign-in stays here.
    async fn mint(
        &self,
        origin: &Origin,
        facts: &Facts,
        machine: &str,
        needs: Needs,
    ) -> Result<Held, Denied> {
        let client = self.client(origin).map_err(refused)?;
        let ask = types::CapabilityRequest {
            machine: machine.to_owned(),
            reads: needs.reads.iter().cloned().collect(),
            writes: needs.writes.iter().cloned().collect(),
        };
        let minted = client
            .create_capability(&ask)
            .await
            .map_err(|error| hub_denied(origin, error))?;
        // Minted again two thirds into its life (ten minutes of fifteen).
        let now = now_s();
        let expires = u64::try_from(minted.expires_unix_ms / 1000).unwrap_or_default();
        Ok(Held {
            needs,
            options: options(facts, Some(&minted)),
            refresh_at: now + expires.saturating_sub(now) * 2 / 3,
        })
    }

    /// This computer's account at `origin` and where machines read it: read once per device
    /// key, so a login, logout or account switch is seen at the next run with no Hub read.
    pub(crate) async fn facts(&self, origin: &Origin) -> Result<Facts, Denied> {
        let client = self.client(origin).map_err(refused)?;
        let Credential::User(session) = client.credential() else {
            unreachable!("a signed-in client speaks as its user")
        };
        let key = session.key().public_key();
        if let Some(facts) = lock(&self.facts)
            .get(origin)
            .filter(|facts| facts.key == key)
        {
            return Ok(facts.clone());
        }
        let hub = |error| hub_denied(origin, error);
        let account = client.get_current_account().await.map_err(hub)?;
        let environment = client.get_execution_environment().await.map_err(hub)?.environment;
        let names = [
            &environment.tensorhub_origin,
            &environment.tensorhub_public_origin,
        ];
        let facts = Facts {
            key,
            account: account.name,
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

    /// The Hub `origin` names: itself when it is the configured Hub or one whose facts were read
    /// (a run's), else the one whose machines read it there or whose public origin it is (a dev
    /// Hub this computer dials on loopback is read elsewhere), as the facts read so far say.
    pub(crate) fn known(&self, origin: &Origin) -> Option<Origin> {
        let facts = lock(&self.facts);
        if *origin == self.hub || facts.contains_key(origin) {
            return Some(origin.clone());
        }
        facts.iter().find(|(_, facts)| facts.names.contains(origin)).map(|(hub, _)| hub.clone())
    }

    /// `origin`'s Hub, reading the configured Hub's facts when not read yet and the ones read so
    /// far do not name it; `origin` itself when none does.
    pub(crate) async fn resolve(&self, origin: &Origin) -> Origin {
        if let Some(hub) = self.known(origin) {
            return hub;
        }
        self.learn().await;
        self.known(origin).unwrap_or_else(|| origin.clone())
    }

    /// Reads the configured Hub's facts when this computer is signed in there and has not.
    pub(crate) async fn learn(&self) {
        if !lock(&self.facts).contains_key(&self.hub) {
            let _ = self.facts(&self.hub).await;
        }
    }

    /// An installation's Hub as a machine records it, as this computer names that Hub: the
    /// known Hub's origin, else its one spelling; local code's stays empty.
    pub(crate) fn named(&self, hub: &str) -> String {
        match Origin::parse(hub) {
            Ok(origin) => self.known(&origin).unwrap_or(origin).into(),
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
    match hub_refusal(origin, error) {
        error if error.kind == Kind::Unavailable => Denied::Unreachable(error.message),
        error => refused(error),
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

fn refused(error: Error) -> Denied {
    Denied::Refused(reason(error))
}
