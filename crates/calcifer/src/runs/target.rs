//! What a run's target names: a published release (`[org/]pkg[/fn][@release]`, `api::target`'s
//! grammar), resolved against what its machine holds and the cards this computer keeps, or local
//! code on this computer.
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use ::api::hub::cards::{self, Card};
use api::{
    calcifer::{On, ReleaseFrom, Target, TargetFunction},
    code::LocalCode,
    config::Origin,
    error::{Error, Kind},
    interface::Interface,
    target::Published,
};

use tensorfs::format::newest_by;

use crate::api::Api;

/// A target resolved.
pub(crate) enum Resolved {
    Release {
        /// `org/pkg`.
        package: String,
        function: Option<String>,
        card: Card,
        /// The release the target named, if it named one.
        release: Option<String>,
        from: ReleaseFrom,
        /// A yanked release the run moved off, in a line.
        moved: Option<String>,
    },
    Local {
        local: Box<crate::local_code::Prepared>,
        /// Its interface once kept; none while `local.install` makes it ready.
        interface: Option<Interface>,
        function: Option<String>,
    },
}

/// The local code and function a target names: a client's by content (`code`, `text` its function), or a project's (`text` a
/// path from its folder `cwd`, read into a copy as a client's is); none for a published target. A client's path is refused:
/// Calcifer reads no client's folder.
fn local(api: &Api, text: &str, code: Option<&LocalCode>, cwd: Option<&Path>) -> Option<Result<(LocalCode, Option<String>), Error>> {
    if let Some(code) = code {
        let function = Some(text.trim()).filter(|text| !text.is_empty()).map(str::to_owned);
        return Some(Ok((code.clone(), function)));
    }
    let home = api.machines.hubs.home();
    match cwd {
        Some(cwd) => crate::local_code::target(text, Some(cwd), &api.machines.user_home).map(|found| {
            let (path, function) = found?;
            Ok((crate::local_code::own(home, &path)?, function))
        }),
        None if api::target::written_as_path(text) => Some(Err(by_path(text))),
        None => None,
    }
}

/// A client's local code named by its path, which Calcifer can't read.
pub(crate) fn by_path(text: &str) -> Error {
    Error::new(Kind::Usage, "target_by_path", format!("{} is a path, and Calcifer reads no client's folder", text.trim()))
        .remedy("send local code by content: its tree's files by PUT /v1/files, and `code` naming the tree (cozy does)")
}

/// `text` resolved for a run `on` its machine: local code made ready there (or its install sent
/// under `key`), else a published release.
pub(crate) async fn resolve(
    api: &Arc<Api>,
    hub: &str,
    text: &str,
    code: Option<&LocalCode>,
    on: &On,
    warm: bool,
    given: bool,
    key: &str,
    agreed: Option<api::calcifer::Agreed>,
) -> Result<Resolved, Error> {
    if let Some(found) = local(api, text, code, None) {
        let (code, function) = found?;
        let prepared = crate::local_code::prepare(api, &code, on, hub, warm, key, agreed).await?;
        let interface = match &prepared.interface {
            Some(document) => Some(parse(document)?),
            None => None,
        };
        let function = match &interface {
            Some(interface) => chosen(interface, function),
            None => function,
        };
        return Ok(Resolved::Local { local: Box::new(prepared), interface, function });
    }
    // A package named without a function, given nothing, is only listed: its machine isn't started to say what it holds.
    let runs = given || Published::parse(text.trim()).is_ok_and(|named| named.function.is_some());
    published(api, hub, text, Some(on), runs).await
}

/// `GET /v1/targets`: the package, its release and why for a run `on` its machine, and the function,
/// checked as a run checks it; local code (a client's `code`, or a project's from its folder `cwd`) as its machine read it, else
/// from its source, installing and starting nothing.
pub(crate) async fn describe(api: &Arc<Api>, hub: &str, text: &str, code: Option<&LocalCode>, cwd: Option<&Path>, on: &On) -> Result<Target, Error> {
    if let Some(found) = local(api, text, code, cwd) {
        let (code, function) = found?;
        let (package, document) = crate::local_code::describe(api, &code, on, hub).await?;
        let interface = parse(&document)?;
        let function = match chosen(&interface, function) {
            Some(function) => Some(super::callable(&package, &interface, &function)?.name.clone()),
            None => None,
        };
        return Ok(answer(package, function, "local".into(), ReleaseFrom::Local, String::new(), document, &interface));
    }
    let Resolved::Release { package, function, card, from, .. } = published(api, hub, text, Some(on), false).await? else {
        unreachable!("a published target resolves to a release");
    };
    let function = match function {
        Some(function) => {
            let callable = super::callable(&package, &card.interface, &function)?;
            if card.interface.runs_recipes(callable) {
                return Err(super::recipes(&package, &card.interface));
            }
            Some(callable.name.clone())
        }
        None => None,
    };
    let (summary, document) = (card.summary().to_owned(), card.interface_document().clone());
    Ok(answer(package, function, card.release.clone(), from, summary, document, &card.interface))
}

/// What `GET /v1/targets` says of a package's `function`, or of the package: what a person runs of it, and a function's contract.
fn answer(
    package: String,
    function: Option<String>,
    release: String,
    release_from: ReleaseFrom,
    summary: String,
    document: serde_json::Value,
    interface: &Interface,
) -> Target {
    let public = interface.public();
    let described = |name: &str| interface.callables.iter().find(|c| c.name == name).map_or("", |c| c.description.as_str());
    let functions = public.iter().map(|name| TargetFunction { function: (*name).to_owned(), description: described(name).to_owned() }).collect();
    let published = release_from != ReleaseFrom::Local;
    let callable = function.as_deref().and_then(|name| interface.callables.iter().find(|c| c.name == name));
    let contract = callable.map(|callable| {
        let text = api::contract::contract(&format!("{package}/{}", callable.name), interface, callable);
        let text = match published {
            true => text.replacen('\n', &format!("\n  release: {release} ({release_from})\n"), 1),
            false => text,
        };
        api::names::shown(&text)
    });
    let request = callable.map(|callable| {
        let mut request = api::interface::request(&document, &callable.name);
        if published {
            request["release"] = release.clone().into();
            request["release_from"] = release_from.to_string().into();
        }
        request
    });
    let next = match function {
        Some(_) => vec![],
        None => public.iter().take(2).map(|name| format!("cozy run {package}/{name} --describe")).collect(),
    };
    Target { package, function, release, release_from, summary, interface: document, functions, contract, request, next }
}

/// The function local code's run calls: the one named, else its only public one.
fn chosen(interface: &Interface, function: Option<String>) -> Option<String> {
    function.or_else(|| only_function(interface).map(str::to_owned))
}

/// A package's one public function, when it has just one.
pub(crate) fn only_function(interface: &Interface) -> Option<&str> {
    let mut public = interface.public().into_iter();
    public.next().filter(|_| public.next().is_none())
}

fn parse(document: &serde_json::Value) -> Result<Interface, Error> {
    Interface::parse(document).map_err(|why| {
        Error::new(Kind::Unavailable, "package_interface_invalid", format!("the package interface is unreadable: {why}")).next("cozy update")
    })
}

/// A published target for a run `on` its machine (none: what this computer knows, for a read that
/// runs nothing): for `x/y`, root package x's function y when the Hub holds root package x, else
/// account x's package y (one read when it is the root's; an older Hub answers the root's absent). A
/// machine holding account x's package y and not root package x asks the Hub nothing.
pub(crate) async fn published(api: &Arc<Api>, hub: &str, text: &str, on: Option<&On>, wake: bool) -> Result<Resolved, Error> {
    let Published { package, function, release, owned } = Published::parse(text.trim())?;
    let held = match (&release, on) {
        (None, Some(on)) => Some(Held::read(api, hub, on, wake).await),
        _ => None,
    };
    let holds = |package: &str| held.as_ref().is_some_and(|held| !held.releases(package).is_empty());
    if let Some(owned) = owned.as_ref().filter(|owned| holds(owned) && !holds(&package)) {
        let (card, from, moved) = release_of(api, hub, owned, None, held.as_ref()).await?;
        return Ok(Resolved::Release { package: owned.clone(), function: None, card, release, from, moved });
    }
    let root = release_of(api, hub, &package, release.as_deref(), held.as_ref()).await;
    let (package, function, (card, from, moved)) = match (root, owned) {
        (Ok(found), _) => (package, function, found),
        (Err(e), Some(owned)) if matches!(e.kind, Kind::NotFound | Kind::Usage) => {
            match release_of(api, hub, &owned, release.as_deref(), held.as_ref()).await {
                Ok(found) => (owned, None, found),
                Err(e) if e.kind == Kind::NotFound => {
                    let first = package.trim_start_matches("_/");
                    return Err(Error::new(
                        Kind::NotFound,
                        "package_not_found",
                        format!("the Hub has neither a package {first} nor {owned}"),
                    )
                    .next(format!("cozy package ls {first}")));
                }
                Err(e) => return Err(e),
            }
        }
        (Err(e), _) => return Err(e),
    };
    Ok(Resolved::Release { package, function, card, release, from, moved })
}

/// What the machine a run goes to holds.
struct Held {
    machine: String,
    environments: Vec<api::machine::Environment>,
    hub: String,
}

impl Held {
    /// The machine `on` names, or the rental `any` would take now, as it answers now (`wake`: `local` starts for it);
    /// nothing for a machine still to come.
    async fn read(api: &Arc<Api>, hub: &str, on: &On, wake: bool) -> Self {
        let (machine, environments) = api.machines.holds(on, wake).await.unwrap_or_default();
        Self { machine, environments, hub: canonical(hub) }
    }

    /// The releases of `package` from this run's Hub.
    fn releases(&self, package: &str) -> Vec<&str> {
        let here = |env: &&api::machine::Environment| env.package == package && !env.release.is_empty() && (env.hub.is_empty() || canonical(&env.hub) == self.hub);
        self.environments.iter().filter(here).map(|env| env.release.as_str()).collect()
    }
}

/// Which release a run binds against and runs, once, before binding, and why, with a line when it
/// moved off a yanked one: the one asked; else the newest its machine holds that the Hub still has and
/// hasn't yanked; else the Hub's newest, which the machine installs. So inputs bind against what will run, the run is
/// pinned to it, and a release moves under a machine only by `cozy package install`. With no machine
/// (`held`), the newest as this computer last read it.
async fn release_of(
    api: &Arc<Api>,
    hub: &str,
    package: &str,
    release: Option<&str>,
    held: Option<&Held>,
) -> Result<(Card, ReleaseFrom, Option<String>), Error> {
    let hubs = &api.machines.hubs;
    let root = hubs.home().root();
    let client = || hubs.reader(hub);
    let (release, from, moved) = match (release, held) {
        (Some(release), _) => (release.to_owned(), ReleaseFrom::Asked, None),
        (None, Some(held)) => unversioned(api, hub, package, held).await?,
        (None, None) => (api.releases.kept(api, hub, package).await?.newest(package)?, ReleaseFrom::Latest, None),
    };
    let card = match (cards::card(client, root, hub, package, &release).await, release.strip_prefix('v')) {
        // `v0.2.0` for the release named `0.2.0`.
        (Err(e), Some(bare)) if e.kind == Kind::NotFound => cards::card(client, root, hub, package, bare).await?,
        (card, _) => card?,
    };
    Ok((card, from, moved))
}

/// An unversioned run's release on the machine that holds `held`. Only a machine holding none reads
/// the Hub for it; one holding only yanked ones moves to the newest and says so, and stays on what
/// it holds when the Hub names nothing else.
async fn unversioned(api: &Arc<Api>, hub: &str, package: &str, held: &Held) -> Result<(String, ReleaseFrom, Option<String>), Error> {
    let holds = held.releases(package);
    let Some(top) = newest_by(holds.iter().copied(), |release| release).map(str::to_owned) else {
        return Ok((api.releases.now(api, hub, package).await?.newest(package)?, ReleaseFrom::Latest, None));
    };
    let yanked = match api.releases.kept(api, hub, package).await {
        Ok(read) => read,
        // Unknown: what it holds runs.
        Err(_) => return Ok((top, ReleaseFrom::Installed, None)),
    };
    // A release the read doesn't name was published since it, or the Hub no longer has it: which, is read again, not on every run.
    let unnamed = holds.iter().any(|release| !yanked.released.contains(*release));
    let yanked = match unnamed && api.releases.read_before(hub, package, RECHECK) {
        true => api.releases.now(api, hub, package).await.unwrap_or(yanked),
        false => yanked,
    };
    // An old installation of a release the Hub no longer has can't be chosen: nothing can install or describe it.
    let holds: Vec<&str> = holds.into_iter().filter(|release| yanked.released.contains(*release)).collect();
    let Some(top) = newest_by(holds.iter().copied(), |release| release).map(str::to_owned) else {
        return Ok((yanked.newest(package)?, ReleaseFrom::Latest, None));
    };
    let shown = package.trim_start_matches("_/");
    let machine = &held.machine;
    if let Some(open) = newest_by(holds.iter().copied().filter(|release| !yanked.yanked.contains(*release)), |release| release) {
        let moved = (open != top).then(|| format!("{shown}@{top} on {machine} is yanked: this run uses {open}, also installed there"));
        return Ok((open.to_owned(), ReleaseFrom::Installed, moved));
    }
    Ok(match api.releases.now(api, hub, package).await.and_then(|read| read.newest(package)) {
        Ok(newest) => {
            let moved = format!("{shown}@{top} on {machine} is yanked: this run moves it to {newest}, the newest");
            (newest, ReleaseFrom::Latest, Some(moved))
        }
        Err(why) => {
            let moved = format!("{shown}@{top} on {machine} is yanked and nothing else can be read ({}): this run uses it", why.message);
            (top, ReleaseFrom::Installed, Some(moved))
        }
    })
}

/// A Hub's one spelling.
fn canonical(hub: &str) -> String {
    Origin::parse(hub).map_or_else(|_| hub.to_owned(), String::from)
}

/// What this computer last read of each package's releases at its Hub, in memory. A run holding a
/// release asks only whether it was yanked: read at most every [`RECENT`] and only for a package in
/// use, behind the run, so a run makes no Hub read for it, a computer at rest makes none at all, and
/// a yank made elsewhere reaches the next run after that read.
#[derive(Default)]
pub(crate) struct Releases(Mutex<HashMap<(String, String), (Instant, Arc<cards::Releases>)>>);

/// A read this old is read again, behind the run that found it.
const RECENT: Duration = Duration::from_secs(600);
/// How often a release its machine holds and the read doesn't name sends a run to the Hub.
const RECHECK: Duration = Duration::from_secs(60);
/// How long a run waits on a package's first read: it holds up a run, so a Hub slower than this
/// leaves the run with what its machine holds.
const LATE: Duration = Duration::from_secs(5);
/// How long a Hub may take over a package's releases, for whoever has nothing else to go on. A Hub working on an answer sends nothing, so there is
/// no progress to measure, and one that takes the request and never answers would hold an install with nothing in any log.
const UNANSWERED: Duration = Duration::from_secs(30);

impl Releases {
    /// `package`'s releases at its Hub now, and kept.
    pub(crate) async fn now(&self, api: &Api, hub: &str, package: &str) -> Result<Arc<cards::Releases>, Error> {
        let reader = api.machines.hubs.reader(hub)?;
        let read = match tokio::time::timeout(UNANSWERED, cards::releases(&reader, package)).await {
            Ok(read) => Arc::new(read?),
            Err(_) => {
                let said = format!("{hub} gave no answer about {package} in {}s", UNANSWERED.as_secs());
                tracing::warn!("{said}");
                return Err(Error::new(Kind::Unavailable, "hub_slow", said).remedy("it's busy or down: asking again is safe"));
            }
        };
        let mut kept = self.0.lock().expect("releases lock");
        kept.insert((canonical(hub), package.to_owned()), (Instant::now(), read.clone()));
        Ok(read)
    }

    /// As last read, then read again behind the caller if that was long ago; never read, read now.
    async fn kept(&self, api: &Arc<Api>, hub: &str, package: &str) -> Result<Arc<cards::Releases>, Error> {
        let key = (canonical(hub), package.to_owned());
        let held = {
            let mut kept = self.0.lock().expect("releases lock");
            kept.get_mut(&key).map(|(at, read)| {
                let stale = at.elapsed() >= RECENT;
                if stale {
                    // One run reads it again; the rest go on.
                    *at = Instant::now();
                }
                (stale, read.clone())
            })
        };
        match held {
            Some((stale, read)) => {
                if stale {
                    let api = api.clone();
                    let (hub, package) = (hub.to_owned(), package.to_owned());
                    tokio::spawn(async move {
                        if let Err(e) = api.releases.now(&api, &hub, &package).await {
                            tracing::debug!("{package}'s releases: {e}");
                        }
                    });
                }
                Ok(read)
            }
            None => match tokio::time::timeout(LATE, self.now(api, hub, package)).await {
                Ok(read) => read,
                Err(_) => Err(Error::new(Kind::Unavailable, "hub_slow", format!("{package}'s releases: no answer in {}s", LATE.as_secs())).remedy("run it again, or name a release: <package>@<release>")),
            },
        }
    }

    /// Whether `package`'s releases were last read longer ago than `age`, or never.
    fn read_before(&self, hub: &str, package: &str, age: Duration) -> bool {
        let kept = self.0.lock().expect("releases lock");
        kept.get(&(canonical(hub), package.to_owned())).is_none_or(|(at, _)| at.elapsed() >= age)
    }

    /// Yanked, un-yanked or published here: the next run reads it, not one read later.
    pub(crate) fn forget(&self, hub: &str, package: &str) {
        self.0.lock().expect("releases lock").remove(&(canonical(hub), package.to_owned()));
    }
}
