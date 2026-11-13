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
    calcifer::{Note, NoteLevel, On, ReleaseFrom, Target, TargetFunction},
    code::LocalCode,
    config::Origin,
    error::{Error, Kind},
    interface::Interface,
    target::Published,
};

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
        /// What the run moved to and from, in a line: off a yanked release (a warning), onto one newer than its machine's, or
        /// an older one than the newest its machine's Runtime doesn't run (said).
        moved: Option<Note>,
        /// Its machine's Runtime moves first, to one that runs it (cl-952): started by whatever sends the run.
        upgrade: Option<crate::floor::Upgrade>,
        /// cozy updates itself first, to a release whose line runs it (cl-978): started by whatever sends the run.
        cozy: Option<crate::cozy::Wanted>,
    },
    Local {
        local: Box<crate::local_code::Prepared>,
        /// Its interface once kept; none while `local.install` makes it ready.
        interface: Option<Interface>,
        function: Option<String>,
    },
}

/// The local code and function a target names: a client's by content (`code`, `text` its function), a project's (`text` a
/// path from its folder `cwd`, read into a copy as a client's is), or a first-party package from the dev home's checkout; none
/// for a published target. A client's path is refused: Calcifer reads no client's folder.
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
        // A first-party package the dev home's checkout holds is its source there (xs-195).
        None => crate::checkout::named(text).map(|(path, function)| Ok((crate::local_code::own(home, &path)?, function))),
    }
}

/// A client's local code named by its path, which Calcifer can't read. API clients upload
/// the tree's files with `PUT /v1/files` and send its `code` reference (see `api::code`).
pub(crate) fn by_path(text: &str) -> Error {
    Error::new(Kind::Usage, "target_by_path", format!("Local code was not uploaded: {}", text.trim()))
        .remedy("Run local code with cozy run ./path, which uploads it automatically.")
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
    let scope = match cwd { Some(path) => crate::private_io::of_path(path)?, None => crate::private_io::current() };
    crate::private_io::context(scope, describe_scoped(api, hub, text, code, cwd, on)).await
}

async fn describe_scoped(api: &Arc<Api>, hub: &str, text: &str, code: Option<&LocalCode>, cwd: Option<&Path>, on: &On) -> Result<Target, Error> {
    if let Some(found) = local(api, text, code, cwd) {
        let (code, function) = found?;
        let (package, document) = crate::local_code::describe(api, &code, on, hub).await?;
        let interface = parse(&document)?;
        let function = match chosen(&interface, function) {
            Some(function) => Some(super::callable(&package, &interface, &function)?.name.clone()),
            None => None,
        };
        let (release, from) = match crate::checkout::of(&code) {
            Some(from) => (from.said(), ReleaseFrom::Source),
            None => ("local".into(), ReleaseFrom::Local),
        };
        return Ok(answer(package, function, release, from, String::new(), document, &interface));
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
    let published = !release_from.local();
    let callable = function.as_deref().and_then(|name| interface.callables.iter().find(|c| c.name == name));
    let contract = callable.map(|callable| {
        let text = api::contract::contract(&format!("{package}/{}", callable.name), interface, callable);
        let said = match release_from {
            ReleaseFrom::Local => None,
            ReleaseFrom::Source => Some(format!("from source: {release}")),
            _ => Some(format!("release: {release} ({release_from})")),
        };
        let text = match said {
            Some(said) => text.replacen('\n', &format!("\n  {said}\n"), 1),
            None => text,
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
    let held = match on {
        Some(on) => Some(Held::read(api, hub, on, wake).await),
        None => None,
    };
    let holds = |package: &str| held.as_ref().is_some_and(|held| !held.releases(package).is_empty());
    if let Some(owned) = owned.as_ref().filter(|owned| release.is_none() && holds(owned) && !holds(&package)) {
        let Found { card, from, moved, upgrade, cozy } = release_of(api, hub, owned, None, held.as_ref()).await?;
        return Ok(Resolved::Release { package: owned.clone(), function: None, card, release, from, moved, upgrade, cozy });
    }
    let root = release_of(api, hub, &package, release.as_deref(), held.as_ref()).await;
    let (package, function, Found { card, from, moved, upgrade, cozy }) = match (root, owned) {
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
    Ok(Resolved::Release { package, function, card, release, from, moved, upgrade, cozy })
}

/// What the machine a run goes to holds and runs.
struct Held {
    machine: String,
    environments: Vec<api::machine::Environment>,
    hub: String,
    /// It runs, or moves to before this run, the open RC's Runtime (cl-922): it takes packages built for it.
    rc: bool,
    /// Its Runtime; empty for a machine still to come.
    runtime: String,
    /// The Runtimes it can move to for a release that needs one, and whether it does (cl-952).
    runtimes: Vec<String>,
    moves: api::release::Moves,
    /// The newer cozy whose line runs Runtimes this one's doesn't (cl-978).
    newer: Option<api::release::Newer>,
}

impl Held {
    /// The machine `on` names, or the rental `any` would take now, as it answers now (`wake`: `local` starts for it);
    /// nothing for a machine still to come.
    async fn read(api: &Arc<Api>, hub: &str, on: &On, wake: bool) -> Self {
        let (machine, environments) = api.machines.holds(on, wake).await.unwrap_or_default();
        let environments = match crate::private_io::current().filter(|_| machine == "local") {
            Some(scope) => api.machines.project_holds(scope).await,
            None => environments,
        };
        let rc = api.machines.takes_rc(&machine, on);
        let (runtime, runtimes, moves, newer) = match api.machines.frame(&machine).filter(|frame| !frame.runtime.is_empty()) {
            Some(frame) => {
                let (runtimes, moves, newer) = api.machines.runtimes(&machine, &frame, rc).await;
                (frame.runtime, runtimes, moves, newer)
            }
            None => (String::new(), vec![], api::release::Moves::No, None),
        };
        Self { machine, environments, hub: canonical(hub), rc, runtime, runtimes, moves, newer }
    }

    /// As `choose` reads it.
    fn machine(&self) -> api::release::Machine<'_> {
        api::release::Machine { runtime: &self.runtime, runtimes: &self.runtimes, moves: self.moves, newer: self.newer.as_ref() }
    }

    /// Of `releases`, the newest this machine runs (`api::release::newest`: prereleases only on the RC).
    fn newest<'a>(&self, releases: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
        api::release::newest(releases, self.rc)
    }

    /// The Hub's releases of `read` this machine takes, newest first, never a yanked one: prereleases too on the RC's Runtime,
    /// elsewhere finals, and with none, a refusal that says how to run the prerelease (cl-952).
    fn candidates<'a>(&self, read: &'a cards::Releases, package: &str) -> Result<Vec<&'a str>, Error> {
        let open = read.released.iter().map(String::as_str).filter(|release| !read.yanked.contains(*release));
        let candidates = api::release::candidates(open, self.rc);
        if !candidates.is_empty() {
            return Ok(candidates);
        }
        let latest = read.newest(package)?;
        let machine = if self.machine.is_empty() { "<machine>" } else { self.machine.as_str() };
        Err(Error::new(
            Kind::Conflict,
            "package_rc_only",
            format!("{package} has only {latest}, built for the open RC's Runtime, which {machine} doesn't run"),
        )
        .next(format!("cozy machine update {machine}")))
    }

    /// The releases of `package` from this run's Hub.
    fn releases(&self, package: &str) -> Vec<&str> {
        let here = |env: &&api::machine::Environment| env.package == package && !env.release.is_empty() && (env.hub.is_empty() || canonical(&env.hub) == self.hub);
        self.environments.iter().filter(here).map(|env| env.release.as_str()).collect()
    }
}

/// The release a run binds against and runs, why, a line when it moved, and its machine's Runtime move when it needs one.
struct Found {
    card: Card,
    from: ReleaseFrom,
    moved: Option<Note>,
    upgrade: Option<crate::floor::Upgrade>,
    cozy: Option<crate::cozy::Wanted>,
}

/// Which release a run binds against and runs, once, before binding: the one asked, else the Hub's newest its machine takes
/// (`unversioned`), which it installs if it holds an older one; on a Runtime that runs it (`api::release::choose`, cl-952). So
/// inputs bind against what will run and the run is pinned to it. With no machine (`held`), as this computer last read it.
async fn release_of(api: &Arc<Api>, hub: &str, package: &str, release: Option<&str>, held: Option<&Held>) -> Result<Found, Error> {
    let shown = package.trim_start_matches("_/");
    let read = std::sync::Mutex::new(HashMap::<String, Card>::new());
    let requirement = |release: String| {
        let read = &read;
        async move {
            let card = card_of(api, hub, package, &release).await?;
            let needs = api::release::required(card.requirements().unwrap_or_default(), "cozy-runtime");
            read.lock().expect("cards lock").insert(release, card);
            Ok::<_, Error>(needs)
        }
    };
    let (asked, from, said) = match (release, held) {
        (Some(release), Some(_)) => (vec![release.to_owned()], ReleaseFrom::Asked, None),
        (Some(release), None) => return Ok(Found { card: card_of(api, hub, package, release).await?, from: ReleaseFrom::Asked, moved: None, upgrade: None, cozy: None }),
        (None, Some(held)) => match unversioned(api, hub, package, held).await? {
            Ok(installed) => return Ok(Found { card: card_of(api, hub, package, &installed).await?, from: ReleaseFrom::Installed, moved: None, upgrade: None, cozy: None }),
            Err(Unversioned { candidates, said }) => (candidates, ReleaseFrom::Latest, said),
        },
        (None, None) => {
            let newest = api.releases.kept(api, hub, package).await?.newest(package)?;
            return Ok(Found { card: card_of(api, hub, package, &newest).await?, from: ReleaseFrom::Latest, moved: None, upgrade: None, cozy: None });
        }
    };
    let held = held.expect("a machine's run");
    let candidates: Vec<&str> = asked.iter().map(String::as_str).collect();
    let machine = &held.machine;
    let chosen = api::release::choose(&candidates, &held.machine(), requirement).await.map_err(|refused| match refused {
        api::release::Refused::Held { release, needs, .. } => Error::new(
            Kind::Conflict,
            "runtime_held",
            format!("{shown} {release} needs cozy-runtime{needs}, and {machine} is held on Runtime {} by hand: `cozy machine update {machine}` lets it move", held.runtime),
        )
        .next(format!("cozy machine update {machine}")),
        api::release::Refused::Unmet { release, needs } => Error::new(
            Kind::Conflict,
            "runtime_unmet",
            format!("{shown} {release} needs cozy-runtime{needs}: {machine} runs {}, and no Runtime it can take meets it", held.runtime),
        )
        .remedy(format!("name a release it runs: {shown}@<release>")),
        api::release::Refused::Major { release, cozy, .. } => crate::cozy::major_refusal(&format!("{shown} {release}"), &cozy),
        api::release::Refused::Unread(e) => e,
    })?;
    let mut cozy = None;
    let (release, moved, upgrade) = match chosen {
        api::release::Chosen::Runs(release) => (release, said, None),
        api::release::Chosen::Cozy { release, cozy: to, .. } => {
            cozy = Some(crate::cozy::Wanted { to, needed_by: format!("{shown} {release}") });
            (release, said, None)
        }
        api::release::Chosen::Older { release, newest, needs } => {
            let why = match held.moves {
                api::release::Moves::No => "it isn't this computer's to move",
                _ => "no Runtime it can take does",
            };
            let message = format!("{shown} {newest} needs cozy-runtime{needs}, which {machine}'s {} doesn't meet, and {why}: this run takes {release}", held.runtime);
            (release, Some(Note { level: NoteLevel::Info, message }), None)
        }
        api::release::Chosen::Moves { release, runtime } => {
            let upgrade = crate::floor::Upgrade { machine: machine.clone(), from: held.runtime.clone(), to: runtime, release: format!("{shown} {release}") };
            (release, said, Some(upgrade))
        }
    };
    let card = read.lock().expect("cards lock").remove(&release).expect("choose read the card it chose");
    let from = if from == ReleaseFrom::Latest && held.releases(package).contains(&release.as_str()) { ReleaseFrom::Installed } else { from };
    Ok(Found { card, from, moved, upgrade, cozy })
}

/// What an install of `package` `on` its machine takes, as a run's release is chosen (`release_of`): the one asked, else the Hub's
/// newest its machine runs; the line it says when it moved; its machine's Runtime move first (cl-952), or cozy's own update
/// (cl-978).
pub(crate) async fn installed_release(api: &Arc<Api>, hub: &str, package: &str, release: Option<&str>, on: &On) -> Result<(String, Option<Note>, Option<crate::floor::Upgrade>, Option<crate::cozy::Wanted>), Error> {
    let held = Held::read(api, hub, on, true).await;
    let Found { card, moved, upgrade, cozy, .. } = release_of(api, hub, package, release, Some(&held)).await?;
    Ok((card.release, moved, upgrade, cozy))
}

/// The card of `release`, with its requirements: `v0.2.0` for the release named `0.2.0` too.
async fn card_of(api: &Arc<Api>, hub: &str, package: &str, release: &str) -> Result<Card, Error> {
    let hubs = &api.machines.hubs;
    let root = hubs.home().root();
    let client = || hubs.reader(hub);
    match (cards::card_requiring(client, root, hub, package, release).await, release.strip_prefix('v')) {
        (Err(e), Some(bare)) if e.kind == Kind::NotFound => cards::card_requiring(client, root, hub, package, bare).await,
        (card, _) => card,
    }
}

/// An unversioned run's releases to choose from, newest first, and the line it says when it moves off what its machine holds.
struct Unversioned {
    candidates: Vec<String>,
    said: Option<Note>,
}

/// An unversioned run's releases on the machine that holds `held`: the Hub's its machine takes (finals off the RC,
/// prereleases on it, never a yanked one), read at most a minute ago, for `choose` to pick from; installed there if it holds an
/// older one, and the run says so ("minimax-h3 0.12.0rc2, newer than rabou's 0.11.1"). Unversioned means newest, so a fix
/// reaches machines that already hold a release (se-128, cl-952). With the Hub unanswering, what the machine holds runs (`Ok`).
async fn unversioned(api: &Arc<Api>, hub: &str, package: &str, held: &Held) -> Result<Result<String, Unversioned>, Error> {
    let holds = held.releases(package);
    let read = match api.releases.fresh(api, hub, package).await {
        Ok(read) => read,
        Err(why) => {
            return match held.newest(holds.iter().copied()) {
                Some(top) => Ok(Ok(top.to_owned())),
                None => Err(why),
            };
        }
    };
    let candidates: Vec<String> = held.candidates(&read, package)?.into_iter().map(str::to_owned).collect();
    let newest = candidates.first().cloned().unwrap_or_default();
    let shown = package.trim_start_matches("_/");
    let machine = &held.machine;
    // What it holds that the Hub still has: an older one is said; a yanked one is said as moved off.
    let had = held.newest(holds.iter().copied().filter(|release| read.released.contains(*release) && *release != newest));
    let said = had.filter(|_| !holds.contains(&newest.as_str())).map(|had| match read.yanked.contains(had) {
        true => Note { level: NoteLevel::Warning, message: format!("{shown}@{had} on {machine} is yanked: this run moves it to {newest}, the newest") },
        false => Note { level: NoteLevel::Info, message: format!("{shown} {newest}, newer than {machine}'s {had}") },
    });
    Ok(Err(Unversioned { candidates, said }))
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
/// How old a read of a package's releases an unversioned run takes: newer releases reach it within this.
const FRESH: Duration = Duration::from_secs(60);
/// How long a run waits on a package's first read: it holds up a run, so a Hub slower than this
/// leaves the run with what its machine holds.
const LATE: Duration = Duration::from_secs(5);

impl Releases {
    /// `package`'s releases at its Hub now, and kept.
    pub(crate) async fn now(&self, api: &Api, hub: &str, package: &str) -> Result<Arc<cards::Releases>, Error> {
        let reader = api.machines.hubs.reader(hub)?;
        // A Hub that takes the request and never answers is the client's to end (`hub_slow`, after 30 s of nothing).
        let read = Arc::new(cards::releases(&reader, package).await.inspect_err(|e| tracing::warn!("{package}'s releases at {hub}: {e}"))?);
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

    /// As read at most [`FRESH`] ago; else read now, and if the Hub doesn't answer in [`LATE`], as last read.
    async fn fresh(&self, api: &Arc<Api>, hub: &str, package: &str) -> Result<Arc<cards::Releases>, Error> {
        let key = (canonical(hub), package.to_owned());
        let kept = self.0.lock().expect("releases lock").get(&key).map(|(at, read)| (at.elapsed() < FRESH, read.clone()));
        if let Some((true, read)) = &kept {
            return Ok(read.clone());
        }
        match (tokio::time::timeout(LATE, self.now(api, hub, package)).await, kept) {
            (Ok(Ok(read)), _) => Ok(read),
            (_, Some((_, read))) => Ok(read),
            (Ok(Err(e)), None) => Err(e),
            (Err(_), None) => Err(Error::new(Kind::Unavailable, "hub_slow", format!("{package}'s releases: no answer in {}s", LATE.as_secs()))
                .remedy("run it again, or name a release: <package>@<release>")),
        }
    }

    /// Yanked, un-yanked or published here: the next run reads it, not one read later.
    pub(crate) fn forget(&self, hub: &str, package: &str) {
        self.0.lock().expect("releases lock").remove(&(canonical(hub), package.to_owned()));
    }
}
