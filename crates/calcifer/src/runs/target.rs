//! What a run's target names: a published release (`[org/]pkg[/fn][@release]`, `protocol::target`'s
//! grammar), resolved on its Hub with the cards and newest releases this computer keeps, or local
//! code on this computer.
use std::{path::Path, sync::Arc};

use hub_client::cards::{self, Card};
use protocol::{
    calcifer::{On, ReleaseFrom, Target},
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
    published(api, hub, text).await
}

/// `GET /v1/targets`: the package, its release and why, and the function, checked as a run checks
/// it; local code is read from its source, installing nothing.
pub(crate) async fn describe(api: &Arc<Api>, hub: &str, text: &str, cwd: Option<&Path>) -> Result<Target, Error> {
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
    let Resolved::Release { package, function, card, from, .. } = published(api, hub, text).await? else {
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
    let public: Vec<&str> = (interface.callables.iter()).filter(|c| !c.internal).map(|c| c.name.as_str()).collect();
    match (function, public.as_slice()) {
        (Some(function), _) => Some(function),
        (None, [only]) => Some((*only).to_owned()),
        (None, _) => None,
    }
}

fn parse(document: &serde_json::Value) -> Result<Interface, Error> {
    Interface::parse(document).map_err(|why| {
        Error::new(Kind::Unavailable, "package_interface_invalid", format!("the package interface is unreadable: {why}"))
    })
}

/// A published target: for `x/y`, root package x's function y when the Hub holds root package x,
/// else account x's package y (one read when it is the root's; an older Hub answers the root's
/// absent).
pub(crate) async fn published(api: &Arc<Api>, hub: &str, text: &str) -> Result<Resolved, Error> {
    let Published { package, function, release, owned } = Published::parse(text.trim())?;
    let root = release_of(api, hub, &package, release.as_deref()).await;
    let (package, function, (card, from)) = match (root, owned) {
        (Ok(found), _) => (package, function, found),
        (Err(e), Some(owned)) if matches!(e.kind, Kind::NotFound | Kind::Usage) => {
            match release_of(api, hub, &owned, release.as_deref()).await {
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
    Ok(Resolved::Release { package, function, card, release, from })
}

/// Which release a run binds against and runs, once, before binding: the one asked, else the
/// newest this computer keeps of the package (refreshed in the background), wherever the run goes
/// (a machine holding an older one installs it first). The run is pinned to it, so the machine
/// runs exactly the release its inputs were bound against.
pub async fn release_of(
    api: &Api,
    hub: &str,
    package: &str,
    release: Option<&str>,
) -> Result<(Card, ReleaseFrom), Error> {
    let hubs = &api.machines.hubs;
    let root = hubs.home().root();
    let client = || hubs.reader(hub);
    let (release, from) = match release {
        Some(release) => (release.to_owned(), ReleaseFrom::Asked),
        None => (cards::latest(client, root, hub, package).await?, ReleaseFrom::Latest),
    };
    Ok((cards::card(client, root, hub, package, &release).await?, from))
}
