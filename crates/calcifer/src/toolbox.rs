//! What can be dropped on a canvas: `GET /v1/tools`, every function that makes media of every package the Hub lists, of what this
//! computer has installed or reads from source and of what a search finds on the Hub, less the tools you took off; and what a tool
//! makes, for a node of it. The list is the same for every machine, never waits on the Hub, and is told changed on `/v1/events`
//! (`tools.changed`), so a page reads it once and again only when told (cl-1126).
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json,
    body::Bytes,
    extract::{Query, State, rejection::{BytesRejection, QueryRejection}},
    response::Response,
};
use futures_util::{StreamExt as _, stream};
use ::api::hub::cards;
use api::{
    calcifer::On,
    choices::{self, Choices},
    config::Origin,
    error::{Error, Kind},
    interface::Interface,
    names::{ROOT, shown},
    calcifer::tool::{self, BrowsedPackage, Browsed, Tool, ToolList, ToolListChange, Tools},
};
use serde::Deserialize;
use serde_json::{Map, Value};
use tensorfs::format::newest_by;

use crate::{
    api::{Api, Refusal, parse},
    checkout, local_code, private_io,
    project::{self, vault},
    runs::{self, target},
};

/// The most `browse` reads of the Hub, and how many packages one page of the Hub's list holds.
const MOST_BROWSED: u32 = 200;
/// How many packages one list reads of the Hub.
const LISTED: u32 = 50;
/// How many cards it reads at once; each is read once, then kept.
const READS: usize = 8;
/// A Hub slower than this leaves a search with what's listed.
pub(crate) const HUB_LATE: Duration = Duration::from_secs(5);
/// A Hub's catalog is read again at most this often while a page is open: the Hub tells nothing of a release (Paul, 11-17: "maybe
/// re-fetch every couple minutes").
const CATALOG_EVERY: Duration = Duration::from_secs(300);
/// A read of the Hub asked again within this is the one already made.
const ASK_AGAIN: Duration = Duration::from_secs(10);
/// The longest one read of a Hub's catalog goes on.
const CATALOG_MOST: Duration = Duration::from_secs(120);

#[derive(Default, Deserialize)]
pub(crate) struct Asked {
    /// Words: what's installed by its name and what it does, and the Hub's own search.
    q: Option<String>,
    hub: Option<String>,
    /// A project: the local code its nodes name comes first, and no other local code is listed.
    project: Option<String>,
}

/// `GET /v1/tools`: the Edit, local code, the dev home's checkout, what this computer has installed, then every package the Hub lists and
/// any you added, each at its newest; with `q`, what fits it and the Hub's search for it. The same for every machine (cl-1126).
pub(crate) async fn list(State(api): State<Arc<Api>>, query: Result<Query<Asked>, QueryRejection>) -> Result<Response, Refusal> {
    let Query(asked) = query?;
    let project = asked.project.clone();
    vault::project_reply(&api, project.as_deref(), list_tools(&api, asked)).await
}

/// Every tool `GET /v1/tools` lists with nothing asked: those `GET /v1/openai/models` names (xs-209).
pub(crate) async fn listed(api: &Arc<Api>) -> Result<Vec<Tool>, Refusal> {
    Ok(list_tools(api, Asked::default()).await?.0.tools)
}

async fn list_tools(api: &Arc<Api>, asked: Asked) -> Result<Json<Tools>, Refusal> {
    let hub = runs::hub(api, asked.hub.as_deref().unwrap_or_default());
    let q = asked.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let mine = mine(api)?;
    let local = match asked.project.as_deref() {
        Some(named) => {
            let root = project::named(api, named).map_err(Refusal)?;
            let scope = private_io::of_output(&project::landing(&root)).map_err(Refusal)?;
            private_io::context(scope, async {
                let named = project::local_code_named(api, named)?;
                Ok::<_, Refusal>(Box::pin(read_local(api, &hub, named)).await)
            }).await?
        }
        None => vec![],
    };
    // A dev home's checkout's first-party packages are their source's, not the Hub's (xs-195), read once per state of it (cl-1126).
    let source = crate::source_tools::listed(api, &hub);
    let held: Vec<Tool> = held(api, &hub).await.into_iter().filter(|tool| !checkout::holds(&tool.package)).chain(fetched(api).await).collect();
    let installed: BTreeSet<String> = held.iter().map(|tool| tool.package.clone()).collect();
    let catalog = api.listing.tools(api, &hub);
    let listed = catalog.iter().filter(|tool| !installed.contains(&tool.package) && !checkout::holds(&tool.package)).cloned();
    let mut tools: Vec<Tool> = std::iter::once(tool::edit()).chain(local).chain(source).chain(held).chain(listed).collect();
    tools.extend(on_your_list(api, &hub, &mine, &tools));
    // Taken off is off at rest, and a search still finds it, as what a box of it asks for must.
    tools.retain(|tool| q.map_or_else(|| !mine.without.contains(&tool.id), |q| says(tool, q)));
    if let Some(q) = q {
        let listed: BTreeSet<String> = tools.iter().map(|tool| tool.package.clone()).collect();
        // A Hub that doesn't answer leaves the search with what's listed.
        if let Ok(Ok(found)) = tokio::time::timeout(HUB_LATE, on_the_hub(api, &hub, q)).await {
            tools.extend(found.into_iter().filter(|tool| !listed.contains(&tool.package) && !checkout::holds(&tool.package)));
        }
    }
    // What's read of the Hub later is told as `tools.changed`.
    crate::hub::checkpoints::families(api, &hub, &mut tools);
    Ok(Json(Tools { tools }))
}

/// Your changes to the list: yours, not a project's, kept in `state.db`.
fn mine(api: &Api) -> Result<ToolList, Refusal> {
    Ok(api.calcifer.store().tool_list()?)
}

/// Whether a package kept on your list is a path, as an earlier cozy kept local code there: it's no Hub's, and not listed.
fn is_local(package: &str) -> bool {
    Path::new(package).is_absolute()
}

/// A package as the Hub's paths name it.
fn owned(package: &str) -> String {
    match package.contains('/') {
        true => package.to_owned(),
        false => format!("{ROOT}/{package}"),
    }
}

/// `POST /v1/tools/list`: a package on your list, beside what the Hub lists, all of it again if some was taken off, or tools off it. Nothing is installed or deleted, and a box
/// of a tool taken off goes on as it was. Local code is a project's, named by its boxes (cl-819): a path is refused.
pub(crate) async fn change(State(api): State<Arc<Api>>, body: Result<Bytes, BytesRejection>) -> Result<Json<ToolList>, Refusal> {
    let asked: ToolListChange = parse(&body?)?;
    if let Some(path) = asked.add.as_deref().filter(|package| api::target::written_as_path(package)) {
        let said = format!("{path} is local code, which belongs to a project. Use it from a box, by its path from the project's folder.");
        return Err(Refusal(Error::new(Kind::Usage, "tool_list_local", said).next("cozy open")));
    }
    let add = asked.add.map(|package| (owned(&package), format!("{}/", shown(&package))));
    let add = add.as_ref().map(|(full, shown_as)| (full.as_str(), shown_as.as_str()));
    let mine = api.calcifer.store().change_tool_list(add, &asked.remove).map_err(|why| Refusal(Error::new(Kind::Unavailable, "tool_list_io", why.to_string())))?;
    crate::events::tools_changed();
    Ok(Json(mine))
}

#[derive(Deserialize)]
pub(crate) struct Browsing {
    q: Option<String>,
    limit: Option<u32>,
    hub: Option<String>,
}

/// `GET /v1/tools/browse`: the Hub's packages that make media, each with what it does. Reading one installs nothing, and each card is read once.
pub(crate) async fn browse(State(api): State<Arc<Api>>, query: Result<Query<Browsing>, QueryRejection>) -> Result<Json<Browsed>, Refusal> {
    let Query(asked) = query?;
    let hub = runs::hub(&api, asked.hub.as_deref().unwrap_or_default());
    let q = asked.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let limit = asked.limit.unwrap_or(LISTED).clamp(1, MOST_BROWSED);
    let reader = api.machines.hubs.reader(&hub).map_err(Refusal)?;
    let found = reader.search_packages(q, limit).await.map_err(|why| Refusal(cards::refusal(why)))?;
    let older = (found.search.total - found.packages.len() as i64).max(0);
    let root = api.machines.hubs.home().root();
    let rows = found.packages.into_iter().filter_map(|row| Some((format!("{}/{}", row.org, row.name), row.latest_release?)));
    let reads = rows.map(|(package, release)| {
        let hub = hub.clone();
        let api = api.clone();
        async move {
            let card = cards::card(|| api.machines.hubs.reader(&hub), root, &hub, &package, &release).await.ok()?;
            let tools: Vec<String> = tool::tools_of(&package, &card.release, &card.interface, card.summary()).into_iter().map(|tool| tool.id).collect();
            (!tools.is_empty()).then(|| BrowsedPackage { package: shown(&package), summary: card.summary().to_owned(), tools })
        }
    });
    let packages = stream::iter(reads).buffered(READS).collect::<Vec<_>>().await.into_iter().flatten().collect();
    Ok(Json(Browsed { packages, older }))
}

/// The tools of each package you added that `listed` lacks, by its newest card kept here: nothing's installed by listing it, and the list
/// never waits on the Hub. One with none kept is read behind the list (`read_newest`), and joins it once read.
fn on_your_list(api: &Arc<Api>, hub: &str, mine: &ToolList, listed: &[Tool]) -> Vec<Tool> {
    let root = api.machines.hubs.home().root();
    let lacking = mine.packages.iter().filter(|package| !is_local(package) && !listed.iter().any(|tool| tool.package == shown(package)));
    let mut wanted = vec![];
    for package in lacking {
        match newest_by(cards::kept_releases(root, hub, package), |release| release) {
            Some(release) => wanted.push((package.clone(), release)),
            None => read_newest(api, hub, package),
        }
    }
    kept_tools(api, hub, wanted)
}

/// Reads `package`'s newest release's card behind the caller, once at a time and at most each [`ASK_AGAIN`]: `tools.changed` once kept.
fn read_newest(api: &Arc<Api>, hub: &str, package: &str) {
    if !asking(&format!("{} {package} newest", canonical(hub))) {
        return;
    }
    let (api, hub, package) = (api.clone(), hub.to_owned(), package.to_owned());
    tokio::spawn(async move {
        let root = api.machines.hubs.home().root();
        let had = newest_by(cards::kept_releases(root, &hub, &package), |release| release);
        let newest = async { cards::releases(&api.machines.hubs.reader(&hub)?, &package).await?.newest(&package) };
        if let Ok(Ok(newest)) = tokio::time::timeout(CATALOG_MOST, newest).await
            && had.as_deref() != Some(newest.as_str())
            && cards::card(|| api.machines.hubs.reader(&hub), root, &hub, &package, &newest).await.is_ok()
        {
            crate::events::tools_changed();
        }
        asked(&format!("{} {package} newest", canonical(&hub)));
    });
}

/// Each `(package, release)`'s tools by its kept card, in the order asked; one not kept is read behind the list (`tools.changed` once it is).
fn kept_tools(api: &Arc<Api>, hub: &str, wanted: Vec<(String, String)>) -> Vec<Tool> {
    let root = api.machines.hubs.home().root();
    let mut tools = vec![];
    for (package, release) in wanted {
        match cards::kept(root, hub, &package, &release) {
            Some(card) => tools.extend(tool::tools_of(&package, &card.release, &card.interface, card.summary())),
            None if asking(&format!("{} {package}@{release}", canonical(hub))) => {
                let (api, hub) = (api.clone(), hub.to_owned());
                tokio::spawn(async move {
                    let root = api.machines.hubs.home().root();
                    let read = cards::card(|| api.machines.hubs.reader(&hub), root, &hub, &package, &release);
                    if let Ok(Ok(_)) = tokio::time::timeout(CATALOG_MOST, read).await {
                        crate::events::tools_changed();
                    }
                    asked(&format!("{} {package}@{release}", canonical(&hub)));
                });
            }
            None => {}
        }
    }
    tools
}

/// The reads of the Hub going behind a list, and when each last began: one at a time, and one asked again within [`ASK_AGAIN`] is the
/// one made.
static ASKING: LazyLock<Mutex<HashMap<String, (bool, Instant)>>> = LazyLock::new(Mutex::default);

/// Whether this caller makes the read `key` names.
fn asking(key: &str) -> bool {
    let mut asking = ASKING.lock().expect("asking lock");
    if asking.get(key).is_some_and(|(going, at)| *going || at.elapsed() < ASK_AGAIN) {
        return false;
    }
    asking.insert(key.to_owned(), (true, Instant::now()));
    true
}

fn asked(key: &str) {
    if let Some((going, _)) = ASKING.lock().expect("asking lock").get_mut(key) {
        *going = false;
    }
}

/// The tools of each local package `named`, by its path as it's written, and saying they're local: what a project's nodes name, and what's on
/// your list, read as the list reads it for every machine (`local_code::read_source`). One that can't be read is left out, and a render says why.
async fn read_local(api: &Arc<Api>, hub: &str, named: Vec<(String, PathBuf)>) -> Vec<Tool> {
    let scope = private_io::current();
    let private = scope.is_some();
    // Each function is read at once, the static describer's reading kept in its project by its tree. What's known answers now; one
    // read anew goes on past the list, its landing told as `tools.changed`, and the next list has it.
    let reads: Vec<_> = named.into_iter().map(|(written, path)| (written, reading(api, hub, scope.clone(), path))).collect();
    let now = tokio::time::Instant::now() + LOCAL_NOW;
    let mut tools = vec![];
    for (written, read) in reads {
        let Ok(Some(described)) = tokio::time::timeout_at(now, read).await else { continue };
        let (package, document) = match described {
            Ok(described) => described,
            Err(why) => {
                if !private { tracing::warn!("{written} isn't listed: {}", why.message); }
                continue;
            }
        };
        let Ok(interface) = Interface::parse(&document) else { continue };
        // Local code's own description isn't read: its words are its name's (`tool::title`).
        tools.extend(tool::tools_of(&package, "local", &interface, "").into_iter().map(|tool| Tool {
            id: format!("{written}/{}", tool.function),
            package: written.clone(),
            installs: Some(package.clone()),
            local: true,
            ..tool
        }));
    }
    tools
}

/// One reading of local code, shared by every list that asks for it while it goes: none if it ended without an answer.
type Reading = futures_util::future::Shared<futures_util::future::BoxFuture<'static, Option<Result<(String, Value), Error>>>>;

/// The readings going, by project, Hub and path: a page's lists, asked again and again while one takes seconds, start one, not one each.
static READING: LazyLock<Mutex<HashMap<(Option<String>, String, PathBuf), Reading>>> = LazyLock::new(Mutex::default);

/// `path`'s reading: the one going, else one started now, run to its end whoever waits for it. One that lands past `LOCAL_NOW` is told
/// as `tools.changed`, once, if it lists something: one that can't be listed would be read and told again forever.
fn reading(api: &Arc<Api>, hub: &str, scope: Option<private_io::Scope>, path: PathBuf) -> Reading {
    use futures_util::FutureExt as _;
    let key = (scope.as_ref().map(|scope| scope.storage.project_id.clone()), hub.to_owned(), path.clone());
    let mut going = READING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(read) = going.get(&key) {
        return read.clone();
    }
    let (api, hub, began, held) = (api.clone(), hub.to_owned(), Instant::now(), key.clone());
    let task = tokio::spawn(private_io::context(scope.clone(), async move {
        let _turn = LOCAL_READS.acquire().await;
        match staged(&api, scope.as_ref(), &path) {
            Ok(code) => Box::pin(local_code::read_source(&api, &code, &hub)).await,
            Err(why) => Err(why),
        }
    }));
    let read = async move {
        let described = task.await.ok();
        READING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&held);
        if began.elapsed() >= LOCAL_NOW && described.as_ref().is_some_and(|read| read.as_ref().is_ok_and(|(_, document)| Interface::parse(document).is_ok())) {
            crate::events::tools_changed();
        }
        described
    }.boxed().shared();
    tokio::spawn(read.clone());
    going.insert(key, read.clone());
    read
}

/// `path`'s code copied into its project, which holds still meanwhile: a lock under way refuses the read, and one asked waits.
fn staged(api: &Api, scope: Option<&private_io::Scope>, path: &Path) -> Result<api::code::LocalCode, Error> {
    let _admitted = scope.map(|scope| vault::try_admit_root(&vault::logical(&scope.storage.view_root))).transpose()?;
    local_code::own(api, path)
}

/// How long a list waits for the local functions its project's boxes name to be read: one whose reading is kept answers within it,
/// and one read anew never holds the list (a palette stood empty 15 s while one was read, e2e 44103425048); its landing is told.
const LOCAL_NOW: Duration = Duration::from_millis(250);
/// How many of them are read at once.
static LOCAL_READS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

/// Whether every word of `q` is in `tool`'s package's name or its label.
fn says(tool: &Tool, q: &str) -> bool {
    let text = format!("{} {} {}", tool.package, tool.package_title, tool.label).to_lowercase();
    q.to_lowercase().split_whitespace().all(|word| text.contains(word))
}

/// The tools of what this computer has installed from `hub`, each package at its newest there, by its kept card.
async fn held(api: &Arc<Api>, hub: &str) -> Vec<Tool> {
    kept_tools(api, hub, installed_releases(api, hub).await)
}

/// The tools of what this computer has installed from public repositories (cl-1127), each at the commit it holds: named as a box
/// names it there (`<url>@<commit>/<folder>/<fn>`), said from its repository (`source`, `git+<url>@<commit>`).
async fn fetched(api: &Arc<Api>) -> Vec<Tool> {
    let Some((_, environments)) = api.machines.holds(&On::Machine("local".into()), false).await else { return vec![] };
    let home = api.machines.hubs.home();
    let mut tools = vec![];
    for env in &environments {
        let Some(origin) = crate::git_code::origin_of_manifest(&env.manifest) else { continue };
        let Some(document) = local_code::kept_for(home, &env.package, &env.manifest) else { continue };
        let Ok(interface) = Interface::parse(&document) else { continue };
        let summary = origin.folder_here().map(|dir| checkout::description(&dir)).unwrap_or_default();
        let (package, source) = (origin.target(), origin.spelled());
        tools.extend(tool::tools_of(&package, &env.release, &interface, &summary).into_iter().map(|tool| Tool {
            id: format!("{package}/{}", tool.function),
            package: package.clone(),
            installs: Some(env.package.clone()),
            source: Some(source.clone()),
            ..tool
        }));
    }
    tools
}

/// A Hub's one spelling.
pub(crate) fn canonical(hub: &str) -> String {
    Origin::parse(hub).map_or_else(|_| hub.to_owned(), String::from)
}

/// What this computer's machine has installed from `hub`, as it last said, starting nothing: each package and its newest release there,
/// as a run naming none takes it: its newest final, else its newest prerelease (`api::release::newest`, cl-1109).
async fn installed_releases(api: &Arc<Api>, hub: &str) -> Vec<(String, String)> {
    let Some((_, environments)) = api.machines.holds(&On::Machine("local".into()), false).await else { return vec![] };
    let here = canonical(hub);
    let mut packages: Vec<&str> = (environments.iter()).filter(|env| !env.release.is_empty() && canonical(&env.hub) == here).map(|env| env.package.as_str()).collect();
    packages.sort_unstable();
    packages.dedup();
    let releases = |package: &'_ str| -> Option<String> {
        api::release::newest(environments.iter().filter(|env| env.package == package && !env.release.is_empty()).map(|env| env.release.as_str())).map(str::to_owned)
    };
    packages.into_iter().filter_map(|package| Some((package.to_owned(), releases(package)?))).collect()
}

/// The tools of the Hub's packages that fit `q`, each at its newest.
async fn on_the_hub(api: &Arc<Api>, hub: &str, q: &str) -> Result<Vec<Tool>, Error> {
    let reader = api.machines.hubs.reader(hub)?;
    let found = reader.search_packages(Some(q), LISTED).await.map_err(cards::refusal)?;
    let wanted = (found.packages.into_iter()).filter_map(|row| Some((format!("{}/{}", row.org, row.name), row.latest_release?))).collect();
    Ok(read_all(api, hub, wanted).await)
}

/// Each Hub's catalog, its tools at rest: every package it lists, at its newest release, by its card. Kept on disk and read at start
/// (`cards::kept_listed`); read again from the Hub when a page connects and each [`CATALOG_EVERY`] while one is open, and once for a Hub
/// never read. A list never waits on it; a read that changed it is told as `tools.changed` (cl-1126).
#[derive(Default)]
pub(crate) struct Listing {
    held: Mutex<HashMap<String, Arc<Vec<Tool>>>>,
    following: std::sync::OnceLock<()>,
}

impl Listing {
    /// `hub`'s catalog as last read. A Hub never read, or never answered, is read behind this.
    pub(crate) fn tools(&self, api: &Arc<Api>, hub: &str) -> Arc<Vec<Tool>> {
        let key = canonical(hub);
        let read = self.held.lock().expect("listing lock").get(&key).cloned();
        if let Some(tools) = read {
            return tools;
        }
        match kept_listing(api, hub) {
            Some(kept) => self.held.lock().expect("listing lock").entry(key).or_insert_with(|| Arc::new(kept)).clone(),
            None => {
                self.refresh(api, hub);
                Arc::default()
            }
        }
    }

    /// Reads `hub`'s catalog again behind the caller, and the newest of each package you added that it lacks.
    pub(crate) fn refresh(&self, api: &Arc<Api>, hub: &str) {
        if !asking(&format!("{} catalog", canonical(hub))) {
            return;
        }
        let (api, hub) = (api.clone(), hub.to_owned());
        tokio::spawn(async move {
            let read = tokio::time::timeout(CATALOG_MOST, every_tool(&api, &hub)).await;
            asked(&format!("{} catalog", canonical(&hub)));
            let tools = match read {
                Ok(Ok(tools)) => tools,
                Ok(Err(why)) => return tracing::debug!("{hub}'s catalog wasn't read: {}", why.message),
                Err(_) => return tracing::debug!("{hub}'s catalog wasn't read in {} s", CATALOG_MOST.as_secs()),
            };
            let changed = {
                let mut held = api.listing.held.lock().expect("listing lock");
                let was = held.insert(canonical(&hub), tools.clone());
                was.is_none_or(|was| was != tools)
            };
            if changed {
                crate::events::tools_changed();
            }
            if let Ok(mine) = api.calcifer.store().tool_list() {
                for package in mine.packages.iter().filter(|package| !is_local(package) && !tools.iter().any(|tool| tool.package == shown(package))) {
                    read_newest(&api, &hub, package);
                }
            }
        });
    }

    /// A page connected: each Hub it may list is read again now, and each [`CATALOG_EVERY`] while a page is open.
    pub(crate) fn page_connected(&self, api: &Arc<Api>) {
        for hub in self.hubs(api) {
            self.refresh(api, &hub);
        }
        self.following.get_or_init(|| {
            let api = api.clone();
            tokio::spawn(async move {
                loop {
                    crate::events::page_open().await;
                    tokio::time::sleep(CATALOG_EVERY).await;
                    if crate::events::pages() {
                        for hub in api.listing.hubs(&api) {
                            api.listing.refresh(&api, &hub);
                        }
                    }
                }
            });
        });
    }

    /// The default Hub, and every other a list asked for.
    fn hubs(&self, api: &Api) -> Vec<String> {
        let mut hubs: BTreeSet<String> = self.held.lock().expect("listing lock").keys().cloned().collect();
        hubs.insert(canonical(&api.machines.hubs.default_origin()));
        hubs.into_iter().collect()
    }
}

/// The tools of every package `hub` lists with a release, each at its newest.
async fn every_tool(api: &Arc<Api>, hub: &str) -> Result<Arc<Vec<Tool>>, Error> {
    let reader = api.machines.hubs.reader(hub)?;
    let mut wanted = vec![];
    loop {
        let page = reader.search_packages_page(None, MOST_BROWSED, wanted.len() as u32).await.map_err(cards::refusal)?;
        let read = page.packages.len();
        wanted.extend(page.packages.into_iter().map(|row| (format!("{}/{}", row.org, row.name), row.latest_release)));
        if read == 0 || wanted.len() as i64 >= page.search.total {
            break;
        }
    }
    let released: Vec<(String, String)> = wanted.into_iter().filter_map(|(package, release)| Some((package, release?))).collect();
    let tools = Arc::new(read_all(api, hub, released.clone()).await);
    cards::keep_listed(api.machines.hubs.home().root(), hub, &released);
    Ok(tools)
}

/// The last whole read of `hub`'s catalog that this computer kept, by the cards it kept. None: it never read one.
fn kept_listing(api: &Api, hub: &str) -> Option<Vec<Tool>> {
    let root = api.machines.hubs.home().root();
    let (_, listed) = cards::kept_listed(root, hub)?;
    let tools = listed.iter().filter_map(|(package, release)| {
        let card = cards::kept(root, hub, package, release)?;
        Some(tool::tools_of(package, &card.release, &card.interface, card.summary()))
    });
    Some(tools.flatten().collect())
}

/// Each `(package, release)`'s tools, in the order asked.
async fn read_all(api: &Arc<Api>, hub: &str, wanted: Vec<(String, String)>) -> Vec<Tool> {
    let root = api.machines.hubs.home().root();
    let reads = wanted.into_iter().map(|(package, release)| async move {
        let client = || api.machines.hubs.reader(hub);
        match cards::card(client, root, hub, &package, &release).await {
            Ok(card) => tool::tools_of(&package, &card.release, &card.interface, card.summary()),
            Err(why) => {
                tracing::debug!("{package}@{release}: {}", why.message);
                vec![]
            }
        }
    });
    stream::iter(reads).buffered(READS).collect::<Vec<_>>().await.into_iter().flatten().collect()
}

/// What a box of `tool` asks for, read as `read` reads it. None: it can't be read.
pub(crate) async fn fields_of(api: &Arc<Api>, tool: &str, dir: &Path) -> Option<Vec<tool::ToolField>> {
    tool_of(api, tool, dir).await.ok().map(|held| held.fields)
}

/// `tool` as `GET /v1/tools` lists it, read as a run of it on this computer would be, from its package's declarations: nothing of it runs.
pub(crate) async fn tool_of(api: &Arc<Api>, tool: &str, dir: &Path) -> Result<Tool, Error> {
    if api::calcifer::node::plain(tool.trim()).trim_start_matches("_/") == tool::EDIT {
        return Ok(tool::edit());
    }
    let hub = api.machines.hubs.default_origin();
    let found = match target::describe(api, &hub, tool, None, Some(dir), &runs::default_on(api)).await {
        Ok(found) => found,
        // This computer's machine can't run it (one held on an older Runtime): the Hub's card says what it asks for all the same.
        // Not for a checkout's package, whose source is what runs.
        Err(why) if checkout::named(tool).is_some() => return Err(why),
        Err(why) => return listed_tool(api, &hub, tool).await.ok_or(why),
    };
    let interface = Interface::parse(&found.interface).map_err(|why| Error::new(Kind::Unavailable, "package_interface_invalid", format!("{tool}: {why}")))?;
    let Some(function) = found.function.as_deref().or(target::only_function(&interface)).map(str::to_owned) else {
        let functions = interface.public().iter().map(|name| format!("{}/{name}", shown(&found.package))).collect::<Vec<_>>().join(", ");
        return Err(Error::new(Kind::Usage, "tool_names_no_function", format!("{tool} is a package, not a tool. Pick one of its functions: {functions}.")));
    };
    tool::tools_of(&found.package, &found.release, &interface, &found.summary).into_iter().find(|held| held.function == function)
        .ok_or_else(|| Error::new(Kind::Usage, "tool_not_a_tool", format!("{tool} isn't a tool: its function has no @app.tool, or makes nothing a box can show.")))
}

/// `tool` as `GET /v1/tools` lists it from the Hub: its package's newest release, by its card. None for local code, or what the Hub hasn't.
async fn listed_tool(api: &Arc<Api>, hub: &str, tool: &str) -> Option<Tool> {
    let id = shown(api::calcifer::node::plain(tool.trim()));
    if let Some(listed) = api.listing.tools(api, hub).iter().find(|listed| listed.id == id) {
        return Some(listed.clone());
    }
    let (package, _) = id.rsplit_once('/')?;
    on_the_hub(api, hub, package).await.ok()?.into_iter().find(|found| found.id == id)
}

/// A box's model choices, bound as a run binds them, refused where their slot can't take them (`checkpoints::check_choices`).
pub(crate) async fn validate_models(api: &Arc<Api>, target: &str, dir: &Path, fields: &Map<String, Value>) -> Result<(), Error> {
    let hub = api.machines.hubs.default_origin();
    let Ok(found) = runs::target::describe(api, &hub, target, None, Some(dir), &runs::default_on(api)).await else { return Ok(()) };
    let Ok(interface) = Interface::parse(&found.interface) else { return Ok(()) };
    let Some(function) = found.function.as_deref().or(target::only_function(&interface)) else { return Ok(()) };
    let Some(callable) = interface.callables.iter().find(|callable| callable.name == function) else { return Ok(()) };
    let mut document: Map<String, Value> = ["models", "lora"].into_iter()
        .filter_map(|field| fields.get(field).filter(|value| !value.is_null()).map(|value| (field.to_owned(), value.clone()))).collect();
    let mut choices = Choices::new(target, function, &callable.models, interface.call_slots(callable));
    choices.take(&mut document)?;
    let choices = choices.into_choices();
    choices::wire(&choices)?;
    crate::hub::checkpoints::check_choices(api, &hub, &crate::hub::checkpoints::slots_of(&interface, callable), &choices).await
}

/// What `tool` makes, `image`, `video`, `audio` or `text`, as its function's output says, and what a new box of it is called, read as a run of it
/// on this computer would be. A tool can be local code, by a path from `dir`, and a package with one public function names that one.
pub(crate) async fn read(api: &Arc<Api>, tool: &str, dir: &Path) -> Result<(String, String), Error> {
    if tool == tool::EDIT {
        let edit = tool::edit();
        return Ok((edit.makes, edit.name));
    }
    let hub = api.machines.hubs.default_origin();
    let found = target::describe(api, &hub, tool, None, Some(dir), &runs::default_on(api)).await?;
    let interface = Interface::parse(&found.interface).map_err(|why| Error::new(Kind::Unavailable, "package_interface_invalid", format!("{tool}: {why}")))?;
    let package = shown(&found.package);
    let name = found.function.as_deref().map(str::to_owned).or_else(|| target::only_function(&interface).map(str::to_owned));
    let Some(name) = name else {
        let functions = interface.public().iter().map(|name| format!("{package}/{name}")).collect::<Vec<_>>().join(", ");
        return Err(Error::new(Kind::Usage, "tool_names_no_function", format!("{tool} is a package, not a tool. Pick one of its functions: {functions}.")).next(format!("cozy run {package}")));
    };
    let callable = interface.callables.iter().find(|callable| callable.name == name);
    let made = callable.and_then(|callable| Some((tool::makes(callable)?.to_owned(), tool::box_name(&package, callable))));
    made.ok_or_else(|| Error::new(Kind::Usage, "tool_makes_no_media", format!("{package}/{name} doesn't make an image, a video, a sound or text.")).next("cozy package ls"))
}
