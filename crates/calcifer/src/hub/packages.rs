//! `/v1/packages`: the Hub's package catalog as this computer's account sees it (anonymous when
//! signed out), and a package's yank and deletion by its owner.
use std::{path::Path as FsPath, sync::Arc};

use axum::{
    Json,
    body::Bytes,
    extract::{
        Path, Query, State,
        rejection::{BytesRejection, QueryRejection},
    },
    http::StatusCode,
};
use protocol::{
    calcifer::{Environment, Kind as RunKind, On, SubmitRun},
    config::Origin,
    error::{Error, Kind},
    hub::{
        At, FoundPackage, PackageDeleted, PackageDetail, PackageInstall, PackageRelease, PackageRemoval, PackageRemovals,
        PackageSearch, PackageUnyanked, PackageYanked,
    },
    machine::{self as v1, Source},
    names::shown,
};
use serde::Deserialize;

use super::{
    after::After,
    asks::{Ask, Prepared},
    body, origin, reader, refusal, signed_in,
};
use crate::api::{Api, IdempotencyKey, Refusal, parse};

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

impl Ask for PackageInstall {
    async fn prepare(self, api: &Arc<Api>) -> Result<Prepared, Refusal> {
        let hub = origin(api, self.hub.as_deref())?;
        let home = api.machines.hubs.home();
        if FsPath::new(&self.package).is_absolute() {
            // Local code: installed live, read as it is at each run; it retires nothing.
            let client = signed_in(api, &hub).ok();
            let source = crate::local_code::source(home, FsPath::new(&self.package), &self.on, client.as_ref().map(|c| (c, &hub)))
                .await
                .map_err(Refusal)?;
            let mut submit = crate::local_code::install_run(home, hub.as_str(), self.on, &source);
            submit.agreed = self.agreed;
            return Ok(Prepared { submit, after: None });
        }
        let package = self.package.trim().to_ascii_lowercase();
        if package.split('/').count() != 2 {
            return Err(Refusal(Error::new(Kind::Usage, "package_ref_invalid", format!("{package:?} is not org/name"))));
        }
        let reader = reader(api, &hub)?;
        let root = home.root().to_owned();
        let chosen = match self.release {
            Some(release) => release,
            None => hub_client::cards::fresh(&reader, &root, &hub, &package).await.map_err(Refusal)?,
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
        let submit = SubmitRun {
            agreed: self.agreed,
            on: self.on,
            hub: hub.to_string(),
            kind: RunKind::Warm,
            target: format!("{package}@{chosen}"),
            spec: v1::RunSpec {
                kind: v1::RunKind::Warm,
                source: Some(Source::Release(v1::Release { package: package.clone(), release: chosen })),
                ..Default::default()
            },
            inputs: vec![],
            out: None,
            warnings: vec![],
        };
        Ok(Prepared { submit, after: Some(After::Retire { package }) })
    }
}

/// `POST /v1/package-removals`: one remove run per package and Hub the machine holds it from,
/// every release of it; a name the machine does not hold is refused before any run.
pub(super) async fn remove(
    State(api): State<Arc<Api>>,
    IdempotencyKey(key): IdempotencyKey,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<PackageRemovals>), Refusal> {
    let PackageRemoval { hub, packages, machine } = parse(&body?)?;
    let hub = origin(&api, hub.as_deref())?;
    let held = crate::machines::fresh(&api.machines, &machine).await.map_err(Refusal)?;
    let envs: Vec<&Environment> = held.status.iter().flat_map(|s| &s.environments).collect();
    if let Some(name) = packages.iter().find(|name| !envs.iter().any(|env| &&env.package == name)) {
        return Err(Refusal(
            Error::not_found("package_not_installed", format!("{name} is not installed on {}", held.name))
                .next(format!("cozy package ls --on {}", held.name)),
        ));
    }
    let canonical = |hub: &str| Origin::parse(hub).map_or_else(|_| hub.to_owned(), String::from);
    let mut targets: Vec<(&str, String)> = (envs.iter())
        .filter(|env| packages.contains(&env.package))
        .map(|env| (env.package.as_str(), canonical(&env.hub)))
        .collect();
    targets.sort();
    targets.dedup();
    let (mut runs, mut replay) = (vec![], true);
    for (i, (package, from)) in targets.into_iter().enumerate() {
        // Local code's installations name no Hub.
        let from = if from.is_empty() { hub.to_string() } else { from };
        let submit = SubmitRun {
            agreed: None,
            on: On::Machine(held.name.clone()),
            hub: from,
            kind: RunKind::Remove,
            target: package.to_owned(),
            spec: v1::RunSpec {
                kind: v1::RunKind::Remove,
                source: Some(Source::Release(v1::Release { package: package.to_owned(), release: String::new() })),
                ..Default::default()
            },
            inputs: vec![],
            out: None,
            warnings: vec![],
        };
        let submitted = api.calcifer.submit(&format!("{key}-{i}"), submit).await?;
        replay &= submitted.replay;
        runs.push(submitted.run);
    }
    let status = if replay && !runs.is_empty() { StatusCode::OK } else { StatusCode::ACCEPTED };
    Ok((status, Json(PackageRemovals { machine: held.name, runs })))
}
