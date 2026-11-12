//! The CLI's one way in for files, the page's: each goes to Calcifer by content (`PUT /v1/files/sha256:<hex>`) and is named by its
//! digest. What Calcifer holds is skipped, and a part it holds is resumed. A folder is its tree document (`api::tree`), sent after
//! its files, and local code is its package's tree (`api::code`). Calcifer reads no path of this computer's.
use std::{
    collections::{BTreeMap, BTreeSet},
    io::SeekFrom,
    path::{Path, PathBuf},
};

use api::{
    calcifer::{Sent, path_key},
    code::LocalCode,
    error::{Error, Kind},
    ids::Digest,
    machine::missing,
    target::{spelled, written_as_path},
};
use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

use crate::{
    Context,
    calcifer_client::Client,
    progress::{Transfer, TransferCounts},
};

/// The most of a file one PUT carries.
const PART: u64 = 64 << 20;

struct Sending {
    line: Transfer,
    counts: TransferCounts,
}

impl Sending {
    fn new(ctx: &Context) -> Self {
        let line = Transfer::stderr(ctx.term.stderr, "Sending files");
        let counts = TransferCounts {
            files_done: 0,
            files_total: None,
            bytes_done: 0,
            bytes_total: None,
            bytes_moved: 0,
        };
        line.update(counts);
        Self { line, counts }
    }

    fn totals(&mut self, files: u64, bytes: u64) {
        self.counts.files_total = Some(files);
        self.counts.bytes_total = Some(bytes);
        self.line.update(self.counts);
    }

    fn position(&self, file_done: u64) {
        self.line.update(TransferCounts {
            bytes_done: self.counts.bytes_done.saturating_add(file_done),
            ..self.counts
        });
    }

    fn finished(&mut self, length: u64) {
        self.counts.files_done += 1;
        self.counts.bytes_done = self.counts.bytes_done.saturating_add(length);
        self.line.update(self.counts);
    }
}

impl Drop for Sending {
    fn drop(&mut self) {
        self.line.finish_and_clear();
    }
}

pub(crate) struct Sender<'a> {
    ctx: &'a Context,
    calcifer: Client,
    project: Option<String>,
    /// An explicit export (a model installed from a file): it may name a project's file, said once.
    export: bool,
}

impl<'a> Sender<'a> {
    pub(crate) async fn new(ctx: &'a Context) -> Result<Self, Error> {
        Ok(Self {
            ctx,
            calcifer: ctx.calcifer().await?,
            project: None,
            export: false,
        })
    }

    pub(crate) async fn for_project(ctx: &'a Context, project: &str) -> Result<Self, Error> {
        Ok(Self {
            ctx,
            calcifer: ctx.calcifer().await?,
            project: Some(project.to_owned()),
            export: false,
        })
    }

    /// What a person exports on purpose: a project's own file goes too, outside its protection, with a warning.
    pub(crate) async fn export(ctx: &'a Context) -> Result<Self, Error> {
        Ok(Self { export: true, ..Self::new(ctx).await? })
    }

    /// Whether `path` (a folder: with its files) is independent of any project, else refused, or for an export warned.
    async fn independent(&self, path: &Path, tree: bool) -> Result<bool, Error> {
        if self.project.is_some() { return Ok(false); }
        let owned = project_input(self.ctx, path, tree).await?;
        if owned && !self.export {
            return Err(Error::new(Kind::Forbidden, "project_source_scope", "This input belongs to a Cozy project and cannot be staged as independent maintenance data")
                .remedy("Use it in a project-scoped run, or explicitly export an independent copy first"));
        }
        if owned { crate::output::warn("this copy leaves its project's protection: what it makes is this machine's, outside the project"); }
        Ok(!owned)
    }

    /// Maintenance may stage code only after every actual source path is proven outside a project.
    pub(crate) async fn independent_code(ctx: &'a Context, path: &Path) -> Result<LocalCode, Error> {
        let plan = source_plan(path).await?;
        source_scope(ctx, &plan, CodeScope::Independent).await?;
        let path = std::fs::canonicalize(path).map_err(|error| unreadable(path, error))?;
        ctx.calcifer().await?.post("/v1/maintenance/source", &serde_json::json!({ "path": path }), None).await
    }

    fn memo(&self) -> PathBuf {
        self.ctx.home.root().join("cache").join("independent-digests-v3")
    }

    /// The file at `path`, held by Calcifer: its digest.
    pub(crate) async fn file(&self, path: &Path) -> Result<Digest, Error> {
        // An exported project file keeps no global memo of its path.
        let independent = self.independent(path, false).await?;
        let mut progress = Sending::new(self.ctx);
        progress.line.phase("Checking file");
        let (at, memo) = (path.to_owned(), (self.project.is_none() && independent).then(|| self.memo()));
        let digest = tokio::task::spawn_blocking(move || api::digests::of(memo.as_deref(), &at))
            .await
            .map_err(|e| Error::internal("digest", e.to_string()))??;
        let length = std::fs::metadata(path)
            .map_err(|e| unreadable(path, e))?
            .len();
        progress.totals(1, length);
        self.bytes(path, digest, length, &mut progress).await?;
        Ok(digest)
    }

    /// The folder at `root`, each file and then its tree document held by Calcifer: the document's digest.
    pub(crate) async fn tree(&self, root: &Path) -> Result<Digest, Error> {
        let independent = self.independent(root, true).await?;
        let mut progress = Sending::new(self.ctx);
        progress.line.phase("Checking folder");
        let (at, memo) = (root.to_owned(), (self.project.is_none() && independent).then(|| self.memo()));
        let staged = tokio::task::spawn_blocking(move || api::stage::stage(&at, memo.as_deref()))
            .await
            .map_err(|e| Error::internal("stage", e.to_string()))??;
        let digest = Digest::of(&staged.document);
        let length = staged.document.len() as u64;
        progress.totals(
            staged.files.len() as u64 + 1,
            staged
                .files
                .iter()
                .map(|file| file.length)
                .sum::<u64>()
                .saturating_add(length),
        );
        for file in &staged.files {
            self.bytes(&file.path, file.digest, file.length, &mut progress)
                .await?;
        }
        progress.line.phase("Sending folder manifest");
        if self
            .calcifer
            .held_for(&digest, self.project.as_deref())
            .await?
            .is_some()
        {
            self.calcifer
                .put_part_for(
                    &digest,
                    staged.document.into(),
                    0,
                    length,
                    self.project.as_deref(),
                )
                .await?;
            progress.counts.bytes_moved = progress.counts.bytes_moved.saturating_add(length);
        }
        progress.finished(length);
        Ok(digest)
    }

    /// The package at `path` (a folder or a script) as local code: its tree, each file and then the document held by Calcifer
    /// (`api::code`). What never goes is said.
    pub(crate) async fn code(&self, path: &Path) -> Result<LocalCode, Error> {
        let Some(project) = self.project.as_deref() else {
            return Err(code_scope_refused());
        };
        let plan = source_plan(path).await?;
        source_scope(self.ctx, &plan, CodeScope::Project(project)).await?;
        let staged = tokio::task::spawn_blocking(move || plan.stage(None))
            .await.map_err(|error| Error::internal("stage", error.to_string()))??;
        self.prepared_code(staged).await
    }

    async fn prepared_code(&self, staged: api::code::Staged) -> Result<LocalCode, Error> {
        let mut progress = Sending::new(self.ctx);
        if !staged.left.is_empty() {
            let warning = format!("left out of the code it sends: {}", staged.left.join(", "));
            progress.line.println(format!("warning: {warning}"));
        }
        let code = staged.code();
        let length = staged.document.len() as u64;
        progress.totals(
            staged.files.len() as u64 + 1,
            staged
                .files
                .iter()
                .map(|file| file.length)
                .sum::<u64>()
                .saturating_add(length),
        );
        for file in &staged.files {
            self.bytes(&file.path, file.digest, file.length, &mut progress)
                .await?;
        }
        progress.line.phase("Sending code manifest");
        if self
            .calcifer
            .held_for(&code.tree, self.project.as_deref())
            .await?
            .is_some()
        {
            self.calcifer
                .put_part_for(
                    &code.tree,
                    staged.document.into(),
                    0,
                    length,
                    self.project.as_deref(),
                )
                .await?;
            progress.counts.bytes_moved = progress.counts.bytes_moved.saturating_add(length);
        }
        progress.finished(length);
        Ok(code)
    }

    /// `path`'s `length` bytes under `digest`, but what Calcifer holds of them, until Calcifer holds the file whole: another sender of
    /// the same file can hold ranges this one skipped and not finish them (cl-864).
    async fn bytes(
        &self,
        path: &Path,
        digest: Digest,
        length: u64,
        progress: &mut Sending,
    ) -> Result<(), Error> {
        progress.line.phase(&format!(
            "Sending {}",
            path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default()
        ));
        for _ in 0..3 {
            let Some(held) = self
                .calcifer
                .held_for(&digest, self.project.as_deref())
                .await?
            else {
                progress.finished(length);
                return Ok(());
            };
            if self.missing(path, digest, length, &held, progress).await? {
                progress.finished(length);
                return Ok(());
            }
        }
        let said = format!(
            "Calcifer holds only part of {} after three sends",
            path.display()
        );
        Err(Error::new(Kind::Unavailable, "file_incomplete", said).next("cozy doctor"))
    }

    /// The bytes of `path` Calcifer doesn't hold (`held`), sent: whether that made the file whole.
    async fn missing(
        &self,
        path: &Path,
        digest: Digest,
        length: u64,
        held: &[(u64, u64)],
        progress: &mut Sending,
    ) -> Result<bool, Error> {
        if length == 0 {
            return self
                .calcifer
                .put_part_for(&digest, bytes::Bytes::new(), 0, 0, self.project.as_deref())
                .await;
        }
        let gaps = missing(held, length);
        let mut file_done = length - gaps.iter().map(|(start, end)| end - start).sum::<u64>();
        progress.position(file_done);
        let mut file = tokio::fs::File::open(path)
            .await
            .map_err(|e| unreadable(path, e))?;
        let mut whole = false;
        for (start, end) in gaps {
            let mut at = start;
            file.seek(SeekFrom::Start(at))
                .await
                .map_err(|e| unreadable(path, e))?;
            while at < end {
                let mut part = vec![0; (end - at).min(PART) as usize];
                file.read_exact(&mut part)
                    .await
                    .map_err(|e| unreadable(path, e))?;
                let n = part.len() as u64;
                whole = self
                    .calcifer
                    .put_part_for(&digest, part.into(), at, length, self.project.as_deref())
                    .await?;
                at += n;
                progress.counts.bytes_moved = progress.counts.bytes_moved.saturating_add(n);
                file_done += n;
                progress.position(file_done);
            }
        }
        Ok(whole)
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

#[derive(Clone, Copy)]
enum CodeScope<'a> { Independent, Project(&'a str) }

fn code_scope_refused() -> Error {
    Error::new(Kind::Forbidden, "project_source_scope", "This package source belongs to a Cozy project")
        .remedy("Run it from that project, or export a separate package copy before installing or publishing")
        .next("cozy run --help")
}

async fn source_plan(path: &Path) -> Result<api::code::SourcePlan, Error> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || api::code::plan(&path))
        .await.map_err(|error| Error::internal("stage", error.to_string()))?
}

async fn source_scope(ctx: &Context, plan: &api::code::SourcePlan, scope: CodeScope<'_>) -> Result<(), Error> {
    let mut directories = BTreeSet::new();
    for path in plan.roots().chain(plan.files()) {
        let canonical = std::fs::canonicalize(path).map_err(|error| unreadable(path, error))?;
        let directory = if canonical.is_dir() { canonical } else {
            canonical.parent().ok_or_else(code_scope_refused)?.to_owned()
        };
        directories.insert(directory);
    }
    let client = ctx.calcifer().await?;
    for directory in directories {
        let directory = directory.to_str().ok_or_else(|| Error::new(Kind::Usage, "project_path", "The source path is not UTF-8"))?;
        let path = crate::calcifer_client::path(&["v1", "projects", directory, "address"]);
        let owner = match client.get::<api::calcifer::project::Address>(&path).await {
            Ok(owner) => Some(owner),
            Err(error) if error.code == "project_not_found" => None,
            Err(error) => return Err(error),
        };
        if let Some(owner) = owner {
            match scope {
                CodeScope::Project(expected) if owner.id == expected => {},
                _ => return Err(code_scope_refused()),
            }
        }
    }
    Ok(())
}

/// Whether a file/tree is a project's own (by its path, an alias or any resolved source): an
/// independent one must not create a global digest memo or CAS copy from it. Tree enumeration hashes nothing.
async fn project_input(ctx: &Context, path: &Path, tree: bool) -> Result<bool, Error> {
    let mut paths = vec![path.to_path_buf()];
    if tree {
        paths.extend(api::tree::walk(path, true)
            .map_err(|error| Error::new(Kind::Usage, "tree_unreadable", error.to_string()))?.into_values());
    }
    let mut directories = BTreeSet::new();
    for path in paths {
        for path in [std::path::absolute(&path).map_err(|error| unreadable(&path, error))?,
            std::fs::canonicalize(&path).map_err(|error| unreadable(&path, error))?] {
            let directory = if path.is_dir() { path } else { path.parent().ok_or_else(code_scope_refused)?.to_owned() };
            directories.insert(directory);
        }
    }
    let client = ctx.calcifer().await?;
    for directory in directories {
        let directory = directory.to_str().ok_or_else(|| Error::new(Kind::Usage, "project_path", "The source path is not UTF-8"))?;
        let route = crate::calcifer_client::path(&["v1", "projects", directory, "address"]);
        match client.get::<api::calcifer::project::Address>(&route).await {
            Ok(_) => return Ok(true),
            Err(error) if error.code == "project_not_found" => {},
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

/// `raw`, when it is written as a path here: its file (a folder too, with `folders`) sent, `raw` its digest and `name` its name.
pub(crate) async fn held(ctx: &Context, raw: &mut String, name: &mut Option<String>, folders: bool) -> Result<(), Error> {
    if !written_as_path(raw) {
        return Ok(());
    }
    let cwd = std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let path = spelled(raw, &cwd, &ctx.user_home);
    // A model made from a file is an explicit export, a project's file too.
    let sender = Sender::export(ctx).await?;
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
