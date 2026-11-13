//! What can be dropped on a canvas: `GET /v1/tools`, every function that makes media of every package the Hub lists, of what a machine has
//! installed and of what a search finds on the Hub, less the tools you took off; and what a tool makes, for a node of it.
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
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
    release::{Moves, Takes},
};
use serde::Deserialize;
use serde_json::{Map, Value};
use tensorfs::format::newest_by;

use crate::{
    api::{Api, Refusal, parse},
    local_code, private_io,
    project::{self, vault},
    runs::{self, target},
};

/// The most `browse` reads of the Hub, and how many packages one page of the Hub's list holds.
const MOST_BROWSED: u32 = 200;
/// How many packages one list reads of the Hub.
const LISTED: u32 = 50;
/// How many cards it reads at once; each is read once, then kept.
const READS: usize = 8;
/// A Hub slower than this leaves the list with what's installed, and says so.
pub(crate) const HUB_LATE: Duration = Duration::from_secs(5);
/// The Hub's list read this recently is the list.
const LISTED_FOR: Duration = Duration::from_secs(60);
/// A Hub that didn't answer is asked again this long after.
const ASK_AGAIN: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
pub(crate) struct Asked {
    /// Words: what's installed by its name and what it does, and the Hub's own search.
    q: Option<String>,
    /// The machine whose installed packages come first; `local` when absent.
    on: Option<String>,
    hub: Option<String>,
    /// A project: the local code its nodes name comes first, and no other local code is listed.
    project: Option<String>,
}

/// `GET /v1/tools`: the Edit, local code, what's installed as it is on that machine, then every package the Hub lists and any you added, each
/// at its newest; with `q`, what fits it and the Hub's search for it.
pub(crate) async fn list(State(api): State<Arc<Api>>, query: Result<Query<Asked>, QueryRejection>) -> Result<Response, Refusal> {
    let Query(asked) = query?;
    let project = asked.project.clone();
    vault::project_reply(&api, project.as_deref(), list_tools(&api, asked)).await
}

async fn list_tools(api: &Arc<Api>, asked: Asked) -> Result<Json<Tools>, Refusal> {
    let hub = runs::hub(api, asked.hub.as_deref().unwrap_or_default());
    let q = asked.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let on = On::Machine(asked.on.unwrap_or_else(|| "local".into()));
    let mine = mine(api)?;
    let local = match asked.project.as_deref() {
        Some(named) => {
            let root = project::named(api, named).map_err(Refusal)?;
            let scope = private_io::of_output(&project::landing(&root)).map_err(Refusal)?;
            private_io::context(scope, async {
                let named = project::local_code_named(api, named)?;
                Ok::<_, Refusal>(read_local(api, &hub, named, &on).await)
            }).await?
        }
        None => vec![],
    };
    let held = held(api, &hub, &on).await;
    let (listed, mut note) = api.listing.tools(api, &hub).await;
    let installed: BTreeSet<&str> = held.iter().map(|tool| tool.package.as_str()).collect();
    // What its machine can't run, on its Runtime or one a run of it moves to (cl-952), isn't offered (run 46790: flux-3-klein@0.7.0,
    // locked to Runtime 0.44). A release that needs a newer Runtime is (cl-975: every 0.25 release, on a machine still on 0.45.0).
    let runner = Runner::of(api, &on).await;
    let runs = |offered: &&Offered| runner.as_ref().is_none_or(|runner| runner.takes(&offered.pair));
    let listed: Vec<Tool> = (listed.iter()).filter(|offered| !installed.contains(offered.tool.package.as_str())).filter(runs).map(|offered| offered.tool.clone()).collect();
    let mut tools: Vec<Tool> = std::iter::once(tool::edit()).chain(local).chain(held).chain(listed).collect();
    let (added, unread) = on_your_list(api, &hub, &mine, &tools).await;
    note = note.or(unread);
    tools.extend(added);
    // Taken off is off at rest, and a search still finds it, as what a box of it asks for must.
    tools.retain(|tool| q.map_or_else(|| !mine.without.contains(&tool.id), |q| says(tool, q)));
    if let Some(q) = q {
        let listed: BTreeSet<String> = tools.iter().map(|tool| tool.package.clone()).collect();
        match tokio::time::timeout(HUB_LATE, on_the_hub(api, &hub, q)).await {
            Ok(Ok(found)) => tools.extend(found.into_iter().filter(|tool| !listed.contains(&tool.package))),
            Ok(Err(why)) => note = Some(format!("only what's installed: the Hub said {}", why.message)),
            Err(_) => note = Some(late()),
        }
    }
    // A Hub that's late leaves them unsaid, and a later list says what's been read by then.
    let _ = tokio::time::timeout(HUB_LATE, crate::hub::checkpoints::families(api, &hub, &mut tools)).await;
    Ok(Json(Tools { tools, note }))
}

fn late() -> String {
    format!("only what's installed: the Hub didn't answer in {} s", HUB_LATE.as_secs())
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
        let said = format!("{path} is local code, which is a project's: a box names it by its path from the project's folder");
        return Err(Refusal(Error::new(Kind::Usage, "tool_list_local", said).next("cozy open")));
    }
    let add = asked.add.map(|package| (owned(&package), format!("{}/", shown(&package))));
    let add = add.as_ref().map(|(full, shown_as)| (full.as_str(), shown_as.as_str()));
    let mine = api.calcifer.store().change_tool_list(add, &asked.remove);
    Ok(Json(mine.map_err(|why| Refusal(Error::new(Kind::Unavailable, "tool_list_io", why.to_string())))?))
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
            let tools: Vec<String> = tool::tools_of(&package, &card.release, &card.interface).into_iter().map(|tool| tool.id).collect();
            (!tools.is_empty()).then(|| BrowsedPackage { package: shown(&package), summary: card.summary().to_owned(), tools })
        }
    });
    let packages = stream::iter(reads).buffered(READS).collect::<Vec<_>>().await.into_iter().flatten().collect();
    Ok(Json(Browsed { packages, older }))
}

/// The tools of each package you added that `listed` lacks, by its card alone: nothing's installed by listing it. Each is the Hub's newest,
/// else the newest card kept here, so the list stands with no Hub. One that has neither says why; one the Hub doesn't have isn't listed.
async fn on_your_list(api: &Arc<Api>, hub: &str, mine: &ToolList, listed: &[Tool]) -> (Vec<Tool>, Option<String>) {
    let root = api.machines.hubs.home().root();
    let newest = async |package: &String| {
        let asked = async { cards::releases(&api.machines.hubs.reader(hub)?, package).await?.newest(package) };
        let why = match tokio::time::timeout(HUB_LATE, asked).await {
            Ok(Ok(release)) => return (Some((package.clone(), release)), None),
            Ok(Err(why)) if why.kind == Kind::NotFound => return (None, None),
            Ok(Err(why)) => format!("only what's installed: the Hub said {}", why.message),
            Err(_) => late(),
        };
        match newest_by(cards::kept_releases(root, hub, package), |release| release) {
            Some(release) => (Some((package.clone(), release)), None),
            None => (None, Some(why)),
        }
    };
    let lacking = mine.packages.iter().filter(|package| !is_local(package) && !listed.iter().any(|tool| tool.package == shown(package)));
    let (wanted, notes): (Vec<_>, Vec<_>) = futures_util::future::join_all(lacking.map(newest)).await.into_iter().unzip();
    (read_all(api, hub, wanted.into_iter().flatten().collect()).await, notes.into_iter().flatten().next())
}

/// The tools of each local package `named`, by its path as it's written, and saying they're local: what a project's nodes name, and what's on
/// your list. One that can't be read is left out, and a render says why.
async fn read_local(api: &Arc<Api>, hub: &str, named: Vec<(String, PathBuf)>, on: &On) -> Vec<Tool> {
    let private = private_io::current().is_some();
    let mut tools = vec![];
    for (written, path) in named {
        let described = match local_code::own(api.machines.hubs.home(), &path) {
            Ok(code) => local_code::describe(api, &code, on, hub).await,
            Err(why) => Err(why),
        };
        let (package, document) = match described {
            Ok(described) => described,
            Err(why) => {
                if !private { tracing::warn!("{written} isn't listed: {}", why.message); }
                continue;
            }
        };
        let Ok(interface) = Interface::parse(&document) else { continue };
        tools.extend(tool::tools_of(&package, "local", &interface).into_iter().map(|tool| Tool {
            id: format!("{written}/{}", tool.function),
            package: written.clone(),
            installs: Some(package.clone()),
            local: true,
            ..tool
        }));
    }
    tools
}

/// Whether every word of `q` is in `tool`'s package's name or its label.
fn says(tool: &Tool, q: &str) -> bool {
    let text = format!("{} {} {}", tool.package, tool.package_title, tool.label).to_lowercase();
    q.to_lowercase().split_whitespace().all(|word| text.contains(word))
}

/// The tools of what `on` has installed from `hub`, each package at its newest there. A release that can't be read is left out.
async fn held(api: &Arc<Api>, hub: &str, on: &On) -> Vec<Tool> {
    read_all(api, hub, installed_releases(api, hub, on).await).await
}

/// A Hub's one spelling.
pub(crate) fn canonical(hub: &str) -> String {
    Origin::parse(hub).map_or_else(|_| hub.to_owned(), String::from)
}

/// What `on` has installed from `hub`: each package and its newest release there, as a run there takes it: on the RC's
/// Runtime a prerelease built for it before the last final, elsewhere finals only (`api::release::newest`, cl-952).
async fn installed_releases(api: &Arc<Api>, hub: &str, on: &On) -> Vec<(String, String)> {
    let Some((machine, environments)) = api.machines.holds(on, true).await else { return vec![] };
    let rc = api.machines.takes_rc(&machine, on);
    let here = canonical(hub);
    let mut packages: Vec<&str> = (environments.iter()).filter(|env| !env.release.is_empty() && canonical(&env.hub) == here).map(|env| env.package.as_str()).collect();
    packages.sort_unstable();
    packages.dedup();
    let releases = |package: &'_ str| -> Option<String> {
        api::release::newest(environments.iter().filter(|env| env.package == package && !env.release.is_empty()).map(|env| env.release.as_str()), rc).map(str::to_owned)
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

/// Each Hub's tools at rest, as last read: every package it lists, at its newest release, by its card. A list reads it again when it's older
/// than [`LISTED_FOR`]; a Hub that doesn't answer leaves the last read, and is asked again after [`ASK_AGAIN`].
#[derive(Default)]
pub(crate) struct Listing(Mutex<HashMap<String, Listed>>);

#[derive(Clone, Default)]
struct Listed {
    tools: Arc<Vec<Offered>>,
    /// When the Hub answered it: a read here, or the one kept from before a restart (`cards::kept_listed`).
    read_at: Option<SystemTime>,
    /// The last read that didn't answer: when, and why.
    failed: Option<(Instant, String)>,
}

impl Listed {
    fn age(&self) -> Option<Duration> {
        self.read_at.map(|at| at.elapsed().unwrap_or_default())
    }
}

impl Listing {
    /// `hub`'s tools, and why they're old or none: its last read, else this one, which goes on past [`HUB_LATE`] for a later list.
    /// Calcifer just started, its last read is the one it kept, so a restart never lists only what's installed (cl-975).
    async fn tools(&self, api: &Arc<Api>, hub: &str) -> (Arc<Vec<Offered>>, Option<String>) {
        let key = canonical(hub);
        let read = self.0.lock().expect("listing lock").get(&key).cloned();
        let kept = match read {
            Some(read) => read,
            None => {
                let kept = kept_listing(api, hub);
                self.0.lock().expect("listing lock").entry(key).or_insert(kept).clone()
            }
        };
        if kept.age().is_some_and(|age| age < LISTED_FOR) {
            return (kept.tools, None);
        }
        let why = match &kept.failed {
            Some((at, why)) if at.elapsed() < ASK_AGAIN => why.clone(),
            _ => {
                let (api, origin) = (api.clone(), hub.to_owned());
                let read = tokio::spawn(async move {
                    let read = every_tool(&api, &origin).await;
                    api.listing.keep(&origin, read.as_ref().map(Arc::clone).map_err(|why| format!("the Hub said {}", why.message)));
                    read
                });
                match tokio::time::timeout(HUB_LATE, read).await {
                    Ok(Ok(Ok(tools))) => return (tools, None),
                    Ok(Ok(Err(why))) => format!("the Hub said {}", why.message),
                    Ok(Err(why)) => format!("reading the Hub's list failed: {why}"),
                    Err(_) => {
                        let why = format!("the Hub didn't answer in {} s", HUB_LATE.as_secs());
                        self.keep(hub, Err(why.clone()));
                        why
                    }
                }
            }
        };
        match kept.age() {
            Some(age) => (kept.tools, Some(format!("the Hub's tools as read {} s ago: {why}", age.as_secs()))),
            None => (kept.tools, Some(format!("only what's installed: {why}"))),
        }
    }

    fn keep(&self, hub: &str, read: Result<Arc<Vec<Offered>>, String>) {
        let mut held = self.0.lock().expect("listing lock");
        let listed = held.entry(canonical(hub)).or_default();
        match read {
            Ok(tools) => *listed = Listed { tools, read_at: Some(SystemTime::now()), failed: None },
            Err(why) => listed.failed = Some((Instant::now(), why)),
        }
    }
}

/// The tools of every package `hub` lists with a release, each at its newest.
async fn every_tool(api: &Arc<Api>, hub: &str) -> Result<Arc<Vec<Offered>>, Error> {
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
    let root = api.machines.hubs.home().root();
    let reads = released.clone().into_iter().map(|(package, release)| async move {
        match cards::card_requiring(|| api.machines.hubs.reader(hub), root, hub, &package, &release).await {
            Ok(card) => offered(&package, &card),
            Err(why) => {
                tracing::debug!("{package}@{release}: {}", why.message);
                vec![]
            }
        }
    });
    let tools = Arc::new(stream::iter(reads).buffered(READS).collect::<Vec<Vec<Offered>>>().await.into_iter().flatten().collect());
    cards::keep_listed(root, hub, &released);
    Ok(tools)
}

/// The last whole read of `hub`'s list that this computer kept, by the cards it kept: none, and the list holds what's installed.
fn kept_listing(api: &Api, hub: &str) -> Listed {
    let root = api.machines.hubs.home().root();
    let Some((at, listed)) = cards::kept_listed(root, hub) else { return Listed::default() };
    let tools = (listed.iter()).filter_map(|(package, release)| Some(offered(package, &cards::kept(root, hub, package, release)?))).flatten().collect();
    Listed { tools: Arc::new(tools), read_at: Some(at), failed: None }
}

/// `package`'s tools at `card`'s release, each with what the release requires of the Runtime pair.
fn offered(package: &str, card: &cards::Card) -> Vec<Offered> {
    let pair: Arc<[String]> = card.requirements().unwrap_or_default().into_iter().filter(|required| on_the_pair(required)).map(str::to_owned).collect();
    tool::tools_of(package, &card.release, &card.interface).into_iter().map(|tool| Offered { tool, pair: pair.clone() }).collect()
}

/// A tool the Hub lists, and what its release requires of the Runtime pair.
#[derive(Clone)]
struct Offered {
    tool: Tool,
    /// Its wheel's requirements on `cozy-runtime` and `tensorfs` (`Requires-Dist`).
    pair: Arc<[String]>,
}

/// The pair a machine runs packages on.
const PAIR: [&str; 2] = ["cozy-runtime", "tensorfs"];

/// The machine the tools at rest are offered for: the pair it runs packages on, and the Runtimes it moves to for a release that
/// needs a newer one, as a run there moves it (`api::release::takes`, cl-952).
struct Runner {
    runtime: String,
    tensorfs: String,
    runtimes: Vec<String>,
    moves: Moves,
}

impl Runner {
    /// `on` as it last said what it runs, asking it nothing. None: it hasn't said since Calcifer started, or what it moves to isn't
    /// read in [`HUB_LATE`], and nothing is held back, as a run on a machine that hasn't said holds nothing back.
    async fn of(api: &Arc<Api>, on: &On) -> Option<Self> {
        let On::Machine(name) = on else { return None };
        let frame = api.machines.frame(name).filter(|frame| !frame.runtime.is_empty())?;
        let (runtimes, moves) = tokio::time::timeout(HUB_LATE, api.machines.runtimes(name, &frame, api.machines.takes_rc(name, on))).await.ok()?;
        Some(Self { runtime: frame.runtime, tensorfs: frame.tensorfs, runtimes, moves })
    }

    /// Whether a run here takes a release whose wheel requires `pair`, as a run decides (`api::release::takes`): its Runtime runs it
    /// (an RC build of the next minor, one capped below it) with its TensorFS, or it moves up to a Runtime that does, which brings
    /// its own TensorFS. A requirement that can't be read holds nothing back, as TensorD's own check decides.
    fn takes(&self, pair: &[String]) -> bool {
        let required = |member: &str| api::release::required(pair.iter().map(String::as_str), member);
        let machine = api::release::Machine { runtime: &self.runtime, runtimes: &self.runtimes, moves: self.moves };
        match required(PAIR[0]).map_or(Takes::Runs, |needs| api::release::takes(&needs, &machine)) {
            Takes::Runs => required(PAIR[1]).is_none_or(|needs| self.tensorfs.is_empty() || api::release::runs(&needs, &self.tensorfs)),
            Takes::Moves(_) => true,
            Takes::Held(_) | Takes::No => false,
        }
    }
}

/// Whether a requirement (`cozy-runtime[media]<0.46,>=0.45.0`) is on a member of the pair, outside any extra.
fn on_the_pair(required: &str) -> bool {
    PAIR.iter().any(|member| api::release::required([required], member).is_some())
}

/// Each `(package, release)`'s tools, in the order asked.
async fn read_all(api: &Arc<Api>, hub: &str, wanted: Vec<(String, String)>) -> Vec<Tool> {
    let root = api.machines.hubs.home().root();
    let reads = wanted.into_iter().map(|(package, release)| async move {
        let client = || api.machines.hubs.reader(hub);
        match cards::card(client, root, hub, &package, &release).await {
            Ok(card) => tool::tools_of(&package, &card.release, &card.interface),
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
        Err(why) => return listed_tool(api, &hub, tool).await.ok_or(why),
    };
    let interface = Interface::parse(&found.interface).map_err(|why| Error::new(Kind::Unavailable, "package_interface_invalid", format!("{tool}: {why}")))?;
    let Some(function) = found.function.as_deref().or(target::only_function(&interface)).map(str::to_owned) else {
        let functions = interface.public().iter().map(|name| format!("{}/{name}", shown(&found.package))).collect::<Vec<_>>().join(", ");
        return Err(Error::new(Kind::Usage, "tool_names_no_function", format!("{tool} is a package: a tool is one of its functions: {functions}")));
    };
    tool::tools_of(&found.package, &found.release, &interface).into_iter().find(|held| held.function == function)
        .ok_or_else(|| Error::new(Kind::Usage, "tool_not_a_tool", format!("{tool} is no tool: its function declares no @app.tool, or makes no media")))
}

/// `tool` as `GET /v1/tools` lists it from the Hub: its package's newest release, by its card. None for local code, or what the Hub hasn't.
async fn listed_tool(api: &Arc<Api>, hub: &str, tool: &str) -> Option<Tool> {
    let id = shown(api::calcifer::node::plain(tool.trim()));
    let (listed, _) = api.listing.tools(api, hub).await;
    if let Some(offered) = listed.iter().find(|offered| offered.tool.id == id) {
        return Some(offered.tool.clone());
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

/// What `tool` makes, `image`, `video` or `audio`, as its function's output says, and what a new box of it is called, read as a run of it
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
        return Err(Error::new(Kind::Usage, "tool_names_no_function", format!("{tool} is a package: a tool is one of its functions: {functions}")).next(format!("cozy run {package}")));
    };
    let callable = interface.callables.iter().find(|callable| callable.name == name);
    let made = callable.and_then(|callable| Some((tool::output_kind(callable)?.to_owned(), tool::box_name(&package, callable))));
    made.ok_or_else(|| Error::new(Kind::Usage, "tool_makes_no_media", format!("{package}/{name} makes no picture, video or sound")).next("cozy package ls"))
}
