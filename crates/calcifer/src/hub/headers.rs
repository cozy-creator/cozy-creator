//! What a model's checkpoints hold, from their headers alone (`cozy model show`, xs-183): each lane's components with their
//! classes, configs, run data and tensor counts, and what the header records of where it came from. Only header objects are
//! read, never a weight. [`lanes`] and [`header`] are how Calcifer reads the headers a reference names, lane by lane.
use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::{Path, Query, State, rejection::QueryRejection},
};
use futures_util::{StreamExt as _, stream};
use ::api::hub::types;
use api::{
    calcifer::hub::{ComponentHeader, LaneHeader, LoraModules, ModelHeaders},
    error::{Error, Kind},
    model_ref::{ModelId, ReleaseVisibility},
};
use serde::Deserialize;
use tensorfs::{
    Endpoint,
    family::Facts,
    format::{ClassRef, Component, Header, ObjectRef},
};

use super::{checkpoints, origin, quantize::distribution, reader, refusal};
use crate::api::{Api, Refusal};

/// Header metadata that says where a checkpoint came from.
const RECORDS: [&str; 6] = ["inherited", "composed", "added", "provenance", "family", "adapter"];
/// How many headers one ask reads at once.
const READS: usize = 4;

#[derive(Deserialize)]
pub(super) struct Ask {
    hub: Option<String>,
    release: Option<String>,
    /// `sha256:<hex>` of a checkpoint's tree.
    checkpoint: Option<String>,
    lane: Option<String>,
    /// Each component's config itself, not only its keys.
    #[serde(default)]
    full: bool,
}

/// One lane's checkpoint on a card.
pub(crate) struct Lane {
    pub(crate) lane: String,
    /// `sha256:<hex>` of its tree.
    pub(crate) checkpoint: String,
    pub(crate) header: types::ObjectRef,
}

/// The lanes `card` holds of a release (none named: its newest public one, else the newest its reader sees, else to its members
/// each lane's newest checkpoint) or of one checkpoint, and the release; `lane` keeps one of them.
pub(crate) fn lanes<'c>(
    card: &'c types::ModelCard,
    model: &ModelId,
    release: Option<&str>,
    checkpoint: Option<&str>,
    lane: Option<&str>,
) -> Result<(Option<&'c types::ModelRelease>, Vec<Lane>), Error> {
    let of = |release: &types::ModelRelease| -> Vec<Lane> {
        (release.lanes.iter())
            .map(|(lane, held)| Lane { lane: lane.clone(), checkpoint: held.checkpoint.clone(), header: held.header.clone() })
            .collect()
    };
    let (named, mut found) = match (release, checkpoint) {
        (_, Some(checkpoint)) => {
            let released = card.releases.iter().find_map(|r| Some((r, r.lanes.iter().find(|(_, held)| held.checkpoint == checkpoint)?)));
            match released {
                Some((release, (lane, held))) => {
                    (Some(release), vec![Lane { lane: lane.clone(), checkpoint: held.checkpoint.clone(), header: held.header.clone() }])
                }
                None => match card.checkpoints.iter().find(|held| held.id == checkpoint) {
                    Some(held) => (None, vec![Lane { lane: held.lane.clone(), checkpoint: held.id.clone(), header: held.header.clone() }]),
                    None => {
                        return Err(Error::not_found("model_checkpoint_not_found", format!("{model} has no checkpoint {checkpoint} you can read"))
                            .next(format!("cozy model show {model}")));
                    }
                },
            }
        }
        (Some(version), None) => match card.releases.iter().find(|r| r.version == version) {
            Some(release) => (Some(release), of(release)),
            None => {
                return Err(Error::not_found("model_release_not_found", format!("{model} has no release {version:?}"))
                    .next(format!("cozy model ls {model}")));
            }
        },
        (None, None) => {
            let newest = (card.releases.iter().find(|r| r.visibility == ReleaseVisibility::Public)).or(card.releases.first());
            match newest {
                Some(release) => (Some(release), of(release)),
                None => {
                    let mut seen = std::collections::BTreeSet::new();
                    let held: Vec<Lane> = (card.checkpoints.iter())
                        .filter(|held| seen.insert(held.lane.clone()))
                        .map(|held| Lane { lane: held.lane.clone(), checkpoint: held.id.clone(), header: held.header.clone() })
                        .collect();
                    if held.is_empty() {
                        return Err(Error::not_found("model_unreleased", format!("{model} has no release or checkpoint you can read"))
                            .next(format!("cozy model ls {model}")));
                    }
                    (None, held)
                }
            }
        }
    };
    if let Some(lane) = lane {
        let had: Vec<String> = found.iter().map(|held| held.lane.clone()).collect();
        found.retain(|held| held.lane == lane);
        if found.is_empty() {
            let shown = named.map_or_else(|| model.to_string(), |r| format!("{model}@{}", r.version));
            return Err(Error::not_found("model_lane_not_found", format!("{shown} has no lane {lane}; it has {}", had.join(", ")))
                .next(format!("cozy model show {shown}")));
        }
    }
    Ok((named, found))
}

/// A checkpoint's header, read whole and verified.
pub(crate) async fn header(endpoint: &Endpoint, object: &types::ObjectRef) -> Result<Header, Error> {
    let object = ObjectRef { sha256: object.sha256.clone(), length: object.length.max(0) as u64 };
    let bytes = (endpoint.document(&object).await)
        .map_err(|why| Error::unavailable("header_unreadable", format!("header {}: {}", object.id(), why.detail)))?;
    Header::from_cbor(&bytes).map_err(|why| Error::new(Kind::Unavailable, "header_invalid", format!("header {}: {why}", object.id())).next("cozy update"))
}

fn class(class: &ClassRef) -> [String; 3] {
    [class.library.clone(), class.version.clone(), class.name.clone()]
}

/// A LoRA component's modules: each key path with an A and a B factor (PEFT's or kohya's spelling), and its `.alpha`.
fn lora(component: &Component) -> Option<LoraModules> {
    let mut modules: BTreeMap<&str, [bool; 3]> = BTreeMap::new();
    for tensor in &component.tensors {
        let key = tensor.key.strip_prefix("base_model.model.").unwrap_or(&tensor.key);
        let found = [(".lora_A.weight", 0), (".lora_down.weight", 0), (".lora_B.weight", 1), (".lora_up.weight", 1), (".alpha", 2)]
            .into_iter()
            .find_map(|(suffix, at)| Some((key.strip_suffix(suffix).filter(|path| !path.is_empty())?, at)));
        if let Some((path, at)) = found {
            modules.entry(path).or_default()[at] = true;
        }
    }
    let paired: Vec<&[bool; 3]> = modules.values().filter(|held| held[0] && held[1]).collect();
    (!paired.is_empty()).then(|| LoraModules {
        modules: paired.len(),
        alphas: paired.iter().filter(|held| held[2]).count(),
        lora_alpha: component.config.get("lora_alpha").and_then(serde_json::Value::as_f64),
    })
}

fn component(component: &Component, full: bool) -> ComponentHeader {
    let mut config_keys: Vec<String> = component.config.keys().cloned().collect();
    config_keys.sort();
    ComponentHeader {
        name: component.name.clone(),
        class: class(&component.class),
        config_file: component.config_file.clone(),
        config_keys,
        config: full.then(|| component.config.clone()).filter(|config| !config.is_empty()),
        assets: component.assets.iter().map(|(path, object)| (path.clone(), object.length)).collect(),
        tensors: component.tensors.len(),
        lora: lora(component),
    }
}

/// What `header`, `lane`'s, holds.
pub(crate) fn shown(lane: &Lane, header: &Header, full: bool) -> LaneHeader {
    let facts = Facts::of(header);
    LaneHeader {
        lane: lane.lane.clone(),
        checkpoint: lane.checkpoint.clone(),
        header: format!("sha256:{}", lane.header.sha256),
        header_bytes: lane.header.length.max(0) as u64,
        family: facts.as_ref().and_then(|facts| facts.family.clone()),
        kind: facts.as_ref().map(checkpoints::content_kind),
        pipeline: header.pipeline().map(|pipeline| class(&pipeline.class)),
        components: header.components().iter().map(|held| component(held, full)).collect(),
        records: (RECORDS.iter())
            .filter_map(|key| Some(((*key).to_owned(), serde_json::to_value(header.metadata.get(*key)?).ok()?)))
            .collect(),
    }
}

/// `GET /v1/models/{org}/{name}/headers`: what each lane of a release, or one checkpoint, holds.
pub(super) async fn show(
    State(api): State<Arc<Api>>,
    Path((org, name)): Path<(String, String)>,
    query: Result<Query<Ask>, QueryRejection>,
) -> Result<Json<ModelHeaders>, Refusal> {
    let Query(ask) = query?;
    let hub = origin(&api, ask.hub.as_deref())?;
    let model = ModelId::parse(&format!("{org}/{name}")).map_err(Refusal)?;
    let client = reader(&api, &hub)?;
    let card = (client.get_model(&model.org, &model.name).await).map_err(|e| refusal(&hub, e))?;
    let (release, lanes) = lanes(&card, &model, ask.release.as_deref(), ask.checkpoint.as_deref(), ask.lane.as_deref()).map_err(Refusal)?;
    let endpoint = distribution(&client, &hub).await?;
    let (endpoint, full) = (&endpoint, ask.full);
    let reads = lanes.into_iter().map(|lane| async move { header(endpoint, &lane.header).await.map(|held| shown(&lane, &held, full)) });
    let read: Vec<Result<LaneHeader, Error>> = stream::iter(reads).buffered(READS).collect().await;
    let lanes = read.into_iter().collect::<Result<Vec<_>, _>>().map_err(Refusal)?;
    let public = card.releases.iter().filter(|r| r.visibility == ReleaseVisibility::Public).map(|r| r.version.as_str());
    Ok(Json(ModelHeaders {
        model: model.to_string(),
        family: checkpoints::family(card.model.family.clone(), lanes.first().and_then(|lane| lane.family.as_deref())).0,
        release: release.map(|r| r.version.clone()),
        visibility: release.map(|r| r.visibility),
        resolves_to: release.and_then(|r| api::release::newest_fix(&r.version, public, false)).map(str::to_owned),
        lanes,
    }))
}
