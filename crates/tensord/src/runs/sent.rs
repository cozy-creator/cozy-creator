//! A run as it was sent (`api::machine::RunRequest`) made this machine's own (`Spec`), and admitted.
use super::Runs;
use crate::{
    api::auth::VerifiedActor,
    gpu_service::Level,
    objects::refusal as refused,
    stored,
};
use api::{error::Error, machine as v1};
use serde_json::Value;
use std::sync::Arc;

/// A run's model choices as the preparation path reads them.
pub(crate) fn choices(sent: Vec<v1::ModelChoice>) -> Result<Vec<stored::ModelChoice>, Error> {
    sent.into_iter()
        .map(|choice| -> Result<stored::ModelChoice, Error> {
            Ok(stored::ModelChoice {
                parameter: choice.parameter,
                repository: choice.repository,
                release: choice.release,
                lane: choice.lane,
                manifest: if choice.manifest.is_empty() {
                    None
                } else {
                    Some(stored::Ref {
                        digest: digest(&choice.manifest)?,
                        length: choice.manifest_length,
                    })
                },
                source: choice.source,
                profiles: choice.profiles,
                trained_on: choice.trained_on,
                adapters: choice
                    .adapters
                    .into_iter()
                    .map(|a| stored::DownloadAdapterRef {
                        component: a.component,
                        model: a.model,
                        release: a.release,
                        lane: a.lane,
                        manifest: a.manifest,
                        scale: a.scale,
                        source: a.source,
                        profiles: a.profiles,
                        ..Default::default()
                    })
                    .collect(),
            })
        })
        .collect()
}

/// A published release by name (th-245): empty is the Hub's newest.
fn release_source(release: v1::Release) -> crate::runs::Source {
    crate::runs::Source::Release { package: release.package, release: release.release }
}

/// One warm set member, kept as it was sent for Status; None for one at `off`, which leaves
/// the set.
fn member(mut item: v1::WarmItem) -> Result<Option<crate::runs::SetItem>, Error> {
    (item.holds, item.held_back) = Default::default();
    let level = match item.level {
        v1::WarmLevel::Off => return Ok(None),
        v1::WarmLevel::Installed => Level::Installed,
        v1::WarmLevel::Downloaded => Level::Downloaded,
        v1::WarmLevel::Imported => Level::Imported,
        v1::WarmLevel::Host => Level::Host,
        v1::WarmLevel::Gpu => Level::Gpu,
    };
    if item.payload.is_some() && level != Level::Gpu {
        return Err(Error::validation("invalid_warm_set", "a prepared program requires GPU warming"));
    }
    let sent = serde_json::to_vec(&item).expect("a warm item serializes");
    let source = match item.source {
        Some(v1::WarmSource::Release(release)) => release_source(release),
        Some(v1::WarmSource::Installation(alias)) => crate::runs::Source::Installation(alias),
        None => {
            return Err(Error::validation(
                "invalid_warm_set",
                "a warm set member names a release or an installation",
            ))
        }
    };
    Ok(Some(crate::runs::SetItem {
        source,
        entrypoint: item.entrypoint,
        models: choices(item.models)?,
        payload: item.payload,
        lane: item.lane,
        level,
        sent,
    }))
}

/// A Run spec as this machine's run sources (`runs`), and the Hub it reads. Its digest (the
/// Hub's origin alone) makes the id idempotent: the same id with another spec is refused.
fn spec_of(mut spec: v1::RunSpec) -> Result<(crate::runs::Spec, Option<v1::HubAccess>), Error> {
    let (warm, remove) = match spec.kind {
        v1::RunKind::Call => (false, false),
        v1::RunKind::Warm => (true, false),
        v1::RunKind::Remove => (false, true),
        v1::RunKind::Update => return Err(Error::validation("invalid_kind", "an update is not a run of code")),
    };
    let local_private = crate::private_io::current();
    // A private request never writes its raw code/payload into a global prepare record.
    let journaled = serde_json::to_vec(&Kept::of(&spec)).unwrap_or_default();
    let input: Value = match &spec.payload {
        None => serde_json::json!({}),
        Some(payload) => crate::boundary_json::parse(payload.get().as_bytes())
            .map_err(|_| Error::validation("invalid_payload", "the payload is not JSON"))?,
    };
    let hub = spec.hub.take();
    let providers = spec.providers.take();
    let tensorfs = spec.tensorfs.take();
    // What the run is, not how it reaches its sources: a spec re-sent with other access or a
    // reformatted payload names the same run.
    let (identity_digest, legacy_digest) = {
        let mut identity = spec.clone();
        identity.payload = None;
        identity.hub = hub.as_ref().map(|hub| v1::HubAccess {
            origin: hub.origin.trim_end_matches('/').into(),
            ..Default::default()
        });
        identity.tensorfs = tensorfs.as_ref().map(|options| v1::TensorFSOptions {
            endpoint: options.endpoint.trim_end_matches('/').into(),
            ..Default::default()
        });
        // Auto transport for the existing Hub is the same run as the older caller's None.
        // A genuinely distinct endpoint remains part of the identity.
        if identity.tensorfs.as_ref().is_some_and(|options| {
            identity.hub.as_ref().is_some_and(|hub| hub.origin == options.endpoint)
        }) {
            identity.tensorfs = None;
        }
        // A JSON value's objects are sorted maps and its numbers exact: one spelling per spec.
        let mut identity = serde_json::to_value(&identity).expect("a run spec serializes");
        identity["payload"] = input.clone();
        if let Some(scope) = &local_private { identity["local_private_project"] = scope.project_id.clone().into(); }
        let encoded = crate::boundary_json::exact(&identity);
        let current = format!("sha256:{}", tensorfs::sha256::hex_digest(&encoded));
        // Old accepted requests included this retired option in their identity. Comparing
        // its former spelling preserves retries without restoring cache-bypass behavior.
        identity["no_memo"] = Value::Bool(true);
        let encoded = crate::boundary_json::exact(&identity);
        let legacy = format!("sha256:{}", tensorfs::sha256::hex_digest(&encoded));
        (current, legacy)
    };
    if hub.as_ref().is_some_and(|hub| !crate::hub::valid_origin(&hub.origin)) {
        return Err(Error::validation("invalid_hub_origin", "the run's Hub origin is invalid"));
    }
    if tensorfs.as_ref().is_some_and(|options| !crate::hub::valid_origin(&options.endpoint)) {
        return Err(Error::validation("invalid_tensorfs_endpoint", "the TensorFS endpoint is invalid"));
    }
    let source = match spec.source {
        Some(v1::Source::Release(release)) => release_source(release),
        Some(v1::Source::Installation(alias)) => crate::runs::Source::Installation(alias),
        Some(v1::Source::Local(local)) => crate::runs::Source::Local(local.manifest),
        // A warm run of model choices alone makes them (and uploads to its destination).
        None if warm => crate::runs::Source::Models,
        None => return Err(Error::validation("invalid_source", "a run spec names its source")),
    };
    // The caller's whole warm set, each member kept as it was sent for Status.
    let set = match spec.set.take().filter(|_| warm) {
        None => None,
        Some(set) => Some(set.items.into_iter().filter_map(|item| member(item).transpose()).collect::<Result<_, _>>()?),
    };
    let models = choices(spec.models)?;
    for source in models.iter().flat_map(|choice| std::iter::once(choice.source.as_str()).chain(choice.adapters.iter().map(|adapter| adapter.source.as_str()))) {
        crate::private_io::check_model_source(source)
            .map_err(|error| Error::validation("private_model_import_unsupported", error.to_string()))?;
    }
    if remove && !matches!(source, crate::runs::Source::Release { .. } | crate::runs::Source::Installation(_)) {
        return Err(Error::validation("invalid_source", "a removal names a release or an installation"));
    }
    if local_private.is_some() && (remove || set.is_some() || (warm && (spec.payload.is_some() || !models.is_empty() || !spec.entrypoint.is_empty()))) {
        return Err(Error::validation("private_warm_unsupported", "private project warm sets and administrative removal are not supported; ordinary public model warming stays available"));
    }
    Ok((crate::runs::Spec {
        local_private,
        warm,
        remove,
        set,
        parent: String::new(),
        source,
        entrypoint: spec.entrypoint,
        input,
        inputs: spec
            .inputs
            .into_iter()
            .map(|file| crate::journal::InputFile {
                input_id: file.field,
                digest: file.digest,
                length: file.length,
                media_type: file.media_type,
                order: file.order,
            })
            .collect(),
        models,
        attention_kernel: spec.attention_kernel,
        gpus: spec.gpus,
        lane: spec.lane,
        hub: None,
        tensorfs,
        providers: providers
            .map(|p| crate::published::Providers {
                huggingface: p.huggingface,
                civitai: p.civitai,
            })
            .unwrap_or_default(),
        weights_destination: spec.weights_destination,
        owner: spec.owner,
        application: String::new(),
        digest: identity_digest,
        legacy_digest: Some(legacy_digest),
        journaled,
    }, hub))
}

fn digest(text: &str) -> Result<Vec<u8>, Error> {
    let invalid = || Error::validation("invalid_digest", "a digest is sha256:<64 hex>");
    let hex = text
        .strip_prefix("sha256:")
        .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(invalid)?;
    (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| invalid()))
        .collect()
}

/// `spec` reading `hub`, under `actor`'s run capability there when the request carried one.
fn reach(actor: &str, mut spec: crate::runs::Spec, hub: Option<v1::HubAccess>) -> Result<crate::runs::Spec, Error> {
    if let Some(hub) = hub {
        // Refused before its capability is offered: an offered one is served at once.
        if !crate::hub::valid_origin(&hub.origin) {
            return Err(refused("hub_access_invalid", format!("{} is not an HTTPS origin", hub.origin)));
        }
        let capability = crate::hub::Capability::offer(actor, &hub).map_err(|(code, message)| match code {
            "workload_key_changed" => Error::conflict(code, message),
            _ => Error::validation(code, message),
        })?;
        let ca = Some(hub.ca_der).filter(|der| !der.is_empty());
        let source = crate::hub::Source::new(&hub.origin, ca, capability).map_err(|e| refused("hub_access_invalid", &e.0))?;
        spec.hub = Some(source);
    }
    Ok(spec)
}

/// A preparing run's spec as the journal keeps it, for a restart to prepare it again. Its access
/// (Hub capability, provider and TensorFS tokens) stays in memory only: a run that came with any
/// waits, `needs_access`, for its sender to send it again.
#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    spec: v1::RunSpec,
    access: bool,
}
impl Kept {
    fn of(spec: &v1::RunSpec) -> Self {
        let mut spec = spec.clone();
        let mut access = spec.providers.take().is_some_and(|p| !p.huggingface.is_empty() || !p.civitai.is_empty());
        if let Some(options) = spec.tensorfs.as_mut() {
            access |= !std::mem::take(&mut options.token).is_empty();
        }
        if let Some(hub) = spec.hub.as_mut() {
            access |= !std::mem::take(&mut hub.capability).is_empty();
        }
        Self { spec, access }
    }
}

/// The runs a restart interrupted while they prepared: each prepares again from its journaled
/// spec, waits `needs_access` for its sender when it came with access, or ends FAILED with why
/// it cannot.
pub(crate) fn resume(runs: &Arc<crate::runs::Runs>) -> std::io::Result<()> {
    resume_scope(runs, None)
}

pub(crate) fn resume_private(runs: &Arc<Runs>, project: &str) -> std::io::Result<()> {
    resume_scope(runs, Some(project))
}

fn resume_scope(runs: &Arc<Runs>, project: Option<&str>) -> std::io::Result<()> {
    for (run, actor, kept) in runs.service.engine.interrupted_for(project)? {
        let spec = serde_json::from_slice::<Kept>(&kept)
            .map_err(|e| refused("preparation_interrupted", format!("its kept spec is unreadable: {e}")));
        if spec.as_ref().is_ok_and(|kept| kept.access) {
            runs.service.engine.park(&run)?;
            tracing::info!(run = %runs.service.engine.caller_id(&run), number = %run, "kept across the restart, needs_access: its sender sends its access again");
            continue;
        }
        let private = runs.service.engine.private_storage(&run)?;
        let spec = spec
            .and_then(|kept| crate::private_io::within(private, || spec_of(kept.spec)))
            .and_then(|(spec, hub)| reach(&actor, spec, hub));
        let failure = match spec {
            Ok(spec) => {
                runs.prepare_later(&actor, &run, spec)?;
                continue;
            }
            Err(error) => crate::journal::Failure::machine(&error.code, error.message),
        };
        runs.service.engine.end_preparation(&run, crate::journal::Outcome::Failed(failure))?;
    }
    Ok(())
}

/// Whether a run sent is new, and its state; or why it was refused.
pub(crate) type Accepted = Result<(bool, v1::RunState), Error>;

impl Runs {
    /// Accepts the runs `requests` name as one admission (`Runs::admit`): each whether it is new and
    /// its state, or why it was refused. A run reads its Hub's public content anonymously, and private
    /// content under its signer's run capability there, which a re-sent spec's may replace and which is
    /// renewed while the run lives.
    pub(crate) fn accept(
        self: &Arc<Self>,
        actor: VerifiedActor,
        requests: Vec<v1::RunRequest>,
    ) -> Result<Vec<Accepted>, Error> {
        let signer = super::read::actor_id(actor);
        let (mut answers, mut sent, mut serving) = (vec![], vec![], vec![]);
        for request in requests {
            match spec_of(request.spec).and_then(|(spec, hub)| reach(&signer, spec, hub)) {
                Ok(spec) => {
                    // Renewed from before it prepares: its first reads wait for the token.
                    let capability = spec.hub.as_ref().and_then(|hub| hub.capability.as_ref());
                    serving.extend(capability.map(|capability| capability.serves(&request.id, &self.service.engine)));
                    sent.push((answers.len(), (request.id.clone(), spec)));
                    answers.push((request.id, None));
                }
                Err(error) => answers.push((request.id, Some(Err(error)))),
            }
        }
        let (at, sent): (Vec<usize>, Vec<_>) = sent.into_iter().unzip();
        let admitted = self.admit(&signer, sent).map_err(crate::objects::Refused::from)?;
        for (at, admitted) in at.into_iter().zip(admitted) {
            answers[at].1 = Some(admitted.map(|(_, new)| new).map_err(Error::from));
        }
        drop(serving);
        Ok(answers
            .into_iter()
            .map(|(id, admitted)| Ok((admitted.expect("each run answered")?, self.state(actor, &id)?)))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(input: &str) -> v1::RunSpec {
        serde_json::from_value(serde_json::json!({
            "source": {"installation": "example"},
            "entrypoint": "generate",
            "payload": {"text": input},
        })).unwrap()
    }

    #[test]
    fn retired_memo_option_keeps_accepted_request_digests_replayable() {
        let sent = request("same input");
        let mut old = serde_json::to_value(&sent).unwrap();
        old["payload"] = serde_json::json!({"text": "same input"});
        old["no_memo"] = Value::Bool(true);
        let old_digest = format!("sha256:{}", tensorfs::sha256::hex_digest(&crate::boundary_json::exact(&old)));
        old.as_object_mut().unwrap().remove("no_memo");
        let normal_digest = format!("sha256:{}", tensorfs::sha256::hex_digest(&crate::boundary_json::exact(&old)));
        let (current, _) = spec_of(sent).unwrap();
        assert_eq!(current.digest, normal_digest);
        assert_eq!(current.legacy_digest.as_deref(), Some(old_digest.as_str()));
        assert_ne!(current.digest, old_digest);

        let (changed, _) = spec_of(request("different input")).unwrap();
        assert_ne!(changed.digest, old_digest);
        assert_ne!(changed.legacy_digest.as_deref(), Some(old_digest.as_str()));
    }
}
