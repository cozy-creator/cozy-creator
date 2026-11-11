//! `cozy/model_quantize`: a new checkpoint in a lane, written from a checkpoint (or a release's
//! original weights) by the recipe a named package declares for that lane. The package is always
//! named, a release or local code: packages that serve the same model can each declare a recipe
//! for it. Its `quantize` (the Runtime's) runs on the machine, which uploads the output as a
//! checkpoint of the model in that lane.
use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ::api::hub::types::ModelCard;
use api::{
    calcifer::{Agreed, On, RunRequest},
    code::LocalCode,
    config::Origin,
    error::{Error, Kind},
    calcifer::hub::ModelQuantize,
    interface::{Interface, RECIPES, Recipe},
    model_ref::{self as refs, ModelRef, ModelId},
    target::Published,
};
use tensorfs::{
    Config, Endpoint,
    format::{Dtype, Header, ObjectRef},
};

use super::{jobs::Planned, reader, refusal, signed_in};
use crate::{
    api::{Api, Refusal},
    runs::target::{Resolved, resolve},
};

/// Legacy spellings, besides TensorFS's canonical BF16/F16/F32 dtype names.
const ORIGINAL: [&str; 3] = ["bf16", "fp16", "f32"];

/// The checkpoint a quantize reads: spelled for the run, its model, and the components its
/// header holds.
struct Source {
    spelled: String,
    model_id: ModelId,
    components: Vec<String>,
}

/// The run of `package`'s `quantize` that writes `lane` from the checkpoint `model` names into the
/// checkpoint's own model, with each calibration model it builds bound to that checkpoint.
pub(super) async fn plan(api: &Arc<Api>, key: &str, hub: &Origin, on: On, agreed: Option<Agreed>, code: Option<LocalCode>, ask: ModelQuantize) -> Result<Planned, Refusal> {
    let lane = refs::label("--lane", &ask.lane).map_err(Refusal)?;
    let model = ModelRef::parse(&ask.model).map_err(Refusal)?;
    let target = match &code {
        Some(_) => RECIPES.to_owned(),
        None if api::target::written_as_path(&ask.package) => return Err(Refusal(crate::runs::target::by_path(&ask.package))),
        None => published(&ask.package)?,
    };
    let (package, interface, release) = match resolve(api, hub, &target, code.as_ref(), &on, false, true, key, agreed).await.map_err(Refusal)? {
        Resolved::Release { package, card, .. } => (package, card.interface, card.release),
        Resolved::Local { local, interface: None, .. } => {
            return Ok(Planned::Preparing(Box::new(local.install.expect("an uninstalled local target is being installed"))));
        }
        Resolved::Local { local, interface: Some(interface), .. } => (local.package, interface, local.source.manifest),
    };
    let source = checkpoint(api, hub, &model, &lane).await?;
    let found = recipes(&interface, &lane, &source.components);
    let named = match release.starts_with("sha256:") {
        true => package.clone(),
        false => format!("{package}@{release}"),
    };
    if found.is_empty() {
        return Err(Refusal(
            Error::not_found(
                "model_quantizer_not_found",
                format!("{named} has no {lane} recipe for this model ({})", source.components.join(", ")),
            )
            .remedy(declared(&interface)),
        ));
    }
    // The checkpoint goes to the source's own model, in the lane; local code's records its package as
    // `local/<name>` and its release as the source's commit: an unpublished recipe, exactly.
    let mut terms = vec![
        source.spelled.clone(),
        source.model_id.to_string(),
        format!("lane={lane}"),
        format!("package={package}"),
        format!("release={release}"),
    ];
    terms.extend(calibration(&interface, &found, &source.spelled));
    Ok(Planned::Request(Box::new(RunRequest { target, code, on: Some(on), hub: hub.to_string(), terms, recipes: true, agreed, ..Default::default() })))
}

/// `org/name[@release]` as the target of its `quantize`.
fn published(named: &str) -> Result<String, Refusal> {
    let invalid = || Refusal(Error::new(Kind::Usage, "package_ref_invalid", format!("{named:?} is not [org/]name[@release]")));
    // `x/y` is account x's package y.
    let (package, release) = match Published::parse(named.trim()).map_err(|_| invalid())? {
        Published { package, function: None, release, .. } => (package, release),
        Published { owned: Some(owned), release, .. } => (owned, release),
        _ => return Err(invalid()),
    };
    Ok(match release {
        Some(release) => format!("{package}/{RECIPES}@{release}"),
        None => format!("{package}/{RECIPES}"),
    })
}

/// Whether a header of `kinds` holds only original weights: 16- or 32-bit floats, beside the
/// integer and boolean buffers (position ids, masks) a model keeps as they are, which are no
/// weights and never quantized.
fn original(kinds: &[String]) -> bool {
    let float = |kind: &String| {
        matches!(Dtype::parse(kind), Dtype::BF16 | Dtype::F16 | Dtype::F32) || ORIGINAL.contains(&kind.as_str())
    };
    let index = |kind: &String| {
        use Dtype::*;
        matches!(Dtype::parse(kind), BOOL | I16 | U16 | I32 | U32 | I64 | U64)
    };
    kinds.iter().any(float) && kinds.iter().all(|kind| float(kind) || index(kind))
}

/// The checkpoint `model` names, as the model's card lists it (its org's members see every checkpoint):
/// a checkpoint by id, or a release's lane of original weights (its only lane when it has one). A
/// 4-bit lane is written only from original weights (owner, 2026-10-22): from a checkpoint holding
/// none it would stack two rounds of 4-bit error.
async fn checkpoint(api: &Api, hub: &Origin, model: &ModelRef, lane: &str) -> Result<Source, Refusal> {
    let (model_id, release, id) = match model {
        ModelRef::Hub { model: model_id, release } => (model_id, release.as_deref(), None),
        ModelRef::Checkpoint { model: model_id, checkpoint } => (model_id, None, Some(checkpoint.as_str())),
        ModelRef::Local(_) | ModelRef::Provider(_) => {
            return Err(Refusal(
                Error::new(Kind::Usage, "model_quantize_source", format!("{model} is not on the Hub: quantize reads a Hub model"))
                    .next(format!("cozy model upload {model} <org/name> --lane <lane>")),
            ));
        }
    };
    let card: ModelCard = (reader(api, hub)?.get_model(&model_id.org, &model_id.name).await).map_err(|e| refusal(hub, e))?;
    let absent =
        |what: String| Refusal(Error::not_found("model_commit_not_found", what).next(format!("cozy model ls {model_id}")));
    let (checkpoint, header, kinds) = match id {
        Some(id) => {
            let found = (card.checkpoints.iter().find(|c| c.id == id))
                .ok_or_else(|| absent(format!("Checkpoint {id} is unavailable in {model_id}")))?;
            let checkpoint = ObjectRef { sha256: found.id.trim_start_matches("sha256:").into(), length: found.length as u64 };
            (checkpoint, found.header.clone(), found.kinds.clone())
        }
        None => {
            let found = match release {
                Some(version) => card.releases.iter().find(|r| r.version == version),
                None => card.releases.first(),
            };
            let found = found.ok_or_else(|| absent(format!("{model_id} has no release {}", release.unwrap_or("yet"))))?;
            let originals: Vec<_> = found.lanes.values().filter(|l| original(&l.kinds)).collect();
            let picked = match (originals.as_slice(), found.lanes.len()) {
                ([one], _) => *one,
                (_, 1) => found.lanes.values().next().expect("one lane"),
                _ => {
                    let lanes: Vec<&String> = found.lanes.keys().collect();
                    return Err(Refusal(
                        Error::new(
                            Kind::Usage,
                            "model_quantize_source",
                            format!("{model_id}@{} has lanes {lanes:?}: choose a checkpoint to quantize", found.version),
                        )
                        .next(format!("cozy model quantize {model_id}@<checkpoint> --lane {lane} --package <package>")),
                    ));
                }
            };
            let checkpoint = ObjectRef { sha256: picked.checkpoint.trim_start_matches("sha256:").into(), length: picked.length as u64 };
            (checkpoint, picked.header.clone(), picked.kinds.clone())
        }
    };
    if matches!(lane, "nvfp4" | "w4a8" | "svdq-nvfp4") && !original(&kinds) {
        let message = format!(
            "{model_id}@{} holds no original weights ({}): a {lane} lane from them stacks two rounds of 4-bit error",
            checkpoint.id(),
            kinds.join(", ")
        );
        return Err(Refusal(Error::new(Kind::Usage, "model_quantize_source_quantized", message).remedy(format!(
            "upload the original weights (cozy model upload <path> {model_id} --lane bf16) and quantize those"
        ))));
    }
    let header = ObjectRef { sha256: header.sha256, length: header.length as u64 };
    let read = (endpoint(api, hub).await?.document(&header).await)
        .map_err(|f| Refusal(Error::unavailable("model_header_unread", f.detail)))?;
    let components = (Header::from_cbor(&read).map_err(|e| Refusal(Error::validation("model_header_invalid", e.to_string())))?)
        .components()
        .iter()
        .map(|c| c.name.clone())
        .collect();
    Ok(Source { spelled: format!("{model_id}@{}", checkpoint.id()), model_id: model_id.clone(), components })
}

/// `hub`'s distribution endpoint as this computer's signed-in user.
async fn endpoint(api: &Api, hub: &Origin) -> Result<Endpoint, Refusal> {
    distribution(&signed_in(api, hub)?, hub).await
}

/// `hub`'s distribution endpoint as `client`: signed in, trusting the private CA the Hub names (a test Hub's), else what's public.
pub(super) async fn distribution(client: &::api::hub::Client, hub: &Origin) -> Result<Endpoint, Refusal> {
    let mut config = Config::from_env();
    if let ::api::hub::Credential::User(session) = client.credential() {
        let ca = (client.get_execution_environment().await).map_err(|e| refusal(hub, e))?.environment.tensorhub_ca_der_b64url;
        let token = (session.bearer(&client.anonymous()).await).map_err(|e| refusal(hub, e))?.token;
        config = Config { token: Some(tensorfs::bearer(token)), ca_der: ca.and_then(|ca| URL_SAFE_NO_PAD.decode(ca.trim()).ok()), ..config };
    }
    Endpoint::with_config(hub.as_str(), &config).map_err(|f| Refusal(Error::validation("endpoint_invalid", f.detail)))
}

/// The recipes of `lane` that read a checkpoint holding `components`.
fn recipes<'i>(interface: &'i Interface, lane: &str, components: &[String]) -> Vec<&'i Recipe> {
    (interface.recipes.iter()).filter(|r| r.lane == lane && r.reads(components)).collect()
}

/// `model.<slot>=<source>` for each model the package's `quantize` constructs on demand (a package
/// with a calibrated recipe serves `quantize_calibrated`) whose class one of `found` reads. The
/// machine binds every slot, so a lane that does not calibrate binds it too (run 19801: int8 of
/// Qwen-Image-2.1-Turbo); it is built only when a recipe calibrates. Another class's is never bound.
fn calibration(interface: &Interface, found: &[&Recipe], source: &str) -> Vec<String> {
    (interface.callables.iter())
        .filter(|c| c.name == RECIPES)
        .flat_map(|c| &c.models)
        .filter(|slot| !slot.weights && found.iter().any(|r| r.model.class == slot.class))
        .map(|slot| format!("model.{}={source}", slot.param))
        .collect()
}

/// What a package declares, for a refusal: each recipe's flag and the model it reads.
fn declared(interface: &Interface) -> String {
    if interface.recipes.is_empty() {
        return "it declares no recipes; name the package that does with --package org/name".into();
    }
    let each: Vec<String> = (interface.recipes.iter())
        .map(|r| {
            let mut reads: Vec<&str> = r.model.component_use.values().flatten().map(String::as_str).collect();
            reads.sort_unstable();
            reads.dedup();
            let reads = if reads.is_empty() { "any checkpoint".to_owned() } else { reads.join(", ") };
            format!("{} ({}: {reads})", r.flag(), r.model.class)
        })
        .collect();
    format!("its recipes: {}", each.join("; "))
}
