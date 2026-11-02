//! Runs as people ask for them (`POST /v1/runs`): the target resolved on its Hub (the card and the
//! newest release this computer keeps), the request bound against the function's interface, input
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
};
use protocol::{
    calcifer::{Kind as RunKind, Note, NoteLevel, On, RunQuote, RunRequest, StagedInput, SubmitRun, Submitted},
    error::{Error, Kind},
    interface::{Callable, Interface},
    machine::{self as v1, WarmLevel},
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use crate::api::{Api, IdempotencyKey, Refusal, parse};

pub(crate) mod bind;
pub(crate) mod clip;
pub(crate) mod files;
pub(crate) mod report;
pub(crate) mod spec;
pub mod target;

use bind::Terms;
use target::Resolved;

/// `POST /v1/runs`: 202 with the run recorded, 200 when its key had recorded it.
pub(crate) async fn submit(
    State(api): State<Arc<Api>>,
    IdempotencyKey(key): IdempotencyKey,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Submitted>), Refusal> {
    let body = body?;
    prebuilt(&body)?;
    let ask: RunRequest = parse(&body)?;
    let submitted = request(&api, &key, ask).await.map_err(Refusal)?;
    let status = if submitted.replay { StatusCode::OK } else { StatusCode::ACCEPTED };
    Ok((status, Json(submitted)))
}

/// `POST /v1/run-quotes`: what a `new` run would rent and its price. Nothing is rented or recorded.
pub(crate) async fn quote(
    State(api): State<Arc<Api>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<RunQuote>, Refusal> {
    let body = body?;
    prebuilt(&body)?;
    let ask: RunRequest = parse(&body)?;
    let bound = match local(&api, &ask) {
        // Local code rents with its install, which the call then follows to its machine.
        Some(found) => {
            let (path, _) = found.map_err(Refusal)?;
            let on = ask.on.clone().unwrap_or_else(|| On::Machine("local".into()));
            let hub = hub(&api, &ask.hub);
            let home = api.machines.hubs.home();
            let origin = protocol::config::Origin::parse(&hub).map_err(Refusal)?;
            let client = api.machines.hubs.client(&hub).ok();
            let source = crate::local_code::source(home, &path, &on, client.as_ref().map(|c| (c, &origin)))
                .await
                .map_err(Refusal)?;
            crate::local_code::install_run(home, &hub, on, &source)
        }
        None => match prepare(&api, "quote", &ask).await.map_err(Refusal)? {
            Prepared::Run { submit, .. } => submit,
            Prepared::Install(_) => unreachable!("a published target installs nothing first"),
        },
    };
    api.machines.hubs.client(&bound.hub).map_err(Refusal)?;
    Ok(Json(api.machines.run_quote(&bound).await.map_err(Refusal)?))
}

#[derive(Deserialize)]
pub(crate) struct Asked {
    target: String,
    #[serde(default)]
    hub: Option<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
}

/// `GET /v1/targets?target=`: what a run of the target calls, as `POST /v1/runs` resolves it.
pub(crate) async fn describe(
    State(api): State<Arc<Api>>,
    query: Result<Query<Asked>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<protocol::calcifer::Target>, Refusal> {
    let Query(asked) = query?;
    let hub = hub(&api, asked.hub.as_deref().unwrap_or_default());
    let resolved = target::describe(&api, &hub, &asked.target, asked.cwd.as_deref()).await.map_err(Refusal)?;
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

/// The local code `ask` names, if it names local code.
fn local(api: &Api, ask: &RunRequest) -> Option<Result<(PathBuf, Option<String>), Error>> {
    let cwd = ask.cwd.as_deref().unwrap_or(Path::new(""));
    crate::local_code::target(&ask.target, cwd, &api.machines.user_home)
}

/// Records `ask` under `key` and starts it; the same key again answers the recorded run without
/// binding anything again, and another ask under it is a conflict. Local code not yet made ready
/// answers the run that makes it ready (`preparing`): its client follows that, then asks again.
pub async fn request(api: &Arc<Api>, key: &str, ask: RunRequest) -> Result<Submitted, Error> {
    let digest = digest(&ask);
    let calcifer = &api.calcifer;
    if let Some((row, recorded)) = calcifer.store().recorded(key).map_err(store)? {
        if recorded != digest {
            return Err(crate::api::conflict(key));
        }
        let run = calcifer.record(&row).map_err(refused)?;
        return Ok(Submitted { run, replay: true, notes: vec![], preparing: false });
    }
    match prepare(api, key, &ask).await? {
        Prepared::Install(run) => Ok(Submitted { run, replay: false, notes: vec![], preparing: true }),
        Prepared::Run { submit, notes } => {
            let submitted = calcifer.submit_under(key, &digest, submit).await.map_err(refused)?;
            Ok(Submitted { notes, ..submitted })
        }
    }
}

/// The ask's identity: what it asks, whatever price it was quoted since.
fn digest(ask: &RunRequest) -> String {
    let asked = RunRequest { agreed: None, ..ask.clone() };
    format!("sha256:{}", hex::encode(Sha256::digest(serde_json::to_vec(&asked).expect("serializes"))))
}

enum Prepared {
    /// The submission, and what binding it said.
    Run { submit: SubmitRun, notes: Vec<Note> },
    /// Local code's install, still running.
    Install(protocol::calcifer::Run),
}

/// Resolves and binds `ask` into the submission Calcifer records.
async fn prepare(api: &Arc<Api>, key: &str, ask: &RunRequest) -> Result<Prepared, Error> {
    let on = ask.on.clone().unwrap_or_else(|| On::Machine("local".into()));
    protocol::calcifer::local_usable(&on.to_string())?;
    let hub = hub(api, &ask.hub);
    let cwd = ask.cwd.as_deref();
    let agreed = ask.agreed.clone();
    let resolved = target::resolve(api, &hub, &ask.target, cwd, &on, ask.warm.is_some(), key, agreed).await?;
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
    let Some(function) = function else {
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
    let (submit, notes) = call(api, code, callable, ask, key, &hub).await?;
    Ok(Prepared::Run { submit, notes })
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
        protocol::calcifer::gpu_count(&gpus.to_string()).map_err(|why| Error::new(Kind::Usage, "gpus_invalid", why))?;
    }
    if let Some(pin) = &ask.attention_kernel {
        protocol::calcifer::attention_pin(pin).map_err(|why| Error::new(Kind::Usage, "attention_kernel_invalid", why))?;
    }
    let home = api.machines.hubs.home().root().to_path_buf();
    let mut notes = vec![];
    let terms = Terms {
        input: ask.input.clone(),
        inputs: ask.terms.clone(),
        input_file: ask.input_file.clone(),
        assets: ask.files.clone(),
        fidelities: ask.fidelities.clone(),
        loras: ask.loras.clone(),
        key: key.to_owned(),
        cwd: ask.cwd.clone(),
        user_home: api.machines.user_home.clone(),
        digests: Some(home.join("cache").join("digests")),
        manifests: Some(home.join("cache").join("trees")),
        clips: clip::Clips { dir: home.join("cache").join("clips"), home: api.machines.hubs.home().clone() },
        uploads: files::dir(&api.calcifer),
    };
    for term in &terms.fidelities {
        protocol::calcifer::fidelity(term).map_err(|why| Error::new(Kind::Usage, "file_fidelity_invalid", why))?;
    }
    let call = spec::Call {
        package: code.package,
        source: code.source,
        callable,
        gpus: ask.gpus.unwrap_or(0),
        lane: ask.lane.clone().unwrap_or_default(),
        attention_kernel: ask.attention_kernel.as_deref().unwrap_or_default(),
    };
    let target = format!("{}/{}", code.package, callable.name);
    let (spec, bound, out) = match ask.warm {
        Some(_) if ask.gpus.is_some() => {
            return Err(Error::new(Kind::Usage, "warm_gpus", "--gpus applies to a run, not to --warm"));
        }
        Some(level) => {
            let (models, payload) = bind::warm_choices(
                &target, code.release, code.interface, callable, &terms, level == WarmLevel::Gpu,
            )
            .await?;
            (call.warm(level, &models, payload)?, None, None)
        }
        None => {
            let mut bound = bind::bind(&target, code.release, code.interface, callable, &terms).await?;
            notes.append(&mut bound.notes);
            if let Some(warning) = bound.warning(&target) {
                notes.push(Note { level: NoteLevel::Warning, message: warning });
            }
            if let Some(warning) = gpuless(api, &code.on, code.interface, callable, &target).await {
                notes.push(Note { level: NoteLevel::Warning, message: warning });
            }
            let out = ask.out.as_ref().map(|dir| terms.place(dir)).transpose()?;
            (call.spec(&bound)?, Some(bound), out)
        }
    };
    let mut submission = call.submission(code.on, hub, &spec, bound.as_ref(), out);
    submission.inputs.extend(code.files.iter().cloned());
    Ok((submission, notes))
}

/// Warns that a GPU function sent to a ready machine with no supported GPU (an NVIDIA GPU, or an
/// Apple Silicon Mac's own) runs on its CPU, slowly, or is refused there, and how to rent one.
async fn gpuless(api: &Api, on: &On, interface: &Interface, callable: &Callable, target: &str) -> Option<String> {
    let On::Machine(name) = on else { return None };
    if !interface.gpu(callable) {
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
    let fold = |name: &str| name.replace('-', "_");
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
                .remedy("update cozy, or call another function of the package"));
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
        format!("{package}/{} runs its recipes", protocol::interface::RECIPES),
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

fn refused(e: crate::Error) -> Error {
    Refusal::from(e).0
}

/// `path` as the request spells it: `~` is the user's home, a relative one starts at `cwd`.
pub(crate) fn place(path: &Path, cwd: Option<&Path>, user_home: &Path) -> Result<PathBuf, Error> {
    let text = path.to_string_lossy();
    let path = Path::new(text.trim());
    if let Ok(rest) = path.strip_prefix("~") {
        return Ok(user_home.join(rest));
    }
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    match cwd {
        Some(cwd) => Ok(cwd.join(path)),
        None => Err(Error::new(
            Kind::Usage,
            "path_relative",
            format!("{} is relative, and the request names no cwd", path.display()),
        )
        .remedy("send an absolute path, or the request's cwd")),
    }
}

/// The kind a submission's spec runs.
pub(crate) fn kind(spec: &v1::RunSpec) -> RunKind {
    match spec.kind {
        v1::RunKind::Warm => RunKind::Warm,
        _ => RunKind::Call,
    }
}

