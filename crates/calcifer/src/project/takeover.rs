//! Takeover (cl-746): `cozy project render … --on X` makes X the render's holder. X's Calcifer walks it, with the code any walk has,
//! keeps what it makes and drives any other machine named; this computer's Calcifer hands it over, then only collects. No job waits on
//! this computer: it may sleep, lose its network or be closed, and X goes on. A machine ends 15 minutes after its last job, so what isn't
//! collected by then is lost; `status` and `render` say when that is.
//!
//! The hand-off is the one way a folder's render leaves it (`Held`, in its bound walk file), and each step of it goes again under the same
//! keys: the take (X's folder for this one is the same each time), the files X lacks (by content, resumed), and each ask (its
//! `Idempotency-Key`, which X keeps a day). Killed anywhere, the next start goes on from the walk file, and no render is asked twice.
//! While X holds, nothing walks here. Edits here reach X's text as they're made, so a deletion stops its job there, and what X writes of
//! the text (a pin as a take lands) comes back. The watcher is level-triggered: it reads what X has, takes it in and fetches what isn't
//! here; X's events only nudge it and draw the board.
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Component, Path as FsPath, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
};
use eventsource_stream::Eventsource as _;
use futures_util::StreamExt as _;
use protocol::{
    calcifer::{Machine, NewMachine, On},
    error::{Envelope, Error, Kind},
    machine::{HELD, missing, parse_held},
    project::{Asked, Board, Event, HeldBy, HeldFile, Holding, JobState, Rendered, Runs, Take, Taken},
};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncSeekExt as _, AsyncWriteExt as _};
use tracing::warn;

use super::{
    Entry, Project, assetfile,
    folder::{self, Folder, Handing, Held},
    format::{self, Doc},
    graph::name_key,
    indexed, now_ms, project_id, remember, status_next, yaml_etag,
};
use crate::{
    api::{Api, Refusal, parse},
    link::Link,
    machines::refusal,
};

/// How often a watcher looks when nothing nudges it, and the longest it waits for a machine that doesn't answer.
const TICK: Duration = Duration::from_secs(5);
const MOST: Duration = Duration::from_secs(60);
/// How often a walk on a rental renews its clock, and how long since something last moved it still does.
const RENEW: Duration = Duration::from_secs(5 * 60);
const QUIET_MS: i64 = 15 * 60 * 1000;
/// Where a file being collected waits until it's whole.
const COLLECTING: &str = ".runs/collecting";

// The machine that holds.

/// `POST /v1/projects/take`: the project another computer's Calcifer hands this one, in `<home>/projects/<name>-<its id>`, the same each
/// time. Its text is that computer's, its versions join these, its local code is written where its tools name it, and what stands in it
/// that isn't here is kept from what was uploaded, or asked for.
pub(super) async fn take(State(api): State<Arc<Api>>, body: Result<Bytes, BytesRejection>) -> Result<Json<Taken>, Refusal> {
    let take: Take = parse(&body?)?;
    let usage = |said: String| Refusal(Error::new(Kind::Usage, "project_take", said));
    let hex = take.from.strip_prefix("project-").filter(|hex| hex.len() >= 8 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let hex = hex.ok_or_else(|| usage(format!("{:?} is no project's id", take.from)))?;
    let name = format::plain_name(&take.name).unwrap_or_else(|| "untitled".to_owned());
    let dir = api.machines.hubs.home().projects().join(format!("{name}-{}", &hex[..8]));
    let io = |e: &dyn std::fmt::Display| Refusal(Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", dir.display())));
    std::fs::create_dir_all(&dir).map_err(|e| io(&e))?;
    let yaml = dir.join(folder::PROJECT);
    let now = std::fs::read_to_string(&yaml).ok();
    let id = project_id(&platform::durable::canonical(&dir).map_err(|e| io(&e))?);
    let walking = api.projects.open.lock().expect("projects lock").get(&id).is_some_and(|project| *project.walking.lock().expect("walking lock"));
    if let Some(now) = now.as_ref().filter(|now| **now != take.yaml) {
        let read = take.base.as_deref().is_some_and(|base| base == yaml_etag(now)) || (take.base.is_none() && !walking);
        if !read {
            let said = "its project.yaml here changed since it was read";
            return Err(Refusal(Error::new(Kind::Conflict, "project_changed", said).remedy("read it again, and take what it says in")));
        }
    }
    if now.as_deref() != Some(take.yaml.as_str()) {
        platform::durable::replace(&yaml, take.yaml.as_bytes(), 0o644).map_err(|e| io(&e))?;
    }
    let dir = platform::durable::canonical(&dir).map_err(|e| io(&e))?;
    indexed(&api, true, |index| {
        let walking = index.projects.get(&id).is_some_and(|known| known.walking);
        index.projects.insert(id.clone(), Entry { path: dir.clone(), used_at_ms: now_ms(), walking });
    });
    api.projects.open(&api, &dir).map_err(Refusal)?;
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let theirs = folder::parse_lock(&take.lock).map_err(Refusal)?;
    let names = names_in(&take.yaml);
    folder.update_lock(|lock| lock.absorb(&theirs, |key| names.contains(key), false)).map_err(Refusal)?;
    let mut wants = vec![];
    for code in &take.code {
        let at = inside(&dir, &code.path).ok_or_else(|| usage(format!("{:?} is no path in a project's folder", code.path)))?;
        if folder::hash(&at).is_ok_and(|got| got == code.sha256) {
            continue;
        }
        match uploaded(&api, &code.sha256) {
            Some(from) => {
                let bytes = std::fs::read(&from).map_err(|e| io(&e))?;
                std::fs::create_dir_all(at.parent().unwrap_or(&dir)).map_err(|e| io(&e))?;
                platform::durable::replace(&at, &bytes, 0o644).map_err(|e| io(&e))?;
            }
            None => wants.push(code.sha256.clone()),
        }
    }
    wants.extend(standing_files(&api, folder, &take.yaml).map_err(Refusal)?);
    Ok(Json(Taken { id, path: dir, wants }))
}

/// `GET /v1/projects/{id}/taken`: what this machine has of a project it holds, for the computer that handed it.
pub(super) async fn taken(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Json<Holding>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let folder = &project.folder;
    let (lock, yaml) = (folder.lock().map_err(Refusal)?, folder.text().map_err(Refusal)?);
    let files = (lock.files.values())
        .filter_map(|file| {
            let here = std::fs::metadata(folder.path(&file.path)).ok().filter(std::fs::Metadata::is_file);
            let machine = if here.is_some() { String::new() } else { file.machine.clone() };
            (here.is_some() || file.stays()).then(|| HeldFile { path: file.path.clone(), sha256: file.sha256.clone(), length: here.map_or(file.length, |meta| meta.len()), machine })
        })
        .collect();
    let busy = *project.walking.lock().expect("walking lock");
    Ok(Json(Holding { etag: yaml_etag(&yaml), yaml, lock: lock.written(), files, busy, board: super::board_of(&project) }))
}

/// The asset names a text holds, by key.
fn names_in(text: &str) -> HashSet<String> {
    protocol::yaml::keys_at(text, &["assets"]).unwrap_or_default().iter().map(|name| name_key(name)).collect()
}

/// `path`, written relative to `dir`, if it stays in it.
fn inside(dir: &FsPath, path: &str) -> Option<PathBuf> {
    let path = FsPath::new(path);
    path.components().all(|part| matches!(part, Component::Normal(_))).then(|| dir.join(path))
}

/// The upload this Calcifer holds of `sha256`, if it does.
fn uploaded(api: &Api, sha256: &str) -> Option<PathBuf> {
    crate::runs::files::held(&api.calcifer, &sha256.parse().ok()?)
}

/// The files what stands in `text` names that aren't here: each uploaded is kept now, and the rest are wanted. A text that doesn't read
/// names none, and its render says why.
fn standing_files(api: &Api, folder: &Folder, text: &str) -> Result<Vec<String>, Error> {
    let lock = folder.lock()?;
    let Ok(doc) = Doc::of(folder, text, &lock) else { return Ok(vec![]) };
    let stand = doc.stand(&lock);
    let named: BTreeSet<&String> = stand.iter().filter_map(|standing| standing.files.as_ref()).flat_map(|files| files.values()).collect();
    let mut wants = vec![];
    for sha256 in named {
        let Some(file) = lock.files.get(sha256) else { continue };
        if file.stays() || folder.path(&file.path).is_file() {
            continue;
        }
        match uploaded(api, sha256) {
            Some(at) => drop(folder.take_held(&at, sha256).map_err(|why| Error::new(Kind::Unavailable, "project_io", why))?),
            None => wants.push(sha256.clone()),
        }
    }
    Ok(wants)
}

/// A rental's walk is work asked for, so it holds the rental while it owes and something moved in the last 15 minutes: a run's progress,
/// here or on another machine, a send, a cut. A walk that stopped moving, or a Calcifer that died, lets the rental go a window later.
pub(super) fn keep_clock(api: &Arc<Api>, project: &Arc<Project>) -> tokio::task::JoinHandle<()> {
    let (api, project) = (api.clone(), project.clone());
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(RENEW).await;
            let quiet = now_ms() - project.moved.load(Ordering::Relaxed);
            if api.machines.rental_here().is_none() || quiet > QUIET_MS {
                continue;
            }
            if let Err(error) = api.machines.keepalive("local").await {
                warn!("renewing this rental for {}'s walk: {}", project.folder.dir.display(), error.message);
            }
        }
    })
}

// The computer that handed it.

/// The render `asked` when it isn't this computer's to walk: handed to the machine named first, or to the one holding it. None: it's
/// walked here.
pub(super) async fn render(api: &Arc<Api>, project: &Arc<Project>, asked: &Runs, key: Option<&(String, String)>) -> Result<Option<(StatusCode, Json<Asked>)>, Refusal> {
    let walk = project.folder.walk_now().map_err(Refusal)?;
    let first = match asked.on.first() {
        Some(On::Machine(name)) if !name.eq_ignore_ascii_case("local") => Some(name.clone()),
        _ => None,
    };
    let machine = match (&walk.held, &first) {
        (Some(held), Some(_)) => held.machine.clone(),
        (Some(held), None) => {
            let said = format!("{} holds this project's render: ask it of {0}, or here once it ends", held.machine);
            return Err(Refusal(Error::new(Kind::Conflict, "project_held", said).next(status_next(&project.folder)[0].clone())));
        }
        // A render going here keeps going here, and drives what this one names too.
        (None, Some(first)) if !*project.walking.lock().expect("walking lock") => first.clone(),
        _ => return Ok(None),
    };
    // A rental still booting takes it: the watcher hands it over once it answers.
    let row = api.machines.here(&machine).map_err(Refusal)?.ok_or_else(|| Refusal(crate::machines::not_found(&machine)))?;
    if row.state == protocol::calcifer::MachineState::Ended {
        return Err(Refusal(Error::new(Kind::Conflict, "machine_ended", format!("{} has ended", row.name)).next("cozy machine ls")));
    }
    let machine = row.name;
    Doc::load(&project.folder).map_err(Refusal)?;
    let mut on = vec![];
    for named in &asked.on {
        match named {
            On::Machine(name) if name.eq_ignore_ascii_case(&machine) => on.insert(0, On::Machine("local".into())),
            On::Machine(name) if name.eq_ignore_ascii_case("local") => {
                let said = format!("{machine} holds this render and never drives this computer: local is left out");
                let _ = project.events.send(Event::Warning { message: said, name: None });
            }
            named => on.push(named.clone()),
        }
    }
    if !on.contains(&On::Machine("local".into())) {
        let said = format!("{machine} holds this render, and drives {}", on.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "));
        let _ = project.events.send(Event::Said { message: said, name: None });
    }
    let (key, digest) = key.cloned().unwrap_or_else(|| (super::new_key(), String::new()));
    let answer = |status, joined| {
        let asked = Asked { id: project_id(&project.folder.dir), path: project.folder.dir.clone(), joined, added: 1, skipped: vec![], queued: vec![], next: status_next(&project.folder), board: project.jobs.board() };
        Ok(Some((status, Json(asked))))
    };
    match walk.held.as_ref().and_then(|held| held.keys.get(&key)) {
        Some(seen) if *seen == digest => return answer(StatusCode::OK, true),
        Some(_) => return Err(Refusal(Error::new(Kind::Conflict, "idempotency_conflict", format!("Idempotency-Key {key} asked another render already")))),
        None => {}
    }
    let joined = walk.held.is_some();
    if !joined {
        project.jobs.begin();
        *project.ended.lock().expect("ended lock") = None;
    }
    let handing = Handing { key: key.clone(), runs: Runs { on, ..asked.clone() } };
    (project.folder.update_walk(|walk| {
        let held = walk.held.get_or_insert_with(|| Held { machine: machine.clone(), ..Held::default() });
        held.asking.push(handing.clone());
        held.keys.insert(key.clone(), digest.clone());
    }))
    .map_err(Refusal)?;
    *project.walking.lock().expect("walking lock") = true;
    remember(api);
    watch(api, project);
    answer(StatusCode::ACCEPTED, joined)
}

/// `body` asked of the holder's copy of this project instead (`cancel`, `approve`, `unpin`), and its answer: what changes the render is
/// the holder's to do. None: nothing holds it but this computer.
pub(super) async fn forward<T: DeserializeOwned>(api: &Arc<Api>, project: &Arc<Project>, action: &str, body: &Bytes) -> Result<Option<T>, Refusal> {
    let Some(held) = project.folder.walk_now().map_err(Refusal)?.held else { return Ok(None) };
    if held.id.is_empty() {
        let said = format!("{} is taking this render over: ask again in a moment", held.machine);
        return Err(Refusal(Error::new(Kind::Unavailable, "project_handing", said)));
    }
    let (_, link) = api.machines.linked(&held.machine).map_err(Refusal)?;
    let answer = call(&link, &held.machine, Method::POST, &format!("/v1/projects/{}/{action}", held.id), Some(body.to_vec()), None).await.map_err(Refusal)?;
    project.asked.notify_one();
    let unread = |e: serde_json::Error| Refusal(Error::new(Kind::Internal, "holder_answer", format!("{}: {e}", held.machine)));
    Ok(Some(serde_json::from_slice(&answer).map_err(unread)?))
}

/// Who holds `project`'s render, and what of it isn't here yet: one place says when what isn't collected is lost, the end of that
/// machine's idle window, which ends it.
pub(super) fn held_by(api: &Api, project: &Project) -> Option<HeldBy> {
    let held = project.folder.walk_now().ok()?.held?;
    let until_ms = api.machines.frame(&held.machine).map(|frame| frame.idle_deadline_unix_ms).filter(|at| *at > 0);
    let (files, bytes) = *project.collecting.lock().expect("collecting lock");
    Some(HeldBy { machine: held.machine, until_ms, files, bytes, heard_at_ms: project.heard.load(Ordering::Relaxed) })
}

/// Starts the project's watcher, or nudges the one going.
pub(super) fn watch(api: &Arc<Api>, project: &Arc<Project>) {
    if project.watching.swap(true, Ordering::SeqCst) {
        return project.asked.notify_one();
    }
    let (api, project) = (api.clone(), project.clone());
    tokio::spawn(async move {
        watching(&api, &project).await;
        project.watching.store(false, Ordering::SeqCst);
        if project.folder.walk_now().is_ok_and(|walk| walk.held.is_some()) {
            watch(&api, &project);
        }
    });
}

/// What the watcher does next.
enum Next {
    Now,
    Later,
    Over,
}

async fn watching(api: &Arc<Api>, project: &Arc<Project>) {
    let (mut mirror, mut wait): (Option<(String, tokio::task::JoinHandle<()>)>, Duration) = (None, TICK);
    loop {
        let held = match project.folder.walk_now() {
            Ok(walk) => match walk.held {
                Some(held) => held,
                None => break,
            },
            Err(error) => {
                warn!("{}: {}", project.folder.dir.display(), error.message);
                tokio::time::sleep(MOST).await;
                continue;
            }
        };
        if !held.id.is_empty() && mirror.as_ref().is_none_or(|(id, _)| *id != held.id) {
            if let Some((_, old)) = mirror.take() {
                old.abort();
            }
            mirror = Some((held.id.clone(), tokio::spawn(mirroring(api.clone(), project.clone(), held.machine.clone(), held.id.clone()))));
        }
        match step(api, project, &held).await {
            Ok(Next::Over) => break,
            Ok(Next::Now) => continue,
            Ok(Next::Later) => wait = TICK,
            Err(error) if error.code == "machine_ended" || (error.code == "project_unknown" && !held.id.is_empty()) => {
                let said = format!("{} is gone before this computer collected all of its render: {}", held.machine, error.message);
                release(api, project, Some(Error::new(Kind::Unavailable, "holder_lost", said).next(status_next(&project.folder)[0].clone()))).await;
                break;
            }
            Err(error) if error.code == "project_changed" && !held.id.is_empty() => continue,
            Err(error) => {
                warn!("{}: {} doesn't answer: {}", project.folder.dir.display(), held.machine, error.message);
                wait = (wait * 2).min(MOST);
            }
        }
        tokio::select! {
            () = project.asked.notified() => {}
            () = tokio::time::sleep(wait) => {}
        }
    }
    if let Some((_, mirror)) = mirror {
        mirror.abort();
    }
}

/// One look at what the holder has, and one move toward this computer having it too.
async fn step(api: &Arc<Api>, project: &Arc<Project>, held: &Held) -> Result<Next, Error> {
    let (row, link) = api.machines.linked(&held.machine)?;
    let folder = &project.folder;
    let holding: Option<Holding> = match held.id.is_empty() {
        true => None,
        false => Some(json(&call(&link, &row.name, Method::GET, &format!("/v1/projects/{}/taken", held.id), None, None).await?, &row.name)?),
    };
    if let Some(holding) = &holding {
        project.heard.store(now_ms(), Ordering::Relaxed);
        if let Some(board) = holding.board.clone() {
            mirrored(api, project, &row.name, Event::Board { board });
        }
        take_in(folder, &row.name, holding)?;
        project.jobs.held(held_by(api, project));
        if holding.yaml != held.base {
            pins_back(folder, &held.base, &holding.yaml)?;
            folder.update_walk(|walk| walk.held.iter_mut().for_each(|mine| mine.base.clone_from(&holding.yaml)))?;
        }
    }
    let here = folder.text()?;
    if holding.is_none() || here != holding.as_ref().map_or("", |holding| holding.yaml.as_str()) || !held.asking.is_empty() {
        hand(api, project, &link, &row.name, holding.as_ref().map(|holding| holding.etag.clone()), &here).await?;
        let id = folder.walk_now()?.held.map(|held| held.id).unwrap_or_default();
        for asking in &held.asking {
            drive(api, project, &link, &row.name, &asking.runs.on).await?;
            ask(project, &link, &row.name, &id, asking).await?;
        }
        return Ok(Next::Now);
    }
    let holding = holding.expect("read");
    let left = collect(api, project, &link, &row.name, &held.id, &holding).await?;
    if !holding.busy && left == 0 {
        release(api, project, None).await;
        return Ok(Next::Over);
    }
    Ok(Next::Later)
}

/// The holder's lock taken in, and each file it has that isn't here listed as on it.
fn take_in(folder: &Folder, machine: &str, holding: &Holding) -> Result<(), Error> {
    let theirs = folder::parse_lock(&holding.lock)?;
    let names = names_in(&folder.text()?);
    folder.update_lock(|lock| {
        lock.absorb(&theirs, |key| names.contains(key), true);
        for file in holding.files.iter().filter(|file| !folder.path(&file.path).is_file()) {
            lock.away(&file.sha256, machine, file.length, Some(&file.path));
        }
    })
}

/// What the holder wrote of the text since `base`, written here too: a pin as a take landed (`--approve`), one an `--outdated` render
/// took away. Here unchanged since, its text is the holder's.
fn pins_back(folder: &Folder, base: &str, there: &str) -> Result<(), Error> {
    let pins = |text: &str| -> Option<BTreeMap<String, (String, Option<String>)>> {
        let doc = Doc::read(folder, text).ok()?;
        Some(doc.assets.iter().map(|asset| (name_key(&asset.name), (asset.name.clone(), asset.pin.clone()))).collect())
    };
    folder.edit_project(|here| {
        if here == base {
            return Ok(there.to_owned());
        }
        let (Some(was), Some(now)) = (pins(base), pins(there)) else { return Ok(here.to_owned()) };
        let mut text = here.to_owned();
        for (key, (name, pin)) in &now {
            if was.get(key).is_some_and(|(_, old)| old != pin) {
                text = match pin {
                    Some(sha256) => assetfile::approved(&text, name, sha256)?,
                    None => assetfile::unapproved(&text, name)?,
                };
            }
        }
        Ok(text)
    })
}

/// The text, the lock and what the holder lacks of the files, given, until it lacks nothing it can be given.
async fn hand(api: &Arc<Api>, project: &Project, link: &Link, machine: &str, etag: Option<String>, here: &str) -> Result<(), Error> {
    let folder = &project.folder;
    let code = code_files(api, project);
    let name = folder.dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let take = Take { from: project_id(&folder.dir), name, yaml: here.to_owned(), base: etag, lock: folder.lock()?.written(), code: code.iter().map(|(file, _)| file.clone()).collect() };
    let body = serde_json::to_vec(&take).expect("a take serializes");
    let mut gave = false;
    loop {
        let taken: Taken = json(&call(link, machine, Method::POST, "/v1/projects/take", Some(body.clone()), None).await?, machine)?;
        folder.update_walk(|walk| {
            for held in walk.held.iter_mut() {
                (held.id, held.base) = (taken.id.clone(), here.to_owned());
            }
        })?;
        if taken.wants.is_empty() || gave {
            if !taken.wants.is_empty() {
                let said = format!("{machine} lacks {} files this computer can't give it either: what reads them fails there", taken.wants.len());
                let _ = project.events.send(Event::Warning { message: said, name: None });
            }
            return Ok(());
        }
        let lock = folder.lock()?;
        for sha256 in &taken.wants {
            let code = code.iter().find(|(file, _)| file.sha256 == *sha256).map(|(_, at)| at.clone());
            let file = lock.files.get(sha256).map(|file| folder.path(&file.path)).filter(|at| at.is_file());
            if let Some(at) = code.or(file) {
                upload(link, machine, &at, sha256).await?;
            }
        }
        gave = true;
    }
}

/// The local code the project's tools name, each file by its path in the folder. What lies outside the folder can't go, and is said.
fn code_files(api: &Arc<Api>, project: &Project) -> Vec<(HeldFile, PathBuf)> {
    let Ok(named) = super::local_code_named(api, &project_id(&project.folder.dir)) else { return vec![] };
    let mut files = vec![];
    for (written, path) in named {
        let listed = match path.is_dir() {
            true => crate::local_code::source_files(&path, &mut vec![]).map(|found| found.into_iter().map(|rel| path.join(rel)).collect()).unwrap_or_default(),
            false => vec![path.clone()],
        };
        for at in listed {
            let Ok(rel) = at.strip_prefix(&project.folder.dir) else {
                let said = format!("{written} is outside the project's folder, so the machine that holds its render can't run it");
                let _ = project.events.send(Event::Warning { message: said, name: None });
                break;
            };
            if let Ok(sha256) = folder::hash(&at) {
                files.push((HeldFile { path: rel.to_string_lossy().into_owned(), sha256, ..HeldFile::default() }, at));
            }
        }
    }
    files
}

/// Each other machine a render names, readied for the holder to drive: it admits the holder's key, and the holder knows it by its
/// name here. A machine X sends to directly, never through this computer.
async fn drive(api: &Arc<Api>, project: &Project, link: &Link, machine: &str, on: &[On]) -> Result<(), Error> {
    let driven = project.folder.walk_now()?.held.map(|held| held.drives).unwrap_or_default();
    let others: Vec<&String> = (on.iter())
        .filter_map(|on| match on {
            On::Machine(name) if name != "local" && !driven.contains(name) => Some(name),
            _ => None,
        })
        .collect();
    if others.is_empty() {
        return Ok(());
    }
    let local: Machine = json(&call(link, machine, Method::GET, "/v1/machines/local", None, None).await?, machine)?;
    let key = local.controller_key.ok_or_else(|| Error::new(Kind::Unavailable, "machine_unanswered", format!("{machine} names no controller key")))?;
    for other in others {
        let (row, theirs) = api.machines.linked(other)?;
        let Some(endpoint) = row.endpoint.as_ref() else { continue };
        call(&theirs, &row.name, Method::PUT, &format!("/v1/machines/local/authorized-keys/{key}"), None, None).await?;
        let add = NewMachine::Add { name: row.name.clone(), address: endpoint.address.clone(), fingerprint: protocol::ids::Digest::from_bytes(endpoint.pin) };
        match call(link, machine, Method::POST, "/v1/machines", Some(serde_json::to_vec(&add).expect("an add serializes")), None).await {
            Err(error) if error.code != "machine_name_taken" => return Err(error),
            _ => {}
        }
        project.folder.update_walk(|walk| walk.held.iter_mut().for_each(|held| {
            held.drives.insert(row.name.clone());
        }))?;
        let said = format!("{machine} drives {} directly", row.name);
        let _ = project.events.send(Event::Said { message: said, name: None });
    }
    Ok(())
}

/// The holder's key taken back from each machine it drove, as far as they answer: one that doesn't keeps it until it ends.
async fn undrive(api: &Api, held: &Held) {
    if held.drives.is_empty() {
        return;
    }
    let Ok((_, link)) = api.machines.linked(&held.machine) else { return };
    let Ok(bytes) = call(&link, &held.machine, Method::GET, "/v1/machines/local", None, None).await else { return };
    let Some(key) = json::<Machine>(&bytes, &held.machine).ok().and_then(|local| local.controller_key) else { return };
    for other in &held.drives {
        if let Ok((row, theirs)) = api.machines.linked(other) {
            let _ = call(&theirs, &row.name, Method::DELETE, &format!("/v1/machines/local/authorized-keys/{key}"), None, None).await;
        }
    }
}

/// One render asked of the holder under its key, which it answers again the same; then it's no longer asked here. A refusal ends it,
/// said as its render's end.
async fn ask(project: &Project, link: &Link, machine: &str, id: &str, asking: &Handing) -> Result<(), Error> {
    let body = serde_json::to_vec(&asking.runs).expect("an ask serializes");
    let answer = call(link, machine, Method::POST, &format!("/v1/projects/{id}/runs"), Some(body), Some(&asking.key)).await;
    match answer {
        Ok(_) => {}
        Err(error) if error.kind == Kind::Unavailable || error.code.starts_with("machine_") => return Err(error),
        Err(error) => {
            let _ = project.events.send(Event::Failed { error: error.clone(), name: None, run: None });
            *project.ended.lock().expect("ended lock") = Some(Event::Ended { error: Some(error), rendered: None });
        }
    }
    project.folder.update_walk(|walk| walk.held.iter_mut().for_each(|held| held.asking.retain(|mine| mine.key != asking.key)))?;
    let said = format!("{machine} took the render over: this computer collects what it makes, and can sleep or close meanwhile");
    let _ = project.events.send(Event::Said { message: said, name: None });
    Ok(())
}

/// Each file of the holder's folder that isn't here, fetched. How many are left.
async fn collect(api: &Api, project: &Project, link: &Link, machine: &str, id: &str, holding: &Holding) -> Result<u64, Error> {
    let folder = &project.folder;
    let wanted: Vec<&HeldFile> = holding.files.iter().filter(|file| file.machine.is_empty() && !folder.path(&file.path).is_file()).collect();
    let tell = |files: usize, bytes: u64| {
        *project.collecting.lock().expect("collecting lock") = (files as u64, bytes);
        project.jobs.held(held_by(api, project));
    };
    let mut bytes: u64 = wanted.iter().map(|file| file.length).sum();
    tell(wanted.len(), bytes);
    for (i, file) in wanted.iter().enumerate() {
        fetch(folder, link, machine, id, file).await?;
        bytes = bytes.saturating_sub(file.length);
        tell(wanted.len() - i - 1, bytes);
    }
    Ok(0)
}

/// `file` from the holder's folder into this one's `files/`, resumed from what came before, and kept only if its bytes are its hash.
async fn fetch(folder: &Folder, link: &Link, machine: &str, id: &str, file: &HeldFile) -> Result<(), Error> {
    let name = file.path.rsplit('/').next().unwrap_or_default();
    let dir = folder.dir.join(COLLECTING);
    let io = |e: std::io::Error| Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", dir.display()));
    tokio::fs::create_dir_all(&dir).await.map_err(io)?;
    let (part, whole) = (dir.join(format!("{name}.part")), dir.join(name));
    let mut have = tokio::fs::metadata(&part).await.map_or(0, |meta| meta.len());
    if have > file.length {
        have = 0;
    }
    let mut headers = HeaderMap::new();
    headers.insert(header::RANGE, HeaderValue::from_str(&format!("bytes={have}-")).expect("ASCII"));
    let answer = link.forward(Method::GET, &format!("/v1/projects/{id}/files/{name}"), headers, None).await.map_err(|e| refusal(machine, e))?;
    let status = answer.status();
    if status == StatusCode::RANGE_NOT_SATISFIABLE && have == file.length {
        // It came whole before.
    } else if status.is_success() {
        let mut out = tokio::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&part).await.map_err(io)?;
        let from = if status == StatusCode::PARTIAL_CONTENT { have } else { 0 };
        out.set_len(from).await.map_err(io)?;
        out.seek(std::io::SeekFrom::Start(from)).await.map_err(io)?;
        let mut body = answer.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| Error::new(Kind::Unavailable, "machine_unanswered", format!("{machine}: {e}")))?;
            out.write_all(&chunk).await.map_err(io)?;
        }
        out.sync_all().await.map_err(io)?;
    } else {
        return Err(refused(machine, status, &answer.bytes().await.unwrap_or_default()));
    }
    tokio::fs::rename(&part, &whole).await.map_err(io)?;
    let kept = tokio::task::block_in_place(|| folder.take(&whole))?;
    if kept != file.sha256 {
        warn!("{}: {name} from {machine} is {kept}, not {}", folder.dir.display(), file.sha256);
    }
    Ok(())
}

/// `path`, of `sha256`, given to the holder's Calcifer (`PUT /v1/files`), from the bytes it lacks.
async fn upload(link: &Link, machine: &str, path: &FsPath, sha256: &str) -> Result<(), Error> {
    let target = format!("/v1/files/sha256:{}", folder::hex_of(sha256));
    let io = |e: std::io::Error| Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", path.display()));
    let length = std::fs::metadata(path).map_err(io)?.len();
    loop {
        let head = link.forward(Method::HEAD, &target, HeaderMap::new(), None).await.map_err(|e| refusal(machine, e))?;
        if head.status() == StatusCode::OK {
            return Ok(());
        }
        let held = head.headers().get(HELD).and_then(|value| value.to_str().ok()).and_then(parse_held).unwrap_or_default();
        let (start, end) = missing(&held, length).first().copied().unwrap_or((0, length));
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(end - start));
        if length > 0 {
            headers.insert(header::CONTENT_RANGE, HeaderValue::from_str(&format!("bytes {start}-{}/{length}", end - 1)).expect("ASCII"));
        }
        let mut file = tokio::fs::File::open(path).await.map_err(io)?;
        file.seek(std::io::SeekFrom::Start(start)).await.map_err(io)?;
        let body = tokio_util::io::ReaderStream::new(tokio::io::AsyncReadExt::take(file, end - start));
        let answer = link.forward(Method::PUT, &target, headers, Some(reqwest::Body::wrap_stream(body))).await.map_err(|e| refusal(machine, e))?;
        match answer.status() {
            StatusCode::OK | StatusCode::CREATED => return Ok(()),
            StatusCode::NO_CONTENT => {}
            status => return Err(refused(machine, status, &answer.bytes().await.unwrap_or_default())),
        }
    }
}

/// The hold is over: the holder ended its walk and everything it made is here, or it's gone (`error`). Its render's end is said now.
async fn release(api: &Arc<Api>, project: &Project, error: Option<Error>) {
    if let Ok(Some(held)) = project.folder.walk_now().map(|walk| walk.held) {
        undrive(api, &held).await;
    }
    if let Err(error) = project.folder.update_walk(|walk| walk.held = None) {
        warn!("{}: {}", project.folder.dir.display(), error.message);
    }
    *project.walking.lock().expect("walking lock") = false;
    remember(api);
    *project.collecting.lock().expect("collecting lock") = (0, 0);
    project.jobs.held(None);
    let ended = project.ended.lock().expect("ended lock").take();
    let ended = match (error, ended) {
        (Some(error), _) => Event::Ended { error: Some(error), rendered: None },
        (None, Some(ended)) => ended,
        (None, None) => ended_of(&project.jobs.board()),
    };
    let _ = project.events.send(ended);
    crate::events::project_changed(&project.folder.dir);
}

/// A render's end as its last board says it, when the holder's own word of it never came here.
fn ended_of(board: &Board) -> Event {
    let done = |noun: &str| board.jobs.iter().filter(|job| job.state == JobState::Done && job.noun == noun && job.number.is_some()).count() as u32;
    let rendered = Rendered { takes: done("take"), versions: done("version"), elapsed_ms: board.elapsed_ms.unwrap_or_default(), execution_ms: 0 };
    let failed: Vec<&Error> = board.jobs.iter().filter(|job| job.state == JobState::Failed).filter_map(|job| job.error.as_ref()).collect();
    let error = match failed.as_slice() {
        [] => None,
        [one] => Some((*one).clone()),
        [first, ..] => {
            let said = format!("rendered {} of {}; {} failed: {}", rendered.takes + rendered.versions, board.jobs.len(), failed.len(), first.message);
            Some(Error::new(first.kind, "render_incomplete", said).cause(first.code.clone()))
        }
    };
    Event::Ended { error, rendered: Some(rendered).filter(|rendered| rendered.takes + rendered.versions > 0) }
}

/// The holder's events for this project, drawn here as this project's: its board, each job, what it says. Its end is kept until what it
/// made is here, and its runs' ids are its own, so their starts aren't told.
async fn mirroring(api: Arc<Api>, project: Arc<Project>, machine: String, id: String) {
    let mut wait = TICK;
    loop {
        if let Ok((_, link)) = api.machines.linked(&machine) {
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT, HeaderValue::from_static("text/event-stream"));
            if let Ok(answer) = link.forward(Method::GET, &format!("/v1/projects/{id}/events"), headers, None).await
                && answer.status().is_success()
            {
                wait = TICK;
                let mut events = answer.bytes_stream().eventsource();
                while let Some(Ok(sent)) = events.next().await {
                    if let Ok(event) = serde_json::from_str::<Event>(&sent.data) {
                        mirrored(&api, &project, &machine, event);
                    }
                }
            }
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(MOST);
    }
}

fn mirrored(api: &Api, project: &Project, machine: &str, event: Event) {
    let here = |named: &mut Option<String>| {
        if named.as_deref() == Some("local") {
            *named = Some(machine.to_owned());
        }
    };
    match event {
        Event::Board { mut board } => {
            board.on.iter_mut().filter(|on| *on == "local").for_each(|on| *on = machine.to_owned());
            board.jobs.iter_mut().for_each(|job| here(&mut job.machine));
            board.held = held_by(api, project);
            project.jobs.mirror(board);
        }
        Event::Job { mut job, seq } => {
            here(&mut job.machine);
            project.jobs.mirror_job(&job, seq);
            let _ = project.events.send(Event::Job { job, seq });
            project.asked.notify_one();
        }
        Event::Ended { .. } => {
            *project.ended.lock().expect("ended lock") = Some(event);
            project.asked.notify_one();
        }
        Event::Started { .. } => {}
        event => {
            if matches!(event, Event::Landed { .. }) {
                project.asked.notify_one();
            }
            let _ = project.events.send(event);
        }
    }
}

/// One request to the holder's Calcifer through its door, and its answer's body; a refusal there is the error.
async fn call(link: &Link, machine: &str, method: Method, target: &str, body: Option<Vec<u8>>, key: Option<&str>) -> Result<Bytes, Error> {
    let mut headers = HeaderMap::new();
    if body.is_some() {
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    if let Some(key) = key.and_then(|key| HeaderValue::from_str(key).ok()) {
        headers.insert("idempotency-key", key);
    }
    let answer = link.forward(method, target, headers, body.map(reqwest::Body::from)).await.map_err(|e| refusal(machine, e))?;
    let status = answer.status();
    let bytes = answer.bytes().await.map_err(|e| Error::new(Kind::Unavailable, "machine_unanswered", format!("{machine}: {e}")))?;
    match status.is_success() {
        true => Ok(bytes),
        false => Err(refused(machine, status, &bytes)),
    }
}

/// What the holder's refusal said, as its own error.
fn refused(machine: &str, status: StatusCode, body: &[u8]) -> Error {
    match serde_json::from_slice::<Envelope>(body) {
        Ok(envelope) => envelope.error,
        Err(_) => Error::new(Kind::Unavailable, "machine_unanswered", format!("{machine} answered {status}")),
    }
}

fn json<T: DeserializeOwned>(bytes: &[u8], machine: &str) -> Result<T, Error> {
    serde_json::from_slice(bytes).map_err(|e| Error::new(Kind::Unavailable, "machine_unanswered", format!("{machine}: {e}")))
}
