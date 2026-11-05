//! `/v1/projects/{id}/assets`: what a project holds, in the page's words. A tool dropped on the canvas, its inputs set one by one, gone
//! and back. Each is one call the CLI sees at once, and `assetfile` alone knows how the file says it.
use std::{cell::RefCell, collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode, header},
};
use serde::{Deserialize, Serialize};
use protocol::{
    error::{Error, Kind},
    project::{NewAsset, AssetBack, AssetDeleted, AssetMade, AssetPatch, AssetPatched, AssetVersions, Version as VersionRow},
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use super::{Doc, IMPORT_KEPT_MS, canvas, cancelled, deleted_why, find, folder, format, graph::name_key, listed, assetfile, model, now_ms};
use crate::api::{Api, Refusal, parse};

/// What to call an asset with no name asked: the tool's last word, with no release.
fn tool_name(tool: &str) -> Option<String> {
    format::plain_name(tool.rsplit('/').next()?.split('@').next()?)
}

fn own_lines(name: &str) -> Error {
    Error::new(Kind::Usage, "project_edit", format!("{name} isn't under `assets:` in {}", folder::PROJECT)).next("cozy project status")
}

/// `POST /v1/projects/{id}/assets`: an asset from a tool, and with `at` its row, in one call. The name goes in the lock first, so a retry
/// under the same key, whatever it finds written, ends with this one asset.
pub(super) async fn create(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<AssetMade>), Refusal> {
    let body = body?;
    let asked: NewAsset = parse(&body)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let key = headers.get("idempotency-key").and_then(|key| key.to_str().ok()).map(|key| (key.to_owned(), hex::encode(Sha256::digest(&body))));
    // With no tool, it's a text asset.
    let kind = match asked.tool.is_empty() {
        true if !asked.fields.contains_key(format::TEXT) => return Err(Refusal(Error::new(Kind::Validation, "asset_says_nothing", "an asset needs a tool, or a text").remedy("name a tool from GET /v1/tools, or set `fields.text`"))),
        true => None,
        false => Some(crate::toolbox::kind(&api, &asked.tool, &folder.dir).await.map_err(Refusal)?),
    };
    let _one = project.importing.lock().expect("importing lock");
    let lock = folder.lock().map_err(Refusal)?;
    let told = key.as_ref().and_then(|(key, _)| lock.assets_made.get(key));
    if let (Some(told), Some((key, digest))) = (told, &key)
        && told.digest != *digest
    {
        return Err(Refusal(Error::new(Kind::Conflict, "idempotency_conflict", format!("Idempotency-Key {key} asked another asset already"))));
    }
    let name = match told {
        Some(told) => told.name.clone(),
        None => {
            let wanted = asked.name.as_deref().and_then(format::plain_name).or_else(|| tool_name(&asked.tool)).unwrap_or_else(|| kind.as_ref().map_or(format::TEXT, |_| "asset").to_owned());
            let doc = Doc::load(folder).map_err(Refusal)?;
            if kind.is_none() {
                format::refuse_tag(&lock, &wanted).map_err(Refusal)?;
            }
            let free = format::free_name(&doc, &lock, &wanted);
            if asked.exact && free != wanted {
                return Err(Refusal(format::name_taken(&doc, &wanted)));
            }
            free
        }
    };
    if let (None, Some((key, digest))) = (told, &key) {
        let at_ms = now_ms();
        (folder.update_lock(|lock| {
            lock.assets_made.retain(|_, row| at_ms - row.at_ms < IMPORT_KEPT_MS);
            lock.assets_made.insert(key.clone(), folder::KeyedAsset { digest: digest.clone(), at_ms, name: name.clone() });
        }))
        .map_err(Refusal)?;
    }
    // Its row before its lines: a page that reads the name has somewhere to put it, and places nothing of its own.
    if let Some(card) = asked.at {
        canvas::set(folder, &id, BTreeMap::from([(name.clone(), Some(card))])).map_err(Refusal)?;
    }
    (folder.edit_project(|text| match assetfile::etag(text, &name) {
        Some(_) => Ok(text.to_owned()),
        None => assetfile::added(text, &name, &asked.tool, &asked.fields).map(|(edited, _)| edited),
    }))
    .map_err(Refusal)?;
    // What it outputs: what `defaults` fill of it before it has a version.
    if let Some(kind) = &kind {
        let plain = format::plain_tool(&asked.tool, folder);
        folder.update_lock(|lock| drop(lock.makes.insert(plain.clone(), kind.clone()))).map_err(Refusal)?;
    }
    let etag = assetfile::etag(&folder.text().map_err(Refusal)?, &name).ok_or_else(|| Refusal(own_lines(&name)))?;
    Ok((StatusCode::CREATED, Json(AssetMade { name, etag })))
}

/// `GET /v1/projects/{id}/assets/{name}`: the asset and each of its versions.
pub(super) async fn read(State(api): State<Arc<Api>>, Path((id, name)): Path<(String, String)>) -> Result<Json<AssetVersions>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let model = model::of(&api, &project).await.map_err(Refusal)?;
    let (doc, lock) = (&model.doc, &model.lock);
    let i = find(doc, &name).map_err(Refusal)?;
    let asset = model.row(&project, i);
    let pinned = asset.approved;
    let versions = (lock.asset(&asset.name).into_iter().flat_map(|held| &held.versions))
        .map(|version| VersionRow {
            number: version.number,
            files: listed(lock, &version.files),
            made_by: version.tool.clone().or_else(|| asset.tool.clone()),
            seed: version.seed,
            runs: version.run.into_iter().collect(),
            approved: Some(version.number) == pinned,
            warnings: version.warnings.clone(),
            kept: version.kept,
            execution_ms: (version.execution_ms > 0).then_some(version.execution_ms),
        })
        .collect();
    Ok(Json(AssetVersions { asset, versions }))
}

/// `PATCH /v1/projects/{id}/assets/{name}`: its inputs set over what the file holds this moment. With `If-Match`, only if it is still
/// what that read: an edit of another asset never conflicts. A render going keeps the text it was asked with.
pub(super) async fn patch(
    State(api): State<Arc<Api>>,
    Path((id, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<AssetPatched>, Refusal> {
    let asked: AssetPatch = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let mut fields = asked.fields;
    if let Some(tool) = asked.tool {
        fields.insert(assetfile::TOOL.to_owned(), Value::String(tool));
    }
    let read = headers.get(header::IF_MATCH).and_then(|value| value.to_str().ok());
    let told = RefCell::new((String::new(), vec![]));
    let lock = folder.lock().map_err(Refusal)?;
    (folder.edit_project(|text| {
        let was = assetfile::etag(text, &name).ok_or_else(|| own_lines(&name))?;
        if read.is_some_and(|read| read != was) {
            return Err(Error::new(Kind::Conflict, "asset_changed", format!("{name} changed since it was read")).remedy("read it again and make the edit on what it holds"));
        }
        let (edited, warnings) = assetfile::set(text, &name, &fields)?;
        Doc::of(folder, &edited, &lock)?;
        *told.borrow_mut() = (was, warnings);
        Ok(edited)
    }))
    .map_err(Refusal)?;
    let (was, warnings) = told.into_inner();
    let etag = assetfile::etag(&folder.text().map_err(Refusal)?, &name).ok_or_else(|| Refusal(own_lines(&name)))?;
    Ok(Json(AssetPatched { name, etag, was, warnings }))
}

/// What `DELETE` answers, for `PUT` to put back: its lines and what project.lock held of it.
#[derive(Serialize, Deserialize)]
struct Deleted {
    lines: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    held: Option<folder::Asset>,
}

/// `DELETE /v1/projects/{id}/assets/{name}`: it, its versions and its row gone, at once, and what it made stands. Its files wait, unnamed, for
/// `reclaim`, and what this answers puts it all back. Its queued work is dropped and its runs canceled first.
pub(super) async fn delete(State(api): State<Arc<Api>>, Path((id, name)): Path<(String, String)>) -> Result<Json<AssetDeleted>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let doc = Doc::load(folder).map_err(Refusal)?;
    let i = doc.find(&name).ok_or_else(|| Refusal(Error::new(Kind::NotFound, "asset_unknown", format!("the project has nothing named {name}")).next("cozy project status")))?;
    let name = doc.assets[i].name.clone();
    let at = canvas::row(folder, &name).map_err(Refusal)?;
    let lines = RefCell::new(String::new());
    (folder.edit_project(|text| {
        let (edited, back) = assetfile::removed(text, &name)?;
        *lines.borrow_mut() = back;
        Ok(edited)
    }))
    .map_err(Refusal)?;
    // Refused, nothing stops. Gone from the file, what lands of it lands nothing.
    cancelled(&api, &project, std::slice::from_ref(&name), &deleted_why)?;
    let held = RefCell::new(None);
    folder.update_lock(|lock| *held.borrow_mut() = lock.assets.remove(&name_key(&name))).map_err(Refusal)?;
    canvas::set(folder, &id, BTreeMap::from([(name.clone(), None)])).map_err(Refusal)?;
    project.walker.lock().expect("walker lock").readings.values_mut().for_each(|reading| reading.moved(&name));
    project.asked.notify_one();
    let text = serde_json::to_string(&Deleted { lines: lines.into_inner(), held: held.into_inner() }).expect("what's deleted serializes");
    Ok(Json(AssetDeleted { name, text, at }))
}

fn taken(name: &str) -> Error {
    Error::new(Kind::Conflict, "asset_exists", format!("the project has a {name} again, so this one stays deleted")).remedy(format!("rename that one, then put this one back, or edit {}", folder::PROJECT)).next(format!("cozy project rename {name} <another name>"))
}

/// `PUT /v1/projects/{id}/assets/{name}`: what `DELETE` answered, back under its name, its versions with it. Taken since by a hand in the file,
/// it's refused and says so.
pub(super) async fn put_back(
    State(api): State<Arc<Api>>,
    Path((id, name)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<AssetMade>, Refusal> {
    let asked: AssetBack = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let back: Deleted = serde_json::from_str(&asked.text).map_err(|_| Refusal(Error::new(Kind::Validation, "asset_text", format!("that isn't what deleting {name} answered"))))?;
    if Doc::load(folder).map_err(Refusal)?.find(&name).is_some() {
        return Err(Refusal(taken(&name)));
    }
    // Its row before its lines, as a new asset's.
    if let Some(card) = asked.at {
        canvas::set(folder, &id, BTreeMap::from([(name.clone(), Some(card))])).map_err(Refusal)?;
    }
    (folder.edit_project(|text| {
        if Doc::read(folder, text)?.find(&name).is_some() {
            return Err(taken(&name));
        }
        assetfile::restored(text, &name, &back.lines)
    }))
    .map_err(Refusal)?;
    if let Some(held) = &back.held {
        folder.update_lock(|lock| drop(lock.assets.insert(name_key(&name), held.clone()))).map_err(Refusal)?;
        project.walker.lock().expect("walker lock").readings.values_mut().for_each(|reading| reading.moved(&name));
    }
    let etag = assetfile::etag(&folder.text().map_err(Refusal)?, &name).ok_or_else(|| Refusal(own_lines(&name)))?;
    Ok(Json(AssetMade { name, etag }))
}
