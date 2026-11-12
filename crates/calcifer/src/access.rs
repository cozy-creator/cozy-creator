//! What a run's send carries for its Hub: where its machine reads the Hub (the Hub's own word,
//! kept per Hub: in a dev or CI Hub it is not the origin this computer dials) and, signed in, a run
//! capability (cl-675) this computer's device key signs offline for the machine's workload key:
//! the checkpoints its unsettled runs name, their private releases, unversioned models it may read,
//! the models they publish into,
//! and the rental the machine is, for at most a day. The machine redeems it at the
//! Hub itself (AuthKit's jwt-bearer grant), again while its runs need it, and the Hub decides each
//! time, so nothing more comes from this computer and its sign-in never leaves it. A run that reads
//! only public releases carries none. Signed out, a run reads public releases at its Hub's origin; a
//! run that publishes needs the sign-in.
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{SystemTime, UNIX_EPOCH},
};

use ::api::hub::Credential;
use api::{
    calcifer::Reason,
    config::Origin,
    error::{Error, Kind},
    machine as v1,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::json;

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
    /// The orgs the account is a member of, and whether it is a platform admin: whose models' private
    /// commits it reads.
    orgs: Vec<String>,
    admin: bool,
    /// The Hub grants model reads (`tensorhub_model_read`, xs-173); one before it refuses a
    /// capability naming them.
    model_reads: bool,
}

impl Facts {
    /// Whether the account may hold private checkpoints of model `org/name`.
    fn reads_privately(&self, model: &str) -> bool {
        let org = model.split('/').next().unwrap_or_default();
        self.model_reads && (self.admin || org == self.account || self.orgs.iter().any(|o| o == org))
    }
}

/// The checkpoints, private releases and unversioned models runs read, and the models they publish into.
#[derive(Default)]
pub(crate) struct Needs {
    reads: BTreeSet<String>,
    models: BTreeSet<String>,
    writes: BTreeSet<String>,
    named_releases: BTreeSet<(String, String)>,
    release_reads: BTreeSet<ReleaseRead>,
}

/// The immutable release roots a machine may resolve, never an org-wide credential.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ReleaseRead {
    model: String,
    release: String,
    checkpoints: Vec<String>,
}

impl Needs {
    fn add(&mut self, spec: &v1::RunSpec) {
        let warm = spec
            .set
            .iter()
            .flat_map(|set| &set.items)
            .flat_map(|item| &item.models);
        for choice in spec.models.iter().chain(warm) {
            self.named_release(
                &choice.model,
                &choice.release,
                &choice.manifest,
                &choice.source,
            );
            for adapter in &choice.adapters {
                self.named_release(
                    &adapter.model,
                    &adapter.release,
                    &adapter.manifest,
                    &adapter.source,
                );
            }
            let adapters = choice
                .adapters
                .iter()
                .map(|adapter| (&adapter.model, &adapter.release, &adapter.manifest, &adapter.source));
            for (model, release, manifest, source) in
                std::iter::once((&choice.model, &choice.release, &choice.manifest, &choice.source)).chain(adapters)
            {
                if !model.starts_with("local/") && is_digest(manifest) {
                    self.reads.insert(manifest.clone());
                } else if release.is_empty() && manifest.is_empty() && source.is_empty()
                    && let Some(model) = model_name(model)
                {
                    self.models.insert(model);
                }
            }
        }
        if !destination(spec).is_empty() {
            self.writes.insert(destination(spec).to_owned());
        }
    }

    fn is_empty(&self) -> bool {
        self.reads.is_empty() && self.models.is_empty() && self.writes.is_empty() && self.release_reads.is_empty()
    }

    fn named_release(&mut self, model: &str, release: &str, checkpoint: &str, source: &str) {
        if !checkpoint.is_empty()
            || !source.is_empty()
            || release.is_empty()
            || release.eq_ignore_ascii_case("latest")
        {
            return;
        }
        if let Ok(model) = api::model_ref::ModelId::parse(model)
            && model.org != "local"
        {
            self.named_releases
                .insert((model.to_string(), release.to_owned()));
        }
    }

    /// Public pinned versions retain their anonymous machine-side resolution path. Only
    /// an owner's explicitly private version needs a frozen release-read authorization.
    async fn private_releases(
        &mut self,
        client: &::api::hub::Client,
        origin: &Origin,
    ) -> Result<(), Denied> {
        let mut requested: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (model, release) in &self.named_releases {
            requested.entry(model).or_default().push(release);
        }
        let failure = |error| {
            let error = hub_refusal(origin, error);
            if error.kind == Kind::Unavailable {
                Denied::Unreachable(error.message)
            } else {
                refused(error)
            }
        };
        for (model, versions) in requested {
            let id = api::model_ref::ModelId::parse(model).map_err(refused)?;
            let public = match client.anonymous().get_model(&id.org, &id.name).await {
                Ok(card) => Some(card),
                Err(::api::hub::Error::Hub { status: 404, .. }) => None,
                Err(error) => return Err(failure(error)),
            };
            let missing: Vec<_> = versions
                .into_iter()
                .filter(|version| {
                    public.as_ref().is_none_or(|card| {
                        !card.releases.iter().any(|release| {
                            release.visibility == api::model_ref::ReleaseVisibility::Public
                                && same_version(&release.version, version)
                        })
                    })
                })
                .collect();
            if missing.is_empty() {
                continue;
            }
            let owner = client.get_model(&id.org, &id.name).await.map_err(failure)?;
            for version in missing {
                if let Some(release) = owner.releases.iter().find(|release| {
                    release.visibility == api::model_ref::ReleaseVisibility::Private
                        && same_version(&release.version, version)
                }) {
                    let canonical = api::model_ref::ModelId::parse(&format!(
                        "{}/{}",
                        owner.model.org, owner.model.name
                    ))
                    .map_err(refused)?;
                    self.release_reads
                        .insert(ReleaseRead::of(&canonical.to_string(), release).map_err(refused)?);
                }
            }
        }
        Ok(())
    }

    /// Their RFC 9396 `authorization_details`, in one order: the same needs sign the same.
    fn details(&self) -> serde_json::Value {
        let reads = self
            .reads
            .iter()
            .map(|commit| json!({"type": "tensorhub_read", "commit": commit}));
        let writes = self
            .writes
            .iter()
            .map(|model| json!({"type": "tensorhub_write", "model": model}));
        let releases = self.release_reads.iter().map(|release| {
            json!({
                "type": "tensorhub_release_read", "model": release.model,
                "release": release.release, "checkpoints": release.checkpoints,
            })
        });
        let models = self.models.iter().map(|model| json!({"type": "tensorhub_model_read", "model": model}));
        reads.chain(releases).chain(models).chain(writes).collect()
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

    /// The Hub access a machine's list (`PUT /v1/upcoming`, `/v1/prepare`) of `specs` at `hub` carries: what this computer's
    /// unsettled runs there and they read, signed for its workload key; signed out, the Hub's origin alone, as a run's.
    pub(crate) async fn list_access(&self, hub: &str, machine: &str, specs: &[v1::RunSpec]) -> Result<Option<v1::HubAccess>, String> {
        let origin = match Origin::parse(hub) {
            Ok(origin) => self.hubs.resolve(&origin).await,
            Err(_) => self.hubs.hub.clone(),
        };
        if self.hubs.client(&origin).is_err() {
            return Ok(Some(v1::HubAccess { origin: origin.to_string(), ..Default::default() }));
        }
        let denied = |denied: Denied| match denied {
            Denied::Refused(reason) => reason.message,
            Denied::Unreachable(message) => message,
        };
        let facts = self.hubs.facts(&origin).await.map_err(denied)?;
        let mut needs = self.needs(&origin, machine, None);
        specs.iter().for_each(|spec| needs.add(spec));
        needs.models.retain(|model| facts.reads_privately(model));
        let client = self.hubs.client(&origin).map_err(|e| e.message)?;
        needs.private_releases(&client, &origin).await.map_err(denied)?;
        let capability = match needs.is_empty() {
            true => String::new(),
            false => {
                let key = self.workload_key(machine).await.map_err(denied)?;
                let Credential::User(session) = client.credential() else { unreachable!("a signed-in client speaks as its user") };
                let now = now_s();
                let mut claims = json!({
                    "sub": facts.user, "aud": facts.resource, "cnf": {"jkt": key},
                    "authorization_details": needs.details(), "iat": now, "exp": now + LIFETIME_S,
                });
                if let Some(rental) = self.rental_at(&origin, machine) {
                    claims["rental"] = rental.into();
                }
                session.key().capability(&claims)
            }
        };
        Ok(Some(v1::HubAccess { origin: facts.reads, ca_der: facts.ca_der, capability }))
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
        needs.models.retain(|model| facts.reads_privately(model));
        let client = self.hubs.client(origin).map_err(refused)?;
        needs.private_releases(&client, origin).await?;
        if needs.is_empty() {
            return Ok(String::new());
        }
        let key = self.workload_key(machine).await?;
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
        let (_, link) = self
            .linked(machine)
            .map_err(|error| Denied::Unreachable(error.message))?;
        let frame = link
            .status()
            .await
            .map_err(|error| Denied::Unreachable(error.to_string()))?;
        match frame.workload_key.is_empty() {
            false => Ok(frame.workload_key),
            true => Err(Denied::Unreachable(format!(
                "{machine} has not said its workload key yet"
            ))),
        }
    }

    /// The rental `machine` is at `origin`: its capability stands only while that rental does.
    fn rental_at(&self, origin: &Origin, machine: &str) -> Option<String> {
        let row = self.store().machine(machine).ok().flatten()?;
        let hub = Origin::parse(&row.hub).ok()?;
        (self.hubs.known(&hub).as_ref().unwrap_or(&hub) == origin)
            .then_some(row.rental_id)
            .flatten()
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

/// A Hub model's one spelling (`org/name`, `_/name` for a first-party one, lowercase) as a
/// capability names it; none for a `local/` alias or a name the Hub would not read.
fn model_name(text: &str) -> Option<String> {
    let text = text.to_ascii_lowercase();
    let (org, name) = text.split_once('/').unwrap_or(("_", &text));
    let slug = |s: &str| (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    (org != "local" && (org == "_" || slug(org)) && slug(name)).then(|| format!("{org}/{name}"))
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
        self.facts_of(origin)
            .await
            .map_err(|error| match error.kind {
                Kind::Unavailable => Denied::Unreachable(error.message),
                _ => refused(error),
            })
    }

    /// `facts`, refused in the Hub's own words.
    async fn facts_of(&self, origin: &Origin) -> Result<Facts, Error> {
        let client = self.client(origin)?;
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
        let hub = |error| hub_refusal(origin, error);
        let account = client.get_current_account().await.map_err(hub)?;
        let environment = client
            .get_execution_environment()
            .await
            .map_err(hub)?
            .environment;
        let metadata = client
            .get_protected_resource_metadata()
            .await
            .map_err(hub)?;
        let model_reads = metadata.authorization_details_types_supported.iter().any(|t| t == "tensorhub_model_read");
        let names = [
            &environment.tensorhub_origin,
            &environment.tensorhub_public_origin,
        ];
        let facts = Facts {
            key,
            account: account.name,
            user: account.user_id,
            resource: metadata.resource,
            orgs: account.orgs,
            admin: account.admin,
            model_reads,
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
        facts
            .iter()
            .find(|(_, facts)| facts.names.contains(origin))
            .map(|(hub, _)| hub.clone())
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

fn same_version(left: &str, right: &str) -> bool {
    left == right
        || left
            .parse::<pep440_rs::Version>()
            .ok()
            .zip(right.parse::<pep440_rs::Version>().ok())
            .is_some_and(|(left, right)| left == right)
}

impl ReleaseRead {
    fn of(model: &str, release: &::api::hub::types::ModelRelease) -> Result<Self, Error> {
        let checkpoints: BTreeSet<_> = release
            .lanes
            .values()
            .map(|lane| lane.checkpoint.clone())
            .collect();
        if checkpoints.is_empty() || checkpoints.iter().any(|id| !is_digest(id)) {
            return Err(Error::new(
                Kind::Validation,
                "private_release_incomplete",
                format!(
                    "{model}@{} has no complete checkpoint mapping",
                    release.version
                ),
            ));
        }
        Ok(Self {
            model: model.to_owned(),
            release: release.version.clone(),
            checkpoints: checkpoints.into_iter().collect(),
        })
    }
}

#[cfg(test)]
mod release_read_tests {
    use super::*;

    #[test]
    fn private_release_authorization_freezes_all_lanes_without_granting_a_namespace() {
        let id = format!("sha256:{}", "a".repeat(64));
        let release: ::api::hub::types::ModelRelease =
            serde_json::from_value(json!({"version":"1.0.0", "visibility":"private", "lanes":{
                "f16":{"checkpoint":id,"header":{"sha256":"header","length":1}},
                "bf16":{"checkpoint":id,"header":{"sha256":"header","length":1}}
            }}))
            .unwrap();
        let mut needs = Needs::default();
        needs
            .release_reads
            .insert(ReleaseRead::of("author/model", &release).unwrap());
        assert_eq!(
            needs.details(),
            json!([{"type":"tensorhub_release_read", "model":"author/model", "release":"1.0.0", "checkpoints":[id]}])
        );
        assert!(needs.reads.is_empty());
        assert!(needs.writes.is_empty());
        assert!(same_version("1.0", "1.0.0"));
    }

    #[test]
    fn exact_checkpoints_local_names_and_provider_sources_need_no_named_release_scope() {
        let mut needs = Needs::default();
        needs.named_release("author/model", "1", "sha256:already-exact", "");
        needs.named_release("local/model", "1", "", "");
        needs.named_release("author/model", "1", "", "hf://author/model");
        assert!(needs.named_releases.is_empty());
    }

    #[test]
    fn local_base_model_does_not_hide_a_private_hub_lora() {
        let local_checkpoint = format!("sha256:{}", "b".repeat(64));
        let spec = v1::RunSpec {
            models: vec![v1::ModelChoice {
                model: "local/base".into(),
                manifest: local_checkpoint,
                adapters: vec![v1::Adapter {
                    model: "author/lora".into(),
                    release: "1.0".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut needs = Needs::default();
        needs.add(&spec);
        assert_eq!(
            needs.named_releases,
            BTreeSet::from([("author/lora".into(), "1.0".into())])
        );
        assert!(needs.reads.is_empty());
    }
}
