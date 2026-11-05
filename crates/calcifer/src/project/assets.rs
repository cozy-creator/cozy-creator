//! `/v1/projects/{id}/assets`: what a project holds, in the page's words. A tool dropped on the canvas, its inputs set one by one, gone
//! and back. Each is one call the CLI sees at once, and `assetfile` alone knows how the file says it.
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode, header},
};
use serde::{Deserialize, Serialize};
use protocol::{
    error::{Error, Kind},
    project::{NewAsset, AssetBack, AssetDeleted, AssetMade, AssetPatch, AssetPatched, AssetVersions, Version as VersionRow, VersionBack, VersionMoved},
    tool,
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use super::{
    Doc, IMPORT_KEPT_MS, Project, Tool, Walk, assetfile, canvas, cancelled, deleted_why, find, folder, format, graph::name_key, listed, model, noun, now_ms, pin_in_file, stale_names,
};
use crate::api::{Api, Refusal, parse};

/// What to call an asset with no name asked: the tool's last word, with no release.
fn tool_name(tool: &str) -> Option<String> {
    format::plain_name(tool.rsplit('/').next()?.split('@').next()?)
}

fn own_lines(name: &str) -> Error {
    Error::new(Kind::Usage, "project_edit", format!("{name} isn't under `assets:` in {}", folder::PROJECT)).next("cozy project status")
}

/// `POST /v1/projects/{id}/assets`: an asset from a tool, and with `at` its row, in one call. The name goes in the lock first, so a retry
/// under the same key, whatever it finds written, ends with this one asset. It reads no network: what the tool makes comes with the ask or is
/// held already, and if neither, it's worked out behind the answer, and a render would learn it anyway.
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
    let text_asset = asked.tool.is_empty();
    if text_asset && !asked.fields.contains_key(format::TEXT) {
        return Err(Refusal(Error::new(Kind::Validation, "asset_says_nothing", "an asset needs a tool, or a text").remedy("name a tool from GET /v1/tools, or set `fields.text`")));
    }
    let plain = (!text_asset).then(|| format::plain_tool(&asked.tool, folder));
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
            let wanted = asked.name.as_deref().and_then(format::plain_name).or_else(|| tool_name(&asked.tool)).unwrap_or_else(|| if text_asset { format::TEXT } else { "asset" }.to_owned());
            let doc = Doc::load(folder).map_err(Refusal)?;
            if text_asset {
                format::refuse_tag(&lock, &wanted).map_err(Refusal)?;
            }
            let free = format::free_name(&doc, &lock, &wanted);
            if asked.exact && free != wanted {
                return Err(Refusal(format::name_taken(&doc, &wanted)));
            }
            free
        }
    };
    // What it outputs: what `defaults` fill of it before it has a version.
    let said = asked.kind.as_ref().filter(|kind| format::KINDS.contains(&kind.as_str())).cloned().or_else(|| (asked.tool == tool::EDIT).then(|| tool::edit().kind));
    let makes = plain.as_ref().and_then(|plain| said.as_ref().filter(|kind| lock.makes.get(plain) != Some(kind)).map(|kind| (plain.clone(), kind.clone())));
    let unknown = plain.as_ref().filter(|plain| said.is_none() && !lock.makes.contains_key(*plain)).cloned();
    let keyed = key.as_ref().filter(|_| told.is_none());
    if keyed.is_some() || makes.is_some() {
        let at_ms = now_ms();
        (folder.update_lock(|lock| {
            if let Some((key, digest)) = keyed {
                lock.assets_made.retain(|_, row| at_ms - row.at_ms < IMPORT_KEPT_MS);
                lock.assets_made.insert(key.clone(), folder::KeyedAsset { digest: digest.clone(), at_ms, name: name.clone() });
            }
            lock.makes.extend(makes.clone());
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
    let etag = assetfile::etag(&folder.text().map_err(Refusal)?, &name).ok_or_else(|| Refusal(own_lines(&name)))?;
    if let Some(plain) = unknown {
        tokio::spawn(learn_kind(api.clone(), project.clone(), asked.tool, plain));
    }
    Ok((StatusCode::CREATED, Json(AssetMade { name, etag })))
}

/// What `tool` makes, for what no one said, kept in project.lock. What can't be read here a render says.
async fn learn_kind(api: Arc<Api>, project: Arc<Project>, tool: String, plain: String) {
    match crate::toolbox::kind(&api, &tool, &project.folder.dir).await {
        Ok(kind) => {
            if let Err(why) = project.folder.update_lock(|lock| drop(lock.makes.insert(plain.clone(), kind.clone()))) {
                tracing::warn!("{tool}: {}", why.message);
            }
        }
        Err(why) => tracing::debug!("{tool}: {}", why.message),
    }
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
    let discarded = lock.asset(&asset.name).into_iter().flat_map(|held| &held.discarded).map(|version| version.number).collect();
    Ok(Json(AssetVersions { asset, versions, discarded }))
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
        let (edited, warnings) = assetfile::set(text, &name, &fields)?;
        // It already says this: a retry of a write whose answer was lost, or the same words from two places. Nothing to choose between.
        if edited != text && read.is_some_and(|read| read != was) {
            return Err(Error::new(Kind::Conflict, "asset_changed", format!("{name} changed since it was read")).remedy("read it again and make the edit on what it holds"));
        }
        Doc::of(folder, &edited, &lock)?;
        *told.borrow_mut() = (was, warnings);
        Ok(edited)
    }))
    .map_err(Refusal)?;
    let (was, warnings) = told.into_inner();
    let etag = assetfile::etag(&folder.text().map_err(Refusal)?, &name).ok_or_else(|| Refusal(own_lines(&name)))?;
    Ok(Json(AssetPatched { name, etag, was, warnings }))
}

/// What `DELETE` answers, for `PUT` to put back: its lines. Its versions wait in project.lock.
#[derive(Serialize, Deserialize)]
struct Deleted {
    lines: String,
}

/// `DELETE /v1/projects/{id}/assets/{name}`: it and its row gone, at once, and what it made stands. Its versions wait as an orphan's, its name free,
/// for `rename`, for `PUT` with what this answers, or for `reclaim`. A render going still makes what it was asked.
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
    let pin = doc.assets[i].pin.clone();
    folder
        .update_lock(|lock| {
            lock.bury(&name_key(&name));
            // What it held goes on record, so `reclaim` can take it.
            if let Some(sha256) = pin.clone().filter(|sha256| !lock.recorded().contains(sha256)) {
                lock.runs.push(folder::RunFile { run: 0, function: String::new(), output: "deleted".to_owned(), sha256 });
            }
        })
        .map_err(Refusal)?;
    canvas::set(folder, &id, BTreeMap::from([(name.clone(), None)])).map_err(Refusal)?;
    project.walker.lock().expect("walker lock").readings.values_mut().for_each(|reading| reading.moved(&name));
    project.asked.notify_one();
    let text = serde_json::to_string(&Deleted { lines: lines.into_inner() }).expect("what's deleted serializes");
    Ok(Json(AssetDeleted { name, text, at }))
}

/// `DELETE /v1/projects/{id}/assets/{name}/versions/{number}`: that version off the asset, at once. Its number stays given and its files on record
/// until `reclaim`, and what's approved goes with it. What read it is out of date only as anything is when what it reads moves.
pub(super) async fn discard(State(api): State<Arc<Api>>, Path((id, name, number)): Path<(String, String, String)>) -> Result<Json<VersionMoved>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let number = number.parse().unwrap_or(0);
    let (doc, lock) = (Doc::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let i = find(&doc, &name).map_err(Refusal)?;
    let name = doc.assets[i].name.clone();
    let noun = noun(&doc, &lock, i);
    let Some(held) = lock.asset(&name).filter(|held| held.version(number).is_some()) else { return Err(Refusal(no_version(&lock, &name, noun, number))) };
    let main = held.version(number).and_then(folder::Version::main);
    let approved = doc.assets[i].pin.as_ref().is_some_and(|pin| main == Some(pin) && !held.versions.iter().any(|other| other.number != number && other.main() == Some(pin)));
    let before = stale_names(&doc, &doc.stand(&lock));
    if approved {
        folder.edit_project(|text| assetfile::unapproved(text, &name)).map_err(Refusal)?;
    }
    folder.update_lock(|lock| lock.asset_mut(&name, "").discard(number)).map_err(Refusal)?;
    moved(&api, &project, name, number, noun, approved, &before).await
}

/// `PUT /v1/projects/{id}/assets/{name}/versions/{number}`: what `DELETE` set aside, back in its place. Its approval too, unless another has come since.
pub(super) async fn undiscard(
    State(api): State<Arc<Api>>,
    Path((id, name, number)): Path<(String, String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<VersionMoved>, Refusal> {
    let asked: VersionBack = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let number = number.parse().unwrap_or(0);
    let (doc, lock) = (Doc::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let i = find(&doc, &name).map_err(Refusal)?;
    let name = doc.assets[i].name.clone();
    let noun = noun(&doc, &lock, i);
    let Some(version) = lock.asset(&name).and_then(|held| held.on_record().find(|version| version.number == number)) else {
        let said = format!("{name} has no {noun} {number} to put back: `reclaim` clears what's discarded");
        return Err(Refusal(Error::new(Kind::NotFound, "version_unknown", said).next(format!("cozy project status {name}"))));
    };
    let main = version.main().filter(|_| asked.approved && doc.assets[i].pin.is_none());
    if let Some(sha256) = main {
        pin_in_file(folder, &name, sha256).map_err(Refusal)?;
    }
    let before = stale_names(&doc, &doc.stand(&lock));
    folder.update_lock(|lock| lock.asset_mut(&name, "").undiscard(number)).map_err(Refusal)?;
    moved(&api, &project, name, number, noun, main.is_some(), &before).await
}

/// Why `name` has no version `number` to discard.
fn no_version(lock: &folder::Lock, name: &str, noun: &str, number: u32) -> Error {
    let key = name_key(name);
    if lock.pending.values().any(|pending| name_key(&pending.asset) == key && pending.version.number == number) {
        let said = format!("{name}'s {noun} {number} is still being made: canceling stops what it would spend");
        return Error::new(Kind::Conflict, "version_running", said).next(format!("cozy project cancel {name}"));
    }
    let held = lock.asset(name);
    if held.is_some_and(|held| held.discarded.iter().any(|version| version.number == number)) {
        let said = format!("{name}'s {noun} {number} is already discarded");
        return Error::new(Kind::NotFound, "version_unknown", said).next(format!("cozy project restore {name} --version {number}"));
    }
    let numbers: Vec<String> = held.into_iter().flat_map(|held| &held.versions).map(|version| version.number.to_string()).collect();
    let said = match numbers.as_slice() {
        [] => format!("{name} has no {noun} yet"),
        numbers => format!("{name} has no {noun} {number}: it has {}", numbers.join(", ")),
    };
    Error::new(Kind::NotFound, "version_unknown", said).next(format!("cozy project status {name}"))
}

/// What a version going or coming back left: what's reading `name` looks again, a cut's film follows, and the answer.
async fn moved(api: &Arc<Api>, project: &Arc<Project>, name: String, number: u32, noun: &str, approved: bool, before: &BTreeSet<String>) -> Result<Json<VersionMoved>, Refusal> {
    project.walker.lock().expect("walker lock").readings.values_mut().for_each(|reading| reading.moved(&name));
    project.asked.notify_one();
    let (doc, lock) = (Doc::load(&project.folder).map_err(Refusal)?, project.folder.lock().map_err(Refusal)?);
    let stand = doc.stand(&lock);
    let i = find(&doc, &name).map_err(Refusal)?;
    if doc.tool(i) == Some(Tool::Cut) {
        Walk::new(api, project).film_follows(&doc, &stand, &lock, i).await;
    }
    let stands = stand[i].version().or(lock.asset(&name).and_then(|held| held.versions.last()).map(|version| version.number));
    let stale = stale_names(&doc, &stand).into_iter().filter(|name| !before.contains(name)).collect();
    Ok(Json(VersionMoved { name, number, noun: noun.to_owned(), approved, stands, stale }))
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
    folder.update_lock(|lock| lock.untombstone(&name)).map_err(Refusal)?;
    project.walker.lock().expect("walker lock").readings.values_mut().for_each(|reading| reading.moved(&name));
    let etag = assetfile::etag(&folder.text().map_err(Refusal)?, &name).ok_or_else(|| Refusal(own_lines(&name)))?;
    Ok(Json(AssetMade { name, etag }))
}
