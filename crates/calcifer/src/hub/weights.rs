//! Models on machines: downloads and uploads are warm runs on the machine `on` names, which
//! fetches, makes and publishes the bytes itself, so no weight passes through this computer; a
//! local file goes to the machine as the run's input. A Hub commit uploads nothing: the Hub
//! records it as a commit of another model (`commits`).
use std::{
    io::Read as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json,
    body::Bytes,
    extract::{State, rejection::BytesRejection},
    http::StatusCode,
};
use protocol::{
    calcifer::{Agreed, Kind as RunKind, On, OutputItem, Run, RunQuote, StagedInput, SubmitRun, Warning},
    config::Origin,
    error::{Error, Kind},
    hub::{Mirrored, ModelDownload, ModelUpload, ModelUploading, RecordedCommit},
    ids::Digest,
    machine::{self as v1, InputFile, ModelChoice},
    model_ref::{self as refs, ModelRef, Repo},
};
use sha2::{Digest as _, Sha256};

use super::{
    after::{After, display},
    asks::{Ask, Prepared, submit},
    origin, refusal, signed_in,
};
use crate::api::{Api, IdempotencyKey, Refusal, parse};

fn usage(code: &str, message: impl Into<String>) -> Refusal {
    Refusal(Error::new(Kind::Usage, code, message))
}

/// What a warm run makes present: its model, the model it starts from (`base`), and the local
/// files it reads.
struct Made {
    choice: ModelChoice,
    base: Option<ModelChoice>,
    staged: Vec<StagedInput>,
    files: Vec<InputFile>,
}

impl From<(ModelChoice, Option<StagedInput>)> for Made {
    fn from((choice, staged): (ModelChoice, Option<StagedInput>)) -> Self {
        Self { choice, base: None, staged: staged.into_iter().collect(), files: vec![] }
    }
}

/// A local path this computer's Calcifer reads: absolute, since a client's working directory is
/// its own.
fn local(raw: &str) -> Result<Option<&Path>, Refusal> {
    match raw {
        raw if Path::new(raw).is_absolute() => Ok(Some(Path::new(raw))),
        raw if raw.starts_with('.') => Err(usage("path_not_absolute", format!("{raw}: name a local file by its absolute path"))),
        _ => Ok(None),
    }
}

/// The model a warm run makes present: a reference, or a local `.safetensors` file, measured.
async fn source(raw: &str) -> Result<(ModelChoice, Option<StagedInput>), Refusal> {
    let Some(path) = local(raw)? else {
        return Ok((ModelRef::parse(raw).map_err(Refusal)?.choice("model"), None));
    };
    let path = path.to_owned();
    let staged = tokio::task::spawn_blocking({
        let path = path.clone();
        move || measure(&path)
    })
    .await
    .map_err(|e| Refusal(Error::internal("measure", e.to_string())))??;
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    let choice = ModelChoice {
        parameter: "model".into(),
        source: format!("object://{}/{}", staged.digest, escape(&name)),
        ..Default::default()
    };
    Ok((choice, Some(staged)))
}

/// The file `named` holds (`PUT /v1/files`, `sha256:<hex>`) as the model a warm run makes present: one `.safetensors` file by its
/// bytes, since a held file has no name to read. Its digest is its name.
fn held(api: &Api, named: &str) -> Result<(ModelChoice, Option<StagedInput>), Refusal> {
    let digest: Digest = named.parse().map_err(|why: &str| usage("file_name_invalid", format!("{named:?}: {why}")))?;
    let path = crate::runs::files::dir(&api.calcifer).join(hex::encode(digest.bytes()));
    if !path.is_file() {
        let said = format!("Calcifer holds no file {named}");
        return Err(Refusal(Error::new(Kind::NotFound, "file_not_held", said).remedy(format!("PUT its bytes to /v1/files/{named} first"))));
    }
    let refused = |why: String| Refusal(Error::new(Kind::Validation, "model_file_refused", why));
    let mut file = std::fs::File::open(&path).map_err(|e| refused(e.to_string()))?;
    let length = file.metadata().map_err(|e| refused(e.to_string()))?.len();
    let mut head = [0; 9];
    file.read_exact(&mut head).map_err(|_| refused(format!("{named} is not a .safetensors file")))?;
    let header = u64::from_le_bytes(head[..8].try_into().expect("8 bytes"));
    if head[8] != b'{' || header.saturating_add(8) > length {
        return Err(refused(format!("{named} is not a .safetensors file")));
    }
    crate::runs::files::touch(&path);
    let choice = ModelChoice { parameter: "model".into(), source: format!("object://{digest}/model.safetensors"), ..Default::default() };
    Ok((choice, Some(StagedInput { path, digest, length })))
}

/// A local model file, measured once: one regular `.safetensors` file.
fn measure(path: &Path) -> Result<StagedInput, Refusal> {
    let refused = |why: String| Refusal(Error::new(Kind::Validation, "model_file_refused", why));
    // A link is its file: a Hugging Face snapshot is all links.
    let regular = std::fs::metadata(path).is_ok_and(|m| m.is_file());
    if !regular || !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("safetensors")) {
        return Err(refused(format!("{} is not one regular .safetensors file", path.display())));
    }
    let mut file = std::fs::File::open(path).map_err(|e| refused(e.to_string()))?;
    let (mut hash, mut buf, mut length) = (Sha256::new(), vec![0; 1 << 20], 0u64);
    loop {
        match file.read(&mut buf).map_err(|e| refused(e.to_string()))? {
            0 => break,
            n => {
                hash.update(&buf[..n]);
                length += n as u64;
            }
        }
    }
    Ok(StagedInput { path: path.to_owned(), digest: Digest::from_bytes(hash.finalize().into()), length })
}

/// A file name as one URI path segment.
fn escape(name: &str) -> String {
    name.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' | b'+' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A plain CozyTensors folder travels as an ordinary retained input tree. The machine admits its
/// headers without importing or fetching the tensor objects a thin folder omits.
async fn folder(api: &Api, path: PathBuf) -> Result<Made, Refusal> {
    let root = api.machines.hubs.home().root().to_owned();
    let staged = tokio::task::spawn_blocking(move || {
        protocol::stage::stage(&path, Some(&root.join("cache/digests")), &root.join("cache/trees"))
    })
    .await
    .map_err(|e| Refusal(Error::internal("stage", e.to_string())))?
    .map_err(Refusal)?;
    let input = InputFile {
        field: "model".into(),
        digest: staged.manifest.digest.to_string(),
        length: staged.manifest.length,
        media_type: protocol::tree::MEDIA.into(),
        order: 0,
    };
    let mut files = staged.files;
    files.push(staged.manifest);
    Ok(Made {
        choice: ModelChoice { parameter: "model".into(), source: "input://model".into(), ..Default::default() },
        base: None,
        staged: files,
        files: vec![input],
    })
}

/// `source_format`: the one format `choice`, a provider source or a local file, converts with.
fn formatted(choice: &mut ModelChoice, format: &str) -> Result<(), Refusal> {
    if choice.source.is_empty() {
        return Err(usage("model_profile_unused", "--source-format converts a provider source or a local file"));
    }
    choice.profiles = vec![format.to_owned()];
    Ok(())
}

/// `base`, `MODEL[:COMPONENT,…]`: choice `base[:COMPONENT,…]` of a Hub model (a release in the
/// commit's `lane`, or a commit) or a `local/` one.
fn base(raw: &str, lane: &str) -> Result<ModelChoice, Refusal> {
    let segment = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    // The last `:` starts a component list unless it is a commit's `sha256:`.
    let (model, names) = match raw.rsplit_once(':') {
        Some((model, names)) if !model.ends_with("@sha256") && names.split(',').all(segment) => (model, Some(names)),
        _ => (raw, None),
    };
    let mut choice = match ModelRef::parse(model).map_err(Refusal)? {
        ModelRef::Provider(_) => return Err(usage("model_base_invalid", format!("--base {model}: name a Hub model or a local/ one"))),
        reference => reference.choice(v1::BASE),
    };
    if !choice.repository.starts_with("local/") && choice.manifest.is_empty() {
        choice.lane = lane.to_owned();
    }
    if let Some(names) = names {
        choice.parameter = format!("{}:{names}", choice.parameter);
    }
    Ok(choice)
}

/// One warm run of `made` on `on`, uploaded to `destination` (a Hub repository, a `local/` alias,
/// or none) in `lane`.
fn warm(hub: &Origin, on: On, agreed: Option<Agreed>, target: String, made: Made, destination: String, lane: String) -> SubmitRun {
    let spec = v1::RunSpec {
        kind: v1::RunKind::Warm,
        lane,
        models: std::iter::once(made.choice).chain(made.base).collect(),
        inputs: made.files,
        weights_destination: destination,
        ..Default::default()
    };
    SubmitRun {
        agreed,
        on,
        hub: hub.to_string(),
        kind: RunKind::Warm,
        target,
        spec,
        inputs: made.staged,
        held: vec![],
        out: None,
        warnings: vec![],
    }
}

impl Ask for ModelDownload {
    async fn prepare(self, api: &Arc<Api>) -> Result<Prepared, Refusal> {
        let hub = origin(api, self.hub.as_deref())?;
        let mut said = vec![];
        let mut destination = match self.alias.as_deref().map(Repo::parse).transpose().map_err(Refusal)? {
            Some(repo) if repo.org == "local" => repo.to_string(),
            Some(_) => return Err(usage("model_alias_invalid", "the name a machine keeps a model under is local/<name>")),
            None => String::new(),
        };
        let mut made = source(&self.model).await?;
        if let Some(format) = &self.source_format {
            match formatted(&mut made.0, format) {
                Ok(()) => {}
                Err(_) => said.push("--source-format converts a provider source or a local file: ignored".to_owned()),
            }
        }
        // A local file is kept under a name: its own, unless it is given one.
        if made.1.is_some() && destination.is_empty() {
            let stem = Path::new(&self.model).file_stem().map(|stem| stem.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let stem: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' }).collect();
            destination = Repo::parse(&format!("local/{stem}")).map_err(Refusal)?.to_string();
            said.push(format!("kept as {destination}"));
        }
        if let Some(lane) = self.lane {
            let lane = refs::label("--lane", &lane).map_err(Refusal)?;
            if made.0.repository.is_empty() || made.0.repository.starts_with("local/") {
                said.push("--lane names a lane of a Hub model: ignored".to_owned());
            } else {
                made.0.lane = lane;
            }
        }
        let mut submit = warm(&hub, self.on, self.agreed, self.model, made.into(), destination, String::new());
        submit.warnings.extend(said.into_iter().map(|message| Warning { code: "model_download".into(), message }));
        Ok(Prepared { submit, after: None })
    }
}

impl Ask for ModelUpload {
    async fn prepare(self, api: &Arc<Api>) -> Result<Prepared, Refusal> {
        let hub = origin(api, self.hub.as_deref())?;
        let destination = Repo::hub(&self.destination).map_err(Refusal)?.to_string();
        let lane = refs::label("--lane", &self.lane).map_err(Refusal)?;
        if self.strength.is_some_and(|s| !s.is_finite() || s.abs() > 10.0) {
            return Err(usage("model_strength_invalid", "--strength is a number between -10 and 10"));
        }
        let commit = matches!(ModelRef::parse(&self.source), Ok(ModelRef::Commit { .. }));
        let converted = !self.source.starts_with("run:") && !commit;
        if self.trained_on.is_some() && !converted {
            return Err(usage("model_trained_on_unused", "--trained-on is read when a LoRA source is converted; this source already was"));
        }
        let display = |offered| After::Display { model: destination.clone(), trigger: self.trigger.clone(), strength: self.strength, offered };
        if let Some(reference) = self.source.strip_prefix("run:") {
            if self.base.is_some() {
                return Err(usage("model_base_invalid", "a run's weights output uploads as it is; upload a model source on a base"));
            }
            // It runs where the weights are, whatever `on` names.
            let submit = from_run(api, reference, &destination, &lane)?;
            return Ok(Prepared { submit, after: Some(display(false)) });
        }
        protocol::calcifer::local_usable(&self.on.to_string()).map_err(Refusal)?;
        // The machine publishes under this computer's sign-in; refuse before anything is made.
        signed_in(api, &hub)?;
        let mut made: Made = match local(&self.source)?.filter(|path| path.is_dir()) {
            Some(path) => {
                if self.trained_on.is_some() {
                    return Err(usage("model_trained_on_unused", "--trained-on is read when a LoRA source is converted; a CozyTensors folder already was"));
                }
                if self.base.is_some() {
                    return Err(usage("model_base_invalid", "a CozyTensors folder uploads as it is; upload a model source on a base"));
                }
                if self.source_format.is_some() {
                    return Err(usage("model_profile_unused", "a CozyTensors folder is already converted; omit --source-format"));
                }
                folder(api, path.to_owned()).await?
            }
            None if self.source.starts_with("sha256:") => held(api, &self.source)?.into(),
            None => source(&self.source).await?.into(),
        };
        if let Some(format) = &self.source_format {
            formatted(&mut made.choice, format)?;
        }
        made.base = self.base.as_deref().map(|raw| base(raw, &lane)).transpose()?;
        let target = match &self.base {
            None => format!("{} → {destination} ({lane})", self.source),
            Some(on) => format!("{} on {on} → {destination} ({lane})", self.source),
        };
        made.choice.trained_on = self.trained_on.clone().unwrap_or_default();
        // A Hub model by name is copied in that lane, as a download's lane reads it; never the
        // machine's pick under another lane's label.
        if !made.choice.repository.is_empty() && !made.choice.repository.starts_with("local/") {
            made.choice.lane = lane.clone();
        }
        let after = display(true);
        let submit = warm(&hub, self.on, self.agreed, target, made, destination, lane);
        Ok(Prepared { submit, after: Some(after) })
    }
}

/// The commit `ask` records rather than uploads: a Hub commit on no base is at the Hub already.
fn at_the_hub(ask: &ModelUpload) -> Option<(Repo, String)> {
    match ModelRef::parse(&ask.source) {
        Ok(ModelRef::Commit { repo, commit }) if ask.base.is_none() => Some((repo, commit)),
        _ => None,
    }
}

/// `POST /v1/model-upload-quotes`: what its run would rent; `null` when it records a commit, which rents nothing.
pub(super) async fn quote(
    State(api): State<Arc<Api>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Option<RunQuote>>, Refusal> {
    let ask: ModelUpload = parse(&body?)?;
    match at_the_hub(&ask) {
        Some(_) => Ok(Json(None)),
        None => Ok(Json(Some(super::asks::quoted(&api, ask).await?))),
    }
}

/// `POST /v1/model-uploads`, any source: a Hub commit on no base is recorded there and nothing runs. Otherwise the mirror is
/// recorded once the upload is admitted, before it is submitted.
pub(super) async fn upload(
    State(api): State<Arc<Api>>,
    IdempotencyKey(key): IdempotencyKey,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<ModelUploading>), Refusal> {
    let ask: ModelUpload = parse(&body?)?;
    let hub = origin(&api, ask.hub.as_deref())?;
    if let Some((from, commit)) = at_the_hub(&ask) {
        let (recorded, mirrored) = record(&api, &hub, ask, from, commit).await?;
        return Ok((StatusCode::OK, Json(ModelUploading { run: None, replay: false, mirrored, recorded: Some(recorded) })));
    }
    let (page, source, destination) = (ask.mirror.clone(), ask.source.clone(), ask.destination.clone());
    let prepared = ask.prepare(&api).await?;
    let mirrored = match page {
        Some(page) => Some(mirror(&api, &hub, &source, &page, &Repo::hub(&destination).map_err(Refusal)?).await?),
        None => None,
    };
    let (status, Json(submitted)) = submit(&api, &key, prepared).await?;
    Ok((status, Json(ModelUploading { run: Some(submitted.run), replay: submitted.replay, mirrored, recorded: None })))
}

/// `run:N[#output]`: the run's weights output, a WARM run on the machine that holds it, uploaded
/// as a commit of `destination` in `lane` without running it again.
fn from_run(api: &Api, reference: &str, destination: &str, lane: &str) -> Result<SubmitRun, Refusal> {
    let (number, output) = match reference.split_once('#') {
        Some((number, output)) => (number, Some(output)),
        None => (reference, None),
    };
    let run = api.calcifer.record(&crate::api::find(api, number)?)?;
    let item = weights(&run, output)?;
    let machine = run.machine.clone().ok_or_else(|| {
        Refusal(Error::new(Kind::Conflict, "run_unplaced", format!("run {} never reached a machine", run.number)))
    })?;
    let hub = Origin::parse(&run.hub).map_err(Refusal)?;
    signed_in(api, &hub)?;
    let checkpoint = item.checkpoint.as_ref().expect("a weights output names its checkpoint");
    let model = ModelChoice {
        parameter: item.name.clone(),
        repository: checkpoint.repo_id.clone(),
        manifest: checkpoint.revision.clone(),
        ..Default::default()
    };
    let spec = v1::RunSpec {
        kind: v1::RunKind::Warm,
        models: vec![model],
        weights_destination: destination.to_owned(),
        lane: lane.to_owned(),
        ..Default::default()
    };
    Ok(SubmitRun {
        agreed: None,
        on: On::Machine(machine),
        hub: run.hub.clone(),
        kind: RunKind::Warm,
        target: format!("upload {}#{} to {destination}", run.number, item.name),
        spec,
        inputs: vec![],
        held: vec![],
        out: None,
        warnings: vec![],
    })
}

/// The weights output named `output`, else the run's only one.
fn weights<'a>(run: &'a Run, output: Option<&str>) -> Result<&'a OutputItem, Refusal> {
    let held: Vec<&OutputItem> = (run.output.iter())
        .filter(|item| item.is_checkpoint() && output.is_none_or(|name| item.name == name))
        .collect();
    match held.as_slice() {
        [item] => Ok(item),
        [] => Err(Refusal(Error::new(
            Kind::NotFound,
            "run_output_absent",
            match output {
                Some(name) => format!("run {} wrote no weights output {name:?}", run.number),
                None => format!("run {} wrote no weights output", run.number),
            },
        ))),
        several => Err(usage(
            "run_output_ambiguous",
            format!("run {} wrote {} weights outputs: name one as {}#<output>", run.number, several.len(), run.number),
        )),
    }
}

/// Records the destination as a mirror of its author's model before anything uploads: an empty
/// `page` names the hf:// or civitai:// source itself.
async fn mirror(api: &Api, hub: &Origin, source: &str, page: &str, repo: &Repo) -> Result<Mirrored, Refusal> {
    let page = match page {
        "" if source.starts_with("hf://") || source.starts_with("civitai://") => source,
        "" => {
            return Err(usage(
                "mirror_source_required",
                format!("{source:?} is not an hf:// or civitai:// source: name the upstream page, --mirror <URL>"),
            ));
        }
        page => page,
    };
    let answer = (signed_in(api, hub)?.mirror_model(&repo.org, &repo.name, page).await).map_err(|e| refusal(hub, e))?;
    let org = answer.author.as_ref().map_or_else(|| repo.org.clone(), |a| a.org.clone());
    let by = answer.author.map(|a| a.reserved.unwrap_or(format!("credits {} ({})", a.name, a.org)));
    Ok(Mirrored { model: answer.model, source: answer.source, by, org, added_handle: answer.added_handle })
}

/// Commit `commit` of `from`, another model on the same Hub, recorded as a commit of the destination when this account may read
/// its whole closure: no machine pulls it and no byte uploads. Its lane is checked first, so a refused record parks and credits
/// nothing. With the mirror recorded before it.
async fn record(api: &Api, hub: &Origin, ask: ModelUpload, from: Repo, commit: String) -> Result<(RecordedCommit, Option<Mirrored>), Refusal> {
    let to = Repo::hub(&ask.destination).map_err(Refusal)?;
    if ask.strength.is_some_and(|s| !s.is_finite() || s.abs() > 10.0) {
        return Err(usage("model_strength_invalid", "--strength is a number between -10 and 10"));
    }
    let client = signed_in(api, hub)?;
    let card = (client.get_model(&from.org, &from.name).await).map_err(|e| refusal(hub, e))?;
    let lanes = card.releases.iter().flat_map(|r| r.lanes.iter().map(|(lane, l)| (lane.as_str(), l.commit.as_str(), l.length)));
    let (held, length) = (card.commits.iter().map(|c| (c.lane.as_str(), c.id.as_str(), c.length)))
        .chain(lanes)
        .find(|(_, id, _)| *id == commit)
        .map(|(lane, _, length)| (lane.to_owned(), length))
        .ok_or_else(|| Refusal(Error::not_found("commit_not_found", format!("{from} has no commit {commit} this account reads"))))?;
    if held != ask.lane {
        return Err(Refusal(
            Error::new(Kind::Usage, "commit_lane_mismatch", format!("{from}@{commit} is lane {held}, not {}", ask.lane))
                .next(format!("cozy model upload {from}@{commit} {to} --lane {held}")),
        ));
    }
    let mirrored = match &ask.mirror {
        Some(page) => Some(mirror(api, hub, &ask.source, page, &to).await?),
        None => None,
    };
    let made = (client.create_model_commit(&to.org, &to.name, &commit, length).await).map_err(|e| refusal(hub, e))?;
    let display = match (ask.trigger, ask.strength) {
        (None, None) => None,
        (trigger, strength) => Some(display(&api.machines, hub, to.to_string(), trigger, strength).await),
    };
    let recorded = RecordedCommit {
        next: vec![format!("cozy model publish {to} <version> {}", made.id)],
        commit: made.id,
        from: format!("{from}@{commit}"),
        lane: made.lane,
        model: to.to_string(),
        display,
    };
    Ok((recorded, mirrored))
}
