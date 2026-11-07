//! `GET /v1/projects/{id}/archive`: every file of a project on this machine as one tar, streamed
//! with backpressure as it is read, so a pod's files can be carried off before it goes. Its
//! `project.db` goes as one copy of it whole, made as the tar begins. What the project names and
//! isn't here is named first in `elsewhere.json`, so a tar cut short still says.
use std::{
    fs,
    io::{self, Write},
    path::Path as FsPath,
    sync::Arc,
};

use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::header,
    response::Response,
};
use serde::Serialize;
use tokio::sync::mpsc;

use super::folder::{File, Lock};
use crate::api::{Api, Refusal};

/// One read of the tar's bytes, and how many wait for a slow reader.
const CHUNK: usize = 1 << 20;
const WAITING: usize = 4;

/// A file the project names that this machine doesn't hold.
#[derive(Serialize)]
struct Elsewhere<'a> {
    sha256: &'a str,
    /// The machine that keeps it; empty when the lock names none, so a pull is what fetches it.
    machine: &'a str,
    #[serde(skip_serializing_if = "is_zero")]
    length: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

pub(super) async fn archive(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Result<Response, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let lock = project.folder.lock().map_err(Refusal)?;
    let dir = project.folder.dir.clone();
    let named = dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
    let name: String = named.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
    let elsewhere = elsewhere(&lock, |file| project.folder.missing(file));
    let manifest = (!elsewhere.is_empty()).then(|| serde_json::to_vec_pretty(&serde_json::json!({"elsewhere": elsewhere})));
    let manifest = manifest.transpose().map_err(crate::machines::internal).map_err(Refusal)?;
    let count = elsewhere.len();
    let copy = dir.join(format!("{COPY}{}", super::new_key()));
    project.folder.snapshot(&copy).map_err(Refusal)?;
    let (sender, mut receiver) = mpsc::channel::<io::Result<Bytes>>(WAITING);
    let prefix = name.clone();
    tokio::task::spawn_blocking(move || {
        let writer = io::BufWriter::with_capacity(CHUNK, Chunks(sender.clone()));
        // A tar that ends cleanly but short would read as whole: a failure fails the stream.
        let written = write(&dir, &prefix, manifest.as_deref(), &copy, writer);
        let _ = fs::remove_file(&copy);
        if let Err(error) = written {
            let _ = sender.blocking_send(Err(error));
        }
    });
    let body = Body::from_stream(futures_util::stream::poll_fn(move |cx| receiver.poll_recv(cx)));
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "application/x-tar")
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}.tar\""))
        .header(header::CONTENT_SECURITY_POLICY, "sandbox")
        .header("cozy-elsewhere", count)
        .body(body)
        .expect("a response"))
}

fn elsewhere(lock: &Lock, missing: impl Fn(&File) -> bool) -> Vec<Elsewhere<'_>> {
    let away = lock.files.values().filter(|file| file.stays() || missing(file));
    away.map(|file| Elsewhere { sha256: &file.sha256, machine: &file.machine, length: file.length }).collect()
}

/// Where the copy of `project.db` a tar carries is made, beside it.
const COPY: &str = ".project.db.copy-";

/// Every regular file under `dir`, in name order, as `<prefix>/<its path>`, and `copy` as its `project.db`. A symlink is not its
/// target's to give away, a `.part` is still being written, and the database itself goes as `copy`: they stay out.
fn write(dir: &FsPath, prefix: &str, manifest: Option<&[u8]>, copy: &FsPath, writer: impl Write) -> io::Result<()> {
    let mut tar = tar::Builder::new(writer);
    if let Some(manifest) = manifest {
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest.len() as u64);
        header.set_mode(0o644);
        tar.append_data(&mut header, format!("{prefix}/elsewhere.json"), manifest)?;
    }
    let db = fs::File::open(copy)?;
    let mut header = tar::Header::new_gnu();
    header.set_size(db.metadata()?.len());
    header.set_mode(0o644);
    tar.append_data(&mut header, FsPath::new(prefix).join(super::db::DB), db)?;
    let own = |name: &str| name.starts_with(super::db::DB) || name.starts_with(COPY);
    let mut dirs = vec![dir.to_owned()];
    while let Some(at) = dirs.pop() {
        let mut entries = fs::read_dir(&at)?.collect::<io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let kind = entry.file_type()?;
            if kind.is_dir() {
                dirs.push(entry.path());
            } else if kind.is_file() && !entry.file_name().to_string_lossy().ends_with(".part") && !(at == dir && own(&entry.file_name().to_string_lossy())) {
                // Gone since the listing: replaced or dropped, so it is no file of this project now.
                let file = match fs::File::open(entry.path()) {
                    Ok(file) => file,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                // Measured on this open file: a replaced project.yaml stays the one it opened.
                let meta = file.metadata()?;
                let mut header = tar::Header::new_gnu();
                header.set_size(meta.len());
                header.set_mode(0o644);
                header.set_mtime(meta.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |at| at.as_secs()));
                let path = entry.path();
                let relative = path.strip_prefix(dir).map_err(io::Error::other)?;
                tar.append_data(&mut header, FsPath::new(prefix).join(relative), io::Read::take(file, meta.len()))?;
            }
        }
    }
    tar.into_inner()?.flush()
}

/// What the tar writes, handed on a chunk at a time: a full channel holds the writer back, and a
/// reader gone fails it.
struct Chunks(mpsc::Sender<io::Result<Bytes>>);

impl Write for Chunks {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let sent = self.0.blocking_send(Ok(Bytes::copy_from_slice(buffer)));
        sent.map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
