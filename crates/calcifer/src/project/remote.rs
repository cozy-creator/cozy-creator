//! Fetch files already stored at the user's Hub when a render needs them.
//! Legacy chunk metadata remains readable so existing project files can be materialized.
use std::{fs::File, sync::Arc};

use ::api::hub::{Client, Credential};
use platform::durable::FileAt as _;
use api::{config::Origin, error::{Error, Kind}};
use tensorfs::{
    err::{Code, Refusal as Failed},
    fetch::{self, is_loopback_origin, Endpoint, Request, SourcePolicy, TokenProvider},
    format::ObjectRef,
};

use super::{Project, folder};
use crate::api::Api;

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
fn signed(api: &Api, hub: &Origin) -> Result<Endpoint, Error> {
    let client = api.machines.hubs.client(hub)?;
    let policy = SourcePolicy { any_host: true, allow_local: is_loopback_origin(hub), max_redirects: 5, ..Default::default() };
    let provider: Arc<dyn TokenProvider> = Arc::new(Signed(client));
    let endpoint = Endpoint::new(&fetch::Client::new(policy), hub, Some(provider)).map_err(failed)?;
    Ok(endpoint)
}

fn failed(error: Failed) -> Error {
    Error::new(Kind::Unavailable, "project_transfer", format!("{}: {}", error.code.as_str(), error.detail))
}

fn local(detail: impl ToString) -> Failed {
    Failed { code: Code::IO_FAILED, detail: detail.to_string() }
}

/// `sha256` as a digest with no scheme.
fn hex(sha256: &str) -> &str {
    sha256.trim_start_matches("sha256:")
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

/// One file fetched as a real file: resuming any partial download, held to its digest, then named.
async fn fetch(project: &Project, endpoint: &Endpoint, wanted: folder::File) -> Result<(), Failed> {
    let (path, sha256) = (wanted.path.clone(), wanted.sha256.clone());
    let Some(objects) = held(endpoint, &wanted).await? else {
        return Err(Failed { code: Code::REF_NOT_FOUND, detail: format!("{path} is not stored at the Hub") });
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
                file.write_all_at(frame, start + within).map_err(named)
            };
            endpoint.read(object, from.saturating_sub(start), &mut landing).await?;
        }
        start = end;
    }
    file.sync_all().map_err(named)?;
    drop(file);
    let checking = part.clone();
    let access = super::vault::admit_work(project).await.map_err(|e| local(e.message))?;
    let got = tokio::task::spawn_blocking(move || {
        let _access = access;
        folder::hash(&checking)
    }).await.map_err(local)?;
    if got.as_deref().map(hex).ok() != Some(hex(&sha256)) {
        let _ = std::fs::remove_file(&part);
        return Err(Failed { code: Code::OBJECT_ID_MISMATCH, detail: format!("{path}: what landed is not its sha256; retry fetching it") });
    }
    // Project.lock holds no name for it: its bytes say what it is.
    let mut head = vec![];
    File::open(&part).and_then(|file| std::io::Read::read_to_end(&mut std::io::Read::take(file, 8192), &mut head)).map_err(named)?;
    let at = infer::get(&head).map_or_else(|| at.clone(), |kind| at.with_extension(kind.extension()));
    std::fs::rename(&part, &at).map_err(named)?;
    folder::sealed(&at);
    Ok(())
}

/// One file of `project` fetched from `hub`, when it holds it: what a cut needs of a piece. Err: why not.
pub(super) async fn pull_one(api: &Api, project: &Project, hub: &str, file: folder::File) -> Result<(), String> {
    let hub = match hub {
        "" => api.machines.hubs.hub.clone(),
        hub => Origin::parse(hub).map_err(|e| e.message)?,
    };
    let endpoint = signed(api, &hub).map_err(|e| e.message)?;
    let _held = project.moving.lock().await;
    crate::events::project_settled(&project.folder.dir);
    let store = project.folder.dir.join(folder::FILES);
    std::fs::create_dir_all(&store).map_err(|e| format!("{}: {e}", store.display()))?;
    fetch(project, &endpoint, file).await.map_err(|e| failed(e).message)
}
