//! `/v1/models`: the Hub's model catalog, and a model's family, releases and deletion as a member
//! of its org. Signed in, a search also lists the caller's own unreleased (private) models and a
//! model lists its commits to its org's members.
use std::{collections::BTreeSet, sync::Arc};

use futures_util::{StreamExt as _, stream};

use axum::{
    Json,
    body::Bytes,
    extract::{
        Path, Query, State,
        rejection::{BytesRejection, QueryRejection},
    },
};
use ::api::hub::types;
use api::{
    config::Origin,
    error::{Error, Kind},
    calcifer::hub::{
        At, Families, FoundModel, ModelAuthor, ModelCommit, ModelDeleted, ModelDetail, ModelEdit, ModelEdited,
        ModelLane, ModelRelease, ModelReleaseAsk, ModelReleased, ModelSearch, Provenance,
    },
    model_ref::{self as refs, Repo},
    names::shown,
};
use serde::Deserialize;

use super::{checkpoints, origin, reader, refusal, signed_in};
use crate::api::{Api, Refusal, parse};

#[derive(Deserialize)]
pub(super) struct Search {
    hub: Option<String>,
    /// Search text, or exactly `org/name`.
    q: Option<String>,
    family: Option<String>,
    /// Tags every row has, and tags none has, comma-separated.
    tag: Option<String>,
    no_tag: Option<String>,
    /// 1–100; 20 when absent.
    limit: Option<u32>,
    #[serde(default)]
    offset: u32,
    role: Option<String>,
}

/// A comma-separated list's items.
fn listed(text: Option<&str>) -> Vec<String> {
    text.unwrap_or_default().split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

/// A page of the Hub's search: its default most.
const PAGE: u32 = 200;
/// The most models a family's search reads for headers that say it with no label.
const READ_AT_MOST: u32 = 1000;
/// How many cards a search reads at once.
const CARDS: usize = 16;

/// `GET /v1/models`: one row per model, its card read for its newest release and lanes, all at once.
pub(super) async fn search(
    State(api): State<Arc<Api>>,
    query: Result<Query<Search>, QueryRejection>,
) -> Result<Json<ModelSearch>, Refusal> {
    let Query(ask) = query?;
    let hub = origin(&api, ask.hub.as_deref())?;
    let client = reader(&api, &hub)?;
    let limit = ask.limit.unwrap_or(20).clamp(1, 100);
    let wanted = ask.offset.saturating_add(limit);
    let role = ask.role.as_deref().map(str::trim).filter(|role| !role.is_empty());
    if role.is_some_and(|role| !matches!(role, "model" | "lora")) {
        return Err(Refusal(Error::new(Kind::Usage, "catalog_role_invalid", "role must be model or lora")));
    }
    let q = ask.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let family = ask.family.as_deref().map(str::trim).filter(|family| !family.is_empty());
    let upstream_paged = role.is_some() && family.is_none();
    // One word that is org/name is that model; anything else is search text.
    let tagged = ask.tag.as_deref().is_some_and(|t| !t.trim().is_empty()) || ask.no_tag.as_deref().is_some_and(|t| !t.trim().is_empty());
    let exact = match (family, q, tagged) {
        (None, Some(one), false) if one.contains('/') && !one.contains(char::is_whitespace) => Repo::parse(one).ok(),
        _ => None,
    };
    let (tags, no_tags) = (listed(ask.tag.as_deref()), listed(ask.no_tag.as_deref()));
    // With a family: what the Hub labels so, page by page, then the rest, whose header alone may say it (`checkpoints::said`).
    let (mut names, mut total, mut newest) = (Vec::new(), 0, None);
    if exact.is_none() {
        let mut held: BTreeSet<(String, String)> = BTreeSet::new();
        // A Hub that pages nothing (no `offset`) answers its first page again: what's new ends it.
        let mut take = |names: &mut Vec<types::ModelSummary>, models: Vec<types::ModelSummary>| {
            let before = names.len();
            names.extend(models.into_iter().filter(|m| held.insert((m.org.clone(), m.name.clone()))));
            names.len() - before
        };
        loop {
            let answer = (client.search_models_by_role(q, family, &tags, &no_tags, limit, if family.is_none() || upstream_paged { ask.offset } else { names.len() as u32 }, role).await).map_err(|e| refusal(&hub, e))?;
            if !role_page_matches(&answer, role) {
                return Err(role_unavailable());
            }
            total = answer.search.total;
            let read = take(&mut names, answer.models);
            if upstream_paged || family.is_none() || read == 0 || names.len() as u32 >= wanted || names.len() as i64 >= total {
                break;
            }
        }
        // Then every other model, the Hub's pages of them, up to READ_AT_MOST: one whose header alone says it has no label to find it by.
        if family.is_some() && !upstream_paged {
            newest = Some(names.len());
            let (mut seen, mut offset) = (BTreeSet::new(), 0);
            while offset < READ_AT_MOST {
                let answer = (client.search_models_by_role(q, None, &tags, &no_tags, PAGE, offset, role).await).map_err(|e| refusal(&hub, e))?;
                if !role_page_matches(&answer, role) { return Err(role_unavailable()); }
                let fresh = answer.models.iter().filter(|m| seen.insert((m.org.clone(), m.name.clone()))).count();
                offset += answer.models.len() as u32;
                take(&mut names, answer.models);
                if fresh == 0 || offset as i64 >= answer.search.total {
                    break;
                }
            }
        }
    }
    let cards = match &exact {
        Some(repo) => vec![client.get_model(&repo.org, &repo.name).await.map_err(|e| refusal(&hub, e))?],
        None => {
            let reads = names.into_iter().map(|model| {
                let client = client.clone();
                async move { client.get_model(&model.org, &model.name).await }
            });
            let cards: Vec<Result<types::ModelCard, _>> = stream::iter(reads).buffered(CARDS).collect().await;
            cards.into_iter().collect::<Result<Vec<_>, _>>().map_err(|e| refusal(&hub, e))?
        }
    };
    if exact.is_some() {
        total = 1;
    }
    let mut models: Vec<FoundModel> = cards.iter().map(found).collect();
    // A model named exactly is shown whatever its role: `model ls` of a LoRA's name finds it.
    let role = role.filter(|_| exact.is_none());
    if role.is_some() && cards.iter().any(|card| card.model.role.is_none()) {
        return Err(role_unavailable());
    }
    if role.is_none() || family.is_some() {
        checkpoints::said(&api, &hub, &cards, &mut models).await;
    }
    if let Some(family) = family.filter(|_| !upstream_paged) {
        let is = |row: &FoundModel| row.family.as_deref().is_some_and(|its| its.eq_ignore_ascii_case(family));
        let labelled = |card: &types::ModelCard| card.model.family.as_deref().is_some_and(|its| its.eq_ignore_ascii_case(family));
        // Of the rest, those its label doesn't already count.
        let from = newest.unwrap_or(cards.len());
        total += cards[from..].iter().zip(&models[from..]).filter(|(card, row)| is(*row) && !labelled(*card)).count() as i64;
        models.retain(is);

    }
    // A publication can change the card's role after the summary page was read.
    retain_role(&mut models, role);
    if exact.is_some() { total = models.len() as i64; }
    Ok(Json(page(models, total, ask.offset, limit, (upstream_paged || family.is_none()) && exact.is_none())))
}

fn retain_role(models: &mut Vec<FoundModel>, role: Option<&str>) {
    if let Some(role) = role { models.retain(|model| model.role.as_deref() == Some(role)); }
}

fn role_unavailable() -> Refusal {
    Refusal(Error::new(Kind::Unavailable, "catalog_role_unavailable", "This Hub cannot separate models and LoRAs yet")
        .remedy("Retry after the Hub catalog has been updated"))
}

fn role_page_matches(answer: &types::ModelSearch, role: Option<&str>) -> bool {
    role.is_none() || (answer.search.role.as_deref() == role && answer.models.iter().all(|model| model.role.as_deref() == role))
}

fn page(models: Vec<FoundModel>, total: i64, offset: u32, limit: u32, already_paged: bool) -> ModelSearch {
    let models = if already_paged { models } else { models.into_iter().skip(offset as usize).take(limit as usize).collect() };
    let older = (total - offset as i64 - models.len() as i64).max(0);
    ModelSearch { models, total, older }
}

#[cfg(test)]
mod pagination_tests {
    use super::*;

    fn rows(names: &[&str]) -> Vec<FoundModel> {
        names.iter().map(|name| FoundModel { model: (*name).into(), ..Default::default() }).collect()
    }

    #[test]
    fn changed_card_roles_cannot_leak_into_a_filtered_page() {
        let mut models = vec![
            FoundModel { model: "author/still-model".into(), role: Some("model".into()), ..Default::default() },
            FoundModel { model: "author/changed-to-lora".into(), role: Some("lora".into()), ..Default::default() },
        ];
        retain_role(&mut models, Some("model"));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "author/still-model");
    }

    #[test]
    fn hub_pages_are_not_offset_twice() {
        let page = page(rows(&["second", "third"]), 4, 1, 2, true);
        assert_eq!(page.models[0].model, "second");
        assert_eq!((page.total, page.older), (4, 1));
    }

    #[test]
    fn resolved_family_and_exact_matches_apply_offset() {
        let page = page(rows(&["first", "second", "third"]), 3, 1, 1, false);
        assert_eq!(page.models[0].model, "second");
        assert_eq!(page.older, 1);
        assert!(super::page(rows(&["exact"]), 1, 1, 20, false).models.is_empty());
    }

    #[test]
    fn empty_pages_need_the_hubs_role_acknowledgment() {
        let old: types::ModelSearch = serde_json::from_value(serde_json::json!({"models": [], "search": {"total": 5}})).unwrap();
        assert!(!role_page_matches(&old, Some("lora")));
        let current: types::ModelSearch = serde_json::from_value(serde_json::json!({"models": [], "search": {"total": 0, "role": "lora"}})).unwrap();
        assert!(role_page_matches(&current, Some("lora")));
        assert!(!role_page_matches(&current, Some("model")));
        assert!(role_page_matches(&old, None));
    }
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
        role: card.model.role.clone(),
        family: card.model.family.clone().filter(|f| !f.is_empty()),
        family_from: None,
        release: release.map(|r| r.version.clone()),
        lanes,
        private: release.is_none(),
        author: card.model.author.clone().map(author),
        kind: None,
        tags: card.model.tags.clone(),
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
    Ok(Json(detail(&api, &hub, &repo, ask.release).await?))
}

/// A model as its card and its header say: its releases (one, when named) and, to its members, its commits.
async fn detail(api: &Api, hub: &Origin, repo: &Repo, release: Option<String>) -> Result<ModelDetail, Refusal> {
    let card = (reader(api, hub)?.get_model(&repo.org, &repo.name).await).map_err(|e| refusal(hub, e))?;
    let facts = checkpoints::of_card(api, hub, &card).await;
    let (family, family_from) = checkpoints::family(card.model.family.clone(), facts.as_ref().and_then(|f| f.family.as_deref()));
    let releases: Vec<ModelRelease> = (card.releases.into_iter())
        .filter(|r| release.as_ref().is_none_or(|s| *s == r.version))
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
    if let Some(release) = release.filter(|_| releases.is_empty()) {
        return Err(Refusal(
            Error::not_found("model_release_not_found", format!("{repo} has no release {release:?}"))
                .next(format!("cozy model ls {repo}")),
        ));
    }
    let commits = (card.commits.into_iter())
        .map(|c| ModelCommit { commit: c.id, lane: c.lane, bytes: c.bytes, releases: c.releases, created_at: c.created_at })
        .collect();
    Ok(ModelDetail {
        model: repo.to_string(),
        role: card.model.role,
        family,
        family_from,
        weights_family: facts.as_ref().and_then(|f| f.family.clone()),
        kind: facts.as_ref().map(super::checkpoints::kind),
        created_at: card.model.created_at,
        source: card.model.source.filter(|s| !s.is_empty()),
        author: card.model.author.map(author),
        trigger: card.model.trigger.filter(|t| !t.is_empty()),
        strength: card.model.strength,
        tags: card.model.tags,
        releases,
        commits,
    })
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

/// `PATCH /v1/models/{org}/{name}`: what its owner sets. Its family (empty or `none` clears the label, and its weights' family
/// shows), its trigger words (empty clears them) and its recommended strength (null clears it); an absent field is kept.
pub(super) async fn edit(
    State(api): State<Arc<Api>>,
    Path((org, name)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<ModelEdited>, Refusal> {
    let edit: ModelEdit = parse(&body?)?;
    let hub = origin(&api, edit.hub.as_deref())?;
    let repo = Repo::hub(&format!("{org}/{name}")).map_err(Refusal)?;
    if edit.family.is_none() && edit.trigger.is_none() && edit.strength.is_none() && !edit.tags_change() {
        return Err(Refusal(Error::new(Kind::Usage, "model_edit_empty", "name what changes: family, tags, trigger or strength")));
    }
    let client = signed_in(&api, &hub)?;
    let mut warnings = Vec::new();
    if edit.family.is_some() || edit.tags_change() {
        let family = edit.family.as_deref().map(str::trim).map(|f| (if f.eq_ignore_ascii_case("none") { "" } else { f }).to_owned());
        let update = types::ModelUpdate { family, tags: None, add_tags: edit.add_tags, remove_tags: edit.remove_tags };
        let answer = (client.update_model(&repo.org, &repo.name, &update).await).map_err(|e| refusal(&hub, e))?;
        warnings = answer.warnings;
    }
    if edit.trigger.is_some() || edit.strength.is_some() {
        let answer = (client.set_model_display(&repo.org, &repo.name, edit.trigger.as_deref(), edit.strength).await).map_err(|e| refusal(&hub, e))?;
        warnings.extend(answer.warnings);
    }
    Ok(Json(ModelEdited { model: detail(&api, &hub, &repo, None).await?, warnings }))
}

/// `GET /v1/families`: the families TensorFS tells from a header, for an owner to pick from.
pub(super) async fn families() -> Json<Families> {
    Json(Families { families: tensorfs::family::families().iter().map(|f| f.name.clone()).collect() })
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
        warnings: release.warnings,
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
