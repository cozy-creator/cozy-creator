//! `/v1/projects/{id}/nodes`: the page's words for what a project holds. A tool dropped on the canvas, its inputs set one by one, gone
//! and back. Each is one call the CLI sees at once, and `nodefile` alone knows how the file says it.
use std::{cell::RefCell, collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode, header},
};
use protocol::{
    error::{Error, Kind},
    project::{NewNode, NodeBack, NodeDeleted, NodeMade, NodePatch, NodePatched, NodeVersions, Version as VersionRow},
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use super::{Doc, IMPORT_KEPT_MS, canvas, find, folder, format, listed, nodefile, now_ms, row};
use crate::api::{Api, Refusal, parse};

/// What to call a node with no name asked: the tool's last word, with no release.
fn tool_name(tool: &str) -> Option<String> {
    format::plain_name(tool.rsplit('/').next()?.split('@').next()?)
}

fn own_lines(name: &str) -> Error {
    Error::new(Kind::Usage, "project_edit", format!("{name} has no lines of its own in {}", folder::PROJECT))
}

/// `POST /v1/projects/{id}/nodes`: a node from a tool, and with `at` its row, in one call. The name goes in the lock first, so a retry
/// under the same key, whatever it finds written, ends with this one node.
pub(super) async fn create(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<NodeMade>), Refusal> {
    let body = body?;
    let asked: NewNode = parse(&body)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let key = headers.get("idempotency-key").and_then(|key| key.to_str().ok()).map(|key| (key.to_owned(), hex::encode(Sha256::digest(&body))));
    let kind = crate::toolbox::kind(&api, &asked.tool, &folder.dir).await.map_err(Refusal)?;
    let _one = project.importing.lock().expect("importing lock");
    let lock = folder.lock().map_err(Refusal)?;
    let told = key.as_ref().and_then(|(key, _)| lock.nodes_made.get(key));
    if let (Some(told), Some((key, digest))) = (told, &key)
        && told.digest != *digest
    {
        return Err(Refusal(Error::new(Kind::Conflict, "idempotency_conflict", format!("Idempotency-Key {key} asked another node already"))));
    }
    let name = match told {
        Some(told) => told.name.clone(),
        None => {
            let wanted = asked.name.as_deref().and_then(format::plain_name).or_else(|| tool_name(&asked.tool)).unwrap_or_else(|| "node".to_owned());
            format::free_name(&Doc::load(folder).map_err(Refusal)?, &lock, &wanted)
        }
    };
    if let (None, Some((key, digest))) = (told, &key) {
        let at_ms = now_ms();
        (folder.update_lock(|lock| {
            lock.nodes_made.retain(|_, row| at_ms - row.at_ms < IMPORT_KEPT_MS);
            lock.nodes_made.insert(key.clone(), folder::KeyedNode { digest: digest.clone(), at_ms, name: name.clone() });
        }))
        .map_err(Refusal)?;
    }
    (folder.edit_project(|text| match nodefile::etag(text, &name) {
        Some(_) => Ok(text.to_owned()),
        None => nodefile::added(text, &name, &asked.tool, &kind, &asked.fields).map(|(edited, _)| edited),
    }))
    .map_err(Refusal)?;
    if let Some(card) = asked.at {
        canvas::set(folder, &id, BTreeMap::from([(name.clone(), Some(card))])).map_err(Refusal)?;
    }
    let etag = nodefile::etag(&folder.text().map_err(Refusal)?, &name).ok_or_else(|| Refusal(own_lines(&name)))?;
    Ok((StatusCode::CREATED, Json(NodeMade { name, etag })))
}

/// `GET /v1/projects/{id}/nodes/{name}`: the node and each of its versions.
pub(super) async fn read(State(api): State<Arc<Api>>, Path((id, name)): Path<(String, String)>) -> Result<Json<NodeVersions>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let text = folder.text().map_err(Refusal)?;
    let (doc, lock) = (Doc::read(folder, &text).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let i = find(&doc, &name).map_err(Refusal)?;
    let mut node = row(&api, &doc, folder, &lock, &doc.stand(&lock), i);
    node.etag = nodefile::etag(&text, &node.name);
    let pinned = node.approved;
    let versions = (lock.asset(&node.name).into_iter().flat_map(|held| &held.versions))
        .map(|version| VersionRow {
            number: version.number,
            files: listed(folder, &lock, &version.files),
            made_by: version.made_by.as_ref().map(|by| by.with.clone()),
            seed: version.seed,
            runs: version.runs.clone(),
            approved: Some(version.number) == pinned,
            warnings: version.warnings.clone(),
            kept: version.kept,
            execution_ms: (version.execution_ms > 0).then_some(version.execution_ms),
        })
        .collect();
    Ok(Json(NodeVersions { node, versions }))
}

/// `PATCH /v1/projects/{id}/nodes/{name}`: its inputs set over what the file holds this moment. With `If-Match`, only if its own lines
/// are still what that read: an edit of another node never conflicts. A render going keeps the text it was asked with.
pub(super) async fn patch(
    State(api): State<Arc<Api>>,
    Path((id, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<NodePatched>, Refusal> {
    let asked: NodePatch = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let mut fields = asked.fields;
    if let Some(tool) = asked.tool {
        fields.insert(nodefile::TOOL.to_owned(), Value::String(tool));
    }
    let read = headers.get(header::IF_MATCH).and_then(|value| value.to_str().ok());
    let told = RefCell::new((String::new(), vec![]));
    (folder.edit_project(|text| {
        let was = nodefile::etag(text, &name).ok_or_else(|| own_lines(&name))?;
        if read.is_some_and(|read| read != was) {
            return Err(Error::new(Kind::Conflict, "node_changed", format!("{name} changed since it was read")).remedy("read it again and make the edit on what it holds"));
        }
        let (edited, warnings) = nodefile::set(text, &name, &fields)?;
        Doc::read(folder, &edited)?;
        *told.borrow_mut() = (was, warnings);
        Ok(edited)
    }))
    .map_err(Refusal)?;
    let (was, warnings) = told.into_inner();
    let etag = nodefile::etag(&folder.text().map_err(Refusal)?, &name).ok_or_else(|| Refusal(own_lines(&name)))?;
    Ok(Json(NodePatched { name, etag, was, warnings }))
}

/// `DELETE /v1/projects/{id}/nodes/{name}`: its lines and its row gone, at once. Its versions and files stay: an orphan keeps them until
/// `reclaim`, so what this answers puts it all back. A render going still makes what it was asked.
pub(super) async fn delete(State(api): State<Arc<Api>>, Path((id, name)): Path<(String, String)>) -> Result<Json<NodeDeleted>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let doc = Doc::load(folder).map_err(Refusal)?;
    let i = doc.find(&name).ok_or_else(|| Refusal(Error::new(Kind::NotFound, "node_unknown", format!("the project has nothing named {name}"))))?;
    let name = doc.assets[i].name.clone();
    let at = canvas::row(folder, &name).map_err(Refusal)?;
    let back = RefCell::new(String::new());
    (folder.edit_project(|text| {
        let (edited, text) = nodefile::removed(text, &name)?;
        *back.borrow_mut() = text;
        Ok(edited)
    }))
    .map_err(Refusal)?;
    canvas::set(folder, &id, BTreeMap::from([(name.clone(), None)])).map_err(Refusal)?;
    Ok(Json(NodeDeleted { name, text: back.into_inner(), at }))
}

/// `PUT /v1/projects/{id}/nodes/{name}`: what `DELETE` answered, back under its name: its versions are there waiting. Taken since by a
/// hand in the file, it's refused and says so.
pub(super) async fn put_back(
    State(api): State<Arc<Api>>,
    Path((id, name)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<NodeMade>, Refusal> {
    let asked: NodeBack = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    (folder.edit_project(|text| {
        if Doc::read(folder, text)?.find(&name).is_some() {
            return Err(Error::new(Kind::Conflict, "node_exists", format!("the project has a {name} again, so this one stays deleted")).remedy(format!("rename that one, then undo, or edit {}", folder::PROJECT)));
        }
        nodefile::restored(text, &name, &asked.text)
    }))
    .map_err(Refusal)?;
    if let Some(card) = asked.at {
        canvas::set(folder, &id, BTreeMap::from([(name.clone(), Some(card))])).map_err(Refusal)?;
    }
    let etag = nodefile::etag(&folder.text().map_err(Refusal)?, &name).ok_or_else(|| Refusal(own_lines(&name)))?;
    Ok(Json(NodeMade { name, etag }))
}
