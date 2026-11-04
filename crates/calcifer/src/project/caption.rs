//! An asset's description, from what it shows: a run, on a machine that's up, of whatever a package's `project-adapter-text` says to ask.
//! The answer goes back for the page to put in the field, and nothing is written. Calcifer knows no model and no question: a package that
//! can read a picture or a video and answer in words declares this adapter, on the file as a `Read`, and owns the words.
use std::sync::Arc;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
    http::HeaderMap,
};
use protocol::{
    calcifer::{Kind as RunKind, On, Run, StagedInput, Status as RunStatus, SubmitRun},
    error::{Error, Kind},
    ids::Digest,
    machine as v1,
    project::{CaptionAsked, Captioned},
};
use serde_json::{Map, Value, json};

use super::{Doc, Host, Package, Renderer, Project, answered, find, folder, landing, local_code_named, media_type, new_key, tools};
use crate::{
    api::{Api, Refusal, parse},
    runs, toolbox,
};

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
    let files = doc.stand(&lock)[i].files.clone().or_else(|| lock.asset(&name)?.versions.last().map(|version| version.files.clone()));
    let Some(files) = files.filter(|files| ["image", "video"].iter().any(|kind| files.contains_key(*kind))) else {
        return Err(Refusal(Error::new(Kind::Conflict, "caption_no_file", format!("{name} has no picture or video yet to describe"))));
    };
    let on = asked.on.map_or_else(|| runs::default_on(&api), On::from);
    if on == On::Machine("local".into()) {
        api.machines.local_up().await.map_err(Refusal)?;
    }
    if api.machines.holds(&on).await.is_none() {
        let said = format!("a caption runs on a machine that's up, and {on} isn't");
        return Err(Refusal(Error::new(Kind::Conflict, "caption_no_machine", said).next("cozy machine rent")));
    }
    let hub = runs::hub(&api, "");
    let host = Host::new(&api);
    let Some(package) = describer(&api, &host, &id, &hub, &on, &folder.dir).await else {
        return Err(Refusal(Error::new(Kind::NotFound, "caption_no_tool", "nothing here or on the Hub reads a picture and answers in words")));
    };
    let adapter = package.adapter("text", None).expect("a describer has a text adapter");
    let records: Map<String, Value> = files.iter().map(|(field, sha256)| (field.clone(), lock.files.get(sha256).map_or_else(|| json!({"sha256": sha256}), folder::File::record))).collect();
    let read = json!({"name": name, "kind": doc.assets[i].kind, "fields": {}, "files": records, "made_by": ""});
    let operation = json!({"version": 2, "asset": name, "kind": "text", "function": "", "fields": {}, "options": {}, "reads": [read], "writes": [], "keep": null});
    let key = headers.get("idempotency-key").and_then(|key| key.to_str().ok()).map_or_else(new_key, str::to_owned);
    let adapted = send(&host, &project, &adapter, operation, &on, &hub, &format!("{key}-adapter")).await?;
    let (calls, warnings) = answered(adapted.result.as_ref());
    let Some(call) = calls.into_iter().next() else {
        return Err(Refusal(Error::new(Kind::Unavailable, "caption_failed", format!("{} asked nothing of {name}", package.name))));
    };
    let asking = package.function(&call.function).map_err(Refusal)?;
    let run = send(&host, &project, &asking, Value::Object(call.input), &on, &hub, &format!("{key}-ask")).await?;
    let answer = package.callable(&call.function).and_then(|callable| protocol::tool::output_text(&callable, run.result.as_ref()?));
    let Some(text) = answer else {
        return Err(Refusal(Error::new(Kind::Unavailable, "caption_failed", format!("{} answered no text", asking.target()))));
    };
    Ok(Json(Captioned { name, text, run: run.number, warnings }))
}

/// The package a description comes from: local code this project's assets name, else what `on` holds, else the Hub's first, that declares
/// `project-adapter-text`. What's there first, so it costs no download. Which words it asks with are that package's.
async fn describer(api: &Arc<Api>, host: &Host, id: &str, hub: &str, on: &On, dir: &std::path::Path) -> Option<Package> {
    let from = std::slice::from_ref(on);
    for (written, _) in local_code_named(api, id).unwrap_or_default() {
        if let Ok(package) = host.package(hub, &written, dir, from).await
            && package.adapter("text", None).is_some()
        {
            return Some(package);
        }
    }
    for (package, release) in toolbox::candidates(api, hub, on).await {
        if let Ok(package) = host.published(hub, &format!("{package}@{release}"), Some(on)).await
            && package.adapter("text", None).is_some()
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
    for (field, path, digest) in tools::files_in(&payload, folder, &lock) {
        let unreadable = |e: std::io::Error| Refusal(Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", path.display())).next("cozy project pull"));
        let length = std::fs::metadata(&path).map_err(unreadable)?.len();
        let order = field.split('.').filter_map(|part| part.parse().ok()).next_back().unwrap_or(0);
        inputs.push(v1::InputFile { field, digest: digest.clone(), length, media_type: media_type(&path).to_owned(), order });
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
    let submit = SubmitRun { on: on.clone(), hub: hub.to_owned(), kind: RunKind::Call, target: renderer.target(), spec, inputs: staged, held: vec![], out: Some(landing(&folder.dir)), warnings: vec![], agreed: None };
    let run = host.submit(key, submit).await.map_err(Refusal)?;
    let run = host.wait(&run.id).await.map_err(Refusal)?;
    if run.status != RunStatus::Completed {
        let why = run.reason.as_ref().map_or_else(|| format!("{:?}", run.status).to_lowercase(), |reason| reason.message.clone());
        return Err(Refusal(Error::new(Kind::Unavailable, "caption_failed", format!("{}: {why}", renderer.target())).next(format!("cozy run show {}", run.number))));
    }
    Ok(run)
}
