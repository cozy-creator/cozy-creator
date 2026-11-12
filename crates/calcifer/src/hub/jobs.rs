//! Calcifer's own jobs (`api::calcifer::hub::job`; cl-841): a model's download, upload and quantization, a package's install,
//! removal, lock and publication. Each is a run, asked by `POST /v1/runs` of its reserved target with its fields as `input`, and
//! quoted by `POST /v1/run-quotes`. Most are a machine's run with a step of Calcifer's once it completes (`after`); a lock, a
//! publication, a commit recorded at the Hub and a removal are Calcifer's own (`Kind::Here`), on no machine.
use std::sync::Arc;

use api::{
    calcifer::{
        Note, NoteLevel, On, Run, RunQuote, RunRequest, SubmitRun, Submitted,
        hub::{PackageLock, PackagePublish, job},
    },
    code::LocalCode,
    config::Origin,
    error::{Error, Kind},
};
use serde::de::DeserializeOwned;

use super::{after::After, author, origin, packages, quantize, weights};
use crate::{
    Here,
    api::{Api, Refusal},
};

/// Whether `target` names one of Calcifer's own jobs.
pub(crate) fn is(target: &str) -> bool {
    job::ALL.contains(&target)
}

/// What a job's request becomes. Planning one changes nothing, at the Hub or on a machine.
pub(super) enum Planned {
    /// A machine's run, what Calcifer does once it completes, what planning it said, and the mirror to record before it goes.
    Run { submit: Box<SubmitRun>, after: Option<After>, notes: Vec<String>, mirror: Option<weights::Mirror> },
    /// A call of a package's function, bound as any is.
    Request(Box<RunRequest>),
    /// Local code's install, still going: its client follows it, then asks again under the same key.
    Preparing(Box<Run>),
    /// Calcifer's own work, called `target` in the run's record.
    Here { target: String, work: Work },
}

/// Work Calcifer carries out itself.
pub(super) enum Work {
    Lock(LocalCode, PackageLock),
    Publish(LocalCode, PackagePublish),
    Record(Box<weights::Record>),
    Remove(packages::Removal),
}

/// A machine's run with nothing more to it.
impl From<SubmitRun> for Planned {
    fn from(submit: SubmitRun) -> Self {
        Self::Run { submit: Box::new(submit), after: None, notes: vec![], mirror: None }
    }
}

/// The job's fields, from the request's `input`.
fn input<T: DeserializeOwned>(ask: &RunRequest) -> Result<T, Refusal> {
    let given = !ask.terms.is_empty() || !ask.files.is_empty() || ask.input_text.is_some() || !ask.loras.is_empty();
    if given {
        let said = format!("{} takes its fields as `input`, and nothing else", ask.target);
        return Err(Refusal(Error::new(Kind::Usage, "job_input", said)));
    }
    let fields = serde_json::Value::Object(ask.input.clone().unwrap_or_default());
    serde_json::from_value(fields).map_err(|why| Refusal(Error::new(Kind::Validation, "job_input", format!("{}: {why}", ask.target))))
}

/// The local code a job is of.
fn code(ask: &RunRequest) -> Result<LocalCode, Refusal> {
    let said = format!("{} is of a package sent by content: the request's `code`", ask.target);
    ask.code.clone().ok_or_else(|| Refusal(Error::new(Kind::Usage, "job_code", said)))
}

async fn plan(api: &Arc<Api>, key: &str, hub: &Origin, ask: &RunRequest) -> Result<Planned, Refusal> {
    let scope=match ask.project.as_deref() {
        Some(named)=>{ let (id,root)={ let store=api.projects.store(api,named).await.map_err(Refusal)?; let(id,root)=store.execution_scope(); (id.to_owned(),root.to_owned()) }; Some(crate::private_io::scope_for_project(&id,&root).map_err(Refusal)?) },
        None=>None,
    };
    crate::private_io::context(scope,plan_scoped(api,key,hub,ask)).await
}

async fn plan_scoped(api: &Arc<Api>, key: &str, hub: &Origin, ask: &RunRequest) -> Result<Planned, Refusal> {
    if ask.project.is_none() { if let Some(code)=&ask.code { crate::maintenance_source::require(api,code).map_err(Refusal)?; } }
    // Unnamed, where it is made present: this computer's machine, else any rental there is.
    let on = match ask.on.as_slice() {
        [] => crate::runs::default_on(api),
        [on] => on.clone(),
        _ => return Err(Refusal(Error::new(Kind::Usage, "on_one", format!("{} goes to one machine: name one with --on", ask.target)))),
    };
    let on = if let Some(named) = ask.project.as_deref() {
        let (project, root) = {
            let store = api.projects.store(api, named).await.map_err(Refusal)?;
            let (project, root) = store.execution_scope();
            (project.to_owned(), root.to_owned())
        };
        crate::private_io::check_project_placement(&project, &root, &[on]).map_err(Refusal)?.into_iter().next()
            .ok_or_else(|| Refusal(Error::new(Kind::Unavailable, "project_execution_unavailable", "no project execution target is available")))?
    } else { on };
    let agreed = ask.agreed;
    // Calcifer's pick is refused as a named machine is, where this OS runs no models itself.
    let usable = || api::calcifer::local_usable(&on.to_string()).map_err(Refusal);
    if ask.on.is_empty() && matches!(ask.target.as_str(), job::MODEL_DOWNLOAD | job::MODEL_QUANTIZE | job::PACKAGE_INSTALL | job::PACKAGE_REMOVE) {
        usable()?;
    }
    match ask.target.as_str() {
        job::MODEL_DOWNLOAD => weights::download(hub, on, agreed, input(ask)?),
        job::MODEL_UPLOAD => weights::upload(api, hub, on, agreed, input(ask)?).await,
        job::MODEL_QUANTIZE => quantize::plan(api, key, hub, on, agreed, ask.code.clone(), input(ask)?).await,
        job::PACKAGE_INSTALL => packages::install(api, hub, on, agreed, ask.code.clone(), input(ask)?).await,
        job::PACKAGE_REMOVE => packages::removal(api, hub, &on, input(ask)?).await,
        job::PACKAGE_LOCK => Ok(Planned::Here { target: ask.target.clone(), work: Work::Lock(code(ask)?, input(ask)?) }),
        job::PACKAGE_PUBLISH => Ok(Planned::Here { target: ask.target.clone(), work: Work::Publish(code(ask)?, input(ask)?) }),
        other => unreachable!("{other} is no job"),
    }
}

/// Records the job `ask` names under `key` as the ask `digest` names, and starts it: `Submitted`, as any run's. The same key
/// again answers the recorded run.
pub(crate) async fn request(api: &Arc<Api>, key: &str, digest: &str, ask: RunRequest) -> Result<Submitted, Error> {
    let calcifer = &api.calcifer;
    if ask.project.is_none() { if let Some(code)=&ask.code { crate::maintenance_source::require(api,code)?; } }
    let project = match ask.project.as_deref() {
        Some(named) => Some({ let store = api.projects.store(api, named).await?; store.project_id().to_owned() }),
        None => None,
    };
    let store = |e: crate::store::StoreError| Error::internal("calcifer_store", e.to_string());
    // Recorded as this very ask: answered as it is. A quantization is recorded as the call it plans, which answers for itself.
    let recorded = match project.as_deref() {
        Some(project) => calcifer.project_store(project).map_err(store)?.recorded(key),
        None => calcifer.maintenance_store().recorded(key),
    }.map_err(store)?;
    if let Some((row, _)) = recorded.filter(|(_, recorded)| recorded == digest) {
        let run = calcifer.record(&row).map_err(crate::runs::refused)?;
        return Ok(Submitted { run, replay: true, notes: vec![], preparing: false, lock: None });
    }
    let hub = origin(api, Some(&ask.hub)).map_err(|Refusal(e)| e)?;
    match plan(api, key, &hub, &ask).await.map_err(|Refusal(e)| e)? {
        Planned::Preparing(run) => Ok(Submitted { run: *run, replay: false, notes: vec![], preparing: true, lock: None }),
        Planned::Request(mut request) => { request.project = project.clone(); Box::pin(crate::runs::request(api, key, *request)).await },
        Planned::Run { mut submit, after, mut notes, mirror } => {
            submit.project = project.clone();
            if let Some(project) = project.as_deref() {
                let store = calcifer.project_store(project).map_err(crate::Error::from).map_err(crate::runs::refused)?;
                submit.out = Some(crate::project::landing(store.execution_scope().1));
            }
            if let Some(mirror) = mirror {
                notes.extend(mirror.record(api, &hub).await.map_err(|Refusal(e)| e)?);
            }
            // Its step is kept first, so a Calcifer that stops in between still does it.
            if let Some(after) = &after {
                calcifer.plan(project.as_deref(), key, after)?;
            }
            let submitted = calcifer.submit_under(key, digest, *submit).await;
            if submitted.is_err() && after.is_some() {
                calcifer.unplan(project.as_deref(), key);
            }
            let notes = notes.into_iter().map(|message| Note { level: NoteLevel::Info, message }).collect();
            Ok(Submitted { notes, ..submitted.map_err(crate::runs::refused)? })
        }
        Planned::Here { target, work } => {
            let kept = serde_json::to_vec(&ask).map_err(|e| Error::internal("job_ask", e.to_string()))?;
            let (its_api, its_hub, its_key) = (api.clone(), hub.clone(), key.to_owned());
            let work = move |here: Here| async move { work.run(&its_api, &its_hub, &its_key, &here).await };
            calcifer.here(project.as_deref(), key, digest, target, &hub, kept, work).await.map_err(crate::runs::refused)
        }
    }
}

/// Carries out again each of Calcifer's own jobs an earlier Calcifer stopped under, from its kept ask, under the same run: each
/// is safe to do again (a lock is `uv lock`; a publication answers a version already published, and the Hub takes the same
/// release declared again; the Hub answers a commit it has; a removal's runs go under keys named for its own). One with a cancel
/// asked settles canceled; one with no ask kept, or whose ask now plans otherwise, fails `calcifer_stopped`.
pub(crate) async fn resume(api: Arc<Api>) {
    let ids=api.calcifer.maintenance_store().unsettled_here().unwrap_or_default();
    resume_ids(api,ids).await;
}

pub(crate) fn resume_project(api: Arc<Api>, project:String)->futures_util::future::BoxFuture<'static,()> {
    use futures_util::FutureExt as _;
    async move {
        let ids=match api.calcifer.project_store(&project).and_then(|store|store.unsettled_here()) { Ok(ids)=>ids,Err(_)=>return };
        resume_ids(api,ids).await;
    }.boxed()
}

async fn resume_ids(api:Arc<Api>, ids:Vec<api::ids::RunId>) {
    let calcifer=&api.calcifer;
    for id in ids {
        let stopped = || Error::new(Kind::Unavailable, "calcifer_stopped", "Calcifer stopped before this ended").remedy("ask again: it is safe");
        let snapshot=calcifer.run_store(id.as_str()).map(|store|(store.run(id.as_str()),store.key(id.as_str()),store.job(id.as_str())));
        let (row, key, kept) = match snapshot {
            Ok((Ok(Some(row)),Ok(Some(key)),Ok(Some(kept)))) => (row,key,kept),
            _=>{ calcifer.ended_here(&id,Err(stopped())); continue; }
        };
        if row.cancel_requested {
            calcifer.ended_here(&id, Err(Error::new(Kind::Conflict, "canceled", "canceled")));
            continue;
        }
        let mut ask = match serde_json::from_slice::<RunRequest>(&kept) {
            Ok(ask) => ask,
            Err(_) => {
                calcifer.ended_here(&id, Err(stopped()));
                continue;
            }
        };
        if let Ok(crate::store::RunStore::Project(store))=calcifer.run_store(id.as_str()) { ask.project=Some(store.project_id().into()); }
        let planned = match origin(&api, Some(&ask.hub)) {
            Ok(hub) => plan(&api, &key, &hub, &ask).await.map(|planned| (hub, planned)),
            Err(refused) => Err(refused),
        };
        match planned {
            Ok((hub, Planned::Here { work, .. })) => {
                let its_api = api.clone();
                let work = move |here: Here| async move { work.run(&its_api, &hub, &key, &here).await };
                calcifer.here_again(id, work);
            }
            Ok(_) => calcifer.ended_here(&id, Err(stopped())),
            Err(Refusal(refused)) => calcifer.ended_here(&id, Err(refused)),
        }
    }
}

/// What the job `ask` names would rent on a `new` machine, renting nothing; none for Calcifer's own work, which rents nothing.
pub(crate) async fn quote(api: &Arc<Api>, ask: RunRequest) -> Result<Option<RunQuote>, Error> {
    if ask.target == job::MODEL_QUANTIZE && ask.code.is_some() {
        let said = "local code runs where it was made ready; a `new` machine takes a published release";
        return Err(Error::new(Kind::Usage, "quote_local", said).next("cozy machine rent"));
    }
    let hub = origin(api, Some(&ask.hub)).map_err(|Refusal(e)| e)?;
    match plan(api, "quote", &hub, &ask).await.map_err(|Refusal(e)| e)? {
        Planned::Here { .. } => Ok(None),
        Planned::Request(request) => Box::pin(crate::runs::priced(api, *request)).await,
        Planned::Run { submit, .. } => {
            api.machines.hubs.client(&submit.hub)?;
            Ok(Some(api.machines.run_quote(&submit).await?))
        }
        Planned::Preparing(_) => unreachable!("a published package installs nothing first"),
    }
}

impl Work {
    /// Carries it out, as the run `here`: its result.
    async fn run(self, api: &Arc<Api>, hub: &Origin, key: &str, here: &Here) -> Result<serde_json::Value, Error> {
        let result = match self {
            Work::Lock(code, ask) => serde_json::to_value(author::lock(api, hub, &code, &ask, here).await?),
            Work::Publish(code, ask) => serde_json::to_value(author::publish(api, hub, &code, &ask, here).await?),
            Work::Record(record) => serde_json::to_value(record.run(api, hub).await.map_err(|Refusal(e)| e)?),
            Work::Remove(removal) => serde_json::to_value(removal.run(api, key).await?),
        };
        result.map_err(|e| Error::internal("job_result", e.to_string()))
    }
}

/// A `On` a job names that is one machine.
pub(super) fn machine(on: &On, what: &str) -> Result<String, Refusal> {
    match on {
        On::Machine(name) => Ok(name.to_ascii_lowercase()),
        other => {
            let said = format!("{what} is of one machine: {other} names none");
            Err(Refusal(Error::new(Kind::Usage, "machine_name_required", said).next("cozy machine ls")))
        }
    }
}
