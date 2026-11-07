//! The CLI's one way in for files, the page's: each goes to Calcifer by content (`PUT /v1/files/sha256:<hex>`) and is named by its
//! digest. What Calcifer holds is skipped, and a part it holds is resumed. A folder is its tree document (`protocol::tree`), sent after
//! its files, and local code is its package's tree (`protocol::code`). Calcifer reads no path of this computer's.
use std::{
    collections::BTreeMap,
    io::SeekFrom,
    path::{Path, PathBuf},
};

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use protocol::{
    calcifer::{Sent, path_key},
    code::LocalCode,
    error::{Error, Kind},
    ids::Digest,
    machine::missing,
    target::{spelled, written_as_path},
};
use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

use crate::{Context, calcifer_client::Client};

/// The most of a file one PUT carries.
const PART: u64 = 64 << 20;

pub(crate) struct Sender<'a> {
    ctx: &'a Context,
    calcifer: Client,
}

impl<'a> Sender<'a> {
    pub(crate) async fn new(ctx: &'a Context) -> Result<Self, Error> {
        Ok(Self { ctx, calcifer: ctx.calcifer().await? })
    }

    fn memo(&self) -> PathBuf {
        self.ctx.home.root().join("cache").join("digests")
    }

    /// The file at `path`, held by Calcifer: its digest.
    pub(crate) async fn file(&self, path: &Path) -> Result<Digest, Error> {
        let (at, memo) = (path.to_owned(), self.memo());
        let digest = tokio::task::spawn_blocking(move || protocol::digests::of(Some(&memo), &at))
            .await
            .map_err(|e| Error::internal("digest", e.to_string()))??;
        let length = std::fs::metadata(path).map_err(|e| unreadable(path, e))?.len();
        self.bytes(path, digest, length).await?;
        Ok(digest)
    }

    /// The folder at `root`, each file and then its tree document held by Calcifer: the document's digest.
    pub(crate) async fn tree(&self, root: &Path) -> Result<Digest, Error> {
        let (at, memo) = (root.to_owned(), self.memo());
        let staged = tokio::task::spawn_blocking(move || protocol::stage::stage(&at, Some(&memo)))
            .await
            .map_err(|e| Error::internal("stage", e.to_string()))??;
        for file in &staged.files {
            self.bytes(&file.path, file.digest, file.length).await?;
        }
        let digest = Digest::of(&staged.document);
        let length = staged.document.len() as u64;
        if self.calcifer.held(&digest).await?.is_some() {
            self.calcifer.put_part(&digest, staged.document.into(), 0, length).await?;
        }
        Ok(digest)
    }

    /// The package at `path` (a folder or a script) as local code: its tree, each file and then the document held by Calcifer
    /// (`protocol::code`). What never goes is said.
    pub(crate) async fn code(&self, path: &Path) -> Result<LocalCode, Error> {
        let (at, memo) = (path.to_owned(), self.memo());
        let staged = tokio::task::spawn_blocking(move || protocol::code::stage(&at, Some(&memo)))
            .await
            .map_err(|e| Error::internal("stage", e.to_string()))??;
        if !staged.left.is_empty() {
            crate::output::warn(&format!("left out of the code it sends: {}", staged.left.join(", ")));
        }
        for file in &staged.files {
            self.bytes(&file.path, file.digest, file.length).await?;
        }
        let code = staged.code();
        let length = staged.document.len() as u64;
        if self.calcifer.held(&code.tree).await?.is_some() {
            self.calcifer.put_part(&code.tree, staged.document.into(), 0, length).await?;
        }
        Ok(code)
    }

    /// `path`'s `length` bytes under `digest`, but what Calcifer holds of them.
    async fn bytes(&self, path: &Path, digest: Digest, length: u64) -> Result<(), Error> {
        let Some(held) = self.calcifer.held(&digest).await? else { return Ok(()) };
        if length == 0 {
            self.calcifer.put_part(&digest, bytes::Bytes::new(), 0, 0).await?;
            return Ok(());
        }
        let gaps = missing(&held, length);
        let bar = match !self.ctx.json && self.ctx.term.stderr && length > PART {
            true => ProgressBar::with_draw_target(Some(length), ProgressDrawTarget::stderr()),
            false => ProgressBar::hidden(),
        };
        bar.set_style(ProgressStyle::with_template("sending {msg}: {bytes}/{total_bytes} at {bytes_per_sec}").expect("a template"));
        bar.set_message(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        bar.set_position(length - gaps.iter().map(|(start, end)| end - start).sum::<u64>());
        let mut file = tokio::fs::File::open(path).await.map_err(|e| unreadable(path, e))?;
        for (start, end) in gaps {
            let mut at = start;
            file.seek(SeekFrom::Start(at)).await.map_err(|e| unreadable(path, e))?;
            while at < end {
                let mut part = vec![0; (end - at).min(PART) as usize];
                file.read_exact(&mut part).await.map_err(|e| unreadable(path, e))?;
                let n = part.len() as u64;
                self.calcifer.put_part(&digest, part.into(), at, length).await?;
                at += n;
                bar.inc(n);
            }
        }
        bar.finish_and_clear();
        Ok(())
    }

    /// `written` (relative to `base`, `~` this user's home) sent when it names a file here, or, `folders` and written as a path, a folder.
    async fn sent(&self, written: &str, base: &Path, folders: bool) -> Result<Option<(PathBuf, Sent)>, Error> {
        let key = path_key(written);
        if key.is_empty() || key.contains("://") || key.starts_with("sha256:") || key.contains('\n') || key.len() > 4096 {
            return Ok(None);
        }
        let path = spelled(key, base, &self.ctx.user_home);
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() => Ok(Some((path.clone(), Sent::File(self.file(&path).await?)))),
            Ok(meta) if meta.is_dir() && folders && written_as_path(key) => Ok(Some((path.clone(), Sent::Tree(self.tree(&path).await?)))),
            _ => Ok(None),
        }
    }
}

/// `raw`, when it is written as a path here: its file (a folder too, with `folders`) sent, `raw` its digest and `name` its name.
pub(crate) async fn held(ctx: &Context, raw: &mut String, name: &mut Option<String>, folders: bool) -> Result<(), Error> {
    if !written_as_path(raw) {
        return Ok(());
    }
    let cwd = std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let path = spelled(raw, &cwd, &ctx.user_home);
    let sender = Sender::new(ctx).await?;
    let digest = match std::fs::metadata(&path) {
        Ok(meta) if meta.is_file() => sender.file(&path).await?,
        Ok(meta) if meta.is_dir() && folders => sender.tree(&path).await?,
        Ok(_) => return Err(Error::new(Kind::Validation, "model_file_refused", format!("{raw} is not one regular file"))),
        Err(e) => return Err(unreadable(&path, e)),
    };
    *name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    *raw = digest.to_string();
    Ok(())
}

/// The files and folders a request names by a path here, sent, by the key each is named under.
pub(crate) struct Paths<'s, 'a> {
    sender: &'s Sender<'a>,
    named: BTreeMap<String, (PathBuf, Sent)>,
}

impl<'s, 'a> Paths<'s, 'a> {
    pub(crate) fn new(sender: &'s Sender<'a>) -> Self {
        Self { sender, named: BTreeMap::new() }
    }

    /// `written`, as a request names it relative to `base`, sent when it names something here. A name that means another file
    /// than it did is refused: the request can carry one.
    pub(crate) async fn add(&mut self, written: &str, base: &Path, folders: bool, by: &str) -> Result<(), Error> {
        let key = path_key(written).to_owned();
        let Some((path, sent)) = self.sender.sent(written, base, folders).await? else { return Ok(()) };
        match self.named.get(&key) {
            Some((was, _)) if *was != path => Err(Error::new(
                Kind::Usage,
                "input_path_ambiguous",
                format!("{key} is {} here and {} in {by}", was.display(), path.display()),
            )
            .remedy("name one of them by a longer path")),
            _ => {
                self.named.insert(key, (path, sent));
                Ok(())
            }
        }
    }

    /// Every string in `value` that names something relative to `base`.
    pub(crate) async fn add_all(&mut self, value: &Value, base: &Path, by: &str) -> Result<(), Error> {
        let mut strings = vec![];
        collect(value, &mut strings);
        for text in strings {
            self.add(&text, base, true, by).await?;
        }
        Ok(())
    }

    pub(crate) fn sent(self) -> BTreeMap<String, Sent> {
        self.named.into_iter().map(|(key, (_, sent))| (key, sent)).collect()
    }
}

fn collect(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => out.push(text.clone()),
        Value::Array(values) => values.iter().for_each(|v| collect(v, out)),
        Value::Object(map) => map.values().for_each(|v| collect(v, out)),
        _ => {}
    }
}

fn unreadable(path: &Path, e: std::io::Error) -> Error {
    Error::new(Kind::Usage, "file_unreadable", format!("{}: {e}", path.display()))
}
