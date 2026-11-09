//! An asset's description, from what it shows: a run, on a machine that's up, of a package's `caption`, given the file. The answer goes
//! back for the page to put in the field, and nothing is written. Calcifer knows no model and no question: a package that can read a
//! picture or a video and answer in words declares a function of that name, and owns the words.
use std::sync::Arc;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
    http::HeaderMap,
};
use api::{
    calcifer::{Kind as RunKind, On, Run, StagedInput, Status as RunStatus, SubmitRun},
    error::{Error, Kind},
    ids::Digest,
    machine as v1,
    interface::Type,
    calcifer::project::{CaptionAsked, Captioned},
};
use serde_json::{Map, Value};

use super::{Doc, Host, Package, Renderer, Project, binder, find, landing, local_code_named, media_type, new_key, order_of, tools};
use crate::{
    api::{Api, Refusal, parse},
    runs, toolbox,
};

/// What a package calls the function that says what a file shows.
const CAPTION: &str = "caption";

/// `POST /v1/projects/{id}/assets/{name}/caption`: its description as a package would say it, for the page to put in the field.
pub(super) async fn caption(
    State(api): State<Arc<Api>>,
    Path((id, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Captioned>, Refusal> {
    let body = body?;
    let asked: CaptionAsked = if body.is_empty() { CaptionAsked::default() } else { parse(&body)? };
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let (doc, lock) = (Doc::load(folder).map_err(Refusal)?, folder.lock().map_err(Refusal)?);
    let i = find(&doc, &name).map_err(Refusal)?;
    let name = doc.assets[i].name.clone();
    let stand = doc.stand(&lock);
    let files = stand[i].files.clone().or_else(|| lock.asset(&name)?.versions.last().map(|version| version.files.clone()));
    let Some(files) = files.filter(|files| ["image", "video"].iter().any(|kind| files.contains_key(*kind))) else {
        return Err(Refusal(Error::new(Kind::Conflict, "caption_no_file", format!("{name} has no picture or video yet to describe")).next(format!("cozy project render {name} --on <machine>"))));
    };
    let on = asked.on.map_or_else(|| runs::default_on(&api), On::from);
    if on == On::Machine("local".into()) {
        api.machines.local_up().await.map_err(Refusal)?;
    }
    if api.machines.holds(&on, true).await.is_none() {
        let said = format!("a caption runs on a machine that's up, and {on} isn't");
        return Err(Refusal(Error::new(Kind::Conflict, "caption_no_machine", said).next("cozy machine rent")));
    }
    let hub = runs::hub(&api, "");
    let host = Host::new(&api);
    let Some(package) = describer(&api, &host, &id, &hub, &on, &folder.dir).await else {
        return Err(Refusal(Error::new(Kind::NotFound, "caption_no_tool", "nothing here or on the Hub reads a picture and answers in words")));
    };
    let asking = package.function(CAPTION).map_err(Refusal)?;
    let callable = package.callable(CAPTION).expect("a describer has a caption");
    let kind = ["image", "video"].into_iter().find(|kind| files.contains_key(*kind)).expect("a picture or a video");
    let tool = format!("{}/{CAPTION}", package.name);
    // Its own file, by mention, in what takes that kind: a caption's only input, through the binder like any call.
    let takes = |ty: &Type| matches!(ty.plain(), Type::Asset(takes) | Type::Media { kind: takes, .. } if takes == kind);
    let Some(field) = callable.request.fields.iter().find(|field| takes(&field.ty)) else {
        return Err(Refusal(Error::new(Kind::NotFound, "caption_no_tool", format!("{tool} reads no {kind}"))));
    };
    let fields = Map::from_iter([(field.name.clone(), Value::String(format!("<{name}>")))]);
    let call = binder::Call { name: &name, tool: &tool, package: &package.name, callable: &callable, fields: &fields, reserved: &[], doc: &doc, own: None, lock: &lock, stand: &stand, gone: &Default::default(), pickup: None };
    let bound = binder::bind(&call).map_err(Refusal)?;
    let (input, warnings) = (bound.payload, bound.warnings);
    let key = headers.get("idempotency-key").and_then(|key| key.to_str().ok()).map_or_else(new_key, str::to_owned);
    let run = send(&host, &project, &asking, Value::Object(input), &on, &hub, &key).await?;
    let Some(text) = api::calcifer::tool::output_text(&callable, run.result.as_ref().unwrap_or(&Value::Null)) else {
        return Err(Refusal(Error::new(Kind::Unavailable, "caption_failed", format!("{} answered no text", asking.target()))));
    };
    Ok(Json(Captioned { name, text, run: run.number, warnings }))
}

/// The package a description comes from: local code this project's assets name, else what `on` holds, else the Hub's first, that declares
/// a `caption`. What's there first, so it costs no download. Which words it asks with are that package's.
async fn describer(api: &Arc<Api>, host: &Host, id: &str, hub: &str, on: &On, dir: &std::path::Path) -> Option<Package> {
    let from = std::slice::from_ref(on);
    for (written, _) in local_code_named(api, id).unwrap_or_default() {
        if let Ok(package) = host.package(hub, &written, dir, from).await
            && package.callable(CAPTION).is_some()
        {
            return Some(package);
        }
    }
    for (package, release) in toolbox::candidates(api, hub, on).await {
        if let Ok(package) = host.published(hub, &format!("{package}@{release}"), Some(on)).await
            && package.callable(CAPTION).is_some()
        {
            return Some(package);
        }
    }
    None
}

/// One function of a package on `payload`, its files sent, to its end. A caption is no one's version: its run is all there is.
async fn send(host: &Host, project: &Project, renderer: &Renderer, payload: Value, on: &On, hub: &str, key: &str) -> Result<Run, Refusal> {
    let folder = &project.folder;
    let lock = folder.lock().map_err(Refusal)?;
    let (mut inputs, mut staged) = (vec![], vec![]);
    for (field, path, digest) in tools::files_in(&payload, folder, &lock, &Default::default()) {
        let unreadable = |e: std::io::Error| Refusal(Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", path.display())).next("cozy project pull"));
        let length = std::fs::metadata(&path).map_err(unreadable)?.len();
        inputs.push(v1::InputFile { order: order_of(&field), field, digest: digest.clone(), length, media_type: media_type(&path).to_owned() });
        let digest: Digest = digest.parse().map_err(|why: &str| Refusal(Error::new(Kind::Internal, "digest", why)))?;
        staged.push(StagedInput { path, digest, length });
    }
    let source = match &renderer.local {
        None => v1::Source::Release(v1::Release { package: renderer.package.clone(), release: renderer.release.clone() }),
        Some(path) => {
            let prepared = host.ready(path, on, hub).await.map_err(Refusal)?;
            staged.extend(prepared.files);
            v1::Source::Local(prepared.source)
        }
    };
    let spec = v1::RunSpec {
        kind: v1::RunKind::Call,
        source: Some(source),
        entrypoint: renderer.function.clone(),
        payload: Some(v1::Json::parse(payload.to_string()).expect("a payload is JSON")),
        inputs,
        ..Default::default()
    };
    let submit = SubmitRun { on: on.clone(), hub: hub.to_owned(), kind: RunKind::Call, target: renderer.target(), spec, inputs: staged, held: vec![], out: Some(landing(&folder.dir)), warnings: vec![], agreed: None, stays: vec![] };
    let run = host.submit(key, submit).await.map_err(Refusal)?;
    let run = host.wait(&run.id).await.map_err(Refusal)?;
    if run.status != RunStatus::Completed {
        let why = run.reason.as_ref().map_or_else(|| format!("{:?}", run.status).to_lowercase(), |reason| reason.message.clone());
        return Err(Refusal(Error::new(Kind::Unavailable, "caption_failed", format!("{}: {why}", renderer.target())).next(format!("cozy run show {}", run.number))));
    }
    Ok(run)
}
