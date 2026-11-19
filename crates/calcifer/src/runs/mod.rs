//! Runs as people ask for them (`POST /v1/runs`): the target resolved on its Hub (the card and the
//! release its machine holds, else its Hub's newest), the request bound against the function's interface, input
//! files sniffed, hashed and staged, video segments cut, and the machine's RunSpec built. Every
//! client sends the same unbound request; none builds a spec.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json,
    body::Bytes,
    extract::{Query, State, rejection::BytesRejection},
    http::StatusCode,
    response::Response,
};
use api::{
    calcifer::{Kind as RunKind, MachineEstimate, MachineKind, MachineState, Note, NoteLevel, On, RunEstimate, RunQuote, RunRequest, StagedInput, SubmitRun, Submitted, Warning},
    code::LocalCode,
    error::{Error, Kind},
    interface::{Callable, Interface},
    machine::{self as v1, WarmLevel},
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use crate::{api::{Api, IdempotencyKey, Refusal, parse}, hub::jobs::Job, project::vault};

pub(crate) mod bind;
pub(crate) mod clip;
pub(crate) mod files;
pub(crate) mod report;
pub(crate) mod spec;
pub(crate) mod target;
pub(crate) mod video_progress;

use bind::Terms;
use target::Resolved;

/// `POST /v1/runs`: 202 with the run recorded, 200 when its key had recorded it.
pub(crate) async fn submit(
    State(api): State<Arc<Api>>,
    IdempotencyKey(key): IdempotencyKey,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Refusal> {
    let body = body?;
    prebuilt(&body)?;
    let ask: RunRequest = parse(&body)?;
    // Every run is kept in its project (cl-1028): here only Calcifer's jobs, which are no one's runs.
    if ask.project.is_none() && ask.warm.is_none() && !crate::hub::jobs::is(&ask.target) {
        return Err(Refusal(api::calcifer::project::unnamed()));
    }
    if ask.project.is_some() {
        return Err(Refusal(Error::new(Kind::Usage, "project_context_required", "Send this run through its project.")));
    }
    let project = ask.project.clone();
    vault::project_reply(&api, project.as_deref(), async {
        let submitted = Box::pin(request(&api, &key, ask)).await.map_err(Refusal)?;
        let status = if submitted.replay { StatusCode::OK } else { StatusCode::ACCEPTED };
        Ok((status, Json(submitted)))
    }).await
}

pub(crate) async fn submit_project(
    State(api): State<Arc<Api>>,
    axum::extract::Path(project): axum::extract::Path<String>,
    IdempotencyKey(key): IdempotencyKey,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Refusal> {
    let bytes = body?;
    prebuilt(&bytes)?;
    let mut ask: RunRequest = parse(&bytes)?;
    let (id,recorded) = { let store = api.projects.store(&api, &project).await.map_err(Refusal)?; (store.project_id().to_owned(),store.recorded(&key)?) };
    // The same key again answers its recorded run: the row inside this admitted owner.
    if let Some((row,recorded))=recorded {
        if recorded!=digest(&ask) {return Err(Refusal(crate::api::conflict(&key)));}
        return vault::project_reply(&api,Some(&project),async {
            Ok((StatusCode::OK,Json(Submitted {run:api.calcifer.record(&row)?,replay:true,notes:vec![],preparing:false,lock:None})))
        }).await;
    }
    if let Some(named) = ask.project.as_deref() {
        let actual = { let store = api.projects.store(&api, named).await.map_err(Refusal)?; store.project_id().to_owned() };
        if actual != id { return Err(Refusal(Error::new(Kind::Validation, "project_scope_mismatch", "The request names a different project from the one it was sent to."))); }
    }
    if ask.project.is_none() {ask.project = Some(id);}
    vault::project_reply(&api, Some(&project), async {
        let submitted = request(&api, &key, ask).await.map_err(Refusal)?;
        let status = if submitted.replay { StatusCode::OK } else { StatusCode::ACCEPTED };
        Ok((status, Json(submitted)))
    }).await
}

/// `POST /v1/run-quotes`: what a `new` run would rent and its price; `null` for one that rents nothing, as Calcifer's own work
/// does. Nothing is rented or recorded.
pub(crate) async fn quote(
    State(api): State<Arc<Api>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Refusal> {
    let body = body?;
    prebuilt(&body)?;
    let ask: RunRequest = parse(&body)?;
    let project = ask.project.clone();
    vault::project_reply(&api, project.as_deref(), async {
        Ok(Json(Box::pin(priced(&api, ask)).await.map_err(Refusal)?))
    }).await
}

/// What `ask` would rent.
pub(crate) async fn priced(api: &Arc<Api>, ask: RunRequest) -> Result<Option<RunQuote>, Error> {
    let ask = match crate::hub::jobs::is(&ask.target) {
        true => match Box::pin(crate::hub::jobs::quote(api, ask)).await? {
            Job::Done(quote) => return Ok(quote),
            Job::Call(call) => *call,
        },
        false => ask,
    };
    drop(project_access(api, ask.project.as_deref()).await?);
    let scope = match ask.project.as_deref() {
        Some(project) => {
            let root = crate::project::named(api, project)?;
            crate::private_io::of_output(&crate::project::landing(&root))?
        },
        None => None,
    };
    crate::private_io::context(scope, priced_scoped(api, ask)).await
}

async fn priced_scoped(api: &Arc<Api>, ask: RunRequest) -> Result<Option<RunQuote>, Error> {
    let home = api.machines.hubs.home();
    // A dev home's checkout's first-party package is local code too (xs-195).
    let code = match &ask.code {
        Some(code) => Some(code.clone()),
        None => crate::checkout::named(&ask.target).map(|(path, _)| crate::local_code::own(api, &path)).transpose()?,
    };
    let bound = match &code {
        // Local code rents with its install, which the call then follows to its machine.
        Some(code) => {
            let on = ask.on.first().cloned().unwrap_or_else(|| default_on(api));
            let hub = hub(api, &ask.hub);
            let origin = api::config::Origin::parse(&hub)?;
            let source = crate::local_code::source(home, code, &api.machines.hubs, &origin).await?;
            crate::maintenance_source::keep_derived(api,code,&source)?;
            crate::local_code::install_run(home, &hub, on, &source)
        }
        None => match prepare(api, "quote", &ask, For::Quote).await? {
            Prepared::Run { submit, .. } => submit,
            Prepared::Install(_) | Prepared::Cozy(..) => unreachable!("a published target installs nothing first"),
        },
    };
    // A `new` run that a machine up or on its way finishes sooner goes there, and rents nothing (cl-915).
    if !matches!(bound.on, On::New(_)) {
        return Ok(None);
    }
    api.machines.hubs.client(&bound.hub)?;
    Ok(Some(api.machines.run_quote(&bound).await?))
}

/// `POST /v1/run-estimates` (`Idempotency-Key`; cl-816): when each machine the run may go to would finish it, as each says now.
/// Nothing is sent, and nothing is installed, moved or updated: local code is read as a quote reads it (cl-1115).
pub(crate) async fn estimate(
    State(api): State<Arc<Api>>,
    IdempotencyKey(key): IdempotencyKey,
    query: Result<Query<Copies>, axum::extract::rejection::QueryRejection>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Refusal> {
    let Query(copies) = query?;
    let body = body?;
    prebuilt(&body)?;
    let ask: RunRequest = parse(&body)?;
    vault::project_reply(&api, ask.project.as_deref(), estimate_asked(&api, &key, &ask, copies)).await
}

async fn estimate_asked(api: &Arc<Api>, key: &str, ask: &RunRequest, copies: Copies) -> Result<Json<RunEstimate>, Refusal> {
    let count = copies.count.unwrap_or(1).clamp(1, COPIES);
    let Prepared::Run { submit, mut notes, offers } = prepare(api, key, ask, For::Estimate(count)).await.map_err(Refusal)? else {
        unreachable!("an estimate installs and updates nothing first");
    };
    if copies.count.is_some_and(|asked| asked != count) {
        notes.push(Note { level: NoteLevel::Warning, message: format!("You can render 1 to {COPIES} copies, so this estimate is for {count}.") });
    }
    let now = crate::store::now_ms();
    let shown = offers.iter().enumerate().flat_map(|(job, offers)| offers.iter().map(move |offer| MachineEstimate { job: job as u32, ..offer.shown(now) }));
    let mut machines: Vec<MachineEstimate> = shown.collect();
    machines.sort_by_key(|machine| (machine.job, !machine.fits, machine.finish_ms));
    if machines.is_empty() {
        notes.push(Note { level: NoteLevel::Warning, message: format!("There's no estimate for {}, because only a machine that's running or starting can give one.", submit.on) });
    }
    // Where it would go, if that machine can run it at all.
    let soonest = match &submit.on {
        On::Machine(name) => machines.iter().any(|machine| machine.job == 0 && machine.fits && machine.machine == *name).then(|| name.clone()),
        _ => None,
    };
    Ok(Json(RunEstimate { machines, soonest, notes }))
}

/// The most copies one estimate quotes: a walk's window.
const COPIES: u32 = 32;

#[derive(Deserialize)]
pub(crate) struct Copies {
    /// How many of the run are quoted at once, as if sent together: 1 to 32.
    #[serde(default)]
    count: Option<u32>,
}

#[derive(Deserialize)]
pub(crate) struct Asked {
    #[serde(default)]
    project: Option<String>,
    /// With `tree`, the function of that local code (empty: none named).
    #[serde(default)]
    target: String,
    #[serde(default)]
    hub: Option<String>,
    /// The machines a run would go to, as `--on` names them.
    #[serde(default)]
    on: Option<String>,
    /// Local code by content: its tree, and where its package is in it.
    #[serde(default)]
    tree: Option<api::ids::Digest>,
    #[serde(default)]
    at: Option<String>,
}

/// `GET /v1/targets?target=`: what a run of the target `on` its machine calls, as `POST /v1/runs` resolves it.
pub(crate) async fn describe(
    State(api): State<Arc<Api>>,
    query: Result<Query<Asked>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, Refusal> {
    let Query(asked) = query?;
    let project = asked.project.clone();
    vault::project_reply(&api, project.as_deref(), describe_asked(&api, asked)).await
}

async fn describe_asked(api: &Arc<Api>, asked: Asked) -> Result<Json<api::calcifer::Target>, Refusal> {
    if let Some(project)=asked.project.as_deref() { drop(api.projects.store(api,project).await.map_err(Refusal)?); }
    let hub = hub(api, asked.hub.as_deref().unwrap_or_default());
    // Of several (named, else its project's), the first: what it calls is the same on each.
    let pinned = match (&asked.on, asked.project.as_deref()) {
        (Some(on), _) => on.split(',').map(str::trim).find(|word| !word.is_empty()).map(|first| On::from(first.to_owned())),
        (None, Some(project)) => crate::project::machines_of(api, project).map_err(Refusal)?.into_iter().next(),
        (None, None) => None,
    };
    let on = pinned.unwrap_or_else(|| default_on(api));
    let code = match (asked.tree, asked.at) {
        (Some(tree), at) => Some(LocalCode { tree, at: at.unwrap_or_default() }),
        (None, _) => None,
    };
    let scope = match asked.project.as_deref() {
        Some(project) => {
            let root = crate::project::named(api, project).map_err(Refusal)?;
            crate::private_io::of_output(&crate::project::landing(&root)).map_err(Refusal)?
        },
        None => None,
    };
    let resolved = crate::private_io::context(scope, target::describe(api, &hub, &asked.target, code.as_ref(), None, &on)).await.map_err(Refusal)?;
    Ok(Json(resolved))
}

/// A run whose spec an older cozy built (`SubmitRun`, its `spec`), which this Calcifer binds itself:
/// refused as the client's age, never read as an unbound request missing its fields.
fn prebuilt(body: &[u8]) -> Result<(), Refusal> {
    let shape: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    if shape.get("spec").is_none() {
        return Ok(());
    }
    let version = ::api::VERSION;
    Err(Refusal(
        Error::new(
            Kind::Usage,
            "client_outdated",
            format!("This cozy is older than {version} and sent the run in an old format. Cozy on this computer runs {version}."),
        )
        .remedy(format!("Use the installed cozy (~/.cozy/bin/cozy), or update this one to {version}.")),
    ))
}

/// Records `ask` under `key` and starts it; the same key again answers the recorded run without
/// binding anything again, and another ask under it is a conflict. Local code not yet made ready
/// answers the run that makes it ready (`preparing`): its client follows that, then asks again.
/// The files it's given join its project, so asking it again still finds them once what held them is gone.
pub(crate) async fn request(api: &Arc<Api>, key: &str, ask: RunRequest) -> Result<Submitted, Error> {
    // A job that plans a call (a quantization) goes on as that call, here: Calcifer's job of that type (cl-1028).
    let job = crate::store::Task::of_job(&ask.target);
    let ask = match crate::hub::jobs::is(&ask.target) {
        true => match Box::pin(crate::hub::jobs::request(api, key, &digest(&ask), ask)).await? {
            Job::Done(submitted) => return Ok(submitted),
            Job::Call(call) => *call,
        },
        false => ask,
    };
    let digest = digest(&ask);
    let calcifer = &api.calcifer;
    let recorded = match ask.project.as_deref() {
        Some(project) => api.projects.store(api, project).await?.recorded(key).map_err(store)?,
        None => calcifer.maintenance_store().recorded(key).map_err(store)?,
    };
    if let Some((row, recorded)) = recorded {
        if recorded != digest {
            return Err(crate::api::conflict(key));
        }
        crate::project::vault::require_path(&row.out_dir)?;
        let run = calcifer.record(&row).map_err(refused)?;
        return Ok(Submitted { run, replay: true, notes: vec![], preparing: false, lock: None });
    }
    // Prepared inputs can name files in the mounted view. Keep the view
    // leased until submission takes them and their history is kept.
    drop(project_access(api, ask.project.as_deref()).await?);
    let prepared = prepare(api, key, &ask, For::Run).await?;
    let lock = match &ask.code {
        Some(code) => made_lock(api, &ask, code).await,
        None => None,
    };
    match prepared {
        Prepared::Install(run) => {
            let notes = updating(api, &run, ask.project.is_some()).await.into_iter().collect();
            Ok(Submitted { run, replay: false, notes, preparing: true, lock })
        }
        // The run waits behind cozy's own update: its client follows it, then asks again (cl-978).
        Prepared::Cozy(run, note) => Ok(Submitted { run, replay: false, notes: vec![note], preparing: true, lock }),
        Prepared::Run { submit, mut notes, offers } => {
            let given = given(&submit);
            let project = submit.out.as_deref().and_then(crate::project::of_landing).map(Path::to_owned);
            let mut offer = offers.into_iter().next().unwrap_or_default().into_iter().find(|offer| offer.machine == submit.on);
            // A quantization's call, or a warm set asked of a machine, is one of Calcifer's jobs (cl-1028); anything else here is a
            // project's run, whose row keeps none.
            let task = match job {
                crate::store::Task::None if submit.kind == ::api::calcifer::Kind::Warm => crate::store::Task::Warm,
                job => job,
            };
            let submitted = match calcifer.submit_under(key, &digest, submit, task).await.map_err(refused) {
                // The machine placement chose went as it was sent (a pod releasing itself that second, cl-920): placed again, once.
                Err(e) if ask.on.len() != 1 && crate::project::GONE.contains(&e.code.as_str()) => {
                    let Prepared::Run { submit, .. } = prepare(api, key, &ask, For::Run).await? else { return Err(e) };
                    offer = None;
                    calcifer.submit_under(key, &digest, submit, task).await.map_err(refused)?
                }
                submitted => submitted?,
            };
            if let Some(offer) = offer.filter(|_| !submitted.replay) {
                calcifer.quoted(&submitted.run.id, offer.quoted());
            }
            if let Some(project) = project.filter(|_| !submitted.replay) {
                // What Calcifer holds is in this computer's store.
                let uploads = crate::project::private_upload_root(api,&project.to_string_lossy()).ok().flatten().map(|root|root.join(".vault-uploads"));
                if let Some(uploads) = uploads {
                    // Even if this request is canceled, the queued worker owns
                    // the lease until its retention work has finished.
                    let access=crate::project::vault::admit_root(&project).await?;
                    let (kept, _access) = tokio::task::spawn_blocking(move || {
                        // Held by this blocking task now: its own reads of the project (a tool it starts) are granted beside it.
                        let access = access.here();
                        (crate::project::keep_given(&project, &given, &uploads), access)
                    }).await.map_err(|e| Error::internal("keep_given", e.to_string()))?;
                    notes.extend(kept?.into_iter().map(|message| Note { level: NoteLevel::Warning, message }));
                } else {
                    notes.push(Note { level: NoteLevel::Warning, message: "The run started, but Cozy couldn't add its private inputs to the project's history because the project was closing.".into() });
                }
            }
            Ok(Submitted { notes, lock, ..submitted })
        }
    }
}

/// The lock Calcifer made for the request's local code as `prepare` locked it, in its project's view: for the client to write
/// into the package's folder (cl-871).
async fn made_lock(api: &Arc<Api>, ask: &RunRequest, code: &LocalCode) -> Option<String> {
    let scope = match ask.project.as_deref() {
        Some(project) => crate::private_io::of_output(&crate::project::landing(&crate::project::named(api, project).ok()?)).ok()?,
        None => None,
    };
    let origin = api::config::Origin::parse(&hub(api, &ask.hub)).ok()?;
    crate::private_io::context(scope, async { crate::local_code::made(api.machines.hubs.home(), code, &origin) }).await
}

/// The files a submission's call is given, with their hashes: not a folder's manifest or its members, nor local code's own.
fn given(submit: &SubmitRun) -> Vec<(PathBuf, String)> {
    let staged = |digest: &str| submit.inputs.iter().find(|input| input.digest.to_string() == digest).map(|input| input.path.clone());
    let files = submit.spec.inputs.iter().filter(|input| input.media_type != api::tree::MEDIA);
    files.filter_map(|input| Some((staged(&input.digest)?, input.digest.clone()))).collect()
}

/// What a run was asked, from the spec it was sent: its payload, the models and LoRAs it chose as `input`'s `models` takes them,
/// and its seed. None for what no one asked with a payload.
pub(crate) fn asked(row: &crate::store::RunRow) -> Option<api::calcifer::RunAsked> {
    let mut input: serde_json::Map<String, serde_json::Value> = serde_json::from_str(row.spec.payload.as_ref()?.get()).ok()?;
    let seed = input.get("seed").and_then(serde_json::Value::as_i64);
    let reference = |source: &str, model: &str, manifest: &str, release: &str| match (source, manifest, release) {
        (source, ..) if !source.is_empty() => source.to_owned(),
        (_, manifest, _) if !manifest.is_empty() => format!("{model}@{manifest}"),
        (_, _, release) if !release.is_empty() => format!("{model}@{release}"),
        _ => model.to_owned(),
    };
    let models = row.spec.models.iter().map(|choice| {
        let lora: Vec<_> = (choice.adapters.iter())
            .map(|a| serde_json::json!({"ref": reference(&a.source, &a.model, &a.manifest, &a.release), "strength": a.scale.parse::<f64>().unwrap_or(1.0)}))
            .collect();
        let mut chosen = serde_json::json!({});
        if !lora.is_empty() {
            chosen["lora"] = lora.into();
        }
        if !choice.model.is_empty() || !choice.source.is_empty() {
            chosen["ref"] = reference(&choice.source, &choice.model, &choice.manifest, &choice.release).into();
        }
        (choice.parameter.clone(), chosen)
    });
    let models: serde_json::Map<_, _> = models.collect();
    if !models.is_empty() {
        input.insert("models".into(), models.into());
    }
    Some(api::calcifer::RunAsked { target: row.target.clone(), input, seed })
}

/// The ask's identity: what it asks, whatever price it was quoted since.
pub(crate) fn digest(ask: &RunRequest) -> String {
    let asked = RunRequest { agreed: None, ..ask.clone() };
    format!("sha256:{}", hex::encode(Sha256::digest(serde_json::to_vec(&asked).expect("serializes"))))
}

enum Prepared {
    /// The submission, what binding it said, and its machines' offers where they were asked, for each copy quoted.
    Run { submit: SubmitRun, notes: Vec<Note>, offers: Vec<Vec<crate::placement::Offer>> },
    /// Local code's install, still running.
    Install(api::calcifer::Run),
    /// cozy's own update for the release the run takes (cl-978), and the line it says.
    Cozy(api::calcifer::Run, Note),
}

/// What a client following `run` before its ask goes again is told, when it is its machine's update to this cozy's minor.
async fn updating(api: &Arc<Api>, run: &api::calcifer::Run, project: bool) -> Option<Note> {
    let machine = run.machine.as_deref().filter(|_| run.kind == RunKind::Update)?;
    let own = crate::update::this_release();
    // `local` on this cozy's TensorD below its Runtime minor moves its Runtime alone (cl-1254): said as a project's move says it (cl-1055).
    if let Some(frame) = api.machines.frame(machine).filter(|frame| crate::update::same_minor(&frame.version, own))
        && let Ok(Some(to)) = api.machines.own_runtime(machine, &frame).await
    {
        let release = match project {
            true => "this project".to_owned(),
            false => format!("cozy {own}"),
        };
        return Some(crate::floor::Upgrade { machine: machine.to_owned(), from: frame.runtime, to, release }.note());
    }
    Some(Note { level: NoteLevel::Info, message: format!("Updating {machine} to cozy {own} first.") })
}

/// What a prepared ask is for.
#[derive(Clone, Copy, PartialEq)]
enum For {
    Run,
    /// A `new` run's price: nothing installs.
    Quote,
    /// Its machines' estimates, for this many copies sent at once: every machine it may go to is asked, and nothing installs.
    Estimate(u32),
}

async fn project_access(api: &Api, project: Option<&str>) -> Result<Option<crate::project::vault::Read>, Error> {
    match project {
        Some(named) => crate::project::vault::admit_project(api, named).await.map(Some),
        None => Ok(None),
    }
}

/// Resolves and binds `ask` into the submission Calcifer records, on the machine of its set that would finish it soonest (cl-816).
async fn prepare(api: &Arc<Api>, key: &str, ask: &RunRequest, purpose: For) -> Result<Prepared, Error> {
    let scope = match ask.project.as_deref() {
        Some(project) => {
            let root = crate::project::named(api, project)?;
            crate::private_io::of_output(&crate::project::landing(&root))?
        },
        None => None,
    };
    crate::private_io::context(scope, Box::pin(prepare_scoped(api, key, ask, purpose))).await
}

async fn prepare_scoped(api: &Arc<Api>, key: &str, ask: &RunRequest, purpose: For) -> Result<Prepared, Error> {
    // Resolved first on the first of its set.
    let set = run_set(api, ask).await?;
    let set = if let Some(named) = ask.project.as_deref() {
        let (project, root) = {
            let store = api.projects.store(api, named).await?;
            store.admit_new_work().map_err(|error|refused(error.into()))?;
            let (project, root) = store.execution_scope();
            (project.to_owned(), root.to_owned())
        };
        crate::private_io::check_project_placement(&project, &root, &set)?
    } else { set };
    let mut on = set.first().cloned().unwrap_or_else(|| default_on(api));
    // A machine on an older minor this computer may move is updated first, which the client follows, then asks again (cl-1069). Before
    // anything is resolved there, local code's install too. Of several, placement does this for each.
    if let (On::Machine(machine), For::Run, 1) = (&on, purpose, set.len()) {
        // Not known yet: its driver asks again before it sends.
        match api.machines.current(&machine).await {
            Ok(Some(update)) => return Ok(Prepared::Install(update)),
            // Nothing published it may move to (cl-1254): refused here, in that line.
            Err(e) if crate::machines::Machines::unpublished(&e) => return Err(e),
            _ => {}
        }
    }
    // Local code installs as it resolves. A package answers for itself first: one named without a
    // function lists its functions on an OS that runs no models here.
    if ask.code.is_some() || crate::checkout::named(&ask.target).is_some() {
        api::calcifer::local_usable(&on.to_string())?;
        // Refused before that install, not after it.
        project_of(api, ask)?;
    }
    let hub = hub(api, &ask.hub);
    let agreed = ask.agreed.clone();
    // Given something to run with, a package of one function runs it; given nothing, it is listed.
    let given = !ask.terms.is_empty() || ask.input.is_some() || ask.input_text.is_some() || !ask.files.is_empty() || ask.warm.is_some();
    // A run resolves on its machine; a quote or an estimate only reads, installing, starting, moving or updating nothing (cl-1115).
    let resolved = match purpose {
        For::Run => Box::pin(target::resolve(api, &hub, &ask.target, ask.code.as_ref(), &on, ask.warm.is_some(), given, key, agreed)).await?,
        For::Quote | For::Estimate(_) => Box::pin(target::foreseen(api, &hub, &ask.target, ask.code.as_ref(), &on)).await?,
    };
    // cozy updates itself first for a release only a newer cozy's line runs (cl-978); a quote or an estimate says so.
    let cozy = match &resolved {
        Resolved::Release { cozy: Some(wanted), .. } if purpose == For::Run => {
            let (run, note) = crate::cozy::update(api, wanted).await?;
            return Ok(Prepared::Cozy(run, note));
        }
        Resolved::Release { cozy: Some(wanted), .. } => Some(Note { level: NoteLevel::Info, message: wanted.said() }),
        _ => None,
    };
    let (package, function, code) = match &resolved {
        Resolved::Release { package, function, card, .. } => {
            let code = Code {
                package,
                interface: &card.interface,
                summary: card.summary(),
                release: Some(&card.release),
                source: spec::Source::Release(Some(&card.release), card.digest().unwrap_or_default()),
                files: &[],
                on: on.clone(),
            };
            (package.as_str(), function.as_deref(), code)
        }
        Resolved::Local { local, interface, function } => {
            let Some(interface) = interface else {
                let install = local.install.clone().expect("an uninstalled local target is being installed");
                return Ok(Prepared::Install(install));
            };
            let code = Code {
                package: &local.package,
                interface,
                summary: local.from.as_ref().map_or("", |from| from.summary.as_str()),
                release: None,
                source: spec::Source::Local { source: &local.source, installation: local.installation.as_deref() },
                files: &local.files,
                on: local.machine.clone().map_or_else(|| on.clone(), On::Machine),
            };
            (local.package.as_str(), function.as_deref(), code)
        }
    };
    // Refused or listed before anything starts or installs.
    let Some(function) = function.or(target::only_function(code.interface).filter(|_| given)) else {
        let release = match &resolved {
            Resolved::Release { release: Some(release), .. } => Some(release.as_str()),
            _ => None,
        };
        return Err(unnamed(package, release, code.interface, ask));
    };
    let callable = callable(package, code.interface, function)?;
    if !ask.recipes && code.interface.runs_recipes(callable) {
        return Err(recipes(package, code.interface));
    }
    if set.len() == 1 {
        api::calcifer::local_usable(&on.to_string())?;
    }
    let (gpu, needs_gpu) = (code.interface.gpu(callable), code.interface.needs_gpu(callable));
    let named = format!("{package}/{}", callable.name);
    let (interface, release, summary) = (code.interface, code.release, code.summary);
    let (mut submit, mut notes) = Box::pin(call(api, code, callable, ask, key, &hub)).await?;
    // The price its renter agreed to (`POST /v1/run-quotes`), which a `new` run is admitted on: dropped here since cl-716 B, so every
    // `--on new` that would rent was refused `quote_unagreed` (cl-1066).
    submit.agreed = ask.agreed.clone();
    if let Resolved::Local { local, .. } = &resolved
        && let Some(from) = &local.from
    {
        submit.warnings.push(Warning { code: api::calcifer::FROM_SOURCE.into(), message: from.line() });
    }
    // Placed by its machines' quotes, the payload bound: a published release among several, and every machine when asked for them.
    let input: Option<serde_json::Value> = submit.spec.payload.as_ref().and_then(|payload| serde_json::from_str(payload.get()).ok());
    // Its size and cost as its function's estimate reads the payload (cl-1286): its own variables, else the guess.
    let cost = input.as_ref().map(|input| std::sync::Arc::new(::needs::Cost::declared(&callable.request, &callable.estimate, input)));
    let size = cost.as_ref().map(|cost| cost.size.clone()).unwrap_or_default();
    let gpus = ask.gpus.unwrap_or(0);
    let (estimates, count) = match purpose {
        For::Estimate(count) => (true, count),
        _ => (false, 1),
    };
    let mut offers = vec![];
    // A machine the run names that has no GPU, where nothing measured says it runs without one: it goes there, warned, as asked (the
    // release session, 11-18: fix, don't refuse); the refusal is placement's, for a run naming none.
    let mut named_gpuless = false;
    // Placed among machines here rather than named: one of several, or a `new` run a machine up or on its way finishes sooner.
    let mut placed = set.len() > 1;
    // One machine is quoted too, for its disk's room (cl-1175): a run whose weights it has no room for never starts to download them.
    let one = purpose == For::Run && matches!(on, On::Machine(_));
    match &resolved {
        Resolved::Release { package, card, .. } if set.len() > 1 || estimates || one => {
            let (chosen, offered, said) = crate::project::run_on(api, &set, &hub, package, &card.release, &callable.name, input.as_ref(), &submit.spec.models, size, gpus, needs_gpu, count, !estimates).await;
            named_gpuless = !ask.on.is_empty() && matches!(&chosen, Err(e) if e.code == "machine_no_gpu");
            // Why a machine quoted nothing is an estimate's answer; a run goes to the one that finishes soonest, and says nothing of the others.
            if estimates {
                notes.extend(said.into_iter().map(|message| Note { level: NoteLevel::Info, message }));
            }
            // None of them can run it (no room for its weights; no GPU, and nothing measured says it runs without one): refused before
            // anything downloads, with the rental its asker may choose instead.
            if let Err(e) = &chosen
                && matches!(e.code.as_str(), "machine_disk_full" | "machine_no_gpu")
                && !estimates
                && !named_gpuless
            {
                return Err(e.clone().next(format!("cozy run {} … --on new", ask.target)));
            }
            if set.len() > 1 {
                on = match chosen {
                    Ok((chosen, fell_back)) => {
                        // None of those named could take it: where a run naming none goes, which isn't what was asked.
                        notes.extend(fell_back.map(|message| Note { level: NoteLevel::Warning, message }));
                        chosen
                    }
                    // An estimate shows each machine's refusal; a run nothing can take is refused, naming them.
                    Err(_) if estimates || named_gpuless => on,
                    Err(e) => return Err(e),
                };
                // Held for a machine on its way that finishes it sooner, it says so (cl-915).
                let now = crate::store::now_ms();
                let waits = offered.first().and_then(|offers| offers.iter().find(|offer| offer.machine == on)).and_then(|offer| offer.waits_for(now));
                notes.extend(waits.map(|message| Note { level: NoteLevel::Info, message: format!("{named}: {message}") }));
                // Gone elsewhere for a disk without room for it, said in a line.
                let roomless = offered.first().and_then(|offers| crate::placement::roomless(offers)).filter(|short| short.machine != on && !estimates);
                notes.extend(roomless.map(|short| Note { level: NoteLevel::Info, message: crate::placement::elsewhere_line(short, &on, "run") }));
            }
            offers = offered;
        }
        Resolved::Release { package, card, .. } if matches!(on, On::New(None)) => {
            let run = Weighed { hub: &hub, package, release: &card.release, function: &callable.name, spec: &submit.spec, size, gpus, gpu: needs_gpu };
            if let Some((machine, said, offered)) = instead_of_renting(api, run).await {
                // Not what was asked: said as a warning, which every client shows.
                notes.push(Note { level: NoteLevel::Warning, message: format!("{named}: {said}") });
                (on, offers, placed) = (machine, offered, true);
            }
        }
        Resolved::Local { local, .. } if estimates => {
            let function = format!("{}/{}", local.source.manifest, callable.name);
            let jobs: Vec<crate::placement::Job> = (0..count)
                .map(|copy| crate::placement::Job { key: copy.to_string(), target: named.clone(), function: function.clone(), size: size.clone(), gpus, gpu: needs_gpu, on: set.clone(), cost: cost.clone(), ..Default::default() })
                .collect();
            let (offered, said) = crate::placement::offers(api, &jobs, false).await;
            notes.extend(said.into_iter().map(|message| Note { level: NoteLevel::Warning, message }));
            offers = offered;
        }
        _ => {}
    }
    if let Resolved::Release { .. } = &resolved {
        submit.on = on.clone();
    }
    api::calcifer::local_usable(&submit.on.to_string())?;
    // What its machines quoted placed it by what says it runs there (cl-1279); local code, which none quotes, and a machine named
    // for it, go where named, warned.
    if (offers.is_empty() || named_gpuless)
        && let Some(warning) = gpuless(api, &submit.on, gpu, &named).await.filter(|_| ask.warm.is_none())
    {
        notes.push(Note { level: NoteLevel::Warning, message: warning });
    }
    // A GPU this computer would run it poorly on, said as a render's box says it, never asked (cl-897).
    if submit.on.is_local() && gpu && ask.warm.is_none() {
        let input: serde_json::Map<String, serde_json::Value> = submit.spec.payload.as_ref().and_then(|payload| serde_json::from_str(payload.get()).ok()).unwrap_or_default();
        if let Some(line) = crate::project::fit::here_now(api, &hub, package, release, &callable.name, interface, summary, &input) {
            notes.push(Note { level: NoteLevel::Warning, message: format!("{line} Rent a GPU to run it faster.") });
        }
    }
    // The machine placement chose, likewise.
    if let (On::Machine(machine), For::Run, true) = (&submit.on, purpose, placed)
        && let Ok(Some(update)) = api.machines.current(machine).await
    {
        return Ok(Prepared::Install(update));
    }
    if let Resolved::Release { moved: Some(moved), .. } = &resolved {
        notes.push(moved.clone());
    }
    notes.extend(cozy);
    // Its machine's Runtime moves first to one that runs the release (cl-952): the run waits behind that update.
    if let (Resolved::Release { upgrade: Some(upgrade), .. }, For::Run) = (&resolved, purpose)
        && matches!(&submit.on, On::Machine(name) if *name == upgrade.machine)
        && api.machines.upgrade(upgrade).await?.is_some()
    {
        notes.push(upgrade.note());
    }
    Ok(Prepared::Run { submit, notes, offers })
}

/// A published function's run asked `--on new`, as placement weighs it.
struct Weighed<'a> {
    hub: &'a str,
    package: &'a str,
    release: &'a str,
    function: &'a str,
    spec: &'a v1::RunSpec,
    size: v1::JobSize,
    gpus: u32,
    gpu: bool,
}

/// Where a `new` run goes instead of a rental (cl-915, Paul 11-12): the soonest finish of the account's rentals up or on their way
/// and the computers added here, when it is sooner than a new rental's boot, preparation and run on the card its Hub would place it on;
/// what it says of that, and the offers. None: it rents. This computer's machine isn't weighed: a run asked to rent wants a rented GPU.
async fn instead_of_renting(api: &Arc<Api>, run: Weighed<'_>) -> Option<(On, String, Vec<Vec<crate::placement::Offer>>)> {
    let list = api.machines.list(false).await.ok()?;
    let set: Vec<On> = list.render_set().into_iter().filter(|on| !on.is_local()).collect();
    if set.is_empty() {
        return None;
    }
    let input: Option<serde_json::Value> = run.spec.payload.as_ref().and_then(|payload| serde_json::from_str(payload.get()).ok());
    let (chosen, offers, _) = crate::project::run_on(api, &set, run.hub, run.package, run.release, run.function, input.as_ref(), &run.spec.models, run.size.clone(), run.gpus, run.gpu, 1, false).await;
    let (chosen, _) = chosen.ok()?;
    let offer = offers.first()?.iter().find(|offer| offer.machine == chosen && offer.fits)?.clone();
    let rental = api.machines.new_rental(run.hub, run.spec).await.ok()?;
    let way = crate::rentals::OnItsWay { ready_at_ms: 0, card: rental.card.clone(), gpus: rental.gpus };
    // Its time on that card: this computer's runs there, else its release's benchmarks (cl-1024), else a minute.
    let target = format!("{}/{}@{}", run.package, run.function, run.release);
    let one_ms = api.calcifer.typical_ms(&target, &way.card, &run.size).map_or(api::estimate::UNKNOWN_MS, |(ms, _)| ms);
    let prepare_ms = api::estimate::prepare_ms(false, rental.weight_bytes, api::estimate::LINK, 0.0);
    let now = crate::store::now_ms();
    let new_finish = now + rental.boot_ms + (prepare_ms + one_ms) as i64;
    if offer.finish_at_ms >= new_finish {
        return None;
    }
    let (here, there) = (crate::placement::within(offer.finish_at_ms - now), crate::placement::within(new_finish - now));
    let card = rental.card.to_uppercase();
    let said = match offer.waits_for(now) {
        Some(waits) => format!("{chosen} is on its way and would finish it {here}, sooner than a new {card} ({there}). The run {waits}, and nothing new is rented."),
        None => format!("{chosen} would finish it {here}, sooner than a new {card} ({there}), so the run goes there and nothing new is rented."),
    };
    Some((chosen, said, offers))
}

/// Where a run goes (cl-873): the machines it names, else `unnamed_set`.
async fn run_set(api: &Arc<Api>, ask: &RunRequest) -> Result<Vec<On>, Error> {
    api::calcifer::machine_set(&ask.on)?;
    match ask.on.is_empty() {
        true => unnamed_set(api, ask.project.as_deref()).await,
        false => Ok(ask.on.clone()),
    }
}

/// Where a run or a project's render that names no machine goes, for the CLI and the page alike (cl-894): its project's machines,
/// else where a render's job would (Paul, 11-06; `MachineList::render_set`), this computer's `local` stopped or not: it starts to
/// quote, so its start counts in its finish.
pub(crate) async fn unnamed_set(api: &Arc<Api>, project: Option<&str>) -> Result<Vec<On>, Error> {
    if let Some(project) = project {
        let pinned = crate::project::machines_of(api, project)?;
        if !pinned.is_empty() {
            return Ok(pinned);
        }
    }
    Ok(api.machines.list(false).await.map_or_else(|_| vec![On::Machine("local".into())], |list| list.render_set()))
}

/// Where a run that names no machine is resolved and described: this computer's, on an OS that runs models itself; on another, a
/// machine this computer has when it has one, else `local`, refused with the way forward. A run itself goes where a render's job would.
pub(crate) fn default_on(api: &Api) -> On {
    let local = On::Machine("local".into());
    if platform::Os::CURRENT.local_inference() {
        return local;
    }
    let machines = api.calcifer.store().live_machines().unwrap_or_default();
    match machines.iter().any(|row| row.kind != MachineKind::Local && row.state != MachineState::Ended) {
        true => On::Any,
        false => local,
    }
}

/// The request's Hub, else Calcifer's.
pub(crate) fn hub(api: &Api, asked: &str) -> String {
    match asked.is_empty() {
        true => api.machines.hubs.default_origin(),
        false => asked.to_owned(),
    }
}

/// What a run calls: a published release, or local code its machine installed.
struct Code<'a> {
    package: &'a str,
    interface: &'a Interface,
    /// What its package does in a line (its card's, or its checkout's), which spells its name (`tool::title`).
    summary: &'a str,
    /// The release its inputs bind against; none for local code.
    release: Option<&'a str>,
    source: spec::Source<'a>,
    /// Files sent with the call (local code's).
    files: &'a [StagedInput],
    /// Where it goes.
    on: On,
}

/// The project `ask` belongs to. A person's run has one, as a commit has a repository; keeping a function warm and `cozy model quantize` are no one's run.
fn project_of(api: &Api, ask: &RunRequest) -> Result<Option<PathBuf>, Error> {
    let project = match ask.project.as_deref() {
        Some(named) => Some(crate::project::named(api, named)?),
        None => None,
    };
    if project.is_none() && ask.warm.is_none() && !ask.recipes {
        return Err(api::calcifer::project::unnamed());
    }
    Ok(project)
}

/// Binds the ask's terms to `callable` of `code` (a warm run, its model choices only) into the
/// submission, and what binding said.
async fn call(
    api: &Arc<Api>,
    code: Code<'_>,
    callable: &Callable,
    ask: &RunRequest,
    key: &str,
    hub: &str,
) -> Result<(SubmitRun, Vec<Note>), Error> {
    if let Some(gpus) = ask.gpus {
        api::calcifer::gpu_count(&gpus.to_string()).map_err(|why| Error::new(Kind::Usage, "gpus_invalid", why))?;
    }
    if let Some(pin) = &ask.attention_kernel {
        api::calcifer::attention_pin(pin).map_err(|why| Error::new(Kind::Usage, "attention_kernel_invalid", why))?;
    }
    let home = api.machines.hubs.home().root().to_path_buf();
    let mut notes = vec![];
    let project = project_of(api, ask)?;
    let scope = crate::private_io::current();
    let digests = crate::private_io::cache(scope.as_ref(), home.join("cache").join("digests"), "digests")?;
    let clips = crate::private_io::cache(scope.as_ref(), home.join("cache").join("clips"), "clips")?;
    let terms = Terms {
        input: ask.input.clone(),
        inputs: ask.terms.clone(),
        input_text: ask.input_text.clone(),
        assets: ask.files.clone(),
        fidelities: ask.fidelities.clone(),
        loras: ask.loras.clone(),
        key: key.to_owned(),
        paths: ask.paths.clone(),
        digests: Some(digests),
        clips: clip::Clips { dir: clips, home: api.machines.hubs.home().clone() },
        project: project.clone().map(|dir| crate::project::Holding::new(api.calcifer.clone(), dir)),
    };
    for term in &terms.fidelities {
        api::calcifer::fidelity(term).map_err(|why| Error::new(Kind::Usage, "file_fidelity_invalid", why))?;
    }
    let call = spec::Call {
        package: code.package,
        source: code.source,
        callable,
        gpus: ask.gpus.unwrap_or(0),
        lane: ask.lane.clone().unwrap_or_default(),
        attention_kernel: ask.attention_kernel.as_deref().unwrap_or_default(),
        attention_capture: ask.attention_capture.as_deref().unwrap_or_default(),
    };
    let target = format!("{}/{}", code.package, callable.name);
    let (spec, bound, out) = match ask.warm {
        Some(level) => {
            if ask.gpus.is_some() {
                notes.push(Note { level: NoteLevel::Warning, message: "The GPU count only applies to a run, so Cozy ignored it while warming up.".into() });
            }
            let (models, payload) = bind::warm_choices(
                &target, code.release, code.interface, callable, &terms, level == WarmLevel::Gpu, &mut notes,
            )
            .await?;
            (call.warm(level, &models, payload)?, None, None)
        }
        None => {
            let mut bound = bind::bind(&target, code.release, code.summary, code.interface, callable, &terms).await?;
            notes.append(&mut bound.notes);
            notes.extend(bound.cut.iter().map(|cut| Note { level: NoteLevel::Warning, message: cut.clone() }));
            if let Some(warning) = bound.warning(&target) {
                notes.push(Note { level: NoteLevel::Warning, message: warning });
            }
            let out = project.as_deref().map(crate::project::landing);
            (call.spec(&bound)?, Some(bound), out)
        }
    };
    // Even a prepare-only private local-code call needs protected input/journal scope.
    let out = out.or_else(|| project.as_deref().filter(|project| crate::project::vault::encrypted(&crate::project::vault::logical(project))).map(crate::project::landing));
    let mut submission = call.submission(code.on, hub, &spec, bound.as_ref(), out);
    if let Some(named) = ask.project.as_deref() {
        let store = api.projects.store(api, named).await?;
        submission.project = Some(store.project_id().to_owned());
        submission.out = project.as_deref().map(crate::project::landing);
    }
    submission.inputs.extend(code.files.iter().cloned());
    Ok((submission, notes))
}

/// Warns that a GPU function sent to a ready machine with no supported GPU (an NVIDIA GPU, or an
/// Apple Silicon Mac's own) runs on its CPU, slowly, or is refused there, and how to rent one.
async fn gpuless(api: &Api, on: &On, gpu: bool, target: &str) -> Option<String> {
    let On::Machine(name) = on else { return None };
    if !gpu {
        return None;
    }
    // What its open Status stream last said, else asked now (cl-1172: a round trip a GPU run).
    let gpuless = match api.machines.named(name).ok().flatten().and_then(|row| api.machines.live_frame(&row.name)) {
        Some(frame) => frame.phase == "ready" && frame.gpus.is_empty(),
        None => api.machines.show(name).await.ok()?.status.is_some_and(|s| s.phase == "ready" && s.gpus.is_empty()),
    };
    gpuless.then(|| {
        format!(
            "{name} has no supported GPU, so {target} runs on its CPU: slowly, or not at all if it loads a model. Rent a GPU to run it properly."
        )
    })
}

/// The function `asked` names: exact, else the one name equal with `-` as `_`.
pub(crate) fn callable<'a>(package: &str, interface: &'a Interface, asked: &str) -> Result<&'a Callable, Error> {
    let fold = |name: &str| name.replace('-', "_").to_ascii_lowercase();
    let found = interface.callables.iter().find(|c| c.name == asked).or_else(|| {
        let mut matches = interface.callables.iter().filter(|c| fold(&c.name) == fold(asked));
        matches.next().filter(|_| matches.next().is_none())
    });
    match found {
        Some(callable) if callable.internal => Err(Error::new(
            Kind::NotFound,
            "callable_internal",
            format!("{package}/{} is an internal function.", callable.name),
        )
        .remedy("Run one of the package's public functions.")),
        Some(callable) => Ok(callable),
        None => {
            if let Some((name, why)) = interface.unavailable.iter().find(|(name, _)| fold(name) == fold(asked)) {
                return Err(Error::new(
                    Kind::Unavailable,
                    "callable_unreadable",
                    format!("This cozy can't read {package}/{name}: {why}."),
                )
                .remedy("Update cozy, or run another function of the package.")
                .next("cozy update"));
            }
            let public = interface.public();
            let mut error =
                Error::new(Kind::NotFound, "callable_unknown", format!("{package} has no function {asked:?}."))
                    .remedy(format!("Its functions are {}.", public.join(", ")));
            for name in public.iter().take(2) {
                error = error.next(format!("cozy run {package}/{name}"));
            }
            Err(error)
        }
    }
}

/// A package's recipes are data, not functions: `cozy model quantize` runs them.
pub(crate) fn recipes(package: &str, interface: &Interface) -> Error {
    let flags: Vec<String> = (interface.recipes.iter().map(|r| r.flag()))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut error = Error::new(
        Kind::Usage,
        "callable_recipe",
        format!("{package}/{} runs the package's recipes, which run through model quantize.", api::interface::RECIPES),
    )
    .remedy(format!("Its recipes are {}.", flags.join(", ")));
    for flag in flags.iter().take(2) {
        error = error.next(format!("cozy model quantize <model> {flag} --package {package}"));
    }
    error
}

/// A request that names no function: with inputs, the commands that run each of the package's
/// first two functions with them; without, its functions (`GET /v1/targets` lists them).
fn unnamed(package: &str, release: Option<&str>, interface: &Interface, ask: &RunRequest) -> Error {
    let public = interface.public();
    let mut error = Error::new(Kind::Usage, "function_unnamed", format!("Pick which function of {package} to run."))
        .remedy(format!("Its functions are {}.", public.join(", ")));
    let inputs: String = ask.terms.iter().map(|i| format!(" {}", quoted(i))).collect();
    let release = release.map(|r| format!("@{r}")).unwrap_or_default();
    for name in public.iter().take(2) {
        error = error.next(format!("cozy run {package}/{name}{release}{inputs}"));
    }
    error
}

/// `input` as a shell word: a value with spaces or quotes is single-quoted after its `key=`.
fn quoted(input: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_-./:=@,+%".contains(c);
    if input.chars().all(plain) {
        return input.to_owned();
    }
    let (key, value) = match input.split_once('=') {
        Some((key, value)) if key.chars().all(plain) => (format!("{key}="), value),
        _ => (String::new(), input),
    };
    format!("{key}'{}'", value.replace('\'', r"'\''"))
}

fn store(e: crate::store::StoreError) -> Error {
    refused(crate::Error::Store(e))
}

pub(crate) fn refused(e: crate::Error) -> Error {
    Refusal::from(e).0
}

/// The kind a submission's spec runs.
pub(crate) fn kind(spec: &v1::RunSpec) -> RunKind {
    match spec.kind {
        v1::RunKind::Warm => RunKind::Warm,
        _ => RunKind::Call,
    }
}

