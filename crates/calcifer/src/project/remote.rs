//! A project's files at its user's Tensorhub account (xs-143), moved the way TensorFS moves weights: by
//! content, so what the Hub holds is never sent again, and a killed transfer goes on from what landed.
//! Push records them as a private commit of the account's one model `project-assets`: objects no commit
//! reaches are swept within days, and a commit stays until its owner deletes it (`cozy model delete`).
//! Each runs in its own task and says how it goes on the project's events: a client that goes stops
//! following, never the work, and a stopping Calcifer waits for no transfer.
use std::{
    fs::File,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
    http::StatusCode,
};
use futures_util::{StreamExt as _, TryStreamExt as _, stream};
use hub_client::{Client, Credential};
use platform::durable::FileAt as _;
use protocol::{
    config::Origin,
    error::{Error, Kind},
    hub::At,
    project::{Event, Opened, Skipped, Transferred},
};
use tensorfs::{
    err::{Code, Refusal as Failed},
    fetch::{self, Endpoint, Payload, SourcePolicy, TokenProvider, policy::is_loopback_origin, token::Request},
    format::{Header, ObjectRef, Tree},
};

use super::{Project, folder, video::Video};
use crate::api::{Api, Refusal};

/// The account's model whose commits keep its projects' files.
const MODEL: &str = "project-assets";
/// What a commit of it is labelled.
const LANE: &str = "project";
/// Files moved at once.
const AT_ONCE: usize = 4;
/// The most one object at the Hub can be (a single PUT to its storage).
const MOST: u64 = 5 << 30;
/// How often a transfer says how far it is, between files.
const TICK: Duration = Duration::from_secs(5);

/// A signed-in client's Bearer on each request.
struct Signed(Client);

#[async_trait]
impl TokenProvider for Signed {
    async fn headers(&self, _: &Request<'_>) -> tensorfs::err::Result<Vec<(String, String)>> {
        let Credential::User(session) = self.0.credential() else { unreachable!("a signed-in client") };
        let bearer = session.bearer(&self.0.anonymous()).await;
        let bearer = bearer.map_err(|e| Failed { code: Code::CREDENTIAL_REQUIRED, detail: e.to_string() })?;
        Ok(vec![("authorization".into(), format!("Bearer {}", bearer.token))])
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

    /// What it left out, each under the path project.yaml writes for it if it names it.
    fn skipped(self, project: &Project) -> Result<Vec<Skipped>, Error> {
        let lock = project.folder.lock()?;
        let original = |path: &str| lock.inputs.iter().find(|(_, input)| input.path == path).map(|(file, _)| file.clone());
        let skipped = self.skipped.into_inner().expect("skipped lock");
        Ok(skipped.into_iter().map(|(path, why)| Skipped { original: original(&path), path, why }).collect())
    }
}

/// The files `lock` names, by path, each with its sha256.
fn named(project: &Project) -> Result<Vec<(String, String)>, Error> {
    let lock = project.folder.lock()?;
    Ok(lock.assets().into_iter().map(|(path, sha256)| (path.to_owned(), sha256.to_owned())).collect())
}

/// One file sent unless the Hub holds it, a link read through; not on this computer, it need only be
/// there. None: it is left out, which stops nothing else.
async fn send(project: &Project, endpoint: &Endpoint, tally: &Tally, path: String, sha256: String) -> Result<Option<ObjectRef>, Failed> {
    let file = project.folder.dir.join(&path);
    let Ok(meta) = std::fs::metadata(&file) else {
        let held = endpoint.object(&sha256).await?;
        if held.is_none() {
            tally.skip(path, "it is neither on this computer nor at the Hub".into());
        }
        return Ok(held);
    };
    if meta.len() > MOST {
        tally.skip(path, format!("it is {} bytes, and the Hub holds files up to {MOST} (5 GiB)", meta.len()));
        return Ok(None);
    }
    let object = ObjectRef { sha256: hex(&sha256).to_owned(), length: meta.len() };
    let payload = || File::open(&file).map(Payload::File).map_err(|e| local(format!("{}: {e}", file.display())));
    if endpoint.upload(&object, &payload).await? {
        tally.moved.fetch_add(1, Ordering::Relaxed);
        let bytes = tally.bytes.fetch_add(object.length, Ordering::Relaxed) + object.length;
        let _ = project.events.send(Event::Moved { file: Some(path), bytes });
    }
    Ok(Some(object))
}

/// Sends every file project.lock names that the Hub lacks, then records all it can as a commit.
async fn push(project: &Project, hub: &Origin, client: &Client, endpoint: &Endpoint) -> Result<Transferred, Error> {
    let account = client.get_current_account().await.map_err(|e| crate::hub::refusal(hub, e).0)?.name;
    let _held = project.moving.lock().await;
    // A file changed at its path is taken in first: the lock names what the project reads now.
    Video::load(&project.folder)?;
    let (assets, tally, before) = (named(project)?, Tally::default(), endpoint.sent());
    let files = assets.len() as u64;
    let sending = stream::iter(assets)
        .map(|(path, sha256)| send(project, endpoint, &tally, path, sha256))
        .buffer_unordered(AT_ONCE)
        .try_collect::<Vec<_>>();
    let objects = ticking(project, || endpoint.sent() - before, sending).await.map_err(failed)?;
    let objects: Vec<ObjectRef> = objects.into_iter().flatten().collect();
    let (mut kept_in, mut commit) = (None, None);
    if !objects.is_empty() {
        let tree = objects.into_iter().map(|object| (format!("{}/{}", folder::ASSETS, object.sha256), object)).collect();
        let name = format!("{account}/{MODEL}");
        let kept = keep(endpoint, &name, &tree).await.map_err(failed)?;
        (kept_in, commit) = (Some(name), Some(kept.id()));
    }
    let (moved, bytes) = (tally.moved.load(Ordering::Relaxed), tally.bytes.load(Ordering::Relaxed));
    Ok(Transferred { files, moved, bytes, skipped: tally.skipped(project)?, kept_in, commit })
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
async fn fetch(project: &Project, endpoint: &Endpoint, tally: &Tally, path: String, sha256: String) -> Result<(), Failed> {
    let Some(object) = endpoint.object(&sha256).await? else {
        tally.skip(path, "it is not at the Hub: cozy project push on the computer that has it".into());
        return Ok(());
    };
    let at = project.folder.dir.join(&path);
    let part = at.with_file_name(format!(".{}.part", at.file_name().expect("a name").to_string_lossy()));
    let named = |e: std::io::Error| local(format!("{}: {e}", part.display()));
    let file = File::options().create(true).truncate(false).write(true).open(&part).map_err(named)?;
    let from = file.metadata().map_err(named)?.len();
    let from = if from > object.length { 0 } else { from };
    file.set_len(from).map_err(named)?;
    if from < object.length {
        let mut landing = |start: u64, frame: &[u8]| {
            tally.bytes.fetch_add(frame.len() as u64, Ordering::Relaxed);
            file.write_all_at(frame, start).map_err(named)
        };
        endpoint.read(&object, from, &mut landing).await?;
    }
    file.sync_all().map_err(named)?;
    drop(file);
    let checking = part.clone();
    let got = tokio::task::spawn_blocking(move || folder::hash(&checking)).await.map_err(local)?;
    if got.as_deref().map(hex).ok() != Some(hex(&sha256)) {
        let _ = std::fs::remove_file(&part);
        return Err(Failed { code: Code::OBJECT_ID_MISMATCH, detail: format!("{path}: what landed is not its sha256; pull again") });
    }
    std::fs::rename(&part, &at).map_err(named)?;
    tally.moved.fetch_add(1, Ordering::Relaxed);
    let _ = project.events.send(Event::Moved { file: Some(path), bytes: tally.bytes.load(Ordering::Relaxed) });
    Ok(())
}

/// Fetches every file project.lock names that `assets/` lacks.
async fn pull(project: &Project, endpoint: &Endpoint) -> Result<Transferred, Error> {
    let _held = project.moving.lock().await;
    let (dir, assets) = (&project.folder.dir, named(project)?);
    let store = dir.join(folder::ASSETS);
    std::fs::create_dir_all(&store).map_err(|e| Error::new(Kind::Unavailable, "project_io", format!("{}: {e}", store.display())))?;
    let (files, tally) = (assets.len() as u64, Tally::default());
    let fetching = stream::iter(assets.into_iter().filter(|(path, _)| !dir.join(path).is_file()))
        .map(|(path, sha256)| fetch(project, endpoint, &tally, path, sha256))
        .buffer_unordered(AT_ONCE)
        .try_collect::<Vec<_>>();
    ticking(project, || tally.bytes.load(Ordering::Relaxed), fetching).await.map_err(failed)?;
    let (moved, bytes) = (tally.moved.load(Ordering::Relaxed), tally.bytes.load(Ordering::Relaxed));
    Ok(Transferred { files, moved, bytes, skipped: tally.skipped(project)?, kept_in: None, commit: None })
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
}

/// `POST /v1/projects/{id}/push`.
pub(super) async fn pushed(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Opened>), Refusal> {
    let (project, hub, client, endpoint) = asked(&api, &id, body)?;
    let path = project.folder.dir.clone();
    tokio::spawn(async move { ended(&project, push(&project, &hub, &client, &endpoint).await) });
    Ok((StatusCode::ACCEPTED, Json(Opened { id, path })))
}

/// `POST /v1/projects/{id}/pull`.
pub(super) async fn pulled(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Opened>), Refusal> {
    let (project, _, _, endpoint) = asked(&api, &id, body)?;
    let path = project.folder.dir.clone();
    tokio::spawn(async move { ended(&project, pull(&project, &endpoint).await) });
    Ok((StatusCode::ACCEPTED, Json(Opened { id, path })))
}
