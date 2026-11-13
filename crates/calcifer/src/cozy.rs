//! cozy updates itself (cl-978). Paul, 11-13: "if they install a package requiring v0.25, do we automatically upgrade their
//! installation, on their local machine, and on a rented remote machine? I feel like we should"; and "patch + minor updates are
//! fine. We probably won't do major updates." A run or install whose package release needs a Runtime only a newer cozy's line
//! runs (`api::release::Chosen::Cozy`) first moves this computer's cozy to the Latest release, by `cozy update`'s own path
//! (`api::update`; Homebrew's by `brew upgrade`): Calcifer's own run (`cozy/update`), which the asking client follows, then asks
//! again. Once nothing else is going, Calcifer restarts on the new binary (`Api::restart_updated`): the page reconnects, `local`
//! (the same binary) moves to it, and rentals follow the new Calcifer's minor as ever.
use std::{path::PathBuf, sync::Arc, time::Duration};

use api::{
    calcifer::{
        Note, NoteLevel, Run, RunRequest, Status,
        hub::{CozyUpdate, CozyUpdated, job},
    },
    config::Origin,
    error::{Error, Kind},
};
use platform::paths::Home;

use crate::{Here, api::Api};

pub(crate) use api::update::{major, newer};

/// What a run's package release asks of cozy: the release (the Hub's) whose line runs it, and the release it is (`qwen-image-2 0.12.0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Wanted {
    pub to: String,
    pub needed_by: String,
}

impl Wanted {
    /// What a quote or an estimate says of it.
    pub(crate) fn said(&self) -> String {
        format!("{} needs cozy {}: a run of it updates this cozy ({}) first", self.needed_by, self.to, crate::update::this_release())
    }
}

/// The update `wanted` asks, to the Latest release, started once per Calcifer: its run, which the run that needs it waits behind,
/// and the line that says so, once (`updating cozy 0.25.0 → 0.26.0 for qwen-image-2 0.12.0`). Refused at once, naming the command
/// that does it by hand, when it can't happen: the Latest unread (offline), of a new major, or older than what's needed; one that
/// failed is refused with what happened.
pub(crate) async fn update(api: &Arc<Api>, wanted: &Wanted) -> Result<(Run, Note), Error> {
    let own = crate::update::this_release();
    let download = api.machines.download_url.as_deref();
    let latest = api::update::release(&api::update::dir(download, None)?, None).await.map_err(|e| {
        let said = format!("{} needs cozy {}, and the Latest cozy release couldn't be read: {}", wanted.needed_by, wanted.to, e.message);
        Error::new(Kind::Unavailable, "cozy_unreachable", said).next("cozy update")
    })?;
    let to = latest.version;
    if major(&to) != major(own) {
        return Err(major_refusal(&wanted.needed_by, &to));
    }
    if newer(&wanted.to, &to) || !newer(&to, own) {
        let said = format!("{} needs cozy {}, and the Latest release is {to}", wanted.needed_by, wanted.to);
        return Err(Error::new(Kind::Conflict, "cozy_not_released", said).next("cozy update"));
    }
    let note = Note { level: NoteLevel::Info, message: format!("updating cozy {own} → {to} for {}", wanted.needed_by) };
    let calcifer = &api.calcifer;
    let key = format!("cozy-update-{to}-{}", api.started_ms);
    let store = |e: crate::store::StoreError| Error::internal("calcifer_store", e.to_string());
    if let Some(row) = calcifer.store().keyed(&key).map_err(store)? {
        let run = calcifer.record(&row).map_err(crate::runs::refused)?;
        return match run.status {
            Status::Failed | Status::Canceled => Err(failure(&run)),
            // Installed, this Calcifer restarts on it once nothing else is going: asked again of the next.
            Status::Completed => Err(Error::new(Kind::Unavailable, api::error::STOPPING, format!("cozy {to} is installed: Calcifer restarts on it once nothing else is going"))
                .remedy("ask again once it is back; nothing was carried out")),
            _ => Ok((run, note)),
        };
    }
    let hub = Origin::parse(&api.machines.hubs.default_origin())?;
    let input = CozyUpdate { to, needed_by: wanted.needed_by.clone() };
    let ask = RunRequest::job(job::COZY_UPDATE, &input, None, hub.to_string(), None);
    let kept = serde_json::to_vec(&ask).map_err(|e| Error::internal("job_ask", e.to_string()))?;
    let its = api.clone();
    let work = move |here: Here| async move { run(&its, input, &here).await };
    let digest = crate::runs::digest(&ask);
    let run = calcifer.here(&key, &digest, job::COZY_UPDATE.into(), &hub, kept, work).await.map_err(crate::runs::refused)?.run;
    Ok((run, note))
}

/// The refusal a failed update ended with, whole.
fn failure(run: &Run) -> Error {
    let kept = run.result.as_ref().and_then(|result| serde_json::from_value::<Error>(result["error"].clone()).ok());
    kept.unwrap_or_else(|| {
        let reason = run.reason.clone().unwrap_or_default();
        let error = Error::new(Kind::Unavailable, "cozy_update_failed", format!("cozy's update didn't finish: {}", reason.message));
        reason.next.into_iter().fold(error, |error, next| error.next(next))
    })
}

/// Carries out `ask` as Calcifer's own run `here`: release `ask.to` put in place of the installed binary; then, once nothing else is
/// going, this run settles and Calcifer restarts on it.
pub(crate) async fn run(api: &Arc<Api>, ask: CozyUpdate, here: &Here) -> Result<serde_json::Value, Error> {
    let own = crate::update::this_release();
    let by_hand = |error: Error| match error.next.is_empty() {
        true => error.next("cozy update"),
        false => error,
    };
    // Carried out again by the cozy it updated to: done.
    if !newer(&ask.to, own) {
        return Ok(serde_json::to_value(CozyUpdated { from: own.into(), to: own.into() }).unwrap_or_default());
    }
    let download = api.machines.download_url.as_deref();
    let dir = api::update::dir(download, Some(&ask.to))?;
    let latest = api::update::release(&dir, Some(&ask.to)).await.map_err(|e| {
        Error::new(Kind::Unavailable, "cozy_unreachable", format!("cozy {} for {} couldn't be read: {}", ask.to, ask.needed_by, e.message)).next("cozy update")
    })?;
    let binary = installed(&api.machines.home)?;
    let installed = api::update::install(&dir, download.is_some(), &latest, &binary, &mut |_| {}).await.map_err(by_hand)?;
    let mut said = false;
    loop {
        let mut tail = api.calcifer.subscribe();
        let Some(why) = api.busy_but(here.id.as_str())? else { break };
        if !std::mem::replace(&mut said, true) {
            here.say(&format!("cozy {} is installed: Calcifer restarts on it once nothing else is going ({why})", installed.to));
        }
        tokio::select! {
            _ = tail.recv() => {}
            () = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    }
    // Restarted once this run is settled, as its followers read it.
    let (restarting, id) = (api.clone(), here.id.clone());
    tokio::spawn(async move {
        for _ in 0..100 {
            let settled = restarting.calcifer.store().run(id.as_str()).ok().flatten().is_some_and(|row| restarting.calcifer.record(&row).is_ok_and(|run| run.status == Status::Completed));
            if settled {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        restarting.restart_updated().await;
    });
    Ok(serde_json::to_value(CozyUpdated { from: own.into(), to: installed.to }).unwrap_or_default())
}

/// Only a cozy of a new major runs `needed_by`: cozy doesn't update itself across one (Paul, 11-13).
pub(crate) fn major_refusal(needed_by: &str, cozy: &str) -> Error {
    let own = crate::update::this_release();
    let said = format!("{needed_by} needs cozy {cozy}, a new major: cozy {own} doesn't update itself across one");
    Error::new(Kind::Conflict, "cozy_major_needed", said).next("cozy update")
}

/// The installed `cozy`, the one Calcifer and the machine run (as the CLI records it): what an update replaces and Calcifer then
/// starts. With none recorded, the installer's `<home>/bin/cozy`, else where this one started from (Homebrew's stable path).
pub(crate) fn installed(home: &Home) -> Result<PathBuf, Error> {
    if let Some(found) = home.launcher() {
        return Ok(found);
    }
    let exe = platform::exe::record().map_err(|e| Error::internal("installed_unrecorded", e.to_string()))?.path.clone();
    Ok(api::update::homebrew(&exe).unwrap_or(exe))
}

/// What this cozy says when an update replaced the binary it runs, where it can't start itself again (`platform::exe::running`):
/// the restart that puts it on the new one.
pub(crate) fn updated(e: platform::Error) -> Error {
    match e {
        platform::Error::Updated { .. } => Error::new(Kind::Unavailable, "cozy_updated", e.to_string()).next("cozy restart"),
        other => Error::new(Kind::Unavailable, "cozy_unreadable", format!("this cozy's binary: {other}")),
    }
}
