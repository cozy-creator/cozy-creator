//! `/v1/models`: the Hub's model catalog, and a model's family, releases and deletion as a member
//! of its org. Signed in, a search also lists the caller's own unreleased (private) models and a
//! model lists its commits to its org's members.
use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    body::Bytes,
    extract::{
        Path, Query, State,
        rejection::{BytesRejection, QueryRejection},
    },
};
use hub_client::types;
use protocol::{
    error::{Error, Kind},
    hub::{
        At, FoundModel, ModelAuthor, ModelCommit, ModelDeleted, ModelDetail, ModelEdit, ModelFamily, ModelLane,
        ModelRelease, ModelReleaseAsk, ModelReleased, ModelSearch, Provenance,
    },
    model_ref::{self as refs, Repo},
    names::shown,
};
use serde::Deserialize;

use super::{origin, reader, refusal, signed_in};
use crate::api::{Api, Refusal, parse};

#[derive(Deserialize)]
pub(super) struct Search {
    hub: Option<String>,
    /// Search text, or exactly `org/name`.
    q: Option<String>,
    family: Option<String>,
    /// 1–100; 20 when absent.
    limit: Option<u32>,
}

/// `GET /v1/models`: one row per model, its card read for its newest release and lanes, all at once.
pub(super) async fn search(
    State(api): State<Arc<Api>>,
    query: Result<Query<Search>, QueryRejection>,
) -> Result<Json<ModelSearch>, Refusal> {
    let Query(ask) = query?;
    let hub = origin(&api, ask.hub.as_deref())?;
    let client = reader(&api, &hub)?;
    let limit = ask.limit.unwrap_or(20).clamp(1, 100);
    let q = ask.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    // One word that is org/name is that model; anything else is search text.
    let exact = match (&ask.family, q) {
        (None, Some(one)) if one.contains('/') && !one.contains(char::is_whitespace) => Repo::parse(one).ok(),
        _ => None,
    };
    let (cards, total) = match exact {
        Some(repo) => (vec![client.get_model(&repo.org, &repo.name).await.map_err(|e| refusal(&hub, e))?], 1),
        None => {
            let answer = (client.search_models(q, ask.family.as_deref(), limit).await).map_err(|e| refusal(&hub, e))?;
            let mut reads = tokio::task::JoinSet::new();
            for (i, model) in answer.models.into_iter().take(limit as usize).enumerate() {
                let client = client.clone();
                reads.spawn(async move { (i, client.get_model(&model.org, &model.name).await) });
            }
            let mut cards = BTreeMap::new();
            while let Some(read) = reads.join_next().await {
                let (i, card) = read.map_err(|e| Refusal(Error::internal("card_read", e.to_string())))?;
                cards.insert(i, card.map_err(|e| refusal(&hub, e))?);
            }
            (cards.into_values().collect(), answer.search.total)
        }
    };
    let models: Vec<FoundModel> = cards.iter().map(found).collect();
    let older = (total - models.len() as i64).max(0);
    Ok(Json(ModelSearch { models, total, older }))
}

/// One search row: the model's newest release and its lanes; for a private model (no release, its
/// owner's), its commits' lanes.
fn found(card: &types::ModelCard) -> FoundModel {
    // The Hub lists releases newest first.
    let release = card.releases.first();
    let lanes = match release {
        Some(r) => r.lanes.keys().cloned().collect(),
        None => (card.commits.iter().map(|c| c.lane.clone()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
    };
    FoundModel {
        model: shown(&format!("{}/{}", card.model.org, card.model.name)),
        family: card.model.family.clone().filter(|f| !f.is_empty()),
        release: release.map(|r| r.version.clone()),
        lanes,
        private: release.is_none(),
        author: card.model.author.clone().map(author),
    }
}

pub(super) fn author(author: types::ModelAuthor) -> ModelAuthor {
    ModelAuthor { org: author.org, name: author.name, parked: author.parked, reserved: author.reserved }
}

#[derive(Deserialize)]
pub(super) struct Info {
    hub: Option<String>,
    /// One release, by version.
    release: Option<String>,
}

/// `GET /v1/models/{org}/{name}`.
pub(super) async fn info(
    State(api): State<Arc<Api>>,
    Path((org, name)): Path<(String, String)>,
    query: Result<Query<Info>, QueryRejection>,
) -> Result<Json<ModelDetail>, Refusal> {
    let Query(ask) = query?;
    let hub = origin(&api, ask.hub.as_deref())?;
    let repo = Repo::parse(&format!("{org}/{name}")).map_err(Refusal)?;
    let card = (reader(&api, &hub)?.get_model(&repo.org, &repo.name).await).map_err(|e| refusal(&hub, e))?;
    let releases: Vec<ModelRelease> = (card.releases.into_iter())
        .filter(|r| ask.release.as_ref().is_none_or(|s| *s == r.version))
        .map(|r| ModelRelease {
            version: r.version,
            created_at: r.created_at,
            mirror: r.mirror,
            by_admin: r.by_admin,
            lanes: (r.lanes.into_iter())
                .map(|(lane, l)| ModelLane {
                    lane,
                    commit: l.commit,
                    bytes: l.bytes,
                    kinds: l.kinds,
                    provenance: l.provenance.map(provenance),
                })
                .collect(),
        })
        .collect();
    if let Some(release) = ask.release.filter(|_| releases.is_empty()) {
        return Err(Refusal(
            Error::not_found("model_release_not_found", format!("{repo} has no release {release:?}"))
                .next(format!("cozy model info {repo}")),
        ));
    }
    let commits = (card.commits.into_iter())
        .map(|c| ModelCommit { commit: c.id, lane: c.lane, bytes: c.bytes, releases: c.releases, created_at: c.created_at })
        .collect();
    Ok(Json(ModelDetail {
        model: repo.to_string(),
        family: card.model.family.filter(|f| !f.is_empty()),
        created_at: card.model.created_at,
        source: card.model.source.filter(|s| !s.is_empty()),
        author: card.model.author.map(author),
        trigger: card.model.trigger.filter(|t| !t.is_empty()),
        strength: card.model.strength,
        releases,
        commits,
    }))
}

fn provenance(p: types::ModelLaneProvenance) -> Provenance {
    Provenance {
        package: p.package,
        release: p.release,
        recipe: p.recipe,
        fingerprint: p.fingerprint,
        source: p.source,
        plan: p.plan,
    }
}

/// `PATCH /v1/models/{org}/{name}`: its discovery family, set or (empty) cleared.
pub(super) async fn edit(
    State(api): State<Arc<Api>>,
    Path((org, name)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<ModelFamily>, Refusal> {
    let ModelEdit { hub, family } = parse(&body?)?;
    let hub = origin(&api, hub.as_deref())?;
    let repo = Repo::hub(&format!("{org}/{name}")).map_err(Refusal)?;
    let Some(family) = family else {
        return Err(Refusal(Error::new(Kind::Usage, "model_edit_empty", "name what changes: family")));
    };
    let client = signed_in(&api, &hub)?;
    let answer = (client.set_model_family(&repo.org, &repo.name, &family).await).map_err(|e| refusal(&hub, e))?;
    Ok(Json(ModelFamily { model: repo.to_string(), family: answer.model.family.filter(|f| !f.is_empty()) }))
}

/// `POST /v1/models/{org}/{name}/releases`.
pub(super) async fn release(
    State(api): State<Arc<Api>>,
    Path((org, name)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<ModelReleased>, Refusal> {
    let ModelReleaseAsk { hub, version, commits } = parse(&body?)?;
    let hub = origin(&api, hub.as_deref())?;
    let repo = Repo::hub(&format!("{org}/{name}")).map_err(Refusal)?;
    let version = refs::label("version", &version).map_err(Refusal)?;
    let commits = (commits.iter().map(|c| refs::commit(c)).collect::<Result<Vec<_>, _>>()).map_err(Refusal)?;
    let client = signed_in(&api, &hub)?;
    let release = (client.publish_model_release(&repo.org, &repo.name, &version, &commits).await)
        .map_err(|e| refusal(&hub, e))?;
    Ok(Json(ModelReleased {
        model: repo.to_string(),
        version: release.version,
        created_at: release.created_at,
        lanes: release.lanes.into_iter().map(|(lane, l)| (lane, l.commit)).collect(),
    }))
}

/// `DELETE /v1/models/{org}/{name}`: the whole model.
pub(super) async fn delete(
    State(api): State<Arc<Api>>,
    Path((org, name)): Path<(String, String)>,
    query: Result<Query<At>, QueryRejection>,
) -> Result<Json<ModelDeleted>, Refusal> {
    deleted(&api, query?.0, &org, &name, None, None).await
}

/// `DELETE /v1/models/{org}/{name}/releases/{version}`: its version is never published again.
pub(super) async fn delete_release(
    State(api): State<Arc<Api>>,
    Path((org, name, version)): Path<(String, String, String)>,
    query: Result<Query<At>, QueryRejection>,
) -> Result<Json<ModelDeleted>, Refusal> {
    deleted(&api, query?.0, &org, &name, Some(version), None).await
}

/// `DELETE /v1/models/{org}/{name}/commits/{commit}`: a commit no release names.
pub(super) async fn delete_commit(
    State(api): State<Arc<Api>>,
    Path((org, name, commit)): Path<(String, String, String)>,
    query: Result<Query<At>, QueryRejection>,
) -> Result<Json<ModelDeleted>, Refusal> {
    let commit = refs::commit(&commit).map_err(Refusal)?;
    deleted(&api, query?.0, &org, &name, None, Some(commit)).await
}

async fn deleted(
    api: &Api,
    at: At,
    org: &str,
    name: &str,
    release: Option<String>,
    commit: Option<String>,
) -> Result<Json<ModelDeleted>, Refusal> {
    let hub = origin(api, at.hub.as_deref())?;
    let repo = Repo::hub(&format!("{org}/{name}")).map_err(Refusal)?;
    let client = signed_in(api, &hub)?;
    let gone = match (&release, &commit) {
        (Some(version), _) => client.delete_model_release(&repo.org, &repo.name, version).await,
        (None, Some(id)) => client.delete_model_commit(&repo.org, &repo.name, id).await,
        (None, None) => client.delete_model(&repo.org, &repo.name).await,
    };
    gone.map_err(|e| refusal(&hub, e))?;
    Ok(Json(ModelDeleted { model: repo.to_string(), release, commit }))
}
