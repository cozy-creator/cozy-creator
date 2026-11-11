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
    calcifer::{Kind as RunKind, MachineEstimate, MachineKind, MachineState, Note, NoteLevel, On, RunEstimate, RunQuote, RunRequest, StagedInput, SubmitRun, Submitted},
    code::LocalCode,
    error::{Error, Kind},
    interface::{Callable, Interface},
    machine::{self as v1, WarmLevel},
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tokio::sync::OwnedRwLockReadGuard;

use crate::{api::{Api, IdempotencyKey, Refusal, parse}, project::vault};

pub(crate) mod bind;
pub(crate) mod clip;
pub(crate) mod files;
pub(crate) mod report;
pub(crate) mod spec;
pub(crate) mod target;

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
    let project = ask.project.clone();
    vault::project_reply(&api, project.as_deref(), async {
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
        Ok(Json(priced(&api, ask).await.map_err(Refusal)?))
    }).await
}

/// What `ask` would rent.
pub(crate) async fn priced(api: &Arc<Api>, ask: RunRequest) -> Result<Option<RunQuote>, Error> {
    let _access = project_access(api, ask.project.as_deref()).await?;
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
    if crate::hub::jobs::is(&ask.target) {
        return crate::hub::jobs::quote(api, ask).await;
    }
    let bound = match &ask.code {
        // Local code rents with its install, which the call then follows to its machine.
        Some(code) => {
            let on = ask.on.clone().unwrap_or_else(|| default_on(api));
            let hub = hub(api, &ask.hub);
            let home = api.machines.hubs.home();
            let origin = api::config::Origin::parse(&hub)?;
            let source = crate::local_code::source(home, code, &api.machines.hubs, &origin).await?;
            crate::local_code::install_run(home, &hub, on, &source)
        }
        None => match prepare(api, "quote", &ask, For::Quote).await? {
            Prepared::Run { submit, .. } => submit,
            Prepared::Install(_) => unreachable!("a published target installs nothing first"),
        },
    };
    api.machines.hubs.client(&bound.hub)?;
    Ok(Some(api.machines.run_quote(&bound).await?))
}

/// `POST /v1/run-estimates` (`Idempotency-Key`; cl-816): when each machine the run may go to would finish it, as each says now.
/// Nothing is sent. Local code not yet ready answers its install (`preparing`): follow it, then ask again under the same key.
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
    let (submit, mut notes, offers) = match prepare(api, key, ask, For::Estimate(count)).await.map_err(Refusal)? {
        Prepared::Install(run) => {
            let notes = updating(&run).into_iter().collect();
            return Ok(Json(RunEstimate { preparing: Some(Box::new(run)), notes, ..RunEstimate::default() }));
        }
        Prepared::Run { submit, notes, offers } => (submit, notes, offers),
    };
    if copies.count.is_some_and(|asked| asked != count) {
        notes.push(Note { level: NoteLevel::Warning, message: format!("count is 1 to {COPIES}: {count} copies quoted") });
    }
    let now = crate::store::now_ms();
    let shown = offers.iter().enumerate().flat_map(|(job, offers)| offers.iter().map(move |offer| MachineEstimate { job: job as u32, ..offer.shown(now) }));
    let mut machines: Vec<MachineEstimate> = shown.collect();
    machines.sort_by_key(|machine| (machine.job, !machine.fits, machine.finish_ms));
    if machines.is_empty() {
        notes.push(Note { level: NoteLevel::Warning, message: format!("{} quotes nothing: only a machine that is up does", submit.on) });
    }
    // Where it would go, if that machine can run it at all.
    let soonest = match &submit.on {
        On::Machine(name) => machines.iter().any(|machine| machine.job == 0 && machine.fits && machine.machine == *name).then(|| name.clone()),
        _ => None,
    };
    Ok(Json(RunEstimate { machines, soonest, notes, preparing: None }))
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
    /// The machine a run would go to.
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
    let hub = hub(api, asked.hub.as_deref().unwrap_or_default());
    let on = asked.on.map_or_else(|| default_on(api), On::from);
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
    let version = env!("CARGO_PKG_VERSION");
    Err(Refusal(
        Error::new(
            Kind::Usage,
            "client_outdated",
            format!("this cozy sent a run its own way, as cozy before {version} did; Calcifer runs cozy {version}, which binds runs itself"),
        )
        .remedy(format!("run the installed cozy (~/.cozy/bin/cozy), or update this one to {version}")),
    ))
}

/// Records `ask` under `key` and starts it; the same key again answers the recorded run without
/// binding anything again, and another ask under it is a conflict. Local code not yet made ready
/// answers the run that makes it ready (`preparing`): its client follows that, then asks again.
/// The files it's given join its project, so asking it again still finds them once what held them is gone.
pub(crate) async fn request(api: &Arc<Api>, key: &str, ask: RunRequest) -> Result<Submitted, Error> {
    let digest = digest(&ask);
    if crate::hub::jobs::is(&ask.target) {
        return crate::hub::jobs::request(api, key, &digest, ask).await;
    }
    let calcifer = &api.calcifer;
    if let Some((row, recorded)) = calcifer.store().recorded(key).map_err(store)? {
        if recorded != digest {
            return Err(crate::api::conflict(key));
        }
        crate::project::vault::require_path(&row.out_dir)?;
        let run = calcifer.record(&row).map_err(refused)?;
        return Ok(Submitted { run, replay: true, notes: vec![], preparing: false, lock: None });
    }
    // Prepared inputs can name files in the mounted view. Keep the view
    // leased until submission takes them and their history is kept.
    let access = project_access(api, ask.project.as_deref()).await?;
    let prepared = prepare(api, key, &ask, For::Run).await?;
    let lock = match &ask.code {
        Some(code) => made_lock(api, &ask, code).await,
        None => None,
    };
    match prepared {
        Prepared::Install(run) => {
            let notes = updating(&run).into_iter().collect();
            Ok(Submitted { run, replay: false, notes, preparing: true, lock })
        }
        Prepared::Run { submit, mut notes, offers } => {
            let given = given(&submit);
            let project = submit.out.as_deref().and_then(crate::project::of_landing).map(Path::to_owned);
            let offer = offers.into_iter().next().unwrap_or_default().into_iter().find(|offer| offer.machine == submit.on);
            let submitted = calcifer.submit_under(key, &digest, submit).await.map_err(refused)?;
            if let Some(offer) = offer.filter(|_| !submitted.replay) {
                calcifer.quoted(&submitted.run.id, offer.quoted());
            }
            if let Some(project) = project.filter(|_| !submitted.replay) {
                // What Calcifer holds is in this computer's store.
                let uploads = match crate::private_io::of_output(&crate::project::landing(&project)) {
                    Ok(Some(scope)) => Some(scope.storage.view_root.join(".vault-uploads")),
                    Ok(None) => Some(files::store().map(|store| store.root().to_owned()).unwrap_or_default()),
                    Err(_) => None,
                };
                if let Some(uploads) = uploads {
                    // Even if this request is canceled, the queued worker owns
                    // the lease until its retention work has finished.
                    let (kept, _access) = tokio::task::spawn_blocking(move || {
                        (crate::project::keep_given(&project, &given, &uploads), access)
                    }).await.map_err(|e| Error::internal("keep_given", e.to_string()))?;
                    notes.extend(kept?.into_iter().map(|message| Note { level: NoteLevel::Warning, message }));
                } else {
                    notes.push(Note { level: NoteLevel::Warning, message: "the run was accepted, but its private inputs could not be added to project history while the view was closing".into() });
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
    let reference = |source: &str, repository: &str, manifest: &str, release: &str| match (source, manifest, release) {
        (source, ..) if !source.is_empty() => source.to_owned(),
        (_, manifest, _) if !manifest.is_empty() => format!("{repository}@{manifest}"),
        (_, _, release) if !release.is_empty() => format!("{repository}@{release}"),
        _ => repository.to_owned(),
    };
    let models = row.spec.models.iter().map(|choice| {
        let lora: Vec<_> = (choice.adapters.iter())
            .map(|a| serde_json::json!({"ref": reference(&a.source, &a.model, &a.manifest, &a.release), "strength": a.scale.parse::<f64>().unwrap_or(1.0)}))
            .collect();
        let mut chosen = serde_json::json!({});
        if !lora.is_empty() {
            chosen["lora"] = lora.into();
        }
        if !choice.repository.is_empty() || !choice.source.is_empty() {
            chosen["ref"] = reference(&choice.source, &choice.repository, &choice.manifest, &choice.release).into();
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
fn digest(ask: &RunRequest) -> String {
    let asked = RunRequest { agreed: None, ..ask.clone() };
    format!("sha256:{}", hex::encode(Sha256::digest(serde_json::to_vec(&asked).expect("serializes"))))
}

enum Prepared {
    /// The submission, what binding it said, and its machines' offers where they were asked, for each copy quoted.
    Run { submit: SubmitRun, notes: Vec<Note>, offers: Vec<Vec<crate::placement::Offer>> },
    /// Local code's install, still running.
    Install(api::calcifer::Run),
}

/// What a client following `run` before its ask goes again is told, when it is its machine's update to this cozy's minor.
fn updating(run: &api::calcifer::Run) -> Option<Note> {
    let machine = run.machine.as_deref().filter(|_| run.kind == RunKind::Update)?;
    Some(Note { level: NoteLevel::Warning, message: format!("{machine} runs another minor than this cozy: updating it to {} first", crate::update::this_release()) })
}

/// What a prepared ask is for.
#[derive(Clone, Copy, PartialEq)]
enum For {
    Run,
    /// A `new` run's price: nothing installs.
    Quote,
    /// Its machines' estimates, for this many copies sent at once: every machine it may go to is asked, and nothing installs but local
    /// code.
    Estimate(u32),
}

async fn project_access(api: &Api, project: Option<&str>) -> Result<Option<OwnedRwLockReadGuard<()>>, Error> {
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
    crate::private_io::context(scope, prepare_scoped(api, key, ask, purpose)).await
}

async fn prepare_scoped(api: &Arc<Api>, key: &str, ask: &RunRequest, purpose: For) -> Result<Prepared, Error> {
    // Named, it goes there; else where a render's job would (Paul, 11-06), resolved first on the first of that set.
    let set = match &ask.on {
        Some(on) => vec![on.clone()],
        None => api.machines.list(false).await.map_or_else(|_| vec![On::Machine("local".into())], |list| list.render_set()),
    };
    let mut on = set.first().cloned().unwrap_or_else(|| default_on(api));
    // Only to a machine on this cozy's minor: one on another is updated first, which the client follows, then asks again. Before
    // anything is resolved there, local code's install too. Of several, placement does this for each.
    if let (On::Machine(machine), For::Run, 1) = (&on, purpose, set.len()) {
        match api.machines.current(machine).await {
            Ok(Some(update)) => return Ok(Prepared::Install(update)),
            Err(e) if e.code == crate::update::OTHER_MINOR => return Err(e),
            // Not known yet: its driver asks again before it sends.
            _ => {}
        }
    }
    // Local code installs as it resolves. A package answers for itself first: one named without a
    // function lists its functions on an OS that runs no models here.
    if ask.code.is_some() {
        api::calcifer::local_usable(&on.to_string())?;
        // Refused before that install, not after it.
        project_of(api, ask)?;
    }
    let hub = hub(api, &ask.hub);
    let agreed = ask.agreed.clone();
    // Given something to run with, a package of one function runs it; given nothing, it is listed.
    let given = !ask.terms.is_empty() || ask.input.is_some() || ask.input_text.is_some() || !ask.files.is_empty() || ask.warm.is_some();
    let resolved = target::resolve(api, &hub, &ask.target, ask.code.as_ref(), &on, ask.warm.is_some(), given, key, agreed).await?;
    let (package, function, code) = match &resolved {
        Resolved::Release { package, function, card, .. } => {
            let code = Code {
                package,
                interface: &card.interface,
                release: Some(&card.release),
                source: spec::Source::Release(Some(&card.release)),
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
    let (mut submit, mut notes) = call(api, code, callable, ask, key, &hub).await?;
    // Placed by its machines' quotes, the payload bound: a published release among several, and every machine when asked for them.
    let size = submit.spec.payload.as_ref().and_then(|payload| serde_json::from_str(payload.get()).ok()).map(|input| v1::JobSize::of(&input)).unwrap_or_default();
    let gpus = ask.gpus.unwrap_or(0);
    let (estimates, count) = match purpose {
        For::Estimate(count) => (true, count),
        _ => (false, 1),
    };
    let mut offers = vec![];
    match &resolved {
        Resolved::Release { package, card, .. } if set.len() > 1 || estimates => {
            let (chosen, offered, said) = crate::project::run_on(api, &set, &hub, package, &card.release, &callable.name, size, gpus, needs_gpu, count, !estimates).await;
            notes.extend(said.into_iter().map(|message| Note { level: NoteLevel::Warning, message }));
            if set.len() > 1 {
                on = chosen;
            }
            offers = offered;
        }
        Resolved::Local { local, .. } if estimates => {
            let function = format!("{}/{}", local.source.manifest, callable.name);
            let jobs: Vec<crate::placement::Job> = (0..count)
                .map(|copy| crate::placement::Job { key: copy.to_string(), target: named.clone(), function: function.clone(), size, gpus, gpu: needs_gpu, on: set.clone(), ..Default::default() })
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
    if let Some(warning) = gpuless(api, &submit.on, gpu, &named).await.filter(|_| ask.warm.is_none()) {
        notes.push(Note { level: NoteLevel::Warning, message: warning });
    }
    // The machine placement chose, on this cozy's minor likewise.
    if let (On::Machine(machine), For::Run, true) = (&submit.on, purpose, set.len() > 1) {
        match api.machines.current(machine).await {
            Ok(Some(update)) => return Ok(Prepared::Install(update)),
            Err(e) if e.code == crate::update::OTHER_MINOR => return Err(e),
            // Not known yet: its driver asks again before it sends.
            _ => {}
        }
    }
    if let Resolved::Release { moved: Some(moved), .. } = &resolved {
        notes.push(Note { level: NoteLevel::Warning, message: moved.clone() });
    }
    Ok(Prepared::Run { submit, notes, offers })
}

/// Where a run that names no machine is resolved and described: this computer's, on an OS that runs models itself; on another, a
/// machine this computer has when it has one, else `local`, refused with the way forward. A run itself goes where a render's job would.
pub(crate) fn default_on(api: &Api) -> On {
    let local = On::Machine("local".into());
    if platform::Os::CURRENT.local_inference() {
        return local;
    }
    let machines = api.calcifer.store().machines().unwrap_or_default();
    match machines.iter().any(|row| row.kind != MachineKind::Local && row.state != MachineState::Ended && api.machines.ordered(row)) {
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
                notes.push(Note { level: NoteLevel::Warning, message: "GPU count applies to a run and is ignored when warming".into() });
            }
            let (models, payload) = bind::warm_choices(
                &target, code.release, code.interface, callable, &terms, level == WarmLevel::Gpu, &mut notes,
            )
            .await?;
            (call.warm(level, &models, payload)?, None, None)
        }
        None => {
            let mut bound = bind::bind(&target, code.release, code.interface, callable, &terms).await?;
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
    let machine = api.machines.show(name).await.ok()?;
    (machine.status.as_ref()).is_some_and(|s| s.phase == "ready" && s.gpus.is_empty()).then(|| {
        format!(
            "{name} has no supported GPU, so {target} runs on its CPU, slowly, or not at all if it loads a model; `--on new` rents one: cozy run {target} … --on new"
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
            format!("{package}/{} is an internal function", callable.name),
        )
        .remedy("call a public function of the package")),
        Some(callable) => Ok(callable),
        None => {
            if let Some((name, why)) = interface.unavailable.iter().find(|(name, _)| fold(name) == fold(asked)) {
                return Err(Error::new(
                    Kind::Unavailable,
                    "callable_unreadable",
                    format!("{package}/{name} is not readable by this cozy: {why}"),
                )
                .remedy("update cozy, or call another function of the package")
                .next("cozy update"));
            }
            let public = interface.public();
            let mut error =
                Error::new(Kind::NotFound, "callable_unknown", format!("{package} has no function {asked:?}"))
                    .remedy(format!("its functions: {}", public.join(", ")));
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
        format!("{package}/{} runs its recipes", api::interface::RECIPES),
    )
    .remedy(format!("its recipes: {}", flags.join(", ")));
    for flag in flags.iter().take(2) {
        error = error.next(format!("cozy model quantize <model> {flag} --package {package}"));
    }
    error
}

/// A request that names no function: with inputs, the commands that run each of the package's
/// first two functions with them; without, its functions (`GET /v1/targets` lists them).
fn unnamed(package: &str, release: Option<&str>, interface: &Interface, ask: &RunRequest) -> Error {
    let public = interface.public();
    let mut error = Error::new(Kind::Usage, "function_unnamed", format!("name the function of {package} to run"))
        .remedy(format!("its functions: {}", public.join(", ")));
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

