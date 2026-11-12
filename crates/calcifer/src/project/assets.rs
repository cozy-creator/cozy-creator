//! `/v1/projects/{id}/assets`: what a project holds, in the page's words. A tool dropped on the canvas, its inputs set one by one, gone;
//! undo brings it back. Each is one call, one transaction, that the CLI sees at once.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State, rejection::BytesRejection},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use api::{
    error::{Error, Kind},
    calcifer::project::{NewAsset, AssetDeleted, AssetMade, Copied, Copies, AssetPatch, AssetPatched, AssetVersions, NewVersion, Version as VersionRow, VersionMoved, VersionPatch, VersionPatched},
    calcifer::tool::{self, FieldType},
};

use super::{
    Doc, Host, Project, Tool, Walk, binder, canvas, cancelled, deleted_why, find, folder, format, graph::{mention, name_key}, history::{self, Took}, keep_file, listed,
    measure_soon, model, noun, stale_names,
    written::{Boxed, Start, Written},
};
use crate::api::{Api, Refusal, parse};

#[cfg(test)]
#[path = "assets_tests.rs"]
mod tests;

/// What to call an asset with no name asked when its tool can't be read: its package's word, numbered, with no release.
fn tool_name(tool: &str) -> Option<String> {
    let (package, _) = tool.rsplit_once('/')?;
    format::plain_name(&tool::first_of(tool::package_word(package.split('@').next()?)))
}

fn own_lines(name: &str) -> Error {
    Error::new(Kind::Usage, "project_edit", format!("the project has no box {name}")).next("cozy status")
}

/// What `tool` makes, as the project last read it, else as reading it now says: none when it can't be read, which refuses nothing.
async fn made_by(api: &Arc<Api>, folder: &folder::Folder, tool: &str) -> Option<String> {
    let plain = format::plain_tool(tool, folder);
    if let Some(kind) = format::kind_made_here(&plain) {
        return Some(kind.to_owned());
    }
    if let Ok(Some(kind)) = folder.reading_lock(|lock| lock.makes.get(&plain).cloned()) {
        return Some(kind);
    }
    crate::toolbox::read(api, tool, &folder.dir).await.ok().map(|(kind, _)| kind)
}

/// `POST /v1/projects/{id}/assets`: an asset from a tool, its row, and what its drop pushed aside, in one transaction. What the tool makes
/// comes with the ask or is held already, so a page waits on no read. A client that says nothing has the tool read, which refuses what can't
/// be one; a Hub that's away refuses nothing, and a render says.
pub(super) async fn create(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<AssetMade>), Refusal> {
    let mut asked: NewAsset = parse(&body?)?;
    let hint = format::node_kind(asked.fields.get(format::NODE_KIND)).map_err(Refusal)?.map(str::to_owned);
    if asked.kind.as_deref().is_some_and(|kind| !format::KINDS.contains(&kind) && kind != format::TEXT) {
        return Err(Refusal(Error::new(Kind::Validation, "asset_kind", "kind must be text, image, video or audio")));
    }
    if let Some(name) = asked.name.as_deref().filter(|_| asked.exact) {
        format::new_name(name).map_err(Refusal)?;
    }
    let project = api.projects.get(&api, &id)?;
    let id = project.id.clone();
    let folder = &project.folder;
    let no_tool = asked.tool.is_empty();
    let declared = asked.kind.clone().or_else(|| hint.as_deref().map(format::node_media).map(str::to_owned));
    let text_asset = no_tool && declared.as_deref().is_none_or(|kind| kind == format::TEXT);
    if text_asset && !asked.fields.contains_key(format::TEXT) {
        return Err(Refusal(Error::new(Kind::Validation, "asset_says_nothing", "an asset needs a tool, text, or a media kind").remedy("name a tool, set `fields.text`, or choose kind image, video or audio")));
    }
    let plain = (!no_tool).then(|| format::plain_tool(&asked.tool, folder));
    let intrinsic_edit = plain.as_deref() == Some(tool::EDIT);
    let said = if intrinsic_edit { Some(tool::edit().makes) } else { declared };
    let unsaid = said.is_none() && !plain.as_ref().is_some_and(|plain| folder.reading_lock(|lock| lock.makes.contains_key(plain)).unwrap_or(false));
    let read = if plain.is_some() && (unsaid || asked.name.is_none()) {
        match crate::toolbox::read(&api, &asked.tool, &folder.dir).await {
            Ok(read) => Some(read),
            Err(why) if unsaid && matches!(why.kind, Kind::NotFound | Kind::Usage | Kind::Validation) => return Err(Refusal(why)),
            Err(_) => None,
        }
    } else {
        None
    };
    let (read_kind, read_name) = read.unzip();
    let said = said.or(read_kind).or_else(|| plain.as_ref().and_then(|plain| folder.reading_lock(|lock| lock.makes.get(plain).cloned()).ok().flatten()));
    let kind = if intrinsic_edit { Some("video") } else if text_asset { Some(format::TEXT) } else { said.as_deref() };
    if let Some(kind) = kind { asked.fields.insert(format::NODE_KIND.to_owned(), Value::String(kind.to_owned())); }
    if !no_tool && ["models", "lora"].iter().any(|field| asked.fields.get(*field).is_some_and(|value| !value.is_null())) {
        crate::toolbox::validate_models(&api, &asked.tool, &folder.dir, &asked.fields).await.map_err(Refusal)?;
    }
    let wanted = asked.name.as_deref().and_then(format::plain_name).or_else(|| read_name.as_deref().and_then(format::plain_name)).or_else(|| tool_name(&asked.tool)).unwrap_or_else(|| if text_asset { format::TEXT } else { said.as_deref().unwrap_or("asset") }.to_owned());
    let made = folder.act(|| {
        // Chosen in the transaction that writes it: no other write comes between.
        let names = folder.reading_boxes(Written::names)?;
        let free = folder.reading_lock(|lock| {
            if text_asset {
                format::refuse_tag(lock, &wanted)?;
            }
            Ok(format::free_of(|name| names.contains(&name_key(name)), lock, &wanted))
        })??;
        if asked.exact && free != wanted {
            return Err(format::name_taken(&Doc::load(folder)?, &wanted));
        }
        let name = free;
        // What it outputs: what `defaults` fill of it before it has a version.
        if let Some((plain, kind)) = plain.as_ref().zip(said.as_ref()).filter(|(plain, kind)| folder.reading_lock(|lock| lock.makes.get(*plain) != Some(*kind)).unwrap_or(true)) {
            folder.update_lock(|lock| drop(lock.makes.insert(plain.clone(), kind.clone())))?;
        }
        // Its row with it: a page that reads the name has somewhere to put it, and places nothing of its own.
        if let Some(card) = asked.at {
            canvas::set(folder, &id, BTreeMap::from([(name.clone(), Some(card))]))?;
        }
        let kept = canvas::pushed(folder, &id, asked.moves.clone(), &asked.placed)?;
        folder.edit(|written| written.added(&name, &asked.tool, &asked.fields).map(drop))?;
        let etag = folder.reading_boxes(|written| written.get(&name).map(Boxed::etag))?.ok_or_else(|| own_lines(&name))?;
        Ok(AssetMade { name, etag, kept })
    });
    Ok((StatusCode::CREATED, Json(made.map_err(Refusal)?)))
}

/// `POST /v1/projects/{id}/copies`: boxes copied, as a person copies them on the canvas, in one transaction: one entry of the history, which
/// undo takes back whole. Each is its box called its name counted up, its fields with a mention of a box copied with it naming that box's copy,
/// and its versions as the lock holds them: the same files by hash, no bytes copied, and its approval naming the same version.
pub(super) async fn copy(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Copied>), Refusal> {
    let asked: Copies = parse(&body?)?;
    if asked.boxes.is_empty() {
        return Err(Refusal(Error::new(Kind::Validation, "copies_none", "name a box to copy")));
    }
    let project = api.projects.get(&api, &id)?;
    let id = project.id.clone();
    let folder = &project.folder;
    let copied = folder.act(|| {
        let written = folder.written()?;
        let mut taken = written.names();
        let mut pairs = Vec::with_capacity(asked.boxes.len());
        for copy in &asked.boxes {
            let from = written.get(&copy.from).ok_or_else(|| own_lines(&copy.from))?.name.clone();
            // Chosen in the transaction that writes it, past the others it makes.
            let name = folder.reading_lock(|lock| format::free_of(|name| taken.contains(&name_key(name)), lock, &from))?;
            taken.insert(name_key(&name));
            pairs.push((from, name, copy.at));
        }
        let copies: BTreeMap<String, String> = pairs.iter().map(|(from, name, _)| (name_key(from), name.clone())).collect();
        let renamed = |inner: &str| mention(inner).and_then(|read| Some(format!("{}{}", copies.get(&name_key(read.name))?, &inner[read.name.len()..])));
        let rows: BTreeMap<String, Option<api::calcifer::project::Card>> = pairs.iter().filter_map(|(_, name, at)| Some((name.clone(), Some((*at)?)))).collect();
        if !rows.is_empty() {
            canvas::set(folder, &id, rows)?;
        }
        let kept = canvas::pushed(folder, &id, asked.moves.clone(), &asked.placed)?;
        folder.edit(|written| pairs.iter().try_for_each(|(from, name, _)| written.copied(from, name, &renamed)))?;
        folder.update_lock(|lock| {
            for (from, name, _) in &pairs {
                let Some(held) = lock.asset(from).cloned() else { continue };
                let key = name_key(name);
                // What waits under its name for a rename or `reclaim` stays an orphan's.
                lock.bury(&key);
                let versions = held.versions.into_iter().map(|version| folder::Version { id: String::new(), ..version }).collect();
                lock.assets.insert(key, folder::Asset { id: String::new(), name: name.clone(), versions, discarded: vec![], tried: None, failed: None, ..held });
            }
        })?;
        let made = pairs.iter().map(|(_, name, _)| -> Result<AssetMade, Error> {
            let etag = folder.reading_boxes(|written| written.get(name).map(Boxed::etag))?.ok_or_else(|| own_lines(name))?;
            Ok(AssetMade { name: name.clone(), etag, kept: BTreeMap::new() })
        });
        Ok(Copied { made: made.collect::<Result<_, _>>()?, kept })
    });
    let copied = copied.map_err(Refusal)?;
    for reading in project.walker.lock().expect("walker lock").readings.values_mut() {
        copied.made.iter().for_each(|made| reading.moved(&made.name));
    }
    Ok((StatusCode::CREATED, Json(copied)))
}

/// `GET /v1/projects/{id}/assets/{name}`: the asset and each of its versions.
pub(super) async fn read(State(api): State<Arc<Api>>, Path((id, name)): Path<(String, String)>, Query(asked): Query<Asked>) -> Result<Json<AssetVersions>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let model = model::of(&api, &project).await.map_err(Refusal)?;
    let (doc, lock) = (&model.doc, &model.lock);
    let i = find(doc, &name).map_err(Refusal)?;
    let asset = model.row(&project, i);
    let pinned = asset.approved;
    let recovered = authored_versions(&project, doc, lock, &model.stand, i);
    let versions = (lock.asset(&asset.name).into_iter().flat_map(|held| &held.versions))
        .map(|version| VersionRow {
            number: version.number,
            files: listed(lock, &version.files).into_iter().map(|(output, mut file)| {
                file.metadata = version.metadata.get(&output).cloned().unwrap_or_default();
                (output, file)
            }).collect(),
            made_by: version.tool.clone().or_else(|| asset.tool.clone()),
            seed: version.seed,
            runs: version.run.into_iter().collect(),
            approved: Some(version.number) == pinned,
            warnings: version.warnings.clone(),
            pickup: version.pickup.clone(),
            text: lock.asset(&asset.name).and_then(|held| held.text_of(version.number)).map(str::to_owned),
            fields: version.fields.clone().or_else(|| recovered.get(&version.inputs).cloned()),
            execution_ms: (version.execution_ms > 0).then_some(version.execution_ms),
        })
        .collect();
    let discarded = lock.asset(&asset.name).into_iter().flat_map(|held| &held.discarded).map(|version| version.number).collect();
    // What can't make the call, a package the Hub doesn't have among them, is said with no call: the asset and its versions still answer.
    let (call, call_warnings) = match asked.call && doc.makes(i) && doc.tool(i).is_none() {
        true => {
            let bound = async {
                let hub = api.machines.hubs.default_origin();
                let on = [crate::runs::default_on(&api)];
                let package = Host::new(&api).package(&hub, &doc.target(i), &project.folder.dir, &on).await?;
                let function = package.maker()?.map(|maker| maker.function).unwrap_or_default();
                let callable = package.callable(&function).ok_or_else(|| Error::new(Kind::NotFound, "callable_unknown", format!("{} has no function {function}", package.name)))?;
                let (fields, reserved, stand) = (doc.inputs_of(i), package.reserved(), &model.stand);
                // A mention of nothing is said, as a render would refuse it, and the rest of the call stands.
                let missing = doc.missing_inputs(i, &reserved, lock);
                let call = binder::Call { name: &asset.name, tool: &doc.target(i), package: &package.name, callable: &callable, fields: &fields, reserved: &reserved, doc, own: Some(i), lock, stand, gone: &Default::default(), pickup: None, inline: &[] };
                let bound = binder::bind(&call)?;
                Ok::<_, Error>((bound.payload, missing.into_iter().chain(bound.warnings).collect::<Vec<String>>()))
            };
            match bound.await {
                Ok((payload, said)) => (Some(serde_json::Value::Object(payload)), said),
                Err(why) => (None, vec![why.message]),
            }
        }
        false => (None, vec![]),
    };
    Ok(Json(AssetVersions { asset, versions, discarded, call, call_warnings }))
}

#[derive(Deserialize)]
pub(super) struct Asked {
    /// The call its binder would send now.
    #[serde(default)]
    call: bool,
}

/// `PATCH /v1/projects/{id}/assets/{name}`: the fields it names set over what it holds this moment, in one transaction, whoever wrote
/// them last; the rest stay (cl-904, Paul 11-12: a page's writes merge field by field, never refused). A render going keeps the text it was
/// asked with.
pub(super) async fn patch(
    State(api): State<Arc<Api>>,
    Path((id, name)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<AssetPatched>, Refusal> {
    let asked: AssetPatch = parse(&body?)?;
    format::node_kind(asked.fields.get(format::NODE_KIND)).map_err(Refusal)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let validated_tool = if ["models", "lora"].iter().any(|field| asked.fields.get(*field).is_some_and(|value| !value.is_null())) {
        let written = folder.written().map_err(Refusal)?;
        let held = written.get(&name).ok_or_else(|| Refusal(own_lines(&name)))?;
        let current = held.get("tool").and_then(Value::as_str).unwrap_or_default();
        let target = asked.tool.as_deref().unwrap_or(current);
        if target.is_empty() {
            None
        } else {
            let mut fields = held.map();
            for (field, value) in &asked.fields {
                if value.is_null() { fields.remove(field); }
                else { fields.insert(field.clone(), value.clone()); }
            }
            crate::toolbox::validate_models(&api, target, &folder.dir, &fields).await.map_err(Refusal)?;
            Some(target.to_owned())
        }
    } else { None };
    let holds = match &asked.tool {
        Some(tool) => Some(pictures(folder, &name, tool, false).map_err(Refusal)?),
        None => None,
    };
    // A tool goes on a box of the kind it makes (owner 11-11), whether or not this project has read it yet.
    let incoming = match asked.tool.as_deref().filter(|tool| !tool.is_empty()) {
        Some(tool) => made_by(&api, folder, tool).await,
        None => None,
    };
    // A tool going on a picture or a video starts from it: in its first required slot of that kind, else at the start of its first prompt.
    let start = match (&asked.tool, &holds) {
        (Some(tool), Some(Some((_, kind)))) if !tool.is_empty() => crate::toolbox::fields_of(&api, tool, &folder.dir).await.and_then(|fields| {
            let slot = fields.iter().find(|field| field.required && field.ty == FieldType::Slot && field.accept.as_deref() == Some(kind.as_str()));
            let prompt = || fields.iter().find(|field| field.ty == FieldType::Prompt);
            slot.map(|field| Start::Slot(field.field.clone())).or_else(|| prompt().map(|field| Start::Prompt(field.field.clone())))
        }),
        _ => None,
    };
    let patched = folder.act(|| {
        let holds = match &asked.tool {
            Some(tool) => Some(pictures(folder, &name, tool, true)?.map(|(number, _)| number)),
            None => None,
        };
        let written = folder.written()?;
        if asked.tool.is_none() && validated_tool.as_ref().is_some_and(|target| written.get(&name).and_then(|held| held.get("tool")).and_then(Value::as_str) != Some(target.as_str())) {
            return Err(Error::new(Kind::Conflict, "asset_changed", format!("{name}'s tool changed while its model choices were checked")).remedy("read the node again before choosing its models"));
        }
        let was = written.get(&name).ok_or_else(|| own_lines(&name))?.etag();
        let mut fields = asked.fields.clone();
        if let (Some(tool), Some(holds)) = (&asked.tool, &holds) {
            let held = written.get(&name).ok_or_else(|| own_lines(&name))?;
            let intrinsic = held.get("tool").and_then(Value::as_str).is_some_and(|target| format::plain_tool(target, folder) == tool::EDIT);
            let current = if intrinsic { "video".to_owned() }
                else if let Some(kind) = format::node_kind(held.get(format::NODE_KIND))? { kind.to_owned() }
                else {
                    let doc = Doc::load(folder)?;
                    let i = doc.find(&name).ok_or_else(|| own_lines(&name))?;
                    doc.assets[i].kind.clone()
                };
            // A different explicit hint is an intentional conversion. Merely assigning/removing
            // a model preserves the current container, including when its output is not cached yet.
            let next = format::node_kind(asked.fields.get(format::NODE_KIND))?.unwrap_or(&current);
            let plain = format::plain_tool(tool, folder);
            if let Some(makes) = incoming.as_deref().filter(|kind| *kind != format::node_media(next)) {
                let mut refused = Error::new(Kind::Validation, "asset_tool_kind", format!("this {next} node cannot use a model that produces {makes}"))
                    .remedy(format!("Choose a model that matches the node's media type, or put {tool} on a new {makes} node that mentions <{name}>"));
                refused.metadata = Some(serde_json::json!({ "makes": makes, "node": next }));
                return Err(refused);
            }
            fields.insert(format::NODE_KIND.to_owned(), Value::String(next.to_owned()));
            if plain == tool::EDIT && !intrinsic && !fields.contains_key("timeline") && !fields.contains_key("tracks") {
                let clips: Vec<Value> = holds.filter(|number| *number > 0).map(|number| json!(format!("<self.{number}>"))).into_iter().collect();
                fields.insert("tracks".to_owned(), json!([{"clips": clips}]));
            }
            written.with_tool(&name, tool, *holds, start.clone(), &mut fields)?;
        } else if written.get(&name).and_then(|held| held.get("tool")).and_then(Value::as_str).is_some_and(|target| format::plain_tool(target, folder) == tool::EDIT) {
            fields.insert(format::NODE_KIND.to_owned(), Value::String("video".to_owned()));
        }
        let mut edited = written.clone();
        let warnings = edited.set(&name, &fields)?;
        folder.edit(|now| {
            *now = edited;
            Ok(())
        })?;
        let etag = folder.reading_boxes(|written| written.get(&name).map(Boxed::etag))?.ok_or_else(|| own_lines(&name))?;
        Ok(AssetPatched { name: name.clone(), etag, was, warnings })
    });
    patched.map(Json).map_err(Refusal)
}

/// Old rows are recoverable only from authored documents with the same complete
/// input key. A bound request cannot reverse text expansion or removed mentions.
fn authored_versions(project: &Project, doc: &Doc, lock: &folder::Lock, stand: &[format::Standing], i: usize) -> BTreeMap<String, Map<String, Value>> {
    let wanted: BTreeSet<&str> = lock.asset(&doc.assets[i].name).into_iter().flat_map(|held| &held.versions)
        .filter(|version| version.fields.is_none() && !version.inputs.is_empty()).map(|version| version.inputs.as_str()).collect();
    let mut found = BTreeMap::new();
    if wanted.is_empty() {
        return found;
    }
    if let Some(key) = stand[i].inputs.as_ref().filter(|key| wanted.contains(key.as_str())) {
        found.insert(key.clone(), doc.authoring_fields(i));
    }
    let mut walker = project.walker.lock().expect("walker lock");
    for reading in walker.readings.values_mut() {
        if found.len() == wanted.len() { break }
        let Some(at) = reading.doc.find(&doc.assets[i].name) else { continue };
        reading.ensure(at, lock);
        if let Some(key) = reading.done[at].inputs.as_ref().filter(|key| wanted.contains(key.as_str())) {
            found.entry(key.clone()).or_insert_with(|| {
                let mut fields = reading.doc.authoring_fields(at);
                for (from, to) in &lock.walk.renames {
                    if doc.find(from).is_none() && doc.find(to).is_some() {
                        fields.values_mut().for_each(|value| super::written::mentions_renamed(value, &|name| super::graph::mention_moved(name, from, to)));
                    }
                }
                fields
            });
        }
    }
    found
}

/// What a change of `tool` needs of the pictures asset `name` holds: `None` with none. A tool going on a picture nothing numbers makes it a version, as if that
/// tool had made it, and answers the one a new prompt starts from: what's approved, else its newest. Going off, or swapped, nothing moves, and any number will do.
/// With `write` unset, nothing is numbered, and what would be is 0. Each number comes with the asset's kind.
fn pictures(folder: &folder::Folder, name: &str, tool: &str, write: bool) -> Result<Option<(u32, String)>, Error> {
    let (doc, lock) = (Doc::load(folder)?, folder.lock()?);
    let Some(i) = doc.find(name) else { return Ok(None) };
    let asset = &doc.assets[i];
    let kinded = |number: Option<u32>| Ok(number.map(|number| (number, asset.kind.clone())));
    let newest = lock.asset(&asset.name).and_then(|held| held.versions.last()).map(|version| version.number);
    let Some(pin) = &asset.pin else { return kinded(newest) };
    let number = |lock: &folder::Lock| lock.asset(&asset.name).and_then(|held|
        asset.selected_version.and_then(|number| held.versions.iter().find(|version| version.number == number && version.main() == Some(pin)))
            .or_else(|| held.versions.iter().rev().find(|version| version.main() == Some(pin))))
        .map(|version| version.number);
    if let Some(number) = number(&lock) {
        return kinded(Some(number));
    }
    if tool.is_empty() || asset.tool.is_some() {
        return kinded(newest.or(Some(0)));
    }
    if !write {
        return kinded(Some(0));
    }
    kinded(Some(constant_numbered(folder, asset, pin)?))
}

/// The file asset holds with no version saying so, made its next version.
fn constant_numbered(folder: &folder::Folder, asset: &format::Asset, pin: &str) -> Result<u32, Error> {
    let number = std::cell::Cell::new((0, true));
    folder.update_lock(|lock| {
        let field = lock.files.get(pin).map_or_else(|| asset.kind.clone(), |file| file.field().to_owned());
        number.set(lock.joined(&asset.name, &asset.kind, folder::Files::from([(field, pin.to_owned())])));
    })?;
    let (number, already) = number.get();
    // Its own picture numbered for the edit: a step back sets it aside again.
    if !already {
        history::take(folder, &asset.name, number, if asset.kind == "video" { "take" } else { "version" }, Took::Absent, Took::Listed);
    }
    Ok(number)
}

/// `DELETE /v1/projects/{id}/assets/{name}`: it and its row gone, at once, and what it made stands. Its name is free, and nothing resolves it.
/// Its files wait, set aside, for Undo until the window ends, then leave the project (`purge`). A render going still makes what it was asked.
pub(super) async fn delete(State(api): State<Arc<Api>>, Path((id, name)): Path<(String, String)>) -> Result<Json<AssetDeleted>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let id = project.id.clone();
    deleted(&api, &project, &id, &name).map(Json)
}

/// Asset `name` and its row gone, as `DELETE` has it, in one transaction.
pub(super) fn deleted(api: &Arc<Api>, project: &Arc<Project>, id: &str, name: &str) -> Result<AssetDeleted, Refusal> {
    let folder = &project.folder;
    let unknown = || Error::new(Kind::NotFound, "asset_unknown", format!("the project has nothing named {name}")).next("cozy status");
    let gone = folder.act(|| {
        // Its name and what it holds approved, from its box alone: a Doc of the whole project took a delete of one of 5,000 boxes 287 ms.
        let held = folder.reading_boxes(|written| written.get(name).map(|held| (held.name.clone(), held.id.clone(), held.get("approved").cloned())))?;
        let (name, box_id, approved) = held.ok_or_else(unknown)?;
        let pin = folder.reading_lock(|lock| format::approved_file(approved.as_ref(), lock))?;
        let at = canvas::row(folder, &name)?;
        folder.edit(|written| written.removed(&name).map(drop))?;
        // Gone from the project, what lands of it lands nothing.
        cancelled(api, project, std::slice::from_ref(&name), &[], &deleted_why).map_err(|Refusal(why)| why)?;
        // What it holds, set aside for Undo: a box that never made anything holds nothing.
        if pin.is_some() || folder.reading_lock(|lock| lock.asset(&name).is_some())? {
            folder.update_lock(|lock| lock.set_aside(&name, &box_id, pin.clone()))?;
        }
        canvas::set(folder, id, BTreeMap::from([(name.clone(), None)]))?;
        Ok(AssetDeleted { name, at })
    });
    let gone = gone.map_err(Refusal)?;
    project.walker.lock().expect("walker lock").readings.values_mut().for_each(|reading| reading.moved(&gone.name));
    project.asked.notify_one();
    super::purge::due(api, &project.id, super::now_ms() + super::purge::window_ms());
    Ok(gone)
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
    let approved = doc.assets[i].selected_version.map_or_else(
        || doc.assets[i].pin.as_ref().is_some_and(|pin| main == Some(pin) && !held.versions.iter().any(|other| other.number != number && other.main() == Some(pin))),
        |selected| selected == number,
    );
    let before = stale_names(&doc, &doc.stand(&lock));
    folder
        .act(|| {
            if approved {
                folder.edit(|written| written.unapproved(&name))?;
            }
            folder.update_lock(|lock| lock.asset_mut(&name, "").discard(number))?;
            history::take(folder, &name, number, noun, Took::Listed, Took::Discarded);
            Ok(())
        })
        .map_err(Refusal)?;
    moved(&api, &project, name, number, noun, approved, &before).await.map(|Json(answer)| Json(VersionMoved { read_by: doc.read_by(i, number), ..answer }))
}

/// `POST /v1/projects/{id}/assets/{name}/versions`: a picture joins it as its next version, so versions of one thing live on one box. The file it held before that
/// is a version first, and what's approved stays. The same file again is the version it is.
pub(super) async fn join(State(api): State<Arc<Api>>, Path((id, name)): Path<(String, String)>, body: Result<Bytes, BytesRejection>) -> Result<Json<VersionMoved>, Refusal> {
    let asked: NewVersion = parse(&body?)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let doc = Doc::load(folder).map_err(Refusal)?;
    let i = find(&doc, &name).map_err(Refusal)?;
    let asset = &doc.assets[i];
    let name = asset.name.clone();
    let kept = keep_file(folder, &asked.source).await?;
    let before = stale_names(&doc, &doc.stand(&folder.lock().map_err(Refusal)?));
    let (number, already) = folder
        .act(|| {
            let initialize = format::can_initialize_approval(folder, &name)?;
            if let Some(pin) = &asset.pin {
                constant_numbered(folder, asset, pin)?;
            }
            let joined = std::cell::Cell::new((0, false));
            folder.update_lock(|lock| joined.set(lock.joined(&name, kept.field, folder::Files::from([(kept.field.to_owned(), kept.file.sha256.clone())]))))?;
            let (number, already) = joined.get();
            if !already {
                history::take(folder, &name, number, noun(&doc, &folder.lock()?, i), Took::Absent, Took::Listed);
            }
            if initialize {
                folder.edit(|written| written.selected(&name, number, &kept.file.sha256))?;
            }
            Ok((number, already))
        })
        .map_err(Refusal)?;
    measure_soon(&api, &project);
    let noun = noun(&doc, &folder.lock().map_err(Refusal)?, i);
    moved(&api, &project, name, number, noun, false, &before).await.map(|Json(answer)| Json(VersionMoved { already, ..answer }))
}

/// `PATCH /v1/projects/{id}/assets/{name}/versions/{number}`: what its main file says of itself, whole, as a person sets it on its box (orbit
/// viewing on for any video). Number 0 is the file a box with no version shows (a dropped video): numbered first, as a join numbers it.
/// Nothing goes out of date: it's how the file shows, not what made it. Undo puts the old map back.
pub(super) async fn patch_version(State(api): State<Arc<Api>>, Path((id, name, number)): Path<(String, String, String)>, body: Result<Bytes, BytesRejection>) -> Result<Json<VersionPatched>, Refusal> {
    let asked: VersionPatch = parse(&body?)?;
    let length = serde_json::to_vec(&asked.metadata).map_or(usize::MAX, |bytes| bytes.len());
    if length > super::METADATA_MAX {
        let said = format!("metadata is {length} bytes, over {}", super::METADATA_MAX);
        return Err(Refusal(Error::new(Kind::Validation, "metadata_too_large", said).remedy("keep it to a few small keys")));
    }
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let asked_number: u32 = number.parse().unwrap_or(0);
    let doc = Doc::load(folder).map_err(Refusal)?;
    let i = find(&doc, &name).map_err(Refusal)?;
    let asset = &doc.assets[i];
    let name = asset.name.clone();
    let after = asked.metadata;
    let number = folder
        .act(|| {
            let number = match (asked_number, &asset.pin) {
                (0, Some(pin)) => constant_numbered(folder, asset, pin)?,
                (number, _) => number,
            };
            let lock = folder.lock()?;
            let noun = noun(&doc, &lock, i);
            let version = lock.asset(&name).and_then(|held| held.version(number)).ok_or_else(|| no_version(&lock, &name, noun, number))?;
            let output = version.main_output().cloned().ok_or_else(|| Error::new(Kind::NotFound, "version_unknown", format!("{name}'s {noun} {number} has no file")).next("cozy status --full"))?;
            let before = version.metadata.get(&output).cloned().unwrap_or_default();
            if before != after {
                folder.update_lock(|lock| set_metadata(lock, &name, number, &output, after.clone()))?;
                history::metadata(folder, &name, number, noun, &output, before, after.clone());
            }
            Ok(number)
        })
        .map_err(Refusal)?;
    Ok(Json(VersionPatched { name, number, metadata: after }))
}

/// Version `number` of `name`: its file `output` says `metadata` of itself, or nothing when it's empty.
pub(super) fn set_metadata(lock: &mut folder::Lock, name: &str, number: u32, output: &str, metadata: Map<String, Value>) {
    let Some(version) = lock.asset_mut(name, "").versions.iter_mut().find(|version| version.number == number) else { return };
    if metadata.is_empty() {
        version.metadata.remove(output);
    } else {
        version.metadata.insert(output.to_owned(), metadata);
    }
}

/// Why `name` has no version `number` to discard.
fn no_version(lock: &folder::Lock, name: &str, noun: &str, number: u32) -> Error {
    let key = name_key(name);
    if lock.pending.values().any(|pending| name_key(&pending.asset) == key && pending.version.number == number) {
        let said = format!("{name}'s {noun} {number} is still being made: canceling stops what it would spend");
        return Error::new(Kind::Conflict, "version_running", said).next("cozy open");
    }
    let held = lock.asset(name);
    if held.is_some_and(|held| held.discarded.iter().any(|version| version.number == number)) {
        let said = format!("{name}'s {noun} {number} is already discarded");
        return Error::new(Kind::NotFound, "version_unknown", said).next("cozy open");
    }
    let numbers: Vec<String> = held.into_iter().flat_map(|held| &held.versions).map(|version| version.number.to_string()).collect();
    let said = match numbers.as_slice() {
        [] => format!("{name} has no {noun} yet"),
        numbers => format!("{name} has no {noun} {number}: it has {}", numbers.join(", ")),
    };
    Error::new(Kind::NotFound, "version_unknown", said).next("cozy status --full")
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
    Ok(Json(VersionMoved { name, number, noun: noun.to_owned(), approved, stands, stale, read_by: vec![], already: false }))
}

fn taken(name: &str) -> Error {
    Error::new(Kind::Conflict, "asset_exists", format!("the project has a {name} again, so this one stays deleted")).remedy("rename that one, then undo again").next("cozy open")
}

/// A box back as `removed` took it, its versions with it, and with `at` its row, in one transaction: its new ETag.
pub(super) fn put_back_box(project: &Project, id: &str, back: Boxed, at: Option<api::calcifer::project::Card>) -> Result<String, Refusal> {
    let folder = &project.folder;
    let name = back.name.clone();
    let etag = folder.act(|| {
        if folder.reading_boxes(|written| written.find(&name).is_some())? {
            return Err(taken(&name));
        }
        if let Some(card) = at {
            canvas::set(folder, id, BTreeMap::from([(name.clone(), Some(card))]))?;
        }
        folder.update_lock(|lock| {
            lock.restore(&name, &back.id);
        })?;
        folder.edit(|written| written.restored(back))?;
        folder.reading_boxes(|written| written.get(&name).map(Boxed::etag))?.ok_or_else(|| own_lines(&name))
    });
    let etag = etag.map_err(Refusal)?;
    project.walker.lock().expect("walker lock").readings.values_mut().for_each(|reading| reading.moved(&name));
    Ok(etag)
}
