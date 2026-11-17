//! A dev home's first-party packages as tools (xs-195): each read from its source in the packages checkout once per state of that
//! source, for whatever machine a list is asked for, and again only when the checkout changes (cl-1126). A list reads nothing: it
//! shows what's been read, and a package read later joins it through `tools.changed`.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use api::{
    calcifer::tool::{self, Tool},
    code::LocalCode,
    interface::Interface,
};
use futures_util::{StreamExt as _, stream};
use platform::watch::{Watcher, Writes};
use serde::{Deserialize, Serialize};
use tokio::{sync::Notify, time::Instant};

use crate::{api::Api, checkout, local_code, toolbox::canonical};

/// How many packages are read at once: a new tree locks and describes with uv.
const READS: usize = 2;
/// A change is read once the checkout has been still this long: a save or a fast-forward is many changes.
const STILL: Duration = Duration::from_millis(300);
/// The longest a busy checkout holds a read back.
const MOST_HELD: Duration = Duration::from_secs(5);
/// A read that failed is tried again this long after, doubling, at most [`TRIES`] times; then at the next change.
const RETRY: Duration = Duration::from_secs(30);
const TRIES: u32 = 5;

/// One package's tools as read for a Hub, and the source they were read from.
#[derive(Clone, Serialize, Deserialize)]
struct Read {
    /// As its folder's pyproject.toml names it: `flux.3-klein`.
    package: String,
    hub: String,
    /// The tree it was read from (`LocalCode::tree`), and what the checkout said of it (`checkout::From::said`).
    tree: String,
    said: Option<String>,
    tools: Vec<Tool>,
}

struct Reader {
    /// By package and Hub; kept across a restart (`kept-source-tools.json`), so a start lists them at once.
    read: Mutex<BTreeMap<(String, String), Read>>,
    /// The Hubs a list has asked for: each is read for.
    hubs: Mutex<BTreeSet<String>>,
    changed: Notify,
    /// The folders watched, and their watch.
    watching: Mutex<Option<(Vec<PathBuf>, Watcher)>>,
}

static READER: OnceLock<Arc<Reader>> = OnceLock::new();

fn kept_file(api: &Api) -> PathBuf {
    api.machines.hubs.home().calcifer().join("kept-source-tools.json")
}

/// The tools of each first-party package the checkout holds, as last read for `hub`. A Hub asked for the first time is read for
/// behind this list.
pub(crate) fn listed(api: &Arc<Api>, hub: &str) -> Vec<Tool> {
    if !checkout::configured() {
        return vec![];
    }
    let reader = READER.get_or_init(|| start(api));
    let hub = canonical(hub);
    if reader.hubs.lock().expect("hubs lock").insert(hub.clone()) {
        reader.changed.notify_one();
    }
    let read = reader.read.lock().expect("read lock");
    read.values().filter(|read| read.hub == hub).flat_map(|read| read.tools.iter().cloned()).collect()
}

fn start(api: &Arc<Api>) -> Arc<Reader> {
    let kept: Vec<Read> = std::fs::read(kept_file(api)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    let reader = Arc::new(Reader {
        read: Mutex::new(kept.into_iter().map(|read| ((read.package.clone(), read.hub.clone()), read)).collect()),
        hubs: Mutex::default(),
        changed: Notify::new(),
        watching: Mutex::default(),
    });
    tokio::spawn(Box::pin(follow(api.clone(), reader.clone())));
    reader
}

/// Reads the checkout each time it changes, once it's still.
async fn follow(api: Arc<Api>, reader: Arc<Reader>) {
    let mut failed = 0;
    loop {
        reader.changed.notified().await;
        let until = Instant::now() + MOST_HELD;
        while tokio::time::timeout_at(until.min(Instant::now() + STILL), reader.changed.notified()).await.is_ok() {}
        let all_read = Box::pin(reader.refresh(&api)).await;
        failed = if all_read { 0 } else { failed + 1 };
        if !all_read && failed <= TRIES {
            let (reader, after) = (reader.clone(), RETRY * 2u32.pow(failed - 1));
            tokio::spawn(async move {
                tokio::time::sleep(after).await;
                reader.changed.notify_one();
            });
        }
    }
}

impl Reader {
    /// Each package's source as it is now, read for each Hub where its tree or commit moved. False: one couldn't be read.
    async fn refresh(self: &Arc<Self>, api: &Arc<Api>) -> bool {
        checkout::moved();
        let packages = checkout::packages();
        self.watch(&packages);
        let hubs: Vec<String> = self.hubs.lock().expect("hubs lock").iter().cloned().collect();
        let mut all_read = true;
        let mut changed = false;
        let wanted: Vec<_> = packages.iter().flat_map(|(name, dir)| hubs.iter().map(move |hub| (name.clone(), dir.clone(), hub.clone()))).collect();
        let reads = stream::iter(wanted).map(|(name, dir, hub)| {
            let (api, reader) = (api.clone(), self.clone());
            async move { (name.clone(), hub.clone(), Box::pin(reader.read_one(&api, &name, &dir, &hub)).await) }
        });
        let mut reads = reads.buffer_unordered(READS);
        // Each joins the list as it's read: a first reading of every package takes minutes.
        while let Some((name, hub, read)) = reads.next().await {
            match read {
                Ok(Some(read)) => {
                    self.read.lock().expect("read lock").insert((name, hub), read);
                    crate::events::tools_changed();
                    changed = true;
                }
                Ok(None) => {}
                Err(why) => {
                    tracing::warn!("{name} from source isn't listed: {}", why.message);
                    all_read = false;
                }
            }
        }
        // A package the checkout no longer holds, or a Hub no list asks for, is gone.
        let held: BTreeSet<&str> = packages.iter().map(|(name, _)| name.as_str()).collect();
        let kept = {
            let mut read = self.read.lock().expect("read lock");
            let before = read.len();
            read.retain(|(name, hub), _| held.contains(name.as_str()) && hubs.contains(hub));
            if read.len() != before {
                crate::events::tools_changed();
                changed = true;
            }
            changed.then(|| read.values().cloned().collect::<Vec<_>>())
        };
        if let Some(kept) = kept {
            let _ = platform::durable::replace(&kept_file(api), &serde_json::to_vec(&kept).expect("serializes"), 0o600);
        }
        all_read
    }

    /// `name`'s tools for `hub` from its source in `dir`, when its tree or what the checkout says of it moved since its last read.
    async fn read_one(&self, api: &Arc<Api>, name: &str, dir: &Path, hub: &str) -> Result<Option<Read>, api::error::Error> {
        let code = {
            let (api, dir) = (api.clone(), dir.to_owned());
            tokio::task::spawn_blocking(move || local_code::own(&api, &dir)).await.map_err(|why| api::error::Error::internal("source_read", why.to_string()))??
        };
        let said = checkout::of(&code).map(|from| from.said());
        let tree = code.tree.to_string();
        let last = self.read.lock().expect("read lock").get(&(name.to_owned(), hub.to_owned())).map(|read| (read.tree.clone(), read.said.clone()));
        if last.as_ref().is_some_and(|(read, was)| *read == tree && *was == said) {
            return Ok(None);
        }
        Ok(Some(Read { package: name.to_owned(), hub: hub.to_owned(), tools: Box::pin(tools(api, &code, dir, hub, said.clone())).await?, tree, said }))
    }

    /// Watches each folder of the checkout's packages, its own entries and what moves its commit, for as long as that's what it holds.
    fn watch(self: &Arc<Self>, packages: &[(String, PathBuf)]) {
        let Some(root) = checkout::root() else { return };
        let mut dirs = vec![(root.to_owned(), Writes::None)];
        if let Some(git) = git_dir(root) {
            dirs.push((git.join("logs"), Writes::Named(&["HEAD"])));
        }
        for (_, dir) in packages {
            dirs.extend(api::code::source_dirs(dir).into_iter().map(|dir| (dir, Writes::All)));
        }
        let paths: Vec<PathBuf> = dirs.iter().map(|(path, _)| path.clone()).collect();
        let mut watching = self.watching.lock().expect("watching lock");
        if watching.as_ref().is_some_and(|(watched, _)| *watched == paths) {
            return;
        }
        let reader = self.clone();
        let dirs: Vec<(&Path, Writes)> = dirs.iter().map(|(path, writes)| (path.as_path(), *writes)).collect();
        match Watcher::new("checkout-watch", &dirs, move || reader.changed.notify_one()) {
            Ok(watcher) => *watching = Some((paths, watcher)),
            Err(why) => tracing::warn!("{}: its packages' changes aren't seen, so a change is listed after a restart: {why}", root.display()),
        }
    }
}

/// `code`'s tools, read as the list reads it, the same for every machine (`local_code::read_source`), each saying `said`. A tree read
/// before is read from what was kept of it: only a new one runs uv.
async fn tools(api: &Arc<Api>, code: &LocalCode, dir: &Path, hub: &str, said: Option<String>) -> Result<Vec<Tool>, api::error::Error> {
    let (package, document) = Box::pin(local_code::read_source(api, code, hub)).await?;
    let interface = Interface::parse(&document)
        .map_err(|why| api::error::Error::new(api::error::Kind::Unavailable, "package_interface_invalid", format!("{package}: {why}")))?;
    let release = said.clone().unwrap_or_else(|| "source".into());
    Ok(tool::tools_of(&package, &release, &interface, &checkout::description(dir)).into_iter().map(|tool| Tool { source: said.clone(), ..tool }).collect())
}

/// The checkout's git directory: `.git`, or the one a worktree's `.git` file names.
fn git_dir(root: &Path) -> Option<PathBuf> {
    let dot = root.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let named = std::fs::read_to_string(&dot).ok()?;
    let at = Path::new(named.strip_prefix("gitdir:")?.trim());
    Some(if at.is_absolute() { at.to_owned() } else { root.join(at) })
}
