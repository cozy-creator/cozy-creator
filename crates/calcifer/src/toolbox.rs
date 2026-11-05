//! What can be dropped on a canvas: `GET /v1/tools`, every function that makes media of what a machine has installed and what the Hub
//! has, and what a tool makes, for a node of it.
use std::{collections::BTreeSet, path::Path, sync::Arc, time::Duration};

use axum::{
    Json,
    extract::{Query, State, rejection::QueryRejection},
};
use futures_util::{StreamExt as _, stream};
use hub_client::cards;
use protocol::{
    calcifer::On,
    config::Origin,
    error::{Error, Kind},
    interface::Interface,
    names::shown,
    tool::{self, Tool, Tools},
};
use serde::Deserialize;
use tensorfs::format::newest_by;

use crate::{
    api::{Api, Refusal},
    local_code,
    runs::{self, target},
};

/// How many packages one list reads of the Hub.
const LISTED: u32 = 50;
/// How many cards it reads at once; each is read once, then kept.
const READS: usize = 8;
/// A Hub slower than this leaves the list with what's installed, and says so.
const HUB_LATE: Duration = Duration::from_secs(5);

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

/// `GET /v1/tools`: installed first, each as it is on that machine, then the Hub's, each at its newest.
pub(crate) async fn list(State(api): State<Arc<Api>>, query: Result<Query<Asked>, QueryRejection>) -> Result<Json<Tools>, Refusal> {
    let Query(asked) = query?;
    let hub = runs::hub(&api, asked.hub.as_deref().unwrap_or_default());
    let q = asked.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let on = On::Machine(asked.on.unwrap_or_else(|| "local".into()));
    let local = match &asked.project {
        Some(project) => named_here(&api, &hub, project, &on).await?,
        None => vec![],
    };
    let mut tools: Vec<Tool> = std::iter::once(tool::edit()).chain(local).chain(held(&api, &hub, &on).await).filter(|tool| q.is_none_or(|q| says(tool, q))).collect();
    let installed: BTreeSet<String> = tools.iter().map(|tool| tool.package.clone()).collect();
    let note = match tokio::time::timeout(HUB_LATE, on_the_hub(&api, &hub, q)).await {
        Ok(Ok(found)) => {
            tools.extend(found.into_iter().filter(|tool| !installed.contains(&tool.package)));
            None
        }
        Ok(Err(why)) => Some(format!("only what's installed: the Hub said {}", why.message)),
        Err(_) => Some(format!("only what's installed: the Hub didn't answer in {} s", HUB_LATE.as_secs())),
    };
    Ok(Json(Tools { tools, note }))
}

/// The tools of each local package `project`'s nodes name, named by its path as they write it, and saying they're local. One that can't be
/// read is left out, and a render says why.
async fn named_here(api: &Arc<Api>, hub: &str, project: &str, on: &On) -> Result<Vec<Tool>, Refusal> {
    let mut tools = vec![];
    for (written, path) in crate::project::local_code_named(api, project)? {
        let Ok((package, document)) = local_code::describe(api, &path, on, hub).await else { continue };
        let Ok(interface) = Interface::parse(&document) else { continue };
        tools.extend(tool::tools_of(&package, "local", &interface).into_iter().map(|tool| Tool {
            id: format!("{written}/{}", tool.function),
            package: written.clone(),
            label: format!("{} (local)", tool.label),
            local: true,
            ..tool
        }));
    }
    Ok(tools)
}

/// Whether every word of `q` is in what `tool` is called or says it does.
fn says(tool: &Tool, q: &str) -> bool {
    let text = format!("{} {} {}", tool.id, tool.label, tool.description).to_lowercase();
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
    let Some((_, environments)) = api.machines.holds(on).await else { return vec![] };
    let here = canonical(hub);
    let mut packages: Vec<&str> = (environments.iter()).filter(|env| !env.release.is_empty() && canonical(&env.hub) == here).map(|env| env.package.as_str()).collect();
    packages.sort_unstable();
    packages.dedup();
    let releases = |package: &'_ str| -> Option<String> {
        newest_by(environments.iter().filter(|env| env.package == package && !env.release.is_empty()).map(|env| env.release.as_str()), |release| release).map(str::to_owned)
    };
    packages.into_iter().filter_map(|package| Some((package.to_owned(), releases(package)?))).collect()
}

/// The tools of the Hub's packages that fit `q`, or its first page, each at its newest.
async fn on_the_hub(api: &Arc<Api>, hub: &str, q: Option<&str>) -> Result<Vec<Tool>, Error> {
    let reader = api.machines.hubs.reader(hub)?;
    let found = reader.search_packages(q, LISTED).await.map_err(cards::refusal)?;
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

/// Every package a machine-side call could use, what `on` holds first, then the Hub's first page: each at its release.
pub(crate) async fn candidates(api: &Arc<Api>, hub: &str, on: &On) -> Vec<(String, String)> {
    let mut found = installed_releases(api, hub, on).await;
    let listed = async { api.machines.hubs.reader(hub).ok()?.search_packages(None, LISTED).await.ok() };
    if let Ok(Some(listed)) = tokio::time::timeout(HUB_LATE, listed).await {
        let new = listed.packages.into_iter().filter_map(|row| Some((format!("{}/{}", row.org, row.name), row.latest_release?)));
        let held: BTreeSet<String> = found.iter().map(|(package, _)| package.clone()).collect();
        found.extend(new.filter(|(package, _)| !held.contains(package)));
    }
    found
}

/// What `tool` makes, `image`, `video` or `audio`, as its function's output says, read as a run of it on this computer would be.
/// A tool can be local code, by a path from `dir`, and a package with one public function names that one.
pub(crate) async fn kind(api: &Arc<Api>, tool: &str, dir: &Path) -> Result<String, Error> {
    if tool == tool::EDIT {
        return Ok(tool::edit().kind);
    }
    let hub = api.machines.hubs.default_origin();
    let found = target::describe(api, &hub, tool, Some(dir), &runs::default_on(api)).await?;
    let interface = Interface::parse(&found.interface).map_err(|why| Error::new(Kind::Unavailable, "package_interface_invalid", format!("{tool}: {why}")))?;
    let package = shown(&found.package);
    let name = found.function.as_deref().map(str::to_owned).or_else(|| target::only_function(&interface).map(str::to_owned));
    let Some(name) = name else {
        let functions = interface.public().iter().map(|name| format!("{package}/{name}")).collect::<Vec<_>>().join(", ");
        return Err(Error::new(Kind::Usage, "tool_names_no_function", format!("{tool} is a package: a tool is one of its functions: {functions}")).next(format!("cozy run {package}")));
    };
    let made = interface.callables.iter().find(|callable| callable.name == name).and_then(tool::output_kind);
    made.map(str::to_owned).ok_or_else(|| Error::new(Kind::Usage, "tool_makes_no_media", format!("{package}/{name} makes no picture, video or sound")).next("cozy package ls"))
}
