//! `/v1/packages`: the Hub's package catalog as this computer's account sees it (anonymous when
//! signed out), and a package's yank and deletion by its owner.
use std::sync::Arc;

use axum::{
    Json,
    body::Bytes,
    extract::{
        Path, Query, State,
        rejection::{BytesRejection, QueryRejection},
    },
};
use api::{
    calcifer::{Agreed, Environment, Kind as RunKind, On, Status, SubmitRun},
    code::LocalCode,
    config::Origin,
    error::{Error, Kind},
    calcifer::hub::{
        At, FoundPackage, PackageDeleted, PackageDetail, PackageInstall, PackageRelease, PackageRemove, PackageSearch, PackageUnyanked,
        PackageYanked, PackagesRemoved, RemovedRelease,
    },
    machine::{self as v1, Source},
    names::shown,
};
use serde::Deserialize;

use super::{
    after::{After, failure, settled},
    body,
    jobs::{Planned, Work},
    origin, reader, refusal, signed_in,
};
use crate::api::{Api, Refusal};

#[derive(Deserialize)]
pub(super) struct Search {
    hub: Option<String>,
    q: Option<String>,
    /// 1–200; 20 when absent.
    limit: Option<u32>,
}

/// `GET /v1/packages`.
pub(super) async fn search(
    State(api): State<Arc<Api>>,
    query: Result<Query<Search>, QueryRejection>,
) -> Result<Json<PackageSearch>, Refusal> {
    let Query(ask) = query?;
    let hub = origin(&api, ask.hub.as_deref())?;
    let q = ask.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let limit = ask.limit.unwrap_or(20).clamp(1, 200);
    let answer = (reader(&api, &hub)?.search_packages(q, limit).await).map_err(|e| refusal(&hub, e))?;
    let packages: Vec<FoundPackage> = (answer.packages.into_iter())
        .map(|p| FoundPackage {
            package: shown(&format!("{}/{}", p.org, p.name)),
            latest_release: p.latest_release,
            created_at: stamp(&p.created_at),
        })
        .collect();
    let older = (answer.search.total - packages.len() as i64).max(0);
    Ok(Json(PackageSearch { packages, total: answer.search.total, older }))
}

#[derive(Deserialize)]
pub(super) struct Info {
    hub: Option<String>,
    release: Option<String>,
}

/// `GET /v1/packages/{org}/{name}`: its releases newest first, or the one named with its publish
/// warnings.
pub(super) async fn info(
    State(api): State<Arc<Api>>,
    Path((org, name)): Path<(String, String)>,
    query: Result<Query<Info>, QueryRejection>,
) -> Result<Json<PackageDetail>, Refusal> {
    let Query(ask) = query?;
    let hub = origin(&api, ask.hub.as_deref())?;
    let package = format!("{org}/{name}");
    let card = (reader(&api, &hub)?.get_package(&org, &name).await)
        .map_err(|e| missing(refusal(&hub, e).0, &package, &name, &hub))?;
    let releases: Vec<PackageRelease> = (card.releases.into_iter())
        .map(|r| PackageRelease {
            yanked: r.yanked.unwrap_or(false) || r.yanked_at.is_some(),
            published: r.cut_at.as_deref().map(stamp),
            release: r.release,
        })
        .collect();
    let mut releases = newest_first(releases, |r| r.release.as_str());
    let mut warnings = vec![];
    if let Some(release) = &ask.release {
        releases.retain(|r| r.release == *release);
        if releases.is_empty() {
            return Err(Refusal(
                Error::not_found("release_not_found", format!("{package} has no release {release} on {hub}"))
                    .next(format!("cozy package info {package}")),
            ));
        }
        warnings = warned(&api, &hub, &org, &name, release).await?;
    }
    Ok(Json(PackageDetail {
        package: format!("{}/{}", card.package.org, card.package.name),
        latest_release: card.package.latest_release,
        releases,
        warnings,
    }))
}

/// A release's publish warnings (a pin of its org's own package since yanked, or with a newer
/// release), as the Hub tells the package's owner; none for anyone else.
async fn warned(api: &Api, hub: &Origin, org: &str, name: &str, release: &str) -> Result<Vec<String>, Refusal> {
    if !hub.device_key(api.machines.hubs.home()).exists() {
        return Ok(vec![]);
    }
    let status = signed_in(api, hub)?.get_package_finalization(org, name, release).await;
    match status.map_err(|e| refusal(hub, e).0) {
        Ok(status) => Ok(status.warnings),
        Err(e) if matches!(e.kind, Kind::Forbidden | Kind::Credential | Kind::NotFound) => Ok(vec![]),
        Err(e) => Err(Refusal(e)),
    }
}

/// A package this Hub does not have, named with the Hub searched and where to look instead.
fn missing(error: Error, package: &str, name: &str, hub: &Origin) -> Refusal {
    if error.kind != Kind::NotFound {
        return Refusal(error);
    }
    let cause = error.cause.clone().unwrap_or(error.code);
    Refusal(
        Error::not_found("package_not_found", format!("no package {package} on {hub}"))
            .remedy("check the name")
            .next(format!("cozy package ls {name}"))
            .cause(cause),
    )
}

/// `POST /v1/packages/{org}/{name}/releases/{release}/yank`.
pub(super) async fn yank(
    State(api): State<Arc<Api>>,
    Path((org, name, release)): Path<(String, String, String)>,
    ask: Result<Bytes, BytesRejection>,
) -> Result<Json<PackageYanked>, Refusal> {
    let At { hub } = body(ask)?;
    let hub = origin(&api, hub.as_deref())?;
    let yanked = (signed_in(&api, &hub)?.yank_package_release(&org, &name, &release).await)
        .map_err(|e| refusal(&hub, e))?;
    api.releases.forget(&hub, &format!("{org}/{name}"));
    Ok(Json(PackageYanked {
        package: format!("{org}/{name}"),
        release: yanked.release,
        changed: yanked.changed,
        yanked_at: stamp(&yanked.yanked_at),
    }))
}

/// `DELETE /v1/packages/{org}/{name}/releases/{release}/yank`: un-yanked, the unversioned default
/// again when it is the newest (a gated candidate's promotion).
pub(super) async fn unyank(
    State(api): State<Arc<Api>>,
    Path((org, name, release)): Path<(String, String, String)>,
    query: Result<Query<At>, QueryRejection>,
) -> Result<Json<PackageUnyanked>, Refusal> {
    let hub = origin(&api, query?.0.hub.as_deref())?;
    let unyanked = (signed_in(&api, &hub)?.unyank_package_release(&org, &name, &release).await)
        .map_err(|e| refusal(&hub, e))?;
    api.releases.forget(&hub, &format!("{org}/{name}"));
    Ok(Json(PackageUnyanked { package: format!("{org}/{name}"), release: unyanked.release, changed: unyanked.changed }))
}

/// `DELETE /v1/packages/{org}/{name}`: with every release; machines that installed it keep their
/// copies.
pub(super) async fn delete(
    State(api): State<Arc<Api>>,
    Path((org, name)): Path<(String, String)>,
    query: Result<Query<At>, QueryRejection>,
) -> Result<Json<PackageDeleted>, Refusal> {
    let hub = origin(&api, query?.0.hub.as_deref())?;
    let deleted = (signed_in(&api, &hub)?.delete_package(&org, &name).await).map_err(|e| refusal(&hub, e))?;
    Ok(Json(PackageDeleted { package: deleted.package }))
}

/// A Hub timestamp in whole seconds: its nanoseconds are noise in a listing.
fn stamp(at: &str) -> String {
    match at.split_once('.') {
        Some((seconds, fraction)) => {
            let zone = fraction.find(|c: char| !c.is_ascii_digit()).map_or("", |at| &fraction[at..]);
            format!("{seconds}{zone}")
        }
        None => at.to_owned(),
    }
}

/// `items` newest first by TensorFS's one version ordering (`format::newest_by`); those whose
/// version does not parse last, as given.
fn newest_first<T>(mut items: Vec<T>, version: impl Fn(&T) -> &str) -> Vec<T> {
    let mut sorted = Vec::with_capacity(items.len());
    loop {
        let versions = items.iter().enumerate().map(|(i, item)| (i, version(item)));
        let Some((newest, _)) = tensorfs::format::newest_by(versions, |(_, v)| *v) else { break };
        sorted.push(items.remove(newest));
    }
    sorted.extend(items);
    sorted
}

/// `cozy/package_install`: the warm run that installs a package's environment on `on`, the request's `code` when it is local
/// code, which retires nothing.
pub(super) async fn install(api: &Arc<Api>, hub: &Origin, on: On, agreed: Option<Agreed>, code: Option<LocalCode>, ask: PackageInstall) -> Result<Planned, Refusal> {
    let home = api.machines.hubs.home();
    if let Some(code) = &code {
        let reader = reader(api, hub)?;
        let source = crate::local_code::source(home, code, (&reader, hub)).await.map_err(Refusal)?;
        let mut submit = crate::local_code::install_run(home, hub.as_str(), on, &source);
        submit.agreed = agreed;
        return Ok(Planned::Run { submit: Box::new(submit), after: Some(After::Keep(source.filed())), notes: vec![], mirror: None });
    }
    if api::target::written_as_path(&ask.package) {
        return Err(Refusal(crate::runs::target::by_path(&ask.package)));
    }
    let package = ask.package.trim().to_ascii_lowercase();
    if package.split('/').count() != 2 {
        return Err(Refusal(Error::new(Kind::Usage, "package_ref_invalid", format!("{package:?} is not org/name"))));
    }
    let reader = reader(api, hub)?;
    let root = home.root().to_owned();
    let chosen = match ask.release {
        Some(release) => release,
        None => api.releases.now(api, hub, &package).await.and_then(|read| read.newest(&package)).map_err(Refusal)?,
    };
    // The release's card, kept while its machine installs: the first run after an install
    // binds against it with no Hub read on its path (an RTX 4090's cold SDXL run waited 0.55 s
    // on it). A failed read is the run's to repeat.
    tokio::spawn({
        let (hub, package, release) = (hub.to_string(), package.clone(), chosen.clone());
        async move {
            let _ = hub_client::cards::card(|| Ok(reader), &root, &hub, &package, &release).await;
        }
    });
    let submit = release_install(hub.as_str(), on, &package, &chosen, agreed);
    Ok(Planned::Run { submit: Box::new(submit), after: Some(After::Retire { package }), notes: vec![], mirror: None })
}

/// The run that installs `release` of `package` on a machine: a warm one naming no function.
fn release_install(hub: &str, on: On, package: &str, release: &str, agreed: Option<api::calcifer::Agreed>) -> SubmitRun {
    SubmitRun {
        agreed,
        stays: vec![],
        on,
        hub: hub.to_owned(),
        kind: RunKind::Warm,
        target: format!("{package}@{release}"),
        spec: v1::RunSpec {
            kind: v1::RunKind::Warm,
            source: Some(Source::Release(v1::Release { package: package.to_owned(), release: release.to_owned() })),
            ..Default::default()
        },
        inputs: vec![],
        held: vec![],
        out: None,
        warnings: vec![],
    }
}

/// Every release of some packages to take off a machine: one remove run per package and Hub it holds it from.
pub(super) struct Removal {
    machine: String,
    /// Each package, and the Hub the machine holds it from.
    targets: Vec<(String, String)>,
}

/// `cozy/package_remove`: what it removes from the machine `on` names; a name that machine does not hold is refused before
/// anything is recorded.
pub(super) async fn removal(api: &Arc<Api>, hub: &Origin, on: &On, ask: PackageRemove) -> Result<Planned, Refusal> {
    let machine = super::jobs::machine(on, "a removal")?;
    let held = crate::machines::fresh(&api.machines, &machine).await.map_err(Refusal)?;
    let envs: Vec<&Environment> = held.status.iter().flat_map(|s| &s.environments).collect();
    if let Some(name) = ask.packages.iter().find(|name| !envs.iter().any(|env| &&env.package == name)) {
        return Err(Refusal(
            Error::not_found("package_not_installed", format!("{name} is not installed on {}", held.name))
                .next(format!("cozy package ls --on {}", held.name)),
        ));
    }
    let canonical = |hub: &str| Origin::parse(hub).map_or_else(|_| hub.to_owned(), String::from);
    // Local code's installations name no Hub.
    let from = |env: &Environment| if env.hub.is_empty() { hub.to_string() } else { canonical(&env.hub) };
    let mut targets: Vec<(String, String)> = envs.iter().filter(|env| ask.packages.contains(&env.package)).map(|env| (env.package.clone(), from(env))).collect();
    targets.sort();
    targets.dedup();
    let target = format!("remove {}", ask.packages.join(", "));
    Ok(Planned::Here { target, work: Work::Remove(Removal { machine: held.name, targets }) })
}

impl Removal {
    /// Sends each remove run under a key named for `key`, so asking again repeats none, and sums what they answer. One that did
    /// not complete fails the removal, saying what did go.
    pub(super) async fn run(self, api: &Arc<Api>, key: &str) -> Result<PackagesRemoved, Error> {
        #[derive(Deserialize, Default)]
        struct Removed {
            #[serde(default)]
            removed: Vec<RemovedRelease>,
            #[serde(default)]
            reclaimed_bytes: u64,
            #[serde(default)]
            retained: Vec<String>,
        }
        let runs = self.targets.iter().enumerate().map(|(i, (package, from))| {
            let submit = SubmitRun {
                agreed: None,
                stays: vec![],
                on: On::Machine(self.machine.clone()),
                hub: from.clone(),
                kind: RunKind::Remove,
                target: package.clone(),
                spec: v1::RunSpec {
                    kind: v1::RunKind::Remove,
                    source: Some(Source::Release(v1::Release { package: package.clone(), release: String::new() })),
                    ..Default::default()
                },
                inputs: vec![],
                held: vec![],
                out: None,
                warnings: vec![],
            };
            let key = format!("{key}-{i}");
            async move { settled(&api.calcifer, &key, submit).await }
        });
        let mut done = PackagesRemoved { machine: self.machine.clone(), ..Default::default() };
        for ((package, _), ran) in self.targets.iter().zip(futures_util::future::join_all(runs).await) {
            match ran {
                Ok(run) if run.status == Status::Completed => {
                    let result: Removed = run.result.and_then(|result| serde_json::from_value(result).ok()).unwrap_or_default();
                    done.reclaimed_bytes += result.reclaimed_bytes;
                    done.retained |= !result.retained.is_empty();
                    done.removed.extend(result.removed);
                }
                Ok(run) => done.failed.push(format!("{package}: {}", failure(&run))),
                Err(e) => done.failed.push(format!("{package}: {e}")),
            }
        }
        if done.failed.is_empty() {
            return Ok(done);
        }
        let went: Vec<String> = done.removed.iter().map(|gone| format!("{}@{}", gone.package, gone.release)).collect();
        let went = if went.is_empty() { String::new() } else { format!("removed {}; ", went.join(", ")) };
        let said = format!("{went}not removed: {}", done.failed.join("; "));
        Err(Error::new(Kind::Unavailable, "package_remove_incomplete", said).next(format!("cozy package ls --on {}", done.machine)))
    }
}
