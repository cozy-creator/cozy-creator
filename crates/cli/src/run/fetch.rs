//! `--output DIR`: a run's outputs, each fetched from Calcifer by its `url` and held to its digest; a folder as its tree document,
//! then each of its files. Calcifer's paths are its own (cl-819): the CLI keeps what it asks for, where it says.
use std::path::{Path, PathBuf};

use api::{
    calcifer::{OutputItem, Run, output_file_name},
    error::{Error, Kind},
    ids::Digest,
    tree,
};

use crate::Context;

/// Each output of `run` a person is told of that has landed, downloaded into `dir`: by name, where it went.
pub(crate) async fn outputs(ctx: &Context, run: &Run, dir: &Path) -> Result<Vec<(String, PathBuf)>, Error> {
    let unwritable = |e: std::io::Error| Error::new(Kind::Unavailable, "download_unwritable", format!("{}: {e}", dir.display()));
    std::fs::create_dir_all(dir).map_err(unwritable)?;
    let calcifer = ctx.calcifer().await?;
    let mut kept = vec![];
    for item in super::follow::for_people(run) {
        let (Some(url), Some(digest)) = (&item.url, &item.sha256) else { continue };
        let to = dir.join(output_file_name(run.number, &item.name, item.index, &item.media_type));
        match item.media_type == tree::MEDIA {
            false => {
                calcifer.download(url, &to).await?;
                held(&to, digest)?;
            }
            true => folder(&calcifer, item, url, digest, &to).await?,
        }
        kept.push((item.name.clone(), to));
    }
    Ok(kept)
}

/// A folder output into `to`: its tree document, checked, then each file it names.
async fn folder(calcifer: &crate::calcifer_client::Client, item: &OutputItem, url: &str, digest: &Digest, to: &Path) -> Result<(), Error> {
    let unwritable = |e: std::io::Error| Error::new(Kind::Unavailable, "download_unwritable", format!("{}: {e}", to.display()));
    std::fs::create_dir_all(to).map_err(unwritable)?;
    let document = to.with_extension("tree.part");
    calcifer.download(url, &document).await?;
    let bytes = std::fs::read(&document).map_err(unwritable)?;
    let _ = std::fs::remove_file(&document);
    if Digest::of(&bytes) != *digest {
        return Err(Error::new(Kind::Unavailable, "download_mismatch", format!("{}: its tree is not {digest}", item.name)));
    }
    let members = tree::parse(&bytes).map_err(|e| Error::new(Kind::Unavailable, "download_mismatch", format!("{}: {e}", item.name)))?;
    let joined = if url.contains('?') { '&' } else { '?' };
    for (path, member) in members {
        let file = path.split('/').fold(to.to_owned(), |at, part| at.join(part));
        std::fs::create_dir_all(file.parent().expect("a member's folder")).map_err(unwritable)?;
        let query: String = url::form_urlencoded::Serializer::new(String::new()).append_pair("member", &path).finish();
        calcifer.download(&format!("{url}{joined}{query}"), &file).await?;
        held(&file, &format!("sha256:{}", member.sha256).parse().map_err(|e: &str| Error::internal("digest", e))?)?;
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
