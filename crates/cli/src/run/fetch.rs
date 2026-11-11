//! `--output DIR`: a run's outputs, each fetched from Calcifer by its `url` and held to its digest; a folder as its tree document,
//! then each of its files. Calcifer's paths are its own (cl-819): the CLI keeps what it asks for, where it says.
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use api::{
    calcifer::{OutputItem, Run, output_file_name},
    error::{Error, Kind},
    ids::Digest,
    tree,
};

use crate::{
    Context,
    calcifer_client::Client,
    progress::{Transfer, TransferCounts},
};

struct Downloading {
    line: Transfer,
    counts: TransferCounts,
    files: u64,
    bytes: u64,
    trees: usize,
}

impl Downloading {
    fn new(ctx: &Context, items: &[&OutputItem]) -> Self {
        let trees = items
            .iter()
            .filter(|item| item.media_type == tree::MEDIA)
            .count();
        let files = items.len() as u64;
        let bytes = items.iter().map(|item| item.length).sum();
        let counts = TransferCounts {
            files_done: 0,
            files_total: (trees == 0).then_some(files),
            bytes_done: 0,
            bytes_total: (trees == 0).then_some(bytes),
            bytes_moved: 0,
        };
        let line = Transfer::stderr(ctx.term.stderr, "Downloading outputs");
        line.update(counts);
        Self {
            line,
            counts,
            files,
            bytes,
            trees,
        }
    }

    async fn download(
        &mut self,
        calcifer: &Client,
        url: &str,
        to: &Path,
        expected: u64,
    ) -> Result<u64, Error> {
        self.line.phase(&format!(
            "Downloading {}",
            crate::output::shown(
                &to.file_name()
                    .map(|name| name.to_string_lossy())
                    .unwrap_or_default()
            )
        ));
        // No body chunks to display for an empty file; keep the original read wrapper.
        if expected == 0 {
            return calcifer.download(url, to).await;
        }
        let observed = Mutex::new((0_u64, 0_u64));
        let baseline = self.counts;
        let answer = calcifer
            .download_progress(url, to, &|at, _total| {
                let mut observed = observed.lock().expect("download progress");
                observed.1 = observed.1.saturating_add(at.saturating_sub(observed.0));
                observed.0 = at;
                self.line.update(TransferCounts {
                    bytes_done: baseline.bytes_done.saturating_add(at),
                    bytes_moved: baseline.bytes_moved.saturating_add(observed.1),
                    ..baseline
                });
            })
            .await;
        self.counts.bytes_moved = baseline
            .bytes_moved
            .saturating_add(observed.into_inner().expect("download progress").1);
        answer
    }

    fn finished(&mut self, length: u64) {
        self.counts.files_done += 1;
        self.counts.bytes_done = self.counts.bytes_done.saturating_add(length);
        self.line.update(self.counts);
    }

    fn members(&mut self, members: &std::collections::BTreeMap<String, tree::Member>) {
        self.files += members.len() as u64;
        self.bytes = self
            .bytes
            .saturating_add(members.values().map(|member| member.length).sum());
        self.trees -= 1;
        self.counts.files_total = (self.trees == 0).then_some(self.files);
        self.counts.bytes_total = (self.trees == 0).then_some(self.bytes);
        self.line.update(self.counts);
    }
}

impl Drop for Downloading {
    fn drop(&mut self) {
        self.line.finish_and_clear();
    }
}

/// Each output of `run` a person is told of that has landed, downloaded into `dir`: by name, where it went.
pub(crate) async fn outputs(
    ctx: &Context,
    run: &Run,
    dir: &Path,
) -> Result<Vec<(String, PathBuf)>, Error> {
    let unwritable = |e: std::io::Error| {
        Error::new(
            Kind::Unavailable,
            "download_unwritable",
            format!("{}: {e}", dir.display()),
        )
    };
    std::fs::create_dir_all(dir).map_err(unwritable)?;
    let calcifer = ctx.calcifer().await?;
    let mut kept = vec![];
    let items: Vec<_> = super::follow::for_people(run)
        .into_iter()
        .filter(|item| item.url.is_some() && item.sha256.is_some())
        .collect();
    if items.is_empty() {
        return Ok(kept);
    }
    let mut progress = Downloading::new(ctx, &items);
    for item in items {
        let (Some(url), Some(digest)) = (&item.url, &item.sha256) else {
            continue;
        };
        let to = dir.join(output_file_name(
            run.number,
            &item.name,
            item.index,
            &item.media_type,
        ));
        match item.media_type == tree::MEDIA {
            false => {
                let written = progress.download(&calcifer, url, &to, item.length).await?;
                progress.line.finalizing("Verifying file");
                held(&to, digest)?;
                progress.finished(written);
            }
            true => folder(&calcifer, item, url, digest, &to, &mut progress).await?,
        }
        kept.push((item.name.clone(), to));
    }
    Ok(kept)
}

/// A folder output into `to`: its tree document, checked, then each file it names.
async fn folder(
    calcifer: &Client,
    item: &OutputItem,
    url: &str,
    digest: &Digest,
    to: &Path,
    progress: &mut Downloading,
) -> Result<(), Error> {
    let unwritable = |e: std::io::Error| {
        Error::new(
            Kind::Unavailable,
            "download_unwritable",
            format!("{}: {e}", to.display()),
        )
    };
    std::fs::create_dir_all(to).map_err(unwritable)?;
    let document = to.with_extension("tree.part");
    let written = progress
        .download(calcifer, url, &document, item.length)
        .await?;
    progress.line.finalizing("Verifying folder manifest");
    let bytes = std::fs::read(&document).map_err(unwritable)?;
    let _ = std::fs::remove_file(&document);
    if Digest::of(&bytes) != *digest {
        return Err(Error::new(
            Kind::Unavailable,
            "download_mismatch",
            format!("{}: its tree is not {digest}", item.name),
        ));
    }
    let members = tree::parse(&bytes).map_err(|e| {
        Error::new(
            Kind::Unavailable,
            "download_mismatch",
            format!("{}: {e}", item.name),
        )
    })?;
    progress.members(&members);
    progress.finished(written);
    let joined = if url.contains('?') { '&' } else { '?' };
    for (path, member) in members {
        let file = path
            .split('/')
            .fold(to.to_owned(), |at, part| at.join(part));
        std::fs::create_dir_all(file.parent().expect("a member's folder")).map_err(unwritable)?;
        let query: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("member", &path)
            .finish();
        let written = progress
            .download(
                calcifer,
                &format!("{url}{joined}{query}"),
                &file,
                member.length,
            )
            .await?;
        progress.line.finalizing("Verifying file");
        held(
            &file,
            &format!("sha256:{}", member.sha256)
                .parse()
                .map_err(|e: &str| Error::internal("digest", e))?,
        )?;
        progress.finished(written);
    }
    Ok(())
}

/// `file` holds the bytes `digest` names, or it's removed and refused.
fn held(file: &Path, digest: &Digest) -> Result<(), Error> {
    let got = api::digests::of(None, file)?;
    if got == *digest {
        return Ok(());
    }
    let _ = std::fs::remove_file(file);
    Err(Error::new(Kind::Unavailable, "download_mismatch", format!("{} came as {got}, not {digest}", file.display())))
}
