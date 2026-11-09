//! Run sources and preparation inside a run (`cozy.machine.v1` Run). A run is accepted at
//! once and prepares inside itself: its code installs, its models resolve at the run's Hub and
//! download, and each stage is the run's progress. Its Hub capability lives only in memory
//! (`hub::Capability`), as do its provider tokens, for the run's life (its children prepare with
//! them). A restart before it completes prepares it again, from its kept spec or, sent with tokens,
//! as its controller sends it again (`journal::interrupt_preparations`).
use crate::{
    api::domain,
    call_memo::{self, Memo},
    execution::Stoppable,
    catalog::HeldGeneration,
    gpu_service::{without_gpus, KeptMember, Level, Member, Plan, Pool},
    hub,
    journal::{CallMemo, Execution, Failure, InputFile, Installation, Outcome, ResultRecord},
    local_source::LocalSources,
    objects::{Objects, Refused},
    published::{Providers, Publisher, Request},
    service::{Call, Service},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use crate::stop::Stop;
use tensorfs::format::ObjectRef;
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
};

mod model_folder;

pub(crate) enum Source {
    /// A published release by name (th-245); empty: the Hub's newest.
    Release { package: String, release: String },
    /// An installation this machine already holds for the signer.
    Installation(String),
    /// Unpublished code written with Write: its manifest's digest.
    Local(String),
    /// No code: a warm run that only makes its model choices (`cozy model upload`).
    Models,
}

/// A published release a run installs: its package and release.
type Published = (String, String);

/// One member of the warm set a warm run carries (`RunSpec.set`).
pub(crate) struct SetItem {
    pub source: Source,
    pub entrypoint: String,
    pub models: Vec<domain::ModelChoice>,
    pub payload: Option<api::machine::Json>,
    /// As `Spec::lane`, for this member's main model.
    pub lane: String,
    pub level: Level,
    /// The item as the caller sent it, for Status.
    pub sent: Vec<u8>,
}

pub(crate) struct Spec {
    /// Prepare only (`kind: warm`): install and download, then succeed.
    pub warm: bool,
    /// `kind: remove`: the caller's installations of the source leave the machine.
    pub remove: bool,
    /// A warm run's whole warm set for its caller, replacing the previous one.
    pub set: Option<Vec<SetItem>>,
    /// A child run's parent execution (a call through its parent's seam).
    pub parent: String,
    pub source: Source,
    pub entrypoint: String,
    pub input: Value,
    pub inputs: Vec<InputFile>,
    pub models: Vec<domain::ModelChoice>,
    pub attention_kernel: String,
    pub gpus: u32,
    /// The main model's lane; empty, and every other slot: the Hub's pick for the hardware. A
    /// model-only upload's commit lane.
    pub lane: String,
    /// The Hub the run reads, under its signer's newest capability there when its request carried one.
    pub hub: Option<hub::Source>,
    /// Provider tokens for source models, held in memory only.
    pub providers: Providers,
    pub tensorfs: Option<api::machine::TensorFSOptions>,
    /// `org/name`: the requester's repository for the run's weights outputs, or where a
    /// model-only warm run puts the source model it made.
    pub weights_destination: String,
    pub owner: String,
    /// The caller's catalog revision: held resolutions are reused only under it (th-245).
    pub catalog_revision: String,
    /// A child run of a callee: the other package's App its parent's environment holds.
    pub application: String,
    /// The spec's identity (its access excluded): a resubmitted id must carry the same.
    pub digest: String,
    /// The spec as sent, kept in the journal while it prepares so a restart prepares it again;
    /// empty for one that cannot (a child run, provider tokens, which live in memory only).
    pub journaled: Vec<u8>,
}

/// What `Runs::draft` makes of a run sent.
enum Drafted {
    /// Its id already names this spec's run.
    Held(Box<Execution>),
    New(Box<crate::journal::Invocation>, Vec<ObjectRef>),
}

pub(crate) struct Runs {
    pub service: Arc<Service>,
    pub objects: Arc<Objects>,
    /// The one resolver of a run's code and models.
    pub publisher: Arc<Publisher>,
    pub local: Arc<LocalSources>,
    /// On a rental: its own Hub, where a run that names none reads anonymously.
    pub own_hub: Option<hub::Source>,
    /// Unfinished runs' preparation: their children prepare with it, and their weights are
    /// fetched and published with its access.
    pub contexts: Mutex<HashMap<String, RunContext>>,
}

/// What a run's children prepare with: its installation, Hub access, owner, catalog revision,
/// attention pin, lane and the model choices addressed to its callables (`<entrypoint>.models.<p>`).
#[derive(Clone, Default)]
pub(crate) struct RunContext {
    pub(crate) installation: String,
    pub(crate) hub: Option<hub::Source>,
    pub(crate) providers: Providers,
    pub(crate) tensorfs: Option<api::machine::TensorFSOptions>,
    pub(crate) weights_destination: String,
    pub(crate) owner: String,
    pub(crate) catalog_revision: String,
    pub(crate) attention_kernel: String,
    pub(crate) gpus: u32,
    pub(crate) lane: String,
    pub(crate) models: Vec<domain::ModelChoice>,
    /// What its weights come under before its call's executor reads them (`Fetch::ahead`).
    pub(crate) ahead: Option<Arc<crate::access::Fetch>>,
}

/// A choice for the run's own slots: a bare parameter, or one under its own name.
fn own_choice(choice: &domain::ModelChoice, entrypoint: &str) -> bool {
    match choice.parameter.split_once(".models.") {
        None => true,
        Some((callable, _)) => callable == entrypoint,
    }
}

/// The slot of `package`'s `interface` a choice's parameter addresses: its full slot path
/// (`<callable>.models.<parameter>`), or that path behind the package's name, as the CLI names
/// another package's callable (`<org>/<name>/<callable>.models.<parameter>`).
fn addressed(parameter: &str, package: &str, interface: &Value) -> Option<String> {
    let path = parameter
        .strip_prefix(package)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(parameter);
    api::interface::entrypoints(interface)
        .any(|entry| api::interface::models(entry).iter().any(|slot| api::interface::slot_path(slot) == Some(path)))
        .then(|| path.to_string())
}

/// `installation`'s internal functions that construct a model: what a function loading none
/// itself calls.
fn internal_models(installation: &Installation) -> Result<Vec<String>, Refused> {
    let interface: Value = serde_json::from_slice(&installation.interface)
        .map_err(|_| refused("package_interface_invalid", "held interface is corrupt"))?;
    let constructs = |entry: &Value| api::interface::models(entry).iter().any(crate::gpu_service::constructed);
    Ok(api::interface::entrypoints(&interface)
        .filter(|entry| api::interface::internal(entry) && constructs(entry))
        .filter_map(|entry| api::interface::name(entry).map(str::to_owned))
        .collect())
}

/// What the checkpoints and components of `each` all share. No components names a whole checkpoint.
fn shared_of(each: &[Vec<(String, Vec<String>)>]) -> Vec<(String, Vec<String>)> {
    let holds = |list: &[(String, Vec<String>)], header: &str, component: Option<&str>| {
        list.iter().any(|(held, components)| held == header && (components.is_empty() || component.is_none_or(|component| components.iter().any(|c| c == component))))
    };
    let mut shared: Vec<(String, Vec<String>)> = vec![];
    for (header, components) in each.first().into_iter().flatten() {
        let names: Vec<Option<&str>> = match components.is_empty() {
            true => vec![None],
            false => components.iter().map(|component| Some(component.as_str())).collect(),
        };
        for component in names.into_iter().filter(|component| each.iter().all(|list| holds(list, header, *component))) {
            match (shared.iter_mut().find(|(held, _)| held == header), component) {
                (Some((_, components)), Some(component)) => components.push(component.to_owned()),
                (Some((_, components)), None) => components.clear(),
                (None, _) => shared.push((header.clone(), component.map(str::to_owned).into_iter().collect())),
            }
        }
    }
    shared
}

/// Whether `installation` declares a public entrypoint `name` for a warm set.
fn declares(installation: &Installation, name: &str) -> Result<bool, Refused> {
    let interface: Value = serde_json::from_slice(&installation.interface)
        .map_err(|_| refused("package_interface_invalid", "held interface is corrupt"))?;
    let entry = api::interface::entrypoint(&interface, name);
    if let Some(entry) = entry {
        public_root(entry)?;
    }
    Ok(entry.is_some())
}
fn public_root(entry: &Value) -> Result<(), Refused> {
    if api::interface::internal(entry) {
        return Err(refused(
            "callable_internal",
            format!("{} is an internal package function, which only its package's own code calls: `cozy run <package>` lists the ones you can", api::interface::name(entry).unwrap_or_default()),
        ));
    }
    Ok(())
}
/// The CLI's `%XX` file-name escape (`cozy model upload`), undone.
fn unescape(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], name.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => { out.push(byte); i += 3; }
            (byte, _) => { out.push(byte); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
fn refused(code: &'static str, message: impl Into<String>) -> Refused {
    Refused {
        code,
        message: message.into(),
    }
}

/// A warm choice made present (`Runs::present`): its header, the commit naming it, its result
/// row, and the folder it was admitted from.
struct Present {
    header: ObjectRef,
    tree: Option<ObjectRef>,
    row: Value,
    folder: Option<model_folder::Folder>,
}

/// A commit's repo files (configs, a tokenizer), which go with its header.
fn repo_files(store: &tensorfs::store::Store, tree: Option<&ObjectRef>) -> Result<std::collections::BTreeMap<String, ObjectRef>, Refused> {
    let Some(tree) = tree else { return Ok(Default::default()) };
    store.tree(tree).map(|t| t.repo_files().map(|(p, r)| (p.to_owned(), r.clone())).collect())
        .map_err(|e| refused("checkpoint_invalid", e.to_string()))
}

/// A named commit (tree) or header already held for an upload; a local one never falls back to a
/// remote repository or its newest release.
fn held_tree(store: &crate::storage::Store, choice: &domain::ModelChoice) -> Result<Option<crate::published::Named>, Refused> {
    let Some(manifest) = choice.manifest.as_ref().filter(|_| !choice.repository.is_empty()) else {
        return Ok(None);
    };
    let id = tensorfs::sha256::hex(&manifest.digest);
    let object = match crate::storage::held(store, &id) {
        Ok(object) => object,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !choice.repository.starts_with("local/") => return Ok(None),
        Err(error) => return Err(refused("checkpoint_absent", format!("held checkpoint sha256:{id}: {error}"))),
    };
    let named = |lane, tree, header| crate::published::Named {
        repository: choice.repository.clone(), release: choice.release.clone(), lane, tree, header: Some(header), reasons: vec![],
    };
    let Some(parsed) = crate::storage::tree_or_header(store, &object).map_err(|error| refused("checkpoint_invalid", error.to_string()))? else {
        return Ok(Some(named(choice.lane.clone(), None, object)));
    };
    let header = parsed.header().map_err(|error| refused("checkpoint_invalid", error.to_string()))?.1.clone();
    Ok(Some(named(parsed.lane, Some(object), header)))
}

/// `header` with its repo `files` as a new commit of `destination` in lane `lane`, uploaded from
/// `store` to the run's TensorFS endpoint under the requester's newest capability at its Hub, else the
/// one the request carried.
#[allow(clippy::too_many_arguments)]
pub(crate) fn upload(
    options: &api::machine::TensorFSOptions, hub: Option<&hub::Source>, destination: &str, lane: &str,
    header: &ObjectRef, files: &std::collections::BTreeMap<String, ObjectRef>, store: &tensorfs::store::Store,
    control: &tensorfs::fetch::UploadControl<'_>,
) -> Result<tensorfs::fetch::Uploaded, Refused> {
    use tensorfs::fetch::{bearer, upload_commit, Client, Endpoint, SourcePolicy};
    let failed = |error: String| refused("upload_failed", error);
    // A lane the precision table names is the weights' width: never int8 weights labelled bf16.
    let read = store.read(header).map_err(|e| e.to_string())
        .and_then(|bytes| tensorfs::format::Header::from_cbor(&bytes).map_err(|e| e.to_string()))
        .map_err(|e| refused("checkpoint_invalid", e))?;
    let held = crate::precision::Profile::of(&read);
    let named = crate::precision::bits(lane);
    if named > 0 && held.bits > 0 && named != held.bits {
        return Err(refused("commit_lane_mismatch", format!(
            "lane {lane} is {named}-bit, and these weights are {} ({}-bit): name the lane they hold (`--lane {}`)", held.family, held.bits, held.family)));
    }
    let commit = tensorfs::format::Tree::of(lane, header.clone(), files).map_err(|e| refused("invalid_request", e.to_string()))?;
    let tree = store.put_tree(&commit).map_err(|e| failed(e.to_string()))?;
    let policy = SourcePolicy { any_host: true, allow_local: true, max_redirects: 8, ..Default::default() };
    let client = match options.ca_der.is_empty() {
        true => Ok(Client::new(policy)),
        false => Client::with_roots(policy, hub::pem(&options.ca_der).as_bytes()),
    }.map_err(|error| failed(error.to_string()))?;
    let token = match hub.and_then(|hub| hub.capability_at(&options.endpoint)) {
        Some(capability) => Some(capability as Arc<dyn tensorfs::fetch::TokenProvider>),
        None => (!options.token.is_empty()).then(|| bearer(options.token.clone())),
    };
    let anonymous = token.is_none();
    let mut endpoint = Endpoint::new(&client, &options.endpoint, token).map_err(|error| failed(error.to_string()))?;
    if anonymous {
        endpoint = endpoint.anonymous();
    }
    let runtime = tensorfs::fetch::runtime().map_err(|error| failed(error.to_string()))?;
    runtime.block_on(upload_commit(&endpoint, destination, &tree, store, control)).map_err(|error| failed(error.to_string()))
}

impl Runs {
    /// The run this id already names, if its spec is `digest`; another spec is a conflict.
    pub(crate) fn existing(&self, actor: &str, id: &str, digest: &str) -> Result<Option<Execution>, Refused> {
        match self.service.engine.get_public(Some(actor), id) {
            Ok(run) if run.canceled_before_acceptance && run.state == crate::journal::State::Canceled => Ok(Some(run)),
            Ok(run) if run.submission.as_ref().map(|s| s.invocation_digest.as_str()) == Some(digest) => Ok(Some(run)),
            Ok(_) => Err(refused("run_id_conflict", "this run id already names another run spec")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Accepts the run (or answers the one this id already names) and starts its preparation:
    /// an admission of one (`admit`).
    pub(crate) fn submit(self: &Arc<Self>, actor: &str, id: &str, spec: Spec) -> Result<Execution, Refused> {
        let (run, _) = self.admit(actor, vec![(id.to_string(), spec)])?.pop().expect("one run admitted")?;
        Ok(run)
    }

    /// Accepts `runs` as one admission (cl-816): each new one is journaled with the others, in one
    /// transaction, before any prepares, so the scheduler packs them together as the machine's
    /// quote did. Each answers its run (the one its id already names, too) and whether it is new,
    /// or why it was refused, alone. An accepted run is answered before its inputs are checked:
    /// they may be gone since.
    pub(crate) fn admit(self: &Arc<Self>, actor: &str, runs: Vec<(String, Spec)>) -> io::Result<Vec<Result<(Execution, bool), Refused>>> {
        // Object sweeps stay out from validation through acceptance.
        let custody = self.objects.guard();
        let mut answers: Vec<Option<Result<(Execution, bool), Refused>>> = vec![];
        let mut drafts = vec![];
        for (id, spec) in runs {
            match self.draft(actor, &id, &spec) {
                Ok(Drafted::New(invocation, references)) => {
                    drafts.push((answers.len(), id, spec, invocation, references));
                    answers.push(None);
                }
                Ok(Drafted::Held(run)) => answers.push(Some(Ok((*run, false)))),
                Err(refusal) => answers.push(Some(Err(refusal))),
            }
        }
        let accepting = (drafts.iter_mut())
            .map(|(_, id, spec, invocation, references)| crate::journal::Accepting {
                id,
                digest: &spec.digest,
                invocation: std::mem::take(&mut **invocation),
                objects: references,
                spec: &spec.journaled,
            })
            .collect();
        let accepted = self.service.engine.accept_runs(actor, accepting)?;
        drop(custody);
        for ((at, _, spec, ..), accepted) in drafts.into_iter().zip(accepted) {
            answers[at] = Some(match accepted {
                Ok((record, new)) => {
                    if new {
                        self.prepare_later(actor, &record.id, spec)?;
                    }
                    Ok((record, new))
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(refused("run_id_conflict", error.to_string())),
                Err(error) => Err(refused("invalid_run_id", error.to_string())),
            });
        }
        Ok(answers.into_iter().map(|answer| answer.expect("each run answered")).collect())
    }

    /// New run `id` as the journal takes it, and the objects it takes custody of, under the
    /// caller's custody guard; or the run the id already names with this spec.
    fn draft(&self, actor: &str, id: &str, spec: &Spec) -> Result<Drafted, Refused> {
        if let Some(run) = self.existing(actor, id, &spec.digest)? {
            return Ok(Drafted::Held(Box::new(run)));
        }
        if !spec.input.is_object() {
            return Err(refused(
                "invalid_request",
                "the payload is a JSON object of the function's parameters",
            ));
        }
        let mut references = Vec::new();
        let unwritten = |input: &str| refused("input_unwritten", format!("input {input} was not written to this machine"));
        for input in &spec.inputs {
            let path = match self.objects.path(actor, &input.digest)? {
                Some((path, length)) if length == input.length => path,
                _ => return Err(unwritten(&input.input_id)),
            };
            references.push(ObjectRef {
                sha256: input.digest.trim_start_matches("sha256:").into(),
                length: input.length,
            });
            if input.media_type != api::tree::MEDIA {
                continue;
            }
            // A tree's files were written too: each its manifest names.
            let files = crate::trees::parse(&std::fs::read(path)?)
                .map_err(|e| refused("invalid_request", format!("input tree {}: {e}", input.input_id)))?;
            for (member, object) in files {
                if self.objects.path(actor, &format!("sha256:{}", object.sha256))?.is_none_or(|(_, held)| held != object.length) {
                    return Err(unwritten(&format!("{}/{member}", input.input_id)));
                }
                references.push(object);
            }
        }
        // A local source is refused here, not accepted to fail: the run takes custody of its
        // objects now, and a refused id is free for the client to write them and send again.
        if let Source::Local(digest) = &spec.source {
            references.extend(self.local.written(actor, digest)?);
        }
        // A written file a warm run makes into a model (`object://sha256:<hex>/<file>`).
        for choice in &spec.models {
            // A folder is bound to one retained input tree, never an arbitrary store path.
            model_folder::input(spec, choice)?;
            let Some(hex) = choice.source.strip_prefix("object://").and_then(|rest| rest.split('/').next()) else {
                continue;
            };
            let sha256 = hex.trim_start_matches("sha256:").to_string();
            match self.objects.path(actor, &format!("sha256:{sha256}"))? {
                Some((_, length)) => references.push(ObjectRef { sha256, length }),
                None => return Err(unwritten(&choice.source)),
            }
        }
        self.objects.retain(&references)?;
        let package = match &spec.source {
            Source::Release { package, .. } => package.clone(),
            Source::Installation(alias) => alias.clone(),
            Source::Local(digest) => format!("local:{digest}"),
            Source::Models => "models".into(),
        };
        let draft = crate::journal::Invocation {
            package,
            generation: String::new(),
            module: String::new(),
            entrypoint: spec.entrypoint.clone(),
            input: spec.input.clone(),
            attention_kernel: spec.attention_kernel.clone(),
            gpus: spec.gpus,
            lane: spec.lane.clone(),
            inputs: spec.inputs.clone(),
            parent: spec.parent.clone(),
            prepares: spec.warm || spec.remove,
            ..Default::default() // root and family: stamped at acceptance
        };
        Ok(Drafted::New(Box::new(draft), references))
    }

    /// The GPUs `run`'s root took as it started; 0: none, or not yet.
    fn root_width(&self, run: &str) -> u32 {
        let engine = &self.service.engine;
        let Ok(record) = engine.get(run) else { return 0 };
        match record.invocation.root.as_str() {
            "" => record.width,
            root => engine.get(root).map_or(0, |root| root.width),
        }
    }

    /// Prepare accepted run `run` on a thread of its own, which ends the run unless it binds.
    pub(crate) fn prepare_later(self: &Arc<Self>, actor: &str, run: &str, spec: Spec) -> io::Result<()> {
        let (this, actor, run) = (self.clone(), actor.to_string(), run.to_string());
        let thread = std::thread::Builder::new().name(format!("prepare-{run}"));
        thread.spawn(move || {
            let stoppable = this.service.engine.stoppable(&run);
            // A panic (a thread the container's task cap refused) ends the run FAILED: no
            // preparation outlives the thread that would end it.
            let prepared = std::panic::AssertUnwindSafe(|| this.prepare(&actor, &run, spec, &stoppable));
            let outcome = std::panic::catch_unwind(prepared).unwrap_or_else(|panic| {
                let text = panic.downcast_ref::<String>().cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|text| text.to_string()));
                Err(refused("preparation_failed", text.unwrap_or_else(|| "the preparation panicked".into())))
            });
            let ended = match outcome {
                Ok(None) => Ok(()),
                Ok(Some(result)) => this.service.engine.end_preparation(&run, Outcome::Completed(result)).map(drop),
                Err(Refused { code, message }) => {
                    let origin = if code.starts_with("invalid") || code.ends_with("_absent") { 6 } else { 3 };
                    let failure = Failure { status: 3, cause: 7, origin, code: code.into(), message };
                    this.service.engine.end_preparation(&run, Outcome::Failed(failure)).map(drop)
                }
            };
            this.local.prepared(&run);
            if let Err(error) = ended {
                tracing::warn!("run {run} preparation: {error}");
            }
        })?;
        Ok(())
    }

    /// A warm run's model choices made present when it names no entrypoint (`cozy package
    /// install`, `cozy model download`) or no code (`cozy model upload`, of a file or a run's output): a
    /// Hub checkpoint downloaded, a provider source or a written file (`object://`) made, a
    /// checkpoint this machine holds (a run's weights output, a `local/` alias) found. With a
    /// weights destination the one choice is put there: a `local/` alias held by name, or a
    /// repository at the run's TensorFS endpoint.
    /// A written file (`object://sha256:<hex>/<name>`, `cozy model upload ./model.safetensors`) as the
    /// import reads it: the signer's stored object, linked under its file's name (the name is its
    /// format to the import). Any other source is the import's as it is.
    fn written_source(&self, actor: &str, source: &str) -> Result<String, Refused> {
        let Some(rest) = source.strip_prefix("object://") else {
            return Ok(source.to_string());
        };
        let (digest, name) = rest.split_once('/').unwrap_or((rest, ""));
        let name = unescape(name);
        let unwritten = || refused("input_unwritten", format!("{source} was not written to this machine"));
        if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
            return Err(refused("invalid_request", format!("{source} names no file")));
        }
        let (object, _) = self.objects.path(actor, digest)?.ok_or_else(unwritten)?;
        let dir = self.publisher.store().root().join("tmp").join(format!("written-{}", digest.trim_start_matches("sha256:")));
        std::fs::create_dir_all(&dir)?;
        let file = dir.join(&name);
        if !file.is_file() {
            std::fs::hard_link(&object, &file).or_else(|_| std::fs::copy(&object, &file).map(drop))?;
        }
        Ok(file.display().to_string())
    }

    fn warm_models(
        &self,
        actor: &str,
        id: &str,
        spec: &Spec,
        observe: &(dyn Fn(&str, u64, u64) + Sync),
        stoppable: &Stoppable,
    ) -> Result<Vec<Value>, Refused> {
        let destination = spec.weights_destination.trim_start_matches("model://");
        // A composition by reference: choice `base[:<component>,…]` is the model the one upload starts from.
        let (bases, choices): (Vec<&domain::ModelChoice>, Vec<&domain::ModelChoice>) =
            spec.models.iter().partition(|choice| choice.parameter.split(':').next() == Some(api::machine::BASE));
        if !destination.is_empty() && choices.len() != 1 {
            return Err(refused("invalid_request", "a weights destination takes one model"));
        }
        let base = match bases.as_slice() {
            [] => None,
            [base] if destination.is_empty() || destination.starts_with("local/") => {
                return Err(refused("invalid_request", format!("{}: a model composed on a base uploads to a Hub model", base.parameter)));
            }
            [base] => {
                let names: Vec<String> = base.parameter.split_once(':').map_or(vec![], |(_, names)| names.split(',').map(str::to_owned).collect());
                if names.iter().any(|n| n.is_empty()) {
                    return Err(refused("invalid_request", format!("{}: name components as base:<component>[,<component>…]", base.parameter)));
                }
                Some((*base, names))
            }
            _ => return Err(refused("invalid_request", "a model has one base")),
        };
        // An upload goes to the run's TensorFS endpoint: a new commit of `org/name`, in the run's
        // lane.
        let endpoint = match (destination.is_empty() || destination.starts_with("local/"), &spec.tensorfs) {
            (true, _) => None,
            (false, Some(_)) if !tensorfs::format::label(&spec.lane) =>
                return Err(refused("invalid_request", format!("an upload to {destination} names its commit's lane"))),
            (false, Some(options)) => Some(options),
            (false, None) => return Err(refused("upload_endpoint_absent",
                format!("uploading to {destination} takes the run's TensorFS endpoint (`tensorfs` options), and it carries none"))),
        };
        let publisher = &self.publisher;
        let hub = spec.hub.clone().or_else(|| self.own_hub.clone());
        let mut models = vec![];
        for choice in choices {
            // On a base, a Hub checkpoint is referenced too: its header and tree are enough.
            let Present { mut header, mut tree, mut row, folder } = self.present(actor, id, spec, choice, base.is_some(), observe, stoppable)?;
            let upload_store = folder.as_ref().map_or(publisher.store().as_ref(), |folder| &folder.store);
            if let Some((base, names)) = &base {
                if folder.is_some() {
                    return Err(refused("invalid_request", "a CozyTensors folder uploads as it is; compose from a model"));
                }
                // Its header and tree only: what the composition keeps stays the Hub's objects.
                let on = self.present(actor, id, spec, base, true, observe, stoppable)?;
                let record = tensorfs::import::base::Base {
                    model: base.repository.clone(),
                    release: on.row["release"].as_str().unwrap_or_default().to_owned(),
                    lane: on.row["lane"].as_str().unwrap_or_default().to_owned(),
                    commit: on.tree.as_ref().map(ObjectRef::id).unwrap_or_default(),
                    header: on.header.id(),
                };
                observe("composing", 0, 0);
                let composed = tensorfs::import::base::on_base(publisher.store(), &on.header, &header, names, &record)
                    .map_err(|e| refused("model_compose_refused", e.to_string()))?;
                row["base"] = on.row;
                row["source_checkpoint"] = row["checkpoint"].take();
                row["checkpoint"] = json!(composed.header.id());
                row["components"] = json!(composed.components);
                (header, tree) = (composed.header, on.tree);
            }
            // A commit's repo files (configs, a tokenizer) go with its header: the base's, for a composition.
            let files = repo_files(upload_store, tree.as_ref())?;
            if let Some(name) = destination.strip_prefix("local/") {
                // A local alias (`cozy model download … local/name`): the model, held by name.
                crate::storage::set_local(publisher.store(), name, &header, &files, &spec.lane)
                    .map_err(|e| refused("local_alias_refused", e.to_string()))?;
                row["published"] = json!({"destination": destination, "checkpoint": header.id(), "converged": false});
            } else if let Some(options) = endpoint {
                let stage = format!("uploading to {destination}");
                let control = tensorfs::fetch::UploadControl {
                    cancelled: &|| stoppable.stop.is_cancelled(),
                    progress: &|done, total| observe(&stage, done, total),
                    commit: &|| stoppable.commit(),
                };
                let uploaded = upload(options, hub.as_ref(), destination, &spec.lane, &header, &files, upload_store, &control)
                    .map_err(|error| match stoppable.stop.is_cancelled() {
                        true => refused(crate::published::CANCELED, "the upload was canceled before its commit; admitted objects remain available"),
                        false => error,
                    })?;
                row["published"] = json!({"destination": destination, "commit": uploaded.commit.id(), "lane": spec.lane,
                    "checkpoint": header.id(), "converged": uploaded.converged,
                    "sent": {"objects": uploaded.sent, "bytes": uploaded.bytes}});
            }
            models.push(row);
        }
        Ok(models)
    }

    /// One warm choice made present: a Hub checkpoint downloaded (its header and tree alone when
    /// `structure`: a composition's base and a Hub source on it), a provider source or a written file (`object://`)
    /// made, a checkpoint this machine holds (a run's weights output, a `local/` alias) found, or
    /// a CozyTensors folder admitted. Its header, the commit that names it when one does, and its
    /// result row.
    #[allow(clippy::too_many_arguments)]
    fn present(
        &self,
        actor: &str,
        id: &str,
        spec: &Spec,
        choice: &domain::ModelChoice,
        structure: bool,
        observe: &(dyn Fn(&str, u64, u64) + Sync),
        stoppable: &Stoppable,
    ) -> Result<Present, Refused> {
        let stop = Some(&stoppable.stop);
        let destination = spec.weights_destination.trim_start_matches("model://");
        let publisher = &self.publisher;
        let hub = spec.hub.clone().or_else(|| self.own_hub.clone());
        let hub_access = || {
            hub.as_ref().ok_or_else(|| {
                refused("hub_access_absent", "this needs the run's Hub access, and it carries none: `cozy login <email>` on the computer that sent it, then run it again")
            })
        };
        let folder = model_folder::prepare(self, actor, spec, choice, stoppable)?;
        // Each model's header, and the tree that names it when one does.
        let (header, tree, row) = if let Some(folder) = &folder {
            let row = json!({"parameter": choice.parameter, "source": choice.source, "tree": folder.tree.id()});
            (folder.header.clone(), Some(folder.tree.clone()), row)
        } else if !choice.source.is_empty() {
            let source = self.written_source(actor, &choice.source)?;
            let import = crate::activity::Import::new(&choice.source, &self.service.engine.caller_id(id));
            let made = publisher.make_source(&source, &choice.profiles, &choice.trained_on, &spec.providers, stop, &|stage, done, total| {
                import.moved(stage, done, total);
                observe(stage, done, total)
            })?;
            drop(import);
            let row = json!({"parameter": choice.parameter, "source": choice.source,
                "resolved": made.resolved, "profiles": made.profiles, "format": made.format,
                "trigger": made.trigger.join(", "), "repository": made.repository, "checkpoint": made.header.id()});
            let tree = crate::storage::local(publisher.store(), &made.repository)?.map(|local| local.tree);
            (made.header, tree, row)
        } else if let (Some(name), None) = (choice.repository.strip_prefix("local/"), &choice.manifest) {
            // A local alias: the model this machine's store holds under that name.
            let local = crate::storage::local(publisher.store(), &choice.repository)
                .map_err(|e| refused("checkpoint_absent", e.to_string()))?
                .filter(|local| publisher.store().holds(&local.header))
                .ok_or_else(|| refused("checkpoint_absent", format!("this machine holds no local/{name}: `cozy model download <source> local/{name} --on <machine>` puts it there")))?;
            let row = json!({"parameter": choice.parameter,
                "repository": choice.repository, "checkpoint": local.header.id()});
            (local.header, Some(local.tree), row)
        } else if choice.repository.is_empty() {
            // A checkpoint this machine already holds (a run's output), by its header.
            let header = choice.manifest.as_ref().ok_or_else(|| {
                refused("invalid_request", format!("model choice {:?} names no model", choice.parameter))
            })?;
            let sha256 = tensorfs::sha256::hex(&header.digest);
            let held = crate::storage::held(publisher.store(), &sha256)
                .map_err(|_| refused("checkpoint_absent", format!("this machine holds no checkpoint sha256:{sha256}: run what made it on this machine, or name a model of the Hub's")))?;
            publisher.held_for(&self.service, hub_access().ok(), &held).map_err(|(code, message)| refused(code, message))?;
            let row = json!({"parameter": choice.parameter, "checkpoint": format!("sha256:{sha256}")});
            (held, None, row)
        } else if let Some(named) = if destination.is_empty() { None } else { held_tree(publisher.store(), choice)? } {
            // A Hub commit held here is the request's to publish only as the Hub serves it to its account.
            let named = match named.tree.is_some() && !named.repository.starts_with("local/") {
                true => publisher.commit(&self.service, hub_access()?, named).map_err(|(code, message)| refused(code, message))?,
                false => named,
            };
            let header = named.header().map_err(|(code, message)| refused(code, message))?.clone();
            let row = json!({"parameter": choice.parameter, "repository": named.repository,
                "lane": named.lane, "tree": named.tree.as_ref().map(ObjectRef::id), "checkpoint": header.id()});
            (header, named.tree, row)
        } else {
            // A Hub commit by name, or by hash; no lane is the Hub's pick for this machine's
            // hardware.
            let named = crate::published::choose(choice, &choice.parameter)
                .map_err(|(code, message)| refused(code, message))?;
            let hardware = self.service.gpu().map(|gpu| gpu.hardware(gpu.width() as u32)).unwrap_or_default();
            let (stage, saving) = (format!("downloading {}", choice.repository), format!("saving {}", choice.repository));
            let wait = crate::activity::wait(crate::activity::Kind::Download, &choice.repository, &self.service.engine.caller_id(id));
            let (named, landed) = publisher.download(&self.service, hub_access()?, named, &hardware, &spec.catalog_revision, structure, stop, &|done, total| {
                // Every byte is here (a total this small is still only the header's): what is left is the disk's,
                // and a slow one has held a run at 100% for over a minute with no word (tfs-143).
                let now = if total >= 1 << 20 && done >= total { &saving } else { &stage };
                wait.moved(now, done, total);
                observe(now, done, total)
            })?;
            drop(wait);
            for reason in &named.reasons {
                let warning = format!("{}: {reason}", named.repository);
                if let Err(error) = self.service.engine.append_warning(id, &warning) {
                    tracing::warn!("run {id}: {warning} ({error})");
                }
            }
            let header = named.header().map_err(|(code, message)| refused(code, message))?.clone();
            let read = publisher.store().read(&header).map_err(|e| e.to_string())
                .and_then(|bytes| tensorfs::format::Header::from_cbor(&bytes).map_err(|e| e.to_string()))
                .map_err(|e| refused("model_resolution_failed", e))?;
            let profile = crate::precision::Profile::of(&read);
            let tree = named.tree.as_ref().map(ObjectRef::id);
            let row = json!({"parameter": choice.parameter, "repository": named.repository,
                "release": named.release, "lane": named.lane, "precision": profile.family, "tree": tree,
                "checkpoint": header.id(), "bytes": {"hub": landed.origin, "volume": landed.volume, "local": landed.local}});
            (header, named.tree, row)
        };
        Ok(Present { header, tree, row, folder })
    }
    /// The run's code and models made ready: Ok(None) once it is dispatchable, Ok(Some) for a
    /// warm run's result.
    fn prepare(&self, actor: &str, id: &str, spec: Spec, stoppable: &Stoppable) -> Result<Option<ResultRecord>, Refused> {
        let engine = self.service.engine.clone();
        let run = id.to_string();
        // Byte counts arrive per chunk: a new stage and a finished count show at once, bytes at
        // most 4 times a second.
        let last = std::sync::Mutex::new((String::new(), std::time::Instant::now()));
        let observe = Arc::new(move |stage: &str, done: u64, total: u64| {
            let mut last = last.lock().unwrap();
            if last.0 == stage && last.1.elapsed() < std::time::Duration::from_millis(250) && (total == 0 || done < total) {
                return;
            }
            *last = (stage.to_string(), std::time::Instant::now());
            let detail = json!({"stage": stage, "bytes_done": done, "bytes_total": total});
            let _ = engine.observe_progress(&run, 0, detail.to_string());
        });
        observe("preparing", 0, 0);
        let warm_result = |installed: Value, models: Vec<Value>| -> Result<Option<ResultRecord>, Refused> {
            let mut value = installed;
            value["models"] = Value::Array(models);
            Ok(Some(ResultRecord {
                value,
                artifacts: vec![],
                asset_bindings: vec![],
                adjustments: vec![],
            }))
        };
        if spec.remove {
            return self.remove(actor, &spec).map(Some);
        }
        if let Some(items) = &spec.set {
            return self.warm_set(actor, id, &spec, items, &observe, stoppable);
        }
        if let Source::Models = spec.source {
            let models = self.warm_models(actor, id, &spec, &*observe, stoppable)?;
            return warm_result(json!({}), models);
        }
        // A warm run naming no entrypoint installs its code, then makes its choices present.
        let install_only = spec.warm && spec.entrypoint.is_empty();
        let held = match &spec.source {
            Source::Release { .. } | Source::Models => None,
            Source::Installation(alias) => Some(
                self.service
                    .engine
                    .installation(actor, alias)?
                    .ok_or_else(|| {
                        refused(
                            "installation_absent",
                            "this machine holds no such installation for this signer",
                        )
                    })?,
            ),
            Source::Local(digest) => {
                let local = &self.local;
                let observer = Box::new({
                    let observe = observe.clone();
                    move |stage: &str, done: u64, total: u64| observe(stage, done, total)
                });
                let installed = local.install(&self.service, actor, digest, Some(&stoppable.stop), id, observer)?;
                self.supersede(actor, &installed)?;
                local.collect(actor, &installed.package);
                Some(installed)
            }
        };
        let held = match held {
            Some(installed) => Some(self.callee_view(installed, &spec.application)?),
            None => None,
        };
        let hub = spec.hub.clone().or_else(|| self.own_hub.clone());
        let release = match &spec.source {
            Source::Release { package, release } => Some((package.clone(), release.clone())),
            _ => None,
        };
        // Its own slots' choices resolve here; the rest address its callables, whose calls
        // resolve them.
        let own: Vec<_> = match install_only {
            true => vec![],
            false => spec.models.iter().filter(|c| own_choice(c, &spec.entrypoint)).cloned().collect(),
        };
        let (installation, plan, precision) = self.resolve(
            actor,
            id,
            held,
            release,
            hub.clone(),
            &spec,
            &spec.entrypoint,
            &own,
            &spec.lane,
            Some(Box::new({
                let observe = observe.clone();
                move |stage: &str, done: u64, total: u64| observe(stage, done, total)
            })),
            Some(&stoppable.stop),
        )?;
        let interface: Value = serde_json::from_slice(&installation.interface)
            .map_err(|_| refused("package_interface_invalid", "held interface is corrupt"))?;
        let generation = self.service.catalog.resolve(&installation.generation)?;
        // A call's weights come from here on, in the Start pass's order, until its executor
        // asks for them itself; its import-only parent starts now unless it already did.
        let mut ahead = None;
        if let (false, Some(pool)) = (spec.warm, self.service.pool()) {
            ahead = plan.as_ref().map(|plan| {
                let order = self.service.engine.get(id).map_or(u64::MAX, |record| record.root_seq());
                let fetch = crate::access::Fetch::new(self.publisher.machine().clone(), hub.clone(), order);
                let room = |missing| self.service.room_for(id, missing);
                match fetch.ahead(&plan.upfront(), &plan.on_demand(), !pool.loaded(plan), &self.service.engine, &room) {
                    Ok(()) => crate::access::meter(self.service.engine.clone(), id.to_string(), &fetch),
                    Err(error) => tracing::warn!("run {id}: its weights' early fetch: {error}"),
                }
                fetch
            });
        }
        if let (false, Some(gpu)) = (spec.warm, self.service.gpu()) {
            gpu.prespawn(generation.clone());
            // A pipeline's next packages: a root's calls go to the other packages' Apps its
            // environment holds, so their parents start now too, under the same budget.
            if spec.parent.is_empty() {
                for callee in generation.record.callees.iter().filter(|callee| !callee.package.starts_with("runtime/")) {
                    match generation.application(&callee.application) {
                        Ok(view) => gpu.prespawn(view),
                        Err(error) => tracing::warn!("run {id}: {}'s parent: {error}", callee.application),
                    }
                }
            }
        }
        let fallback = (!generation.record.sdk_fallback.is_empty())
            .then(|| format!("{} {}: {}", installation.package, installation.release, generation.record.sdk_fallback));
        for warning in fallback.into_iter().chain(precision) {
            if let Err(error) = self.service.engine.append_warning(id, &warning) {
                tracing::warn!("run {id}: {warning} ({error})");
            }
        }
        let declared = api::interface::entrypoint(&interface, &spec.entrypoint);
        if declared.is_none() && !install_only {
            return Err(refused(
                "invalid_entrypoint",
                format!("{0} declares no entrypoint {1:?}: `cozy run {0}` lists its functions", installation.package, spec.entrypoint),
            ));
        }
        if spec.parent.is_empty() {
            if let Some(entry) = declared {
                public_root(entry)?;
            }
        }
        // A choice for another callable goes to the call that names it: one of this package's,
        // or a slot a callee App of its environment declares. One naming neither is a warning.
        for choice in spec.models.iter().filter(|c| !install_only && !own_choice(c, &spec.entrypoint)) {
            let declared = addressed(&choice.parameter, &installation.package, &interface).is_some()
                || generation.record.callees.iter().any(|callee| {
                    addressed(&choice.parameter, &callee.package, &callee.interface).is_some()
                });
            if !declared {
                let warning = format!(
                    "model choice {:?} names no model slot of this function or the packages it calls; it is ignored",
                    choice.parameter
                );
                if let Err(error) = self.service.engine.append_warning(id, &warning) {
                    tracing::warn!("run {id}: {warning} ({error})");
                }
            }
        }
        if spec.warm {
            // A package that does not import says so now, not at its first call.
            if install_only && self.service.gpu().is_some() {
                observe("importing it", 0, 0);
            }
            if let Some((code, said)) = self.service.gpu().filter(|_| install_only).and_then(|gpu| gpu.import_verdict(&generation, &stoppable.stop)) {
                let warning = format!("{code}: {said}. Its calls will fail the same way");
                if let Err(error) = self.service.engine.append_warning(id, &warning) {
                    tracing::warn!("run {id}: {warning} ({error})");
                }
            }
            let models = match install_only {
                true => self.warm_models(actor, id, &spec, &*observe, stoppable)?,
                false => vec![],
            };
            // A function that loads no model itself runs on what it calls, and which of its package's internal functions
            // that is, only it knows: H3's shot takes one transformer with references and another without. So what they all
            // load, and that only.
            let plans: Vec<_> = match plan {
                None if !install_only => {
                    let mut plans = vec![];
                    for callee in internal_models(&installation)? {
                        let own: Vec<_> = spec.models.iter().filter(|c| own_choice(c, &callee)).cloned().collect();
                        let report = observe.clone();
                        let resolved = self.resolve(actor, id, Some(installation.clone()), None, hub.clone(), &spec, &callee, &own, &spec.lane,
                            Some(Box::new(move |stage: &str, done: u64, total: u64| report(stage, done, total))), Some(&stoppable.stop));
                        match resolved {
                            Ok((_, plan, ..)) => plans.extend(plan),
                            Err(error) if plans.is_empty() => tracing::warn!("run {id}: {callee}'s weights aren't fetched ahead: {}", error.message),
                            Err(_) => {}
                        }
                    }
                    plans
                }
                plan => plan.into_iter().collect(),
            };
            // A function's warm run ends holding what its constructions read (`downloaded`), its
            // on-demand models' too: only if that fits. It is a guess, so it never makes room.
            let mut moved = tensorfs::fetch::Landed::default();
            if !plans.is_empty() {
                let each: Vec<_> = plans.iter().map(Plan::read).collect();
                // Only what they all share: that's sure to be read. The rest comes when its run begins, which on a slow link costs no
                // more time than fetching it now, as fetching ahead is canceled then, and on a fast one costs seconds, while bytes for a
                // function that isn't called are paid for wherever downloads are billed.
                let fits = |absent: u64| {
                    let free = crate::reclaim::Disk::measure(self.publisher.store().root())?.free();
                    match absent > free {
                        true => Err(refused("machine_disk_full", format!("its weights' {absent} bytes don't fit in the {free} free"))),
                        false => Ok(()),
                    }
                };
                let weights = format!("{}'s weights", installation.package);
                moved = self.download(hub.clone(), &shared_of(&each), fits, &*observe, stoppable, &weights, &self.service.engine.caller_id(id))?;
            }
            // The installed release, its interface and the installation a call may name, in the
            // warm run's result.
            let installed = json!({"package": installation.package, "release": installation.release,
                "installation": installation.alias, "interface": interface,
                "bytes": {"hub": moved.origin, "volume": moved.volume, "local": moved.local}});
            return warm_result(installed, models);
        }
        let entry = declared.expect("checked above");
        let degree = match (&plan, self.service.pool()) {
            (Some(plan), _) => plan.degree,
            (None, Some(pool)) => pool
                .degree(entry, spec.gpus),
            (None, None) => 0,
        };
        let widths = match self.service.pool() {
            Some(pool) if spec.parent.is_empty() => pool.widths(entry, &generation.record, spec.gpus),
            _ => vec![],
        };
        // A memoized callable's run is answered from the success this machine holds for its
        // signer under the same computation, and starts nothing. A writer (weights outputs,
        // published media) has an effect beyond its result: it always runs.
        let writer = api::interface::weights_outputs(entry).next().is_some()
            || api::interface::publishes(entry)
            || !spec.weights_destination.is_empty();
        let memo = match writer {
            false => call_memo::of(
                &generation.record,
                &spec.application,
                api::interface::invocable(entry),
                &spec.input,
                &spec.inputs,
                (&spec.attention_kernel, &spec.lane),
                call_memo::models(plan.as_ref(), &own),
            ),
            true => Memo::Off,
        };
        let (memo, answer) = match memo {
            Memo::Key(computation, basis) => {
                let answer = self.objects.recall(actor, &computation, id)?;
                let basis = basis.name().into();
                (CallMemo { memoized: answer.is_some(), computation, basis, ..Default::default() }, answer)
            }
            Memo::Unmet(why) => (CallMemo { unmemoized: why, ..Default::default() }, None),
            Memo::Off => (CallMemo::default(), None),
        };
        // Its children prepare with its context, and its weights travel under its access.
        if answer.is_none() {
            let context = RunContext {
                installation: installation.alias.clone(),
                hub,
                providers: spec.providers.clone(),
                tensorfs: spec.tensorfs.clone(),
                weights_destination: spec.weights_destination.clone(),
                owner: spec.owner.clone(),
                catalog_revision: spec.catalog_revision.clone(),
                attention_kernel: spec.attention_kernel.clone(),
                gpus: spec.gpus,
                lane: spec.lane.clone(),
                models: spec.models.clone(),
                ahead,
            };
            self.contexts.lock().unwrap().insert(id.into(), context);
        }
        let bound = self.service.bind_prepared(
            id,
            &installation.generation,
            Call {
                entrypoint: spec.entrypoint,
                input: spec.input,
                attention_kernel: spec.attention_kernel,
                gpus: spec.gpus,
                lane: spec.lane.clone(),
                inputs: spec.inputs,
                degree,
                widths,
                parent: spec.parent,
                application: spec.application,
            },
            plan.as_ref().map(|plan| plan.id.as_str()).unwrap_or_default(),
            memo,
            answer,
        );
        if bound.is_err() {
            self.end(id);
        }
        bound?;
        Ok(None)
    }

    /// Every chunk of `selected` (header, components) here from `hub`, its bytes the warm run's
    /// progress and the machine's download of `weights` for run `caller`; `room` is asked for those absent first.
    #[allow(clippy::too_many_arguments)]
    fn download(&self, hub: Option<hub::Source>, selected: &[(String, Vec<String>)], room: impl FnOnce(u64) -> Result<(), Refused>,
        observe: &(dyn Fn(&str, u64, u64) + Sync), stoppable: &Stoppable, weights: &str, caller: &str) -> Result<tensorfs::fetch::Landed, Refused> {
        let fetch = crate::access::Fetch::new(self.publisher.machine().clone(), hub, u64::MAX);
        room(fetch.absent(selected)?)?;
        let wait = crate::activity::wait(crate::activity::Kind::Download, weights, caller);
        let report = |done, total| {
            wait.moved("", done, total);
            observe("downloading its weights", done, total)
        };
        fetch.download(selected, &stoppable.stop, &report).map_err(|e| match stoppable.stop.is_cancelled() {
            true => refused(crate::published::CANCELED, "the run was canceled while it downloaded"),
            false => {
                let text = e.to_string();
                match text.starts_with("machine_disk_full") {
                    true => refused("machine_disk_full", text + &crate::reclaim::advice(self.publisher.store().root(), 0)),
                    false => refused("model_fetch_failed", text),
                }
            }
        })
    }

    /// A warm set member, built one way whether a warm run makes it or a restart brings it back
    /// (`restore_warm_sets`). Its weights come behind every run's, under `hub`: the run's access
    /// with its signer's newest capability. Without one (none survives a restart) they come from
    /// the rental's own Hub, else anonymously from its installation's Hub: a public release
    /// resumes, a private one waits for its owner's next run.
    fn member(&self, held: HeldGeneration, installation: &Installation, plan: Option<Plan>, level: Level,
        hub: Option<hub::Source>, payload: Option<api::machine::Json>) -> Arc<Member> {
        let hub = hub.or_else(|| {
            let own = self.own_hub.clone().filter(|own| {
                installation.hub.is_empty() || hub::origin_key(&own.origin).as_deref() == Some(installation.hub.as_str())
            });
            own.or_else(|| hub::Source::new(&installation.hub, None, None).ok())
        });
        let fetch = plan.is_some().then(|| crate::access::Fetch::new(self.publisher.machine().clone(), hub, u64::MAX));
        Member::new(held, plan, level, fetch, payload)
    }

    /// Each caller's warm set as the journal keeps it, built again (`member`) and brought up in
    /// the background: a restarted machine re-warms without its controller. A member whose
    /// installation is gone leaves the set.
    pub(crate) fn restore_warm_sets(&self) -> io::Result<()> {
        let Some(gpu) = self.service.gpu() else {
            return Ok(());
        };
        let kept = self.service.engine.with_journal(|journal| journal.warm_sets())?;
        let actors: std::collections::BTreeSet<&String> = kept.iter().map(|(actor, _)| actor).collect();
        for actor in actors {
            let all = kept.iter().filter(|(of, _)| of == actor);
            let (records, members): (Vec<String>, Vec<Arc<Member>>) = all
                .clone()
                .filter_map(|(_, record)| Some((record.clone(), self.restore_member(&gpu, actor, record)?)))
                .unzip();
            if records.len() < all.count() {
                self.service.engine.with_journal(|journal| journal.replace_warm_set(actor, &records))?;
            }
            gpu.set_members(actor, members);
        }
        self.service.rewarm(&gpu)
    }

    /// One kept member, with its plan as it was bound while the store still holds its weights.
    fn restore_member(&self, gpu: &Pool, actor: &str, record: &str) -> Option<Arc<Member>> {
        let kept: KeptMember = serde_json::from_str(record).ok()?;
        let installed = self.service.engine.installation(actor, &kept.installation).ok()??;
        let held = self.service.catalog.resolve(&installed.generation).ok()?;
        let bound = match kept.preparation.as_str() {
            "" => None,
            id => self.service.engine.preparation(actor, id).ok()?,
        };
        let plan = bound.and_then(|bound| gpu.plan(&bound).ok()).filter(|plan| gpu.holds(plan));
        let item: api::machine::WarmItem = serde_json::from_slice(&STANDARD.decode(&kept.item).ok()?).ok()?;
        Some(self.member(held, &installed, plan, Level::named(&kept.level)?, None, item.payload))
    }

    /// A warm run's set: every member installed and its weights downloaded as a warm run of it
    /// alone does, the set kept in the journal, and one pass to bring the members up. The
    /// result lists each member's `level` and why it is lower than asked (`held_back`).
    fn warm_set(
        &self,
        actor: &str,
        id: &str,
        spec: &Spec,
        items: &[SetItem],
        observe: &Arc<impl Fn(&str, u64, u64) + Send + Sync + 'static>,
        stoppable: &Stoppable,
    ) -> Result<Option<ResultRecord>, Refused> {
        let hub = spec.hub.clone().or_else(|| self.own_hub.clone());
        let (mut members, mut kept, mut rows) = (vec![], vec![], vec![]);
        for item in items {
            let (held, release) = match &item.source {
                Source::Installation(alias) => {
                    let held = self.service.engine.installation(actor, alias)?;
                    let absent = || refused("installation_absent", "this machine holds no such installation for this signer");
                    (Some(held.ok_or_else(absent)?), None)
                }
                Source::Release { package, release } => (None, Some((package.clone(), release.clone()))),
                _ => return Err(refused("invalid_request", "a warm set member names a release or an installation")),
            };
            // `installed` stops at the code: nothing of the function is resolved or downloaded.
            let (entrypoint, choices) = match item.level {
                Level::Installed => ("", &[][..]),
                _ => (item.entrypoint.as_str(), &item.models[..]),
            };
            let report = observe.clone();
            let (installation, plan, _) = self.resolve(
                actor,
                id,
                held,
                release,
                hub.clone(),
                spec,
                entrypoint,
                choices,
                &item.lane,
                Some(Box::new(move |stage: &str, done: u64, total: u64| report(stage, done, total))),
                Some(&stoppable.stop),
            )?;
            if item.level > Level::Installed && !declares(&installation, &item.entrypoint)? {
                return Err(refused(
                    "invalid_entrypoint",
                    format!("{0} declares no entrypoint {1:?}: `cozy run {0}` lists its functions", installation.package, item.entrypoint),
                ));
            }
            // A request prepares a function's own GPU model; one with none warms what it calls, as ever.
            let unbound = item.payload.is_some() && plan.is_none();
            if unbound {
                let warning = format!("{} binds no GPU model of its own: its inputs are not warmed, only the models it calls", item.entrypoint);
                if let Err(error) = self.service.engine.append_warning(id, &warning) {
                    tracing::warn!("run {id}: {warning} ({error})");
                }
            }
            // A function that loads no model itself (an orchestrator) runs on the ones it calls.
            // The interface names no call graph, and internal functions exist only to be called:
            // it brings its package's internal functions' models, each a member of its own.
            let mut plans = vec![plan];
            if item.level > Level::Installed && plans[0].is_none() {
                for callee in internal_models(&installation)? {
                    let own: Vec<_> = item.models.iter().filter(|c| own_choice(c, &callee)).cloned().collect();
                    let report = observe.clone();
                    let (_, plan, _) = self.resolve(
                        actor,
                        id,
                        Some(installation.clone()),
                        None,
                        hub.clone(),
                        spec,
                        &callee,
                        &own,
                        &item.lane,
                        Some(Box::new(move |stage: &str, done: u64, total: u64| report(stage, done, total))),
                        Some(&stoppable.stop),
                    )?;
                    plans.push(plan);
                }
            }
            rows.push((json!({"package": installation.package, "release": installation.release,
                "entrypoint": item.entrypoint}), plans.len()));
            for plan in plans {
                // `downloaded` is all the function may read on this disk once its warm run ends, its on-demand models'
                // too, so its first call waits on no download (cl-790). Room is made for it as for a run.
                if let (Level::Downloaded, Some(plan)) = (item.level, &plan) {
                    let room = |absent| self.service.room_for(id, absent)
                        .map_err(|why| refused("machine_disk_full", why.trim_start_matches("machine_disk_full: ")));
                    let weights = format!("{}'s weights", installation.package);
                    self.download(hub.clone(), &plan.read(), room, &**observe, stoppable, &weights, &self.service.engine.caller_id(id))?;
                }
                kept.push(serde_json::to_string(&KeptMember {
                    installation: installation.alias.clone(),
                    level: item.level.name().into(),
                    preparation: plan.as_ref().map(|plan| plan.id.clone()).unwrap_or_default(),
                    item: STANDARD.encode(&item.sent),
                })
                .map_err(io::Error::other)?);
                let held = self.service.catalog.resolve(&installation.generation)?;
                members.push(self.member(held, &installation, plan, item.level, hub.clone(), item.payload.clone().filter(|_| !unbound)));
            }
        }
        self.service
            .engine
            .with_journal(|journal| journal.replace_warm_set(actor, &kept))?;
        observe("warming", 0, 0);
        let (holds, programs) = match self.service.gpu() {
            Some(gpu) => {
                gpu.set_members(actor, members);
                gpu.keep_preparing(&self.service.engine, actor, stoppable);
                (gpu.members(actor), gpu.prepared_members(actor))
            }
            None => (members.iter().map(|member| without_gpus(member.level)).collect(), vec![None; members.len()]),
        };
        // An item's level is its lowest member's, and why that one is held back.
        let mut holds = holds.into_iter();
        let mut programs = programs.into_iter();
        let rows: Vec<Value> = rows
            .into_iter()
            .map(|(mut row, count)| {
                let lowest = holds.by_ref().take(count).min_by_key(|(level, _)| *level);
                let prepared: Vec<bool> = programs.by_ref().take(count).flatten().collect();
                if !prepared.is_empty() { row["prepared"] = prepared.iter().all(|ready| *ready).into(); }
                if let Some((level, held_back)) = lowest {
                    row["level"] = level.name().into();
                    if !held_back.is_empty() {
                        row["held_back"] = held_back.into();
                    }
                }
                row
            })
            .collect();
        Ok(Some(ResultRecord {
            value: json!({"set": rows}),
            artifacts: vec![],
            asset_bindings: vec![],
            adjustments: vec![],
        }))
    }

    /// A remove run: the caller's installations its source names leave the journal, their
    /// warm set members and the GPU pool, and each environment no installation or unfinished
    /// run names any more is deleted unless a process still holds it (reclaim deletes it once
    /// that ends). A release names its package at the run's Hub (any Hub for local/ code, or
    /// for an installation recorded before installations named theirs).
    fn remove(&self, actor: &str, spec: &Spec) -> Result<ResultRecord, Refused> {
        let hub = spec.hub.clone().or_else(|| self.own_hub.clone());
        let origin = hub.and_then(|hub| hub::origin_key(&hub.origin)).unwrap_or_default();
        let named = |installed: &Installation| match &spec.source {
            Source::Installation(alias) => &installed.alias == alias,
            Source::Release { package, release } => {
                &installed.package == package
                    && (release.is_empty() || &installed.release == release)
                    && (package.starts_with("local/") || installed.hub.is_empty() || installed.hub == origin)
            }
            Source::Local(_) | Source::Models => false,
        };
        let (removed, reclaimed, retained) = self.service.uninstall(actor, named)?;
        for gone in removed.iter().filter(|i| i.package.starts_with("local/")) {
            self.local.collect(actor, &gone.package);
        }
        let rows: Vec<Value> = removed
            .iter()
            .map(|i| json!({"package": i.package, "release": i.release, "installation": i.alias, "hub": i.hub}))
            .collect();
        Ok(ResultRecord {
            value: json!({"removed": rows, "reclaimed_bytes": reclaimed, "retained": retained}),
            artifacts: vec![],
            asset_bindings: vec![],
            adjustments: vec![],
        })
    }

    /// A local package is one installation: the state just installed replaces its earlier
    /// ones, whose executors end and environments' records leave. One an unfinished run names
    /// stays until a later install finds it idle.
    fn supersede(&self, actor: &str, current: &Installation) -> Result<(), Refused> {
        let unsettled = self.service.engine.with_journal(|j| j.unsettled_generations())?;
        let preparing = self.local.preparing();
        self.service.uninstall(actor, |prior| {
            prior.package == current.package
                && prior.alias != current.alias
                && prior.alias.starts_with("local-")
                && !unsettled.contains(&prior.generation)
                && !preparing.contains(&prior.generation)
        })?;
        Ok(())
    }

    /// The run's installation, model plan and precision warnings, from the one resolver
    /// (`Publisher`): a release and a Hub-named model need the run's Hub access; held code and
    /// the models this machine holds (`local/` aliases, checkpoints by hash) do not. Unobserved,
    /// it is a hint's: its downloads go behind the ones calls wait on.
    #[allow(clippy::too_many_arguments)]
    fn resolve(
        &self,
        actor: &str,
        id: &str,
        held: Option<Installation>,
        release: Option<Published>,
        hub: Option<hub::Source>,
        spec: &Spec,
        entrypoint: &str,
        choices: &[domain::ModelChoice],
        lane: &str,
        observe: Option<crate::published::Observer>,
        stop: Option<&Stop>,
    ) -> Result<(Installation, Option<crate::gpu_service::Plan>, Vec<String>), Refused> {
        let (package, release) = release.unwrap_or_default();
        let request = Request {
            run: id.into(),
            source: hub,
            package,
            release,
            installed: held,
            owner: spec.owner.clone(),
            catalog_revision: spec.catalog_revision.clone(),
            providers: spec.providers.clone(),
            entrypoint: entrypoint.into(),
            choices: choices.to_vec(),
            gpus: spec.gpus,
            lane: lane.into(),
        };
        let prepared = self
            .publisher
            .prepare_now(&self.service, actor, &request, observe, stop)
            .map_err(|(code, message)| refused(code, message))?;
        Ok((prepared.installation, prepared.plan, prepared.warnings))
    }

    /// A child call of a running attempt: a run `<request>` under its parent's signer,
    /// idempotent on `intent`, preparing inside itself with its parent's context and the
    /// choices addressed to its callable.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn child(
        self: &Arc<Self>,
        parent: &Execution,
        request: &str,
        intent: &str,
        application: &str,
        entrypoint: &str,
        input: Value,
        inputs: Vec<InputFile>,
        passed: Vec<domain::ModelChoice>,
    ) -> Result<Execution, Refused> {
        let actor = parent.submission.as_ref().map(|s| s.actor.clone()).unwrap_or_default();
        // A call the same attempt made already is its run: nothing prepares.
        if let Some(existing) = self.existing(&actor, request, intent)? {
            return Ok(existing);
        }
        let spec =
            self.child_spec(&actor, &parent.id, application, entrypoint, input, inputs, intent, &passed)?;
        self.submit(&actor, request, spec)
    }

    /// The installation as a callee's child runs see it: the callee's package, release and
    /// interface (its weights and bindings are its own), in the caller's environment.
    fn callee_view(&self, installed: Installation, application: &str) -> Result<Installation, Refused> {
        if application.is_empty() {
            return Ok(installed);
        }
        let held = self.service.catalog.resolve(&installed.generation)?;
        if application == held.record.application {
            return Ok(installed);
        }
        let (package, release, interface) = held.record.app(application).ok_or_else(|| {
            refused("child_undeclared", format!("this environment holds no App {application}"))
        })?;
        Ok(Installation {
            package: package.into(),
            release: release.into(),
            interface: serde_json::to_vec(interface).map_err(io::Error::other)?,
            ..installed
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn child_spec(
        &self,
        actor: &str,
        parent: &str,
        application: &str,
        entrypoint: &str,
        input: Value,
        inputs: Vec<InputFile>,
        digest: &str,
        passed: &[domain::ModelChoice],
    ) -> Result<Spec, Refused> {
        let context = self.contexts.lock().unwrap().get(parent).cloned().ok_or_else(|| {
            refused("child_call_refused", "the caller's preparation context is gone")
        })?;
        let installed = self.service.engine.installation(actor, &context.installation)?
            .ok_or_else(|| refused("child_call_refused", "the caller's installation is gone"))?;
        let caller = installed.package.clone();
        let installed = self.callee_view(installed, application)?;
        let callee_interface: Value = serde_json::from_slice(&installed.interface)
            .map_err(|_| refused("package_interface_invalid", "held interface is corrupt"))?;
        let prefix = format!("{entrypoint}.");
        let mut spec = Spec {
            remove: false,
            warm: false,
            set: None,
            parent: parent.into(),
            source: Source::Installation(context.installation),
            entrypoint: entrypoint.into(),
            input,
            inputs,
            // A child takes the caller's choices addressed to a slot its own interface declares
            // (a callee's included), named by that slot's path; any other was a warning at the
            // caller's preparation.
            models: context
                .models
                .into_iter()
                .filter_map(|choice| {
                    let path = addressed(&choice.parameter, &installed.package, &callee_interface)
                        .filter(|path| path.starts_with(&prefix))?;
                    Some(domain::ModelChoice { parameter: path, ..choice })
                })
                .collect(),
            attention_kernel: context.attention_kernel,
            // The caller's count caps a child's group, else the GPUs its root took; the child's own
            // declaration picks within it.
            gpus: match context.gpus {
                0 => self.root_width(parent),
                gpus => gpus,
            },
            // The asked lane follows the main model into the package's own children (long_form's
            // segments); another package's model (qwen-image-2 for references) takes its default.
            lane: if installed.package == caller { context.lane.clone() } else { String::new() },
            hub: context.hub,
            providers: context.providers,
            tensorfs: context.tensorfs,
            weights_destination: String::new(),
            owner: context.owner,
            catalog_revision: context.catalog_revision,
            application: application.into(),
            digest: digest.into(),
            journaled: vec![],
        };
        // Both demand and prefetch overlay the call's choices on the same inherited ones. One
        // naming a model replaces the slot's; one of LoRAs alone keeps the slot's model and stacks
        // them after its LoRAs: the same LoRA again takes the new strength, and strength 0 drops
        // it. Declaration and data-access validation stay in the existing resolver.
        for choice in passed {
            let Some(path) = addressed(&choice.parameter, &installed.package, &callee_interface)
                .filter(|path| path.starts_with(&prefix))
            else {
                tracing::warn!("{}: a call's model choice {} names no slot of {entrypoint}: ignored", installed.package, choice.parameter);
                continue;
            };
            let prior = spec.models.iter().position(|prior| prior.parameter == path).map(|at| spec.models.remove(at));
            let names_model = !choice.repository.is_empty() || choice.manifest.is_some() || !choice.source.is_empty();
            let mut chosen = match prior {
                Some(prior) if !names_model => prior,
                _ => domain::ModelChoice { parameter: path, adapters: vec![], ..choice.clone() },
            };
            for adapter in &choice.adapters {
                let named = |held: &&mut domain::DownloadAdapterRef| {
                    domain::DownloadAdapterRef { scale: adapter.scale.clone(), ..(*held).clone() } == *adapter
                };
                match chosen.adapters.iter_mut().find(named) {
                    Some(held) => held.scale = adapter.scale.clone(),
                    None => chosen.adapters.push(adapter.clone()),
                }
            }
            chosen.adapters.retain(|adapter| adapter.scale != "0");
            spec.models.push(chosen);
        }
        Ok(spec)
    }

    /// `model_prefetch`: an acknowledged hint about a future child call. TensorD resolves
    /// its models, downloading behind every download a call waits on, and asks the GPU pool
    /// to prewarm a Runtime construction when admitted.
    pub(crate) fn prefetch(
        self: &Arc<Self>, parent: &Execution, application: &str, entrypoint: &str,
        passed: Vec<domain::ModelChoice>,
    ) -> Result<(), Refused> {
        let actor = parent.submission.as_ref().map(|s| s.actor.clone()).unwrap_or_default();
        let spec = self.child_spec(&actor, &parent.id, application, entrypoint,
            json!({}), vec![], "", &passed)?;
        let Some(gpu) = self.service.gpu() else {
            return Ok(());
        };
        // Downloads may outlast the hint's GPU phase; the GPU load waits for its root's lane.
        let (root, family) = (parent.root_seq(), parent.family().map(String::from));
        let owner = parent.id.clone();
        let (runs, entrypoint) = (self.clone(), entrypoint.to_string());
        let application = application.to_string();
        let started = std::thread::Builder::new().name("child-prefetch".into()).spawn(move || {
            let prepared = (|| {
                    let Source::Installation(alias) = &spec.source else { unreachable!() };
                    let held = match runs.service.engine.installation(&actor, alias)? {
                        Some(installed) => Some(runs.callee_view(installed, &spec.application)?),
                        None => None,
                    };
                    let (hub, models) = (spec.hub.clone(), spec.models.clone());
                    runs.resolve(&actor, "", held, None, hub, &spec, &spec.entrypoint, &models, &spec.lane, None, None)
                })();
            match prepared {
                Ok((installation, Some(plan), _)) if plan.degree > 0 && !plan.slots.is_empty() => {
                    match runs.service.catalog.resolve(&installation.generation)
                        .and_then(|held| held.application(&application)) {
                        Ok(held) => {
                            // Its weights come after its root's own, under the child's access.
                            let hub = spec.hub.clone().or_else(|| runs.own_hub.clone());
                            let fetch = crate::access::Fetch::new(runs.publisher.machine().clone(), hub, root);
                            gpu.prefetch(&runs.service.engine, held, plan, (root, family.clone()), owner.clone(), fetch)
                        }
                        Err(error) => tracing::warn!("prefetch of {entrypoint}: {error}"),
                    }
                }
                Ok(_) => (),
                Err(refusal) => {
                    tracing::warn!("prefetch of {entrypoint}: {}: {}", refusal.code, refusal.message)
                }
            }
        });
        if let Err(error) = started {
            tracing::warn!("prefetch of a child: {error}");
        }
        Ok(())
    }

    /// Observe required-input readiness without consuming the existing fetch owner.
    pub(crate) fn source_ready(&self, id: &str) -> crate::access::Ready {
        self.contexts.lock().unwrap().get(id)
            .and_then(|context| context.ahead.as_ref())
            .map_or(crate::access::Ready::Yes, |fetch| fetch.ready())
    }

    /// The fetches a run's preparation started, for its call to end once its executor asks.
    pub(crate) fn ahead(&self, id: &str) -> Option<Arc<crate::access::Fetch>> {
        self.contexts.lock().unwrap().get_mut(id).and_then(|context| context.ahead.take())
    }

    /// The run's TensorFS options for its executors: its Hub through the gateway when it reads
    /// there under its signer's run capability, so they hold no token of the Hub's (cl-675).
    pub(crate) fn tensorfs(&self, id: &str) -> Option<api::machine::TensorFSOptions> {
        let contexts = self.contexts.lock().unwrap();
        let context = contexts.get(id)?;
        let options = context.tensorfs.clone()?;
        let Some(capability) = context.hub.as_ref().and_then(|hub| hub.capability_at(&options.endpoint)) else {
            return Some(options);
        };
        match crate::gateway::url() {
            Ok(endpoint) => Some(api::machine::TensorFSOptions { endpoint, token: capability.secret().into(), ..Default::default() }),
            Err(error) => {
                tracing::warn!("the Hub gateway: {error}");
                Some(options)
            }
        }
    }

    /// A run's weights output, the commit `tree` its Runtime wrote here, as a new commit of its
    /// weights destination in lane `lane` (the output's name): that commit; none when the run
    /// has no Hub destination.
    pub(crate) fn publish(&self, id: &str, tree: &str, lane: &str) -> Result<Option<(String, ObjectRef)>, Refused> {
        let (destination, options, hub) = {
            let contexts = self.contexts.lock().unwrap();
            let Some(context) = contexts.get(id) else { return Ok(None) };
            let destination = context.weights_destination.trim_start_matches("model://").to_owned();
            if destination.is_empty() || destination.starts_with("local/") {
                return Ok(None);
            }
            let options = context.tensorfs.clone().ok_or_else(|| refused("upload_endpoint_absent",
                format!("uploading to {destination} takes the run's TensorFS endpoint, and it carries none")))?;
            (destination, options, context.hub.clone())
        };
        let store = self.publisher.store();
        let tree = crate::storage::held(store, tree).map_err(|e| refused("checkpoint_absent", format!("{tree}: {e}")))?;
        let parsed = store.tree(&tree).map_err(|e| refused("checkpoint_invalid", e.to_string()))?;
        let header = parsed.header().map_err(|e| refused("checkpoint_invalid", e.to_string()))?.1.clone();
        let files = parsed.repo_files().map(|(p, r)| (p.to_owned(), r.clone())).collect();
        let control = tensorfs::fetch::UploadControl { cancelled: &|| false, progress: &|_, _| {}, commit: &|| true };
        let uploaded = upload(&options, hub.as_ref(), &destination, lane, &header, &files, store, &control)?;
        Ok(Some((destination, uploaded.commit)))
    }

    /// The Hub a run reads and writes: its own access, else the machine's.
    pub(crate) fn hub(&self, id: &str) -> Option<hub::Source> {
        let context = self.contexts.lock().unwrap().get(id).and_then(|c| c.hub.clone());
        context.or_else(|| self.own_hub.clone())
    }

    /// A run ended: its children prepare no more, and its access is forgotten.
    pub(crate) fn end(&self, id: &str) {
        self.contexts.lock().unwrap().remove(id);
    }
}
