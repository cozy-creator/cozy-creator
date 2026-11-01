//! `/v1/model-quantizations`: a new commit in a lane, written from a commit (or a release's
//! original weights) by the recipe a named package declares for that lane. The package is always
//! named, a release or local code: packages that serve the same model can each declare a recipe
//! for it. Its `quantize` (the Runtime's) runs on the machine, which uploads the output as a
//! commit of the model in that lane.
use std::{path::Path, sync::Arc};

use axum::{
    Json,
    body::Bytes,
    extract::{State, rejection::BytesRejection},
    http::StatusCode,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hub_client::types::ModelCard;
use protocol::{
    calcifer::{Run, RunQuote, RunRequest, Submitted},
    config::Origin,
    error::{Error, Kind},
    hub::ModelQuantize,
    interface::{Interface, RECIPES, Recipe},
    model_ref::{self as refs, ModelRef, Repo},
    target::Published,
};
use tensorfs::{
    Config, Endpoint,
    format::{Dtype, Header, ObjectRef},
};

use super::{origin, reader, refusal, signed_in};
use crate::{
    api::{Api, IdempotencyKey, Refusal, parse},
    runs::target::{Resolved, resolve},
};

/// Legacy spellings, besides TensorFS's canonical BF16/F16/F32 dtype names.
const ORIGINAL: [&str; 3] = ["bf16", "fp16", "f32"];

/// The commit a quantize reads: spelled for the run, its repository, and the components its
/// header holds.
struct Source {
    spelled: String,
    repo: Repo,
    components: Vec<String>,
}

/// What an ask becomes: the run of the package's `quantize`, or local code's install first.
enum Planned {
    Run(RunRequest),
    Install(Box<Run>),
}

/// `POST /v1/model-quantizations`.
pub(super) async fn create(
    State(api): State<Arc<Api>>,
    IdempotencyKey(key): IdempotencyKey,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Submitted>), Refusal> {
    let ask: ModelQuantize = parse(&body?)?;
    let submitted = match plan(&api, &key, ask).await? {
        Planned::Install(run) => Submitted { run: *run, replay: false, notes: vec![], preparing: true },
        Planned::Run(request) => crate::runs::request(&api, &key, request).await.map_err(Refusal)?,
    };
    let status = if submitted.replay { StatusCode::OK } else { StatusCode::ACCEPTED };
    Ok((status, Json(submitted)))
}

/// `POST /v1/model-quantization-quotes`: a published package's run only; local code runs where it
/// was made ready.
pub(super) async fn quote(
    State(api): State<Arc<Api>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<RunQuote>, Refusal> {
    let ask: ModelQuantize = parse(&body?)?;
    if Path::new(&ask.package).is_absolute() {
        return Err(Refusal(Error::new(
            Kind::Usage,
            "quote_local",
            "local code runs where it was made ready; a `new` machine takes a published release",
        )));
    }
    let Planned::Run(request) = plan(&api, "quote", ask).await? else {
        unreachable!("a published package installs nothing first");
    };
    let body = serde_json::to_vec(&request).expect("a request serializes");
    crate::runs::quote(State(api), Ok(Bytes::from(body))).await
}

/// The run of `package`'s `quantize` that writes `lane` from the commit `model` names into the
/// commit's own model, with each calibration model it builds bound to that commit.
async fn plan(api: &Arc<Api>, key: &str, ask: ModelQuantize) -> Result<Planned, Refusal> {
    let hub = origin(api, ask.hub.as_deref())?;
    let lane = refs::label("--lane", &ask.lane).map_err(Refusal)?;
    let model = ModelRef::parse(&ask.model).map_err(Refusal)?;
    let target = match Path::new(&ask.package).is_absolute() {
        true => format!("{}/{RECIPES}", ask.package.trim_end_matches('/')),
        false => published(&ask.package)?,
    };
    let (package, interface, release) = match resolve(api, &hub, &target, None, &ask.on, false, key, ask.agreed).await.map_err(Refusal)? {
        Resolved::Release { package, card, .. } => (package, card.interface, card.release),
        Resolved::Local { local, interface: None, .. } => {
            return Ok(Planned::Install(Box::new(local.install.expect("an uninstalled local target is being installed"))));
        }
        Resolved::Local { local, interface: Some(interface), .. } => (local.package, interface, local.source.manifest),
    };
    let source = commit(api, &hub, &model, &lane).await?;
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
    // The commit goes to the source's own model, in the lane; local code's records its package as
    // `local/<name>` and its release as the source's commit: an unpublished recipe, exactly.
    let mut terms = vec![
        source.spelled.clone(),
        source.repo.to_string(),
        format!("lane={lane}"),
        format!("package={package}"),
        format!("release={release}"),
    ];
    terms.extend(calibration(&interface, &found, &source.spelled));
    Ok(Planned::Run(RunRequest {
        target,
        on: Some(ask.on),
        hub: hub.to_string(),
        terms,
        recipes: true,
        agreed: ask.agreed,
        ..Default::default()
    }))
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

/// The commit `model` names, as the model's card lists it (its org's members see every commit):
/// a commit by id, or a release's lane of original weights (its only lane when it has one). A
/// 4-bit lane is written only from original weights (owner, 2026-10-22): from a commit holding
/// none it would stack two rounds of 4-bit error.
async fn commit(api: &Api, hub: &Origin, model: &ModelRef, lane: &str) -> Result<Source, Refusal> {
    let (repo, release, id) = match model {
        ModelRef::Hub { repo, release } => (repo, release.as_deref(), None),
        ModelRef::Commit { repo, commit } => (repo, None, Some(commit.as_str())),
        ModelRef::Local(_) | ModelRef::Provider(_) => {
            return Err(Refusal(
                Error::new(Kind::Usage, "model_quantize_source", format!("{model} is not on the Hub: quantize reads a Hub model"))
                    .next(format!("cozy model upload {model} <org/name> --lane <lane>")),
            ));
        }
    };
    let card: ModelCard = (reader(api, hub)?.get_model(&repo.org, &repo.name).await).map_err(|e| refusal(hub, e))?;
    let absent =
        |what: String| Refusal(Error::not_found("model_commit_not_found", what).next(format!("cozy model info {repo}")));
    let (commit, header, kinds) = match id {
        Some(id) => {
            let found = (card.commits.iter().find(|c| c.id == id))
                .ok_or_else(|| absent(format!("{repo} has no commit {id} you may read")))?;
            let commit = ObjectRef { sha256: found.id.trim_start_matches("sha256:").into(), length: found.length as u64 };
            (commit, found.header.clone(), found.kinds.clone())
        }
        None => {
            let found = match release {
                Some(version) => card.releases.iter().find(|r| r.version == version),
                None => card.releases.first(),
            };
            let found = found.ok_or_else(|| absent(format!("{repo} has no release {}", release.unwrap_or("yet"))))?;
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
                            format!("{repo}@{} has lanes {lanes:?}: name the commit to quantize", found.version),
                        )
                        .next(format!("cozy model quantize {repo}@<commit> --lane {lane} --package <package>")),
                    ));
                }
            };
            let commit = ObjectRef { sha256: picked.commit.trim_start_matches("sha256:").into(), length: picked.length as u64 };
            (commit, picked.header.clone(), picked.kinds.clone())
        }
    };
    if matches!(lane, "nvfp4" | "w4a8" | "svdq-nvfp4") && !original(&kinds) {
        let message = format!(
            "{repo}@{} holds no original weights ({}): a {lane} lane from them stacks two rounds of 4-bit error",
            commit.id(),
            kinds.join(", ")
        );
        return Err(Refusal(Error::new(Kind::Usage, "model_quantize_source_quantized", message).remedy(format!(
            "upload the original weights (cozy model upload <path> {repo} --lane bf16) and quantize those"
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
    Ok(Source { spelled: format!("{repo}@{}", commit.id()), repo: repo.clone(), components })
}

/// `hub`'s distribution endpoint as this computer's signed-in user, trusting the private CA the
/// Hub names (a test Hub's).
async fn endpoint(api: &Api, hub: &Origin) -> Result<Endpoint, Refusal> {
    let client = signed_in(api, hub)?;
    let hub_client::Credential::User(session) = client.credential() else {
        unreachable!("a signed-in client");
    };
    let ca = (client.get_execution_environment().await).map_err(|e| refusal(hub, e))?.environment.tensorhub_ca_der_b64url;
    let token = (session.bearer(&client.anonymous()).await).map_err(|e| refusal(hub, e))?.token;
    let config = Config {
        token: Some(tensorfs::bearer(token)),
        ca_der: ca.and_then(|ca| URL_SAFE_NO_PAD.decode(ca.trim()).ok()),
        ..Config::from_env()
    };
    Endpoint::with_config(hub.as_str(), &config).map_err(|f| Refusal(Error::validation("endpoint_invalid", f.detail)))
}

/// The recipes of `lane` that read a commit holding `components`.
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
