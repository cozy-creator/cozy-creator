//! What can be dropped on a canvas: `GET /v1/tools`, every function that makes media of your list and of what a machine has installed, and of
//! what a search finds on the Hub; your list, which you add to from the Hub's packages; and what a tool makes, for a node of it.
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
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
    config::Origin,
    error::{Error, Kind},
    interface::Interface,
    names::{ROOT, shown},
    calcifer::tool::{self, BrowsedPackage, Browsed, Tool, ToolList, ToolListChange, Tools},
};
use serde::Deserialize;
use tensorfs::format::newest_by;

use crate::{
    api::{Api, Refusal, parse},
    local_code, private_io,
    project::{self, vault},
    runs::{self, target},
};

/// Your list until you change it.
const DEFAULTS: [&str; 2] = ["_/qwen-image-2", "_/minimax-h3"];
/// The most `browse` reads of the Hub.
const MOST_BROWSED: u32 = 200;
/// How many packages one list reads of the Hub.
const LISTED: u32 = 50;
/// How many cards it reads at once; each is read once, then kept.
const READS: usize = 8;
/// A Hub slower than this leaves the list with what's installed, and says so.
pub(crate) const HUB_LATE: Duration = Duration::from_secs(5);

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

/// `GET /v1/tools`: the Edit and your list, then local code and what's installed, each as it is on that machine, and with `q` the Hub's, each at its newest.
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
    let (cards, mut note) = on_your_list(api, &hub, &mine, &held).await;
    let place = |tool: &Tool| mine.packages.iter().position(|package| shown(package) == tool.package);
    let (mut first, rest): (Vec<Tool>, Vec<Tool>) = held.into_iter().partition(|tool| place(tool).is_some());
    first.extend(cards);
    first.sort_by_key(place);
    // Taken off the list is off it at rest, and a search still finds it, as what a box of it asks for must.
    let shows = |tool: &Tool| q.map_or_else(|| !mine.without.contains(&tool.id), |q| says(tool, q));
    let mut tools: Vec<Tool> = std::iter::once(tool::edit()).chain(first).chain(local).chain(rest).filter(shows).collect();
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

/// Your list, the defaults until it's changed: it's yours, not a project's, kept in `state.db`.
fn mine(api: &Api) -> Result<ToolList, Refusal> {
    let kept = api.calcifer.store().tool_list()?;
    Ok(kept.unwrap_or_else(|| ToolList { packages: DEFAULTS.map(str::to_owned).to_vec(), without: vec![] }))
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

/// `POST /v1/tools/list`: a package on your list, all of it again if some was taken off, or tools off it. Nothing is installed or deleted, and a box
/// of a tool taken off goes on as it was. Local code is a project's, named by its boxes (cl-819): a path is refused.
pub(crate) async fn change(State(api): State<Arc<Api>>, body: Result<Bytes, BytesRejection>) -> Result<Json<ToolList>, Refusal> {
    let asked: ToolListChange = parse(&body?)?;
    if let Some(path) = asked.add.as_deref().filter(|package| api::target::written_as_path(package)) {
        let said = format!("{path} is local code, which is a project's: a box names it by its path from the project's folder");
        return Err(Refusal(Error::new(Kind::Usage, "tool_list_local", said).next("cozy open")));
    }
    let add = asked.add.map(|package| (owned(&package), format!("{}/", shown(&package))));
    let add = add.as_ref().map(|(full, shown_as)| (full.as_str(), shown_as.as_str()));
    let mine = api.calcifer.store().change_tool_list(&DEFAULTS, add, &asked.remove);
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

/// The tools of each package of your list `held` lacks, by its card alone: nothing's installed by listing it. Each is the Hub's newest, else the
/// newest card kept here, so the list stands with no Hub. One that has neither says why; one the Hub doesn't have isn't listed.
async fn on_your_list(api: &Arc<Api>, hub: &str, mine: &ToolList, held: &[Tool]) -> (Vec<Tool>, Option<String>) {
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
    let lacking = mine.packages.iter().filter(|package| !is_local(package) && !held.iter().any(|tool| tool.package == shown(package)));
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

/// What `on` has installed from `hub`: each package and its newest release there.
async fn installed_releases(api: &Arc<Api>, hub: &str, on: &On) -> Vec<(String, String)> {
    let Some((_, environments)) = api.machines.holds(on, true).await else { return vec![] };
    let here = canonical(hub);
    let mut packages: Vec<&str> = (environments.iter()).filter(|env| !env.release.is_empty() && canonical(&env.hub) == here).map(|env| env.package.as_str()).collect();
    packages.sort_unstable();
    packages.dedup();
    let releases = |package: &'_ str| -> Option<String> {
        newest_by(environments.iter().filter(|env| env.package == package && !env.release.is_empty()).map(|env| env.release.as_str()), |release| release).map(str::to_owned)
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
    let hub = api.machines.hubs.default_origin();
    let found = target::describe(api, &hub, tool, None, Some(dir), &runs::default_on(api)).await.ok()?;
    let interface = Interface::parse(&found.interface).ok()?;
    let function = found.function.as_deref().or(target::only_function(&interface))?.to_owned();
    tool::tools_of(&found.package, "", &interface).into_iter().find(|held| held.function == function).map(|held| held.fields)
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
