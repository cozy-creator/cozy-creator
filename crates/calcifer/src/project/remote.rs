//! A project's files at its user's Tensorhub account (xs-143), moved the way TensorFS moves weights: by
//! content, so what the Hub holds is never sent again, and a killed transfer goes on from what landed.
//! Push records them as a private commit of the account's one model `project-assets`: objects no commit
//! reaches are swept within days, and a commit stays until its owner deletes it (`cozy model delete`).
//! A file too big for one object goes as parts, each its own object, which project.lock lists.
//! Each runs in its own task and says how it goes on the project's events: a client that goes stops
//! following, never the work, and a stopping Calcifer waits for no transfer.
use std::{
    fs::File,
    future::Future,
    io::{Read as _, Seek as _, SeekFrom},
    path::Path as FilePath,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
    http::StatusCode,
};
use futures_util::{StreamExt as _, TryStreamExt as _, stream};
use ::api::hub::{Client, Credential};
use platform::durable::FileAt as _;
use api::{
    config::Origin,
    error::{Error, Kind},
    calcifer::hub::At,
    calcifer::project::{Event, Opened, Skipped, Transferred},
};
use sha2::{Digest as _, Sha256};
use tensorfs::{
    err::{Code, Refusal as Failed},
    fetch::{self, is_loopback_origin, Endpoint, Payload, Request, SourcePolicy, TokenProvider},
    format::{Header, ObjectRef, Tree},
};

use super::{Project, folder};
use crate::api::{Api, Refusal};

/// The account's model whose commits keep its projects' files.
const MODEL: &str = "project-assets";
/// What a commit of it is labelled.
const LANE: &str = "project";
/// Files moved at once.
const AT_ONCE: usize = 4;
/// The most one object at the Hub can be (a single PUT to its storage).
const MOST: u64 = 5 << 30;
/// What a file over `MOST` is split into, so a killed transfer goes on from the part it reached.
const PART: u64 = 1 << 30;
/// How often a transfer says how far it is, between files.
const TICK: Duration = Duration::from_secs(5);

/// A signed-in client's Bearer on each request.
struct Signed(Client);

impl TokenProvider for Signed {
    fn headers<'a>(&'a self, _: &'a Request<'a>) -> fetch::Boxed<'a, tensorfs::err::Result<Vec<(String, String)>>> {
        Box::pin(async move {
            let Credential::User(session) = self.0.credential() else { unreachable!("a signed-in client") };
            let bearer = session.bearer(&self.0.anonymous()).await;
            let bearer = bearer.map_err(|e| Failed { code: Code::CREDENTIAL_REQUIRED, detail: e.to_string() })?;
            Ok(vec![("authorization".into(), format!("Bearer {}", bearer.token))])
        })
    }
}

/// `hub` as this computer's signed-in user, and its objects.
fn signed(api: &Api, hub: &Origin) -> Result<(Client, Endpoint), Error> {
    let client = api.machines.hubs.client(hub)?;
    let policy = SourcePolicy { any_host: true, allow_local: is_loopback_origin(hub), max_redirects: 5, ..Default::default() };
    let provider: Arc<dyn TokenProvider> = Arc::new(Signed(client.clone()));
    let endpoint = Endpoint::new(&fetch::Client::new(policy), hub, Some(provider)).map_err(failed)?;
    Ok((client, endpoint))
}

fn failed(error: Failed) -> Error {
    Error::new(Kind::Unavailable, "project_transfer", format!("{}: {}", error.code.as_str(), error.detail))
}

fn local(detail: impl ToString) -> Failed {
    Failed { code: Code::IO_FAILED, detail: detail.to_string() }
}

/// `work`, saying how far it is (`moved`) every `TICK` that moved something.
async fn ticking<T>(project: &Project, moved: impl Fn() -> u64, work: impl Future<Output = T>) -> T {
    let mut work = std::pin::pin!(work);
    let (mut tick, mut said) = (tokio::time::interval(TICK), 0);
    loop {
        tokio::select! {
            done = &mut work => return done,
            _ = tick.tick() => {
                let now = moved();
                if now != said {
                    said = now;
                    let _ = project.events.send(Event::Moved { file: None, bytes: now });
                }
            }
        }
    }
}

/// `sha256` as a digest with no scheme.
fn hex(sha256: &str) -> &str {
    sha256.trim_start_matches("sha256:")
}

/// What a transfer moved so far: files, and their bytes.
#[derive(Default)]
struct Tally {
    moved: AtomicU64,
    bytes: AtomicU64,
    /// By path, why each was left out.
    skipped: Mutex<Vec<(String, String)>>,
}

impl Tally {
    fn skip(&self, path: String, why: String) {
        self.skipped.lock().expect("skipped lock").push((path, why));
    }

    /// What it left out.
    fn skipped(self) -> Vec<Skipped> {
        let skipped = self.skipped.into_inner().expect("skipped lock");
        skipped.into_iter().map(|(path, why)| Skipped { path, why }).collect()
    }
}

/// The files project.lock names, but what stays on a machine and isn't here: that is no file of this computer or the Hub's.
fn named(project: &Project) -> Result<Vec<folder::File>, Error> {
    let at = &project.folder;
    Ok(at.lock()?.files.into_values().filter(|file| !file.stays() || at.path(&file.path).is_file()).collect())
}

/// The sha256 of the file at `path`, and its parts.
fn parts_of(path: &FilePath) -> std::io::Result<(String, Vec<ObjectRef>)> {
    let mut file = File::open(path)?;
    let (mut whole, mut parts, mut buffer) = (Sha256::new(), vec![], vec![0; 1 << 20]);
    loop {
        let (mut part, mut length) = (Sha256::new(), 0);
        while length < PART {
            let most = buffer.len().min((PART - length) as usize);
            let read = file.read(&mut buffer[..most])?;
            if read == 0 {
                break;
            }
            whole.update(&buffer[..read]);
            part.update(&buffer[..read]);
            length += read as u64;
        }
        if length > 0 {
            parts.push(ObjectRef { sha256: hex::encode(part.finalize()), length });
        }
        if length < PART {
            return Ok((hex::encode(whole.finalize()), parts));
        }
    }
}

/// A file over `MOST`'s parts: those project.lock lists, else read from the file, checked against its
/// sha256 and listed there, so a second push need not read it again. None: the file isn't its sha256.
async fn split(project: &Project, file: &folder::File, at: &FilePath) -> Result<Option<Vec<ObjectRef>>, Failed> {
    if !file.parts.is_empty() {
        return Ok(Some(file.parts.clone()));
    }
    let reading = at.to_owned();
    let (sha256, parts) = tokio::task::spawn_blocking(move || parts_of(&reading)).await.map_err(local)?.map_err(local)?;
    if sha256 != hex(&file.sha256) {
        return Ok(None);
    }
    let listed = |lock: &mut folder::Lock| {
        if let Some(entry) = lock.files.get_mut(&file.sha256) {
            entry.parts.clone_from(&parts);
        }
    };
    project.folder.update_lock(listed).map_err(local)?;
    Ok(Some(parts))
}

/// What the Hub holds of `file`: the objects it goes as, or None if any is not there.
async fn held(endpoint: &Endpoint, file: &folder::File) -> Result<Option<Vec<ObjectRef>>, Failed> {
    if file.parts.is_empty() {
        return Ok(endpoint.object(&file.sha256).await?.map(|object| vec![object]));
    }
    for part in &file.parts {
        if endpoint.object(&part.sha256).await?.is_none() {
            return Ok(None);
        }
    }
    Ok(Some(file.parts.clone()))
}

/// One file sent unless the Hub holds it; not on this computer, it need only be there. The objects
/// it went as: none, it is left out, which stops nothing else.
async fn send(project: &Project, endpoint: &Endpoint, tally: &Tally, file: folder::File) -> Result<Vec<ObjectRef>, Failed> {
    let at = project.folder.dir.join(&file.path);
    let Ok(meta) = std::fs::metadata(&at) else {
        let held = held(endpoint, &file).await?;
        if held.is_none() {
            tally.skip(file.path, "it is neither on this computer nor at the Hub".into());
        }
        return Ok(held.unwrap_or_default());
    };
    let open = || File::open(&at).map_err(|e| local(format!("{}: {e}", at.display())));
    let objects = match meta.len() > MOST {
        true => match split(project, &file, &at).await? {
            Some(parts) => parts,
            None => {
                tally.skip(file.path, "its bytes on this computer are not its sha256".into());
                return Ok(vec![]);
            }
        },
        false => vec![ObjectRef { sha256: hex(&file.sha256).to_owned(), length: meta.len() }],
    };
    let (mut moved, mut start) = (false, 0);
    for object in &objects {
        let payload = || {
            let mut opened = open()?;
            match objects.len() {
                1 => Ok(Payload::File(opened)),
                _ => {
                    opened.seek(SeekFrom::Start(start)).map_err(|e| local(format!("{}: {e}", at.display())))?;
                    Ok(Payload::Part(opened, object.length))
                }
            }
        };
        if endpoint.upload(object, &payload).await? {
            moved = true;
            tally.bytes.fetch_add(object.length, Ordering::Relaxed);
        }
        start += object.length;
    }
    if moved {
        tally.moved.fetch_add(1, Ordering::Relaxed);
        let bytes = tally.bytes.load(Ordering::Relaxed);
        let _ = project.events.send(Event::Moved { file: Some(file.path), bytes });
    }
    Ok(objects)
}

/// Sends every file project.lock names that the Hub lacks, then records all it can as a commit.
async fn push(project: &Project, hub: &Origin, client: &Client, endpoint: &Endpoint) -> Result<Transferred, Error> {
    if super::vault::encrypted(&super::vault::logical(&project.folder.dir)) {
        return Err(Error::new(Kind::Conflict, "encrypted_project_push_unavailable", "encrypted project backup is not available yet; the existing push protocol uploads plaintext")
            .remedy("keep a copy of the encrypted backing, or explicitly export a plaintext copy before using ordinary project push"));
    }
    let account = client.get_current_account().await.map_err(|e| crate::hub::refusal(hub, e).0)?.name;
    let _held = project.moving.lock().await;
    crate::events::project_settled(&project.folder.dir);
    let (named, tally, before) = (named(project)?, Tally::default(), endpoint.sent());
    let files = named.len() as u64;
    let sending = stream::iter(named)
        .map(|file| send(project, endpoint, &tally, file))
        .buffer_unordered(AT_ONCE)
        .try_collect::<Vec<_>>();
    let objects = ticking(project, || endpoint.sent() - before, sending).await.map_err(failed)?;
    let objects: Vec<ObjectRef> = objects.into_iter().flatten().collect();
    let (mut kept_in, mut commit) = (None, None);
    if !objects.is_empty() {
        let tree = objects.into_iter().map(|object| (format!("{}/{}", folder::FILES, object.sha256), object)).collect();
        let name = format!("{account}/{MODEL}");
        let kept = keep(endpoint, &name, &tree).await.map_err(failed)?;
        (kept_in, commit) = (Some(name), Some(kept.id()));
    }
    let (moved, bytes) = (tally.moved.load(Ordering::Relaxed), tally.bytes.load(Ordering::Relaxed));
    Ok(Transferred { files, moved, bytes, skipped: tally.skipped(), kept_in, commit })
}

/// `files` as a commit of model `name`, its header naming no tensors, sent and recorded.
async fn keep(endpoint: &Endpoint, name: &str, files: &std::collections::BTreeMap<String, ObjectRef>) -> Result<ObjectRef, Failed> {
    let unread = |e: tensorfs::format::Error| local(e);
    let header = Header::new(vec![], None, vec![], Default::default()).and_then(|header| header.to_cbor()).map_err(unread)?;
    let header_object = ObjectRef::of(&header);
    let tree = Tree::of(LANE, header_object.clone(), files).and_then(|tree| tree.to_json()).map_err(unread)?;
    let tree_object = ObjectRef::of(&tree);
    for (object, bytes) in [(&header_object, header), (&tree_object, tree)] {
        let bytes = Bytes::from(bytes);
        endpoint.upload(object, &move || Ok(Payload::Bytes(bytes.clone()))).await?;
    }
    endpoint.commit(name, &tree_object).await?;
    Ok(tree_object)
}

/// One file fetched as a real file: from the bytes an earlier pull left, held to its digest, then named.
async fn fetch(project: &Project, endpoint: &Endpoint, tally: &Tally, wanted: folder::File) -> Result<(), Failed> {
    let (path, sha256) = (wanted.path.clone(), wanted.sha256.clone());
    let Some(objects) = held(endpoint, &wanted).await? else {
        tally.skip(path, "it is not at the Hub: cozy push on the computer that has it".into());
        return Ok(());
    };
    let at = project.folder.dir.join(&path);
    let part = at.with_file_name(format!(".{}.part", at.file_name().expect("a name").to_string_lossy()));
    let named = |e: std::io::Error| local(format!("{}: {e}", part.display()));
    let file = File::options().create(true).truncate(false).write(true).open(&part).map_err(named)?;
    let total: u64 = objects.iter().map(|object| object.length).sum();
    let from = file.metadata().map_err(named)?.len();
    let from = if from > total { 0 } else { from };
    file.set_len(from).map_err(named)?;
    let mut start = 0;
    for object in &objects {
        let end = start + object.length;
        if from < end {
            let mut landing = |within: u64, frame: &[u8]| {
                tally.bytes.fetch_add(frame.len() as u64, Ordering::Relaxed);
                file.write_all_at(frame, start + within).map_err(named)
            };
            endpoint.read(object, from.saturating_sub(start), &mut landing).await?;
        }
        start = end;
    }
    file.sync_all().map_err(named)?;
    drop(file);
    let checking = part.clone();
    let got = tokio::task::spawn_blocking(move || folder::hash(&checking)).await.map_err(local)?;
    if got.as_deref().map(hex).ok() != Some(hex(&sha256)) {
        let _ = std::fs::remove_file(&part);
        return Err(Failed { code: Code::OBJECT_ID_MISMATCH, detail: format!("{path}: what landed is not its sha256; pull again") });
    }
    // Project.lock holds no name for it: its bytes say what it is.
    let mut head = vec![];
    File::open(&part).and_then(|file| std::io::Read::read_to_end(&mut std::io::Read::take(file, 8192), &mut head)).map_err(named)?;
    let at = infer::get(&head).map_or_else(|| at.clone(), |kind| at.with_extension(kind.extension()));
    std::fs::rename(&part, &at).map_err(named)?;
    folder::sealed(&at);
    tally.moved.fetch_add(1, Ordering::Relaxed);
    let _ = project.events.send(Event::Moved { file: Some(path), bytes: tally.bytes.load(Ordering::Relaxed) });
    Ok(())
}

/// Fetches every file project.lock names that `files/` lacks.
async fn pull(project: &Project, endpoint: &Endpoint) -> Result<Transferred, Error> {
    let _held = project.moving.lock().await;
    crate::events::project_settled(&project.folder.dir);
    let (dir, named) = (&project.folder.dir, named(project)?);
    let store = dir.join(folder::FILES);
    std::fs::create_dir_all(&store).map_err(|e| Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", store.display())))?;
    let (files, tally) = (named.len() as u64, Tally::default());
    let fetching = stream::iter(named.into_iter().filter(|file| !dir.join(&file.path).is_file()))
        .map(|file| fetch(project, endpoint, &tally, file))
        .buffer_unordered(AT_ONCE)
        .try_collect::<Vec<_>>();
    ticking(project, || tally.bytes.load(Ordering::Relaxed), fetching).await.map_err(failed)?;
    let (moved, bytes) = (tally.moved.load(Ordering::Relaxed), tally.bytes.load(Ordering::Relaxed));
    Ok(Transferred { files, moved, bytes, skipped: tally.skipped(), kept_in: None, commit: None })
}

/// One file of `project` fetched from `hub`, when it holds it: what a cut needs of a piece. Err: why not.
pub(super) async fn pull_one(api: &Api, project: &Project, hub: &str, file: folder::File) -> Result<(), String> {
    let hub = match hub {
        "" => api.machines.hubs.hub.clone(),
        hub => Origin::parse(hub).map_err(|e| e.message)?,
    };
    let (_, endpoint) = signed(api, &hub).map_err(|e| e.message)?;
    let _held = project.moving.lock().await;
    crate::events::project_settled(&project.folder.dir);
    let store = project.folder.dir.join(folder::FILES);
    std::fs::create_dir_all(&store).map_err(|e| format!("{}: {e}", store.display()))?;
    let tally = Tally::default();
    fetch(project, &endpoint, &tally, file).await.map_err(|e| failed(e).message)?;
    tally.skipped().pop().map_or(Ok(()), |skipped| Err(skipped.why))
}

/// What a transfer asks for, refused now when this computer is not signed in.
fn asked(api: &Arc<Api>, id: &str, body: Result<Bytes, BytesRejection>) -> Result<(Arc<Project>, Origin, Client, Endpoint), Refusal> {
    let at: At = crate::hub::body(body)?;
    let (project, hub) = (api.projects.get(api, id)?, crate::hub::origin(api, at.hub.as_deref())?);
    let (client, endpoint) = signed(api, &hub).map_err(Refusal)?;
    Ok((project, hub, client, endpoint))
}

/// How a transfer ended, on the project's events.
fn ended(project: &Project, done: Result<Transferred, Error>) {
    let (done, error) = match done {
        Ok(done) => (Some(done), None),
        Err(error) => (None, Some(error)),
    };
    let _ = project.events.send(Event::Transferred { done, error });
    crate::events::project_settled(&project.folder.dir);
}

async fn admitted(project: &Project) -> Result<tokio::sync::OwnedRwLockReadGuard<()>, Error> {
    let access = super::vault::admit_root(&project.root).await?;
    project.require_view()?;
    Ok(access)
}

/// `POST /v1/projects/{id}/push`.
pub(super) async fn pushed(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Opened>), Refusal> {
    let (project, hub, client, endpoint) = asked(&api, &id, body)?;
    let path = project.folder.dir.clone();
    tokio::spawn(async move {
        let _access = match admitted(&project).await {
            Ok(access) => access,
            Err(error) => {
                let _ = project.events.send(Event::Transferred { done: None, error: Some(error) });
                return;
            }
        };
        ended(&project, push(&project, &hub, &client, &endpoint).await);
    });
    Ok((StatusCode::ACCEPTED, Json(super::opened(id, &path))))
}

/// `POST /v1/projects/{id}/pull`.
pub(super) async fn pulled(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Opened>), Refusal> {
    let (project, _, _, endpoint) = asked(&api, &id, body)?;
    let opened = super::opened(id, &project.folder.dir);
    tokio::spawn(async move {
        let _access = match admitted(&project).await {
            Ok(access) => access,
            Err(error) => {
                let _ = project.events.send(Event::Transferred { done: None, error: Some(error) });
                return;
            }
        };
        ended(&project, pull(&project, &endpoint).await);
        // What came is no one's to measure until a render otherwise.
        super::measure_soon(&api, &project);
    });
    Ok((StatusCode::ACCEPTED, Json(opened)))
}
