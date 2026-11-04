//! What a run's target names: a published release (`[org/]pkg[/fn][@release]`, `protocol::target`'s
//! grammar), resolved against what its machine holds and the cards this computer keeps, or local
//! code on this computer.
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hub_client::cards::{self, Card};
use protocol::{
    calcifer::{On, ReleaseFrom, Target},
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

/// `text` resolved for a run `on` its machine: local code made ready there (or its install sent
/// under `key`), else a published release.
pub(crate) async fn resolve(
    api: &Arc<Api>,
    hub: &str,
    text: &str,
    cwd: Option<&Path>,
    on: &On,
    warm: bool,
    key: &str,
    agreed: Option<protocol::calcifer::Agreed>,
) -> Result<Resolved, Error> {
    let user_home = &api.machines.user_home;
    if let Some(local) = crate::local_code::target(text, cwd.unwrap_or(Path::new("")), user_home) {
        let (path, function) = local?;
        let prepared = crate::local_code::prepare(api, &path, on, hub, warm, key, agreed).await?;
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
    published(api, hub, text, Some(on)).await
}

/// `GET /v1/targets`: the package, its release and why for a run `on` its machine, and the function,
/// checked as a run checks it; local code is read from its source, installing nothing.
pub(crate) async fn describe(api: &Arc<Api>, hub: &str, text: &str, cwd: Option<&Path>, on: &On) -> Result<Target, Error> {
    let user_home = &api.machines.user_home;
    if let Some(local) = crate::local_code::target(text, cwd.unwrap_or(Path::new("")), user_home) {
        let (path, function) = local?;
        let (package, document) = crate::local_code::describe(api.machines.hubs.home(), &path).await?;
        let interface = parse(&document)?;
        let function = match chosen(&interface, function) {
            Some(function) => Some(super::callable(&package, &interface, &function)?.name.clone()),
            None => None,
        };
        return Ok(Target {
            package,
            function,
            release: "local".into(),
            release_from: ReleaseFrom::Local,
            summary: String::new(),
            interface: document,
        });
    }
    let Resolved::Release { package, function, card, from, .. } = published(api, hub, text, Some(on)).await? else {
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
    Ok(Target {
        function,
        release: card.release.clone(),
        release_from: from,
        summary: card.summary().to_owned(),
        interface: card.interface_document().clone(),
        package,
    })
}

/// The function local code's run calls: the one named, else its only public one.
fn chosen(interface: &Interface, function: Option<String>) -> Option<String> {
    function.or_else(|| only_function(interface).map(str::to_owned))
}

/// A package's one public function, when it has just one (a project adapter is `cozy project`'s).
pub(crate) fn only_function(interface: &Interface) -> Option<&str> {
    let mut public = interface.public().into_iter().filter(|name| !name.starts_with("project-adapter-"));
    public.next().filter(|_| public.next().is_none())
}

fn parse(document: &serde_json::Value) -> Result<Interface, Error> {
    Interface::parse(document).map_err(|why| {
        Error::new(Kind::Unavailable, "package_interface_invalid", format!("the package interface is unreadable: {why}"))
    })
}

/// A published target for a run `on` its machine (none: what this computer knows, for a read that
/// runs nothing): for `x/y`, root package x's function y when the Hub holds root package x, else
/// account x's package y (one read when it is the root's; an older Hub answers the root's absent). A
/// machine holding account x's package y and not root package x asks the Hub nothing.
pub(crate) async fn published(api: &Arc<Api>, hub: &str, text: &str, on: Option<&On>) -> Result<Resolved, Error> {
    let Published { package, function, release, owned } = Published::parse(text.trim())?;
    let held = match (&release, on) {
        (None, Some(on)) => Some(Held::read(api, hub, on).await),
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
    environments: Vec<protocol::machine::Environment>,
    hub: String,
}

impl Held {
    /// The machine `on` names, or the rental `any` would take now, as it answers now; nothing for
    /// a machine still to come.
    async fn read(api: &Arc<Api>, hub: &str, on: &On) -> Self {
        let (machine, environments) = api.machines.holds(on).await.unwrap_or_default();
        Self { machine, environments, hub: canonical(hub) }
    }

    /// The releases of `package` from this run's Hub.
    fn releases(&self, package: &str) -> Vec<&str> {
        let here = |env: &&protocol::machine::Environment| env.package == package && !env.release.is_empty() && (env.hub.is_empty() || canonical(&env.hub) == self.hub);
        self.environments.iter().filter(here).map(|env| env.release.as_str()).collect()
    }
}

/// Which release a run binds against and runs, once, before binding, and why, with a line when it
/// moved off a yanked one: the one asked; else the newest its machine holds that isn't yanked; else
/// the Hub's newest, which the machine installs. So inputs bind against what will run, the run is
/// pinned to it, and a release moves under a machine only by `cozy package update`. With no machine
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
/// How long a run waits on a package's first read: it holds up a run, so a Hub slower than this
/// leaves the run with what its machine holds.
const LATE: Duration = Duration::from_secs(5);

impl Releases {
    /// `package`'s releases at its Hub now, and kept.
    pub(crate) async fn now(&self, api: &Api, hub: &str, package: &str) -> Result<Arc<cards::Releases>, Error> {
        let read = Arc::new(cards::releases(&api.machines.hubs.reader(hub)?, package).await?);
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
                Err(_) => Err(Error::new(Kind::Unavailable, "hub_slow", format!("{package}'s releases: no answer in {}s", LATE.as_secs()))),
            },
        }
    }

    /// Yanked, un-yanked or published here: the next run reads it, not one read later.
    pub(crate) fn forget(&self, hub: &str, package: &str) {
        self.0.lock().expect("releases lock").remove(&(canonical(hub), package.to_owned()));
    }
}
