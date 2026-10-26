//! What a run's send carries for its Hub: where its machine reads the Hub (the Hub's own word,
//! kept per Hub: in a dev or CI Hub it is not the origin this computer dials) and, signed in, a run
//! capability (cl-675) this computer's device key signs offline for the machine's workload key:
//! the commits this computer's unsettled runs on that machine name by hash, the repositories they
//! publish into, and the rental the machine is, for at most a day. The machine redeems it at the
//! Hub itself (AuthKit's jwt-bearer grant), again while its runs need it, and the Hub decides each
//! time, so nothing more comes from this computer and its sign-in never leaves it. A run that reads
//! only public releases carries none. Signed out, a run reads public releases at its Hub's origin; a
//! run that publishes needs the sign-in.
use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hub_client::Credential;
use serde_json::json;
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

/// A capability's life: AuthKit's most. The Hub ends it sooner at each redemption, once its rental
/// has ended.
const LIFETIME_S: u64 = 24 * 60 * 60;

/// What a send carries besides the spec the run was submitted with.
#[derive(Default)]
pub(crate) struct Access {
    pub hub: Option<v1::HubAccess>,
    pub providers: Option<v1::ProviderAccess>,
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
    /// The AuthKit user a capability is for (`sub`).
    user: String,
    /// What the Hub's tokens are for (RFC 9728): a capability's `aud`.
    resource: String,
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

    fn is_empty(&self) -> bool {
        self.reads.is_empty() && self.writes.is_empty()
    }

    /// Their RFC 9396 `authorization_details`, in one order: the same needs sign the same.
    fn details(&self) -> serde_json::Value {
        let reads = self.reads.iter().map(|commit| json!({"type": "tensorhub_read", "commit": commit}));
        let writes = self.writes.iter().map(|model| json!({"type": "tensorhub_write", "model": model}));
        reads.chain(writes).collect()
    }
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
        let capability = match (spec.tensorfs.is_none(), &run.machine) {
            (true, Some(machine)) => self.capability(&origin, &facts, machine, run, spec).await?,
            _ => String::new(),
        };
        Ok(Access {
            hub: Some(v1::HubAccess {
                origin: facts.reads,
                ca_der: facts.ca_der,
                capability,
            }),
            providers,
            owner: Some(facts.account),
        })
    }

    /// The capability `run`, sent to `machine` as `spec`, reads and writes `origin` under: what this
    /// computer's unsettled runs there need, signed for the machine's workload key; empty when they
    /// need nothing private.
    async fn capability(
        &self,
        origin: &Origin,
        facts: &Facts,
        machine: &str,
        run: &RunRow,
        spec: &v1::RunSpec,
    ) -> Result<String, Denied> {
        let mut needs = self.needs(origin, machine, Some(run));
        needs.add(spec);
        if needs.is_empty() {
            return Ok(String::new());
        }
        let key = self.workload_key(machine).await?;
        let client = self.hubs.client(origin).map_err(refused)?;
        let Credential::User(session) = client.credential() else {
            unreachable!("a signed-in client speaks as its user")
        };
        let now = now_s();
        let mut claims = json!({
            "sub": facts.user, "aud": facts.resource, "cnf": {"jkt": key},
            "authorization_details": needs.details(), "iat": now, "exp": now + LIFETIME_S,
        });
        if let Some(rental) = self.rental_at(origin, machine) {
            claims["rental"] = rental.into();
        }
        Ok(session.key().capability(&claims))
    }

    /// `machine`'s workload key, as its status last said it, else as it says it now.
    async fn workload_key(&self, machine: &str) -> Result<String, Denied> {
        if let Some(key) = self.seen_workload_key(machine) {
            return Ok(key);
        }
        let (_, link) = self.linked(machine).map_err(|error| Denied::Unreachable(error.message))?;
        let frame = link.status().await.map_err(|error| Denied::Unreachable(error.to_string()))?;
        match frame.workload_key.is_empty() {
            false => Ok(frame.workload_key),
            true => Err(Denied::Refused(Reason {
                code: "machine_takes_no_capability".into(),
                message: format!("{machine} runs a TensorD that takes no run capability (cl-675): update it with cozy machine update"),
                ..Default::default()
            })),
        }
    }

    /// The rental `machine` is at `origin`: its capability stands only while that rental does.
    fn rental_at(&self, origin: &Origin, machine: &str) -> Option<String> {
        let row = self.store().machine(machine).ok().flatten()?;
        let hub = Origin::parse(&row.hub).ok()?;
        (self.hubs.known(&hub).as_ref().unwrap_or(&hub) == origin).then_some(row.rental_id).flatten()
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
        let resource = client.get_protected_resource_metadata().await.map_err(hub)?.resource;
        let names = [
            &environment.tensorhub_origin,
            &environment.tensorhub_public_origin,
        ];
        let facts = Facts {
            key,
            account: account.name,
            user: account.user_id,
            resource,
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
