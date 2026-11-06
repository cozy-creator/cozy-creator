use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::failure::{fail, Failure, Kind, Result};
use crate::fetch::{self, Fetcher, ObjectStore, Origin, VolumeOrigin, Want};
use crate::format::{Header, ObjectRef, Repo, Tree};
use crate::precision::{Ask, Gpus};
use crate::store::Store;

use super::provider::from_native;
use super::{io_failure, Checkpoint, Endpoint, Download, Fetched, Ref};

/// What a reference names: the root object (a commit, or a bare header), and the release and
/// lane it resolved to (empty: none).
struct Root {
    object: ObjectRef,
    release: String,
    lane: String,
}

#[derive(Clone)]
pub(super) struct Remote {
    pub store: Store,
    pub endpoint: Endpoint,
    pub mirror: Option<PathBuf>,
    pub offline: bool,
}

impl Remote {
    pub async fn fetch(&self, objects: &[ObjectRef], want: &Download<'_>) -> Result<Fetched> {
        let unique: BTreeMap<_, _> = objects.iter().map(|o| (&o.sha256, o.clone())).collect();
        let objects: Vec<_> = unique.into_values().collect();
        let _hold = self.store.hold(&objects)?;
        let total = objects.iter().map(|o| o.length).sum();
        if let Some(report) = want.progress {
            report(0, total)?;
        }
        if self.offline && self.mirror.is_none() {
            for object in &objects {
                if !self.store.reuse(object)? {
                    return fail(Kind::NotFound, format!("{} is not cached", object.id()));
                }
            }
            if let Some(report) = want.progress {
                report(total, total)?;
            }
            return Ok(Fetched {
                local: total,
                ..Fetched::default()
            });
        }
        let store: Arc<dyn ObjectStore> = Arc::new(self.store.clone());
        let mirror = self
            .mirror
            .as_ref()
            .map(|p| VolumeOrigin::new(p, Duration::from_secs(24 * 60 * 60)));
        let (origin, volume): (Arc<dyn Origin>, _) = if self.offline {
            // A configured mirror is a local-files source, not permission to
            // contact the remote endpoint. Use the existing verified downloader
            // with a file-only origin and no network fallback.
            (Arc::new(mirror.expect("offline source has a mirror")), None)
        } else {
            (self.endpoint.inner.clone(), mirror)
        };
        // Constructing/dropping this fetcher scopes cancellation to this operation.
        // All network tasks run on the caller's Tokio runtime, never a nested one.
        let fetcher = Fetcher::new(
            store,
            self.endpoint.transport.clone(),
            volume,
            fetch::Config::default(),
        );
        let progress = fetch::Progress::default();
        let selected = Want {
            priority: want.priority,
            progress: Some(&progress),
            ..Want::default()
        };
        let mut work = std::pin::pin!(fetcher.objects(&origin, &objects, &selected));
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let result = loop {
            tokio::select! {
                biased;
                result = &mut work => break result.map_err(from_native)?,
                _ = tick.tick(), if want.progress.is_some() => {
                    if let Some(report) = want.progress {
                        report(progress.done(), total)?;
                    }
                }
            }
        };
        fetcher.settled().await;
        if let Some(report) = want.progress {
            report(total, total)?;
        }
        Ok(result)
    }

    /// What `reference` names: a bare object (`sha256:` repo id), a commit by id, or the lane of
    /// its release this machine picks for `gpus` (its lane, when it names one; spec §7). Offline,
    /// or when the endpoint cannot be reached, a lane the store holds of what it recorded.
    async fn root(&self, reference: &Ref, gpus: &Gpus) -> Result<Root> {
        let bare = |object| Root { object, release: String::new(), lane: String::new() };
        if let Some(hex) = reference.repo_id.strip_prefix("sha256:") {
            return Ok(bare(self.object(hex).await?));
        }
        let release = reference.release.as_deref();
        if let Some(hex) = release.and_then(|r| r.strip_prefix("sha256:")) {
            return Ok(bare(self.object(hex).await?));
        }
        let answer = match (self.offline, &self.mirror) {
            (false, _) => Some(self.endpoint.inner.release(&reference.repo_id, release).await),
            (true, None) => None,
            (true, Some(mirror)) => {
                // An absolute directory cannot be interpreted as an HTTP endpoint, and carries no
                // provider even if Config supplies one for the Hub.
                let path = std::path::absolute(mirror).map_err(|error| io_failure(mirror.display(), error))?;
                let path = path.to_str().ok_or_else(|| Failure { kind: Kind::Invalid, detail: "mirror path is not UTF-8".into() })?;
                let local = fetch::Endpoint::new(&self.endpoint.transport, path, None).map_err(from_native)?;
                Some(local.release(&reference.repo_id, release).await)
            }
        };
        let ask = Ask { pin: reference.lane.clone(), ..Ask::default() };
        let chosen = crate::lanes::resolve(&self.store, &reference.repo_id, release, answer, gpus, &ask).map_err(from_native)?;
        for reason in &chosen.reasons {
            tracing::warn!("{}: {reason}", reference.repo_id);
        }
        Ok(Root { object: chosen.lane.commit, release: chosen.release, lane: chosen.lane.label })
    }

    /// A bare object by digest: from the store, the offline mirror, or the endpoint.
    async fn object(&self, hex: &str) -> Result<ObjectRef> {
        let at = |length| ObjectRef { sha256: hex.to_owned(), length };
        let path = self.store.path(&at(0))?;
        match std::fs::metadata(&path) {
            Ok(metadata) => return Ok(at(metadata.len())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_failure(path.display(), error)),
        }
        if self.offline {
            let Some(mirror) = &self.mirror else {
                return fail(Kind::NotFound, format!("sha256:{hex} is not cached"));
            };
            let path = mirror.join(at(0).key());
            let metadata = std::fs::metadata(&path).map_err(|error| io_failure(path.display(), error))?;
            return Ok(at(metadata.len()));
        }
        let object = self.endpoint.inner.object(hex).await.map_err(from_native)?;
        object.ok_or_else(|| Failure { kind: Kind::NotFound, detail: format!("no object sha256:{hex}") })
    }

    /// Records what a remote resolution named, so the store resolves it again offline.
    fn remember(&self, reference: &Ref, root: &Root) -> Result<()> {
        if reference.repo_id.starts_with("local/") || reference.repo_id.starts_with("sha256:") {
            return Ok(());
        }
        self.store.update_ref(&reference.repo_id, |old| {
            let mut repo = old.unwrap_or_else(|| Repo::new(&reference.repo_id));
            match root.release.is_empty() || root.lane.is_empty() {
                true => repo.add_commit(root.object.clone()),
                false => repo.set_lane(&root.release, &root.lane, root.object.clone()),
            }
            Some(repo)
        })?;
        Ok(())
    }

    /// The commit (or bare header) `reference` names, opened: its header fetched.
    pub async fn open(&self, mut reference: Ref, gpus: &Gpus) -> Result<Checkpoint> {
        let root = self.root(&reference, gpus).await?;
        let _root_hold = self.store.hold(std::slice::from_ref(&root.object))?;
        self.fetch(std::slice::from_ref(&root.object), &Download::default()).await?;
        let bytes = self.store.read(&root.object)?;
        let (tree, header, lane, mut extra) = match Tree::from_json(&bytes) {
            Ok(tree) => {
                let header = tree.header()?.1.clone();
                let extra = tree.repo_files().map(|(_, r)| r.clone()).collect();
                (Some(root.object.clone()), header, Some(tree.lane), extra)
            }
            Err(_) => {
                Header::from_cbor(&bytes)?;
                (None, root.object.clone(), None, Vec::new())
            }
        };
        let _metadata_hold = self.store.hold(std::slice::from_ref(&header))?;
        self.fetch(std::slice::from_ref(&header), &Download::default()).await?;
        let raw = match &tree {
            Some(tree) => {
                self.remember(&reference, &root)?;
                extra.push(tree.clone());
                self.store.at_commit(tree)?
            }
            None => self.store.checkpoint(&header)?,
        };
        reference.release = (!root.release.is_empty()).then_some(root.release).or(reference.release);
        reference.lane = lane.filter(|l| !l.is_empty()).or(reference.lane);
        let named = (!reference.repo_id.starts_with("sha256:")).then_some(reference);
        Checkpoint::managed(raw, named, tree, Some(self.clone()), extra, self.store.clone())
    }

    /// The commit `reference` names as a plain folder: its repo files and header, and the
    /// objects `want` selects.
    pub async fn snapshot(&self, reference: &Ref, want: &Download<'_>, local_dir: Option<&Path>) -> Result<PathBuf> {
        let root = self.root(reference, &Gpus::default()).await?;
        let commit = root.object.clone();
        let _root_hold = self.store.hold(std::slice::from_ref(&commit))?;
        self.fetch(std::slice::from_ref(&commit), &Download::default()).await?;
        let tree = self.store.tree(&commit)?;
        let header = tree.header()?.1.clone();
        let _header_hold = self.store.hold(std::slice::from_ref(&header))?;
        self.fetch(std::slice::from_ref(&header), &Download::default()).await?;
        let payload = super::selection::objects(&self.store.checkpoint(&header)?, &want.selection)?;
        let mut objects: Vec<_> = tree.repo_files().map(|(_, r)| r.clone()).collect();
        objects.extend(payload.iter().cloned());
        let _object_hold = self.store.hold(&objects)?;
        self.fetch(&objects, want).await?;
        self.remember(reference, &root)?;
        let identity = match &want.selection {
            super::Selection::All => commit.sha256.clone(),
            _ => crate::sha256::hex_digest(format!("{}:{:?}", commit.sha256, want.selection).as_bytes()),
        };
        let used = std::iter::once(&commit).chain(std::iter::once(&header)).chain(&objects);
        self.store.used(&identity, used)?;
        let dest = local_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.store.root().join("snapshots").join(identity));
        // The selected closure, not every payload that happens to be in the shared cache.
        self.store.write_folder(&tree, &dest, &payload)?;
        self.store.maintain(0)?;
        Ok(dest)
    }

    pub async fn file(
        &self,
        reference: &Ref,
        filename: &str,
        local_dir: Option<&Path>,
    ) -> Result<PathBuf> {
        let root = self.root(reference, &Gpus::default()).await?;
        let commit = root.object.clone();
        let _root_hold = self.store.hold(std::slice::from_ref(&commit))?;
        self.fetch(std::slice::from_ref(&commit), &Download::default())
            .await?;
        let tree = self.store.tree(&commit)?;
        let object = tree.files.get(filename).ok_or_else(|| Failure {
            kind: Kind::NotFound,
            detail: format!("no repo file {filename:?}"),
        })?;
        let _object_hold = self.store.hold(std::slice::from_ref(object))?;
        self.fetch(std::slice::from_ref(object), &Download::default())
            .await?;
        self.remember(reference, &root)?;
        self.store.used(&commit.sha256, [&commit, object])?;
        let folder = local_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.store.root().join("snapshots").join(&commit.sha256));
        let dest = folder.join(filename);
        self.store.materialize(object, &dest)?;
        self.store.maintain(0)?;
        Ok(dest)
    }
}
