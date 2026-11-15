//! `--previews DIR` (cr-282): while a run is followed, each new picture its machine makes of what it's making lands
//! in `DIR` as `<number>-preview-<n>.webp`, `n` counting them. Asking is what keeps the machine making them: a run no
//! one asks about makes none. They are lossy: one made between two asks is skipped.
use std::{path::Path, time::Duration};

use api::{calcifer::Run, error::Error};

use crate::{Context, calcifer_client::Client, output};

/// How often a followed run is asked for a newer preview.
const ASK: Duration = Duration::from_secs(1);

/// Saving a run's previews while it is followed; `end` stops it and says how many landed.
pub(crate) struct Saving {
    task: tokio::task::JoinHandle<()>,
    saved: std::sync::Arc<std::sync::atomic::AtomicU64>,
    dir: std::path::PathBuf,
}

impl Saving {
    pub(crate) fn start(calcifer: Client, run: &Run, dir: &Path) -> Result<Self, Error> {
        std::fs::create_dir_all(dir).map_err(|e| {
            Error::new(api::error::Kind::Validation, "previews_dir", format!("--previews {}: {e}", dir.display()))
        })?;
        let saved = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        // A project's run is asked of its project, as it is followed.
        let (address, number, into, count) = (crate::calcifer_client::RunAddress::of(run), run.number, dir.to_path_buf(), saved.clone());
        let task = tokio::spawn(async move {
            let mut held = String::new();
            loop {
                if let Ok(Some((tag, _step, picture))) = calcifer.preview(&address, &held).await {
                    let n = tag.trim_matches('"').trim_start_matches('p');
                    if std::fs::write(into.join(format!("{number}-preview-{n}.webp")), &picture).is_ok() {
                        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    held = tag;
                }
                tokio::time::sleep(ASK).await;
            }
        });
        Ok(Self { task, saved, dir: dir.to_path_buf() })
    }

    pub(crate) fn end(self, ctx: &Context) {
        self.task.abort();
        let saved = self.saved.load(std::sync::atomic::Ordering::Relaxed);
        output::say(ctx, &format!("previews: {saved} in {}", self.dir.display()));
    }
}
