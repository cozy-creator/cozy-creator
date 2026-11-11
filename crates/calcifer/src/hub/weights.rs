//! Models on machines: downloads and uploads are warm runs on the machine `on` names, which
//! fetches, makes and publishes the bytes itself, so no weight passes through this computer; a
//! local file goes to the machine as the run's input. A Hub checkpoint uploads nothing: the Hub
//! records it as a checkpoint of another model (`checkpoints`).
use std::{
    io::Read as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use api::{
    calcifer::{Agreed, Kind as RunKind, On, OutputItem, Run, StagedInput, SubmitRun, Warning},
    config::Origin,
    error::{Error, Kind},
    calcifer::hub::{ModelDownload, ModelUpload, RecordedCheckpoint},
    ids::Digest,
    machine::{self as v1, InputFile, ModelChoice},
    model_ref::{self as refs, ModelRef, ModelId},
};

use super::{
    after::{After, display},
    jobs::{Planned, Work},
    refusal, signed_in,
};
use crate::api::{Api, Refusal};

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

/// The model a warm run makes present: a reference, or a held `.safetensors` file (`PUT /v1/files`) by its client's `name`.
fn source(raw: &str, name: Option<&str>) -> Result<(ModelChoice, Option<StagedInput>), Refusal> {
    match raw.starts_with("sha256:") {
        true => model_file(raw, name),
        false => Ok((ModelRef::parse(raw).map_err(Refusal)?.choice("model"), None)),
    }
}

/// The file `named` holds (`sha256:<hex>`), if Calcifer holds it.
fn held_file(named: &str) -> Result<(PathBuf, Digest), Refusal> {
    let digest: Digest = named.parse().map_err(|why: &str| usage("file_name_invalid", format!("{named:?}: {why}")))?;
    let Some(path) = crate::runs::files::held(&digest) else {
        let said = format!("Calcifer holds no file {named}");
        return Err(Refusal(Error::new(Kind::NotFound, "file_not_held", said).remedy(format!("PUT its bytes to /v1/files/{named} first"))));
    };
    Ok((path, digest))
}

/// The held file `named` as the model a warm run makes present: one `.safetensors` file by its bytes, named for the object as its
/// client named it.
fn model_file(named: &str, name: Option<&str>) -> Result<(ModelChoice, Option<StagedInput>), Refusal> {
    let (path, digest) = held_file(named)?;
    let shown = name.unwrap_or(named);
    let refused = |why: String| Refusal(Error::new(Kind::Validation, "model_file_refused", why));
    let mut file = std::fs::File::open(&path).map_err(|e| refused(e.to_string()))?;
    let length = file.metadata().map_err(|e| refused(e.to_string()))?.len();
    // Complete native checkpoints are verified by TensorFS on the worker; they do not go
    // through a safetensors converter or borrow unstaged bytes from the shared cache.
    let native = name.and_then(|name| Path::new(name).extension()).is_some_and(|extension| extension.eq_ignore_ascii_case("cozytensors"));
    if !native {
        let mut head = [0; 9];
        file.read_exact(&mut head).map_err(|_| refused(format!("{shown} is not a .safetensors or .cozytensors file")))?;
        let header = u64::from_le_bytes(head[..8].try_into().expect("8 bytes"));
        if head[8] != b'{' || header.saturating_add(8) > length {
            return Err(refused(format!("{shown} is not a .safetensors or .cozytensors file")));
        }
    }
    let object = name.and_then(|name| Path::new(name).file_name()).map_or_else(|| "model.safetensors".into(), |n| n.to_string_lossy().into_owned());
    let choice = ModelChoice { parameter: "model".into(), source: format!("object://{digest}/{}", escape(&object)), ..Default::default() };
    Ok((choice, Some(StagedInput { path, digest, length })))
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

/// The CozyTensors folder a held tree document names (`api::tree`), each of its files held: an ordinary retained input
/// tree. The machine admits its headers without importing or fetching the tensor objects a thin folder omits. None: `path`
/// holds no tree document.
fn folder(path: &Path, digest: Digest) -> Result<Option<Made>, Refusal> {
    let Ok(document) = std::fs::read(path) else { return Ok(None) };
    let Ok(members) = api::tree::parse(&document) else { return Ok(None) };
    let mut staged = vec![];
    for (name, member) in members {
        let (path, digest) = held_file(&format!("sha256:{}", member.sha256)).map_err(|Refusal(e)| Refusal(Error { message: format!("{name} of the folder: {}", e.message), ..e }))?;
        staged.push(StagedInput { path, digest, length: member.length });
    }
    let length = document.len() as u64;
    staged.push(StagedInput { path: path.to_owned(), digest, length });
    let input = InputFile { field: "model".into(), digest: digest.to_string(), length, media_type: api::tree::MEDIA.into(), order: 0 };
    Ok(Some(Made {
        choice: ModelChoice { parameter: "model".into(), source: "input://model".into(), ..Default::default() },
        base: None,
        staged,
        files: vec![input],
    }))
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
/// checkpoint's `lane`, or a checkpoint) or a `local/` one.
fn base(raw: &str, lane: &str) -> Result<ModelChoice, Refusal> {
    let segment = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    // The last `:` starts a component list unless it is a checkpoint's `sha256:`.
    let (model, names) = match raw.rsplit_once(':') {
        Some((model, names)) if !model.ends_with("@sha256") && names.split(',').all(segment) => (model, Some(names)),
        _ => (raw, None),
    };
    let mut choice = match ModelRef::parse(model).map_err(Refusal)? {
        ModelRef::Provider(_) => return Err(usage("model_base_invalid", format!("--base {model}: name a Hub model or a local/ one"))),
        reference => reference.choice(v1::BASE),
    };
    if !choice.model.starts_with("local/") && choice.manifest.is_empty() {
        choice.lane = lane.to_owned();
    }
    if let Some(names) = names {
        choice.parameter = format!("{}:{names}", choice.parameter);
    }
    Ok(choice)
}

/// One warm run of `made` on `on`, uploaded to `destination` (a Hub model, a `local/` alias,
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
        stays: vec![],
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

/// `cozy/model_download`: the warm run that makes `ask.model` present on `on`.
pub(super) fn download(hub: &Origin, on: On, agreed: Option<Agreed>, ask: ModelDownload) -> Result<Planned, Refusal> {
    let mut said = vec![];
    let mut destination = match ask.alias.as_deref().map(ModelId::parse).transpose().map_err(Refusal)? {
        Some(model_id) if model_id.org == "local" => model_id.to_string(),
        Some(_) => return Err(usage("model_alias_invalid", "the name a machine keeps a model under is local/<name>")),
        None => String::new(),
    };
    let mut made = source(&ask.model, ask.name.as_deref())?;
    if let Some(format) = &ask.source_format {
        match formatted(&mut made.0, format) {
            Ok(()) => {}
            Err(_) => said.push("--source-format converts a provider source or a local file: ignored".to_owned()),
        }
    }
    // A file of its client's is kept under a name: its own, unless it is given one.
    if made.1.is_some() && destination.is_empty() {
        let Some(name) = ask.name.as_deref() else {
            return Err(usage("model_alias_required", "a held file has no name of its own: name the alias, local/<name>"));
        };
        let stem = Path::new(name).file_stem().map(|stem| stem.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let stem: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' }).collect();
        destination = ModelId::parse(&format!("local/{stem}")).map_err(Refusal)?.to_string();
        said.push(format!("kept as {destination}"));
    }
    if let Some(lane) = ask.lane {
        let lane = refs::label("--lane", &lane).map_err(Refusal)?;
        if made.0.model.is_empty() || made.0.model.starts_with("local/") {
            said.push("--lane names a lane of a Hub model: ignored".to_owned());
        } else {
            made.0.lane = lane;
        }
    }
    let target = ask.name.clone().unwrap_or(ask.model);
    let mut submit = warm(hub, on, agreed, target, made.into(), destination, String::new());
    submit.warnings.extend(said.into_iter().map(|message| Warning { code: "model_download".into(), message }));
    Ok(submit.into())
}

/// A strength past the -10 to 10 a model takes, held to the nearest, and the warning that says so. Only what is no number is refused.
fn strength_within(strength: Option<f64>) -> Result<(Option<f64>, Option<String>), Refusal> {
    match strength {
        Some(given) if !given.is_finite() => Err(usage("model_strength_invalid", "--strength is a number between -10 and 10")),
        Some(given) if given.abs() > 10.0 => {
            let held = given.clamp(-10.0, 10.0);
            Ok((Some(held), Some(format!("--strength {given} is past what a model takes: it is {held}"))))
        }
        kept => Ok((kept, None)),
    }
}

/// `cozy/model_upload`: the warm run that makes `ask.source` and uploads it as a checkpoint of its destination, with the mirror to
/// record before it goes. A Hub checkpoint on no base is at the Hub already: recording it there is Calcifer's own work.
pub(super) async fn upload(api: &Arc<Api>, hub: &Origin, on: On, agreed: Option<Agreed>, mut ask: ModelUpload) -> Result<Planned, Refusal> {
    let (strength, said) = strength_within(ask.strength)?;
    ask.strength = strength;
    let destination = ModelId::hub(&ask.destination).map_err(Refusal)?;
    let lane = refs::label("--lane", &ask.lane).map_err(Refusal)?;
    let mirror = ask.mirror.clone().map(|page| Mirror { source: ask.source.clone(), page, to: destination.clone() });
    if let Ok(ModelRef::Checkpoint { model: from, checkpoint }) = ModelRef::parse(&ask.source)
        && ask.base.is_none()
    {
        let target = format!("{from}@{checkpoint} → {destination} ({lane})");
        let record = Record::read(api, hub, ask, from, checkpoint.to_string(), said, mirror).await?;
        return Ok(Planned::Here { target, work: Work::Record(Box::new(record)) });
    }
    let destination = destination.to_string();
    let warnings: Vec<Warning> = said.into_iter().map(|message| Warning { code: "input_limit".into(), message }).collect();
    let checkpoint = matches!(ModelRef::parse(&ask.source), Ok(ModelRef::Checkpoint { .. }));
    let converted = !ask.source.starts_with("run:") && !checkpoint;
    if ask.trained_on.is_some() && !converted {
        return Err(usage("model_trained_on_unused", "--trained-on is read when a LoRA source is converted; this source already was"));
    }
    let display = |offered| After::Display { model: destination.clone(), trigger: ask.trigger.clone(), strength: ask.strength, offered };
    if let Some(reference) = ask.source.strip_prefix("run:") {
        if ask.base.is_some() {
            return Err(usage("model_base_invalid", "a run's weights output uploads as it is; upload a model source on a base"));
        }
        // It runs where the weights are, whatever `on` names.
        let mut submit = from_run(api, reference, &destination, &lane)?;
        submit.warnings.extend(warnings);
        return Ok(Planned::Run { submit: Box::new(submit), after: Some(display(false)), notes: vec![], mirror });
    }
    api::calcifer::local_usable(&on.to_string()).map_err(Refusal)?;
    // The machine publishes under this computer's sign-in; refuse before anything is made.
    signed_in(api, hub)?;
    let tree = match ask.source.starts_with("sha256:") {
        true => {
            let (path, digest) = held_file(&ask.source)?;
            folder(&path, digest)?
        }
        false => None,
    };
    let mut made: Made = match tree {
        Some(made) => {
            if ask.trained_on.is_some() {
                return Err(usage("model_trained_on_unused", "--trained-on is read when a LoRA source is converted; a CozyTensors folder already was"));
            }
            if ask.base.is_some() {
                return Err(usage("model_base_invalid", "a CozyTensors folder uploads as it is; upload a model source on a base"));
            }
            if ask.source_format.is_some() {
                return Err(usage("model_profile_unused", "a CozyTensors folder is already converted; omit --source-format"));
            }
            made
        }
        None => source(&ask.source, ask.name.as_deref())?.into(),
    };
    if let Some(format) = &ask.source_format {
        formatted(&mut made.choice, format)?;
    }
    made.base = ask.base.as_deref().map(|raw| base(raw, &lane)).transpose()?;
    let shown = ask.name.as_deref().unwrap_or(&ask.source);
    let target = match &ask.base {
        None => format!("{shown} → {destination} ({lane})"),
        Some(on) => format!("{shown} on {on} → {destination} ({lane})"),
    };
    made.choice.trained_on = ask.trained_on.clone().unwrap_or_default();
    // A Hub model by name is copied in that lane, as a download's lane reads it; never the
    // machine's pick under another lane's label.
    if !made.choice.model.is_empty() && !made.choice.model.starts_with("local/") {
        made.choice.lane = lane.clone();
    }
    let after = display(true);
    let mut submit = warm(hub, on, agreed, target, made, destination.clone(), lane);
    submit.warnings.extend(warnings);
    Ok(Planned::Run { submit: Box::new(submit), after: Some(after), notes: vec![], mirror })
}

/// `run:N[#output]`: the run's weights output, a WARM run on the machine that holds it, uploaded
/// as a checkpoint of `destination` in `lane` without running it again.
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
        model: checkpoint.model_id.clone(),
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
        stays: vec![],
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

/// A destination to record as a mirror of its author's model, before anything uploads: an empty `page` names the hf:// or
/// civitai:// source itself.
pub(super) struct Mirror {
    source: String,
    page: String,
    to: ModelId,
}

impl Mirror {
    /// Records it at `hub`: what it then says, for whoever asked.
    pub(super) async fn record(&self, api: &Api, hub: &Origin) -> Result<Vec<String>, Refusal> {
        let (source, model_id) = (&self.source, &self.to);
        let page = match self.page.as_str() {
            "" if source.starts_with("hf://") || source.starts_with("civitai://") => source,
            "" => {
                return Err(usage(
                    "mirror_source_required",
                    format!("{source:?} is not an hf:// or civitai:// source: name the upstream page, --mirror <URL>"),
                ));
            }
            page => page,
        };
        let answer = (signed_in(api, hub)?.mirror_model(&model_id.org, &model_id.name, page).await).map_err(|e| refusal(hub, e))?;
        let org = answer.author.as_ref().map_or_else(|| model_id.org.clone(), |a| a.org.clone());
        let by = answer.author.map(|a| a.reserved.unwrap_or(format!("credits {} ({})", a.name, a.org)));
        let by = by.map(|by| format!(". {by}")).unwrap_or_default();
        let mut said = vec![format!("{} mirrors {}{by}", answer.model, answer.source)];
        said.extend(answer.added_handle.map(|handle| format!("added {handle} to {org}'s claim handles")));
        Ok(said)
    }
}

/// A checkpoint of another model on the same Hub, to record as a checkpoint of the destination: no machine pulls it and no byte uploads.
/// Read and checked against its lane first (`read`), so a refused record parks and credits nothing.
pub(super) struct Record {
    ask: ModelUpload,
    from: ModelId,
    checkpoint: String,
    length: i64,
    said: Option<String>,
    mirror: Option<Mirror>,
}

impl Record {
    /// Checkpoint `checkpoint` of `from`, when this account may read it and it is of the lane `ask` names.
    async fn read(api: &Api, hub: &Origin, ask: ModelUpload, from: ModelId, checkpoint: String, said: Option<String>, mirror: Option<Mirror>) -> Result<Self, Refusal> {
        let to = ModelId::hub(&ask.destination).map_err(Refusal)?;
        let client = signed_in(api, hub)?;
        let card = (client.get_model(&from.org, &from.name).await).map_err(|e| refusal(hub, e))?;
        let lanes = card.releases.iter().flat_map(|r| r.lanes.iter().map(|(lane, l)| (lane.as_str(), l.checkpoint.as_str(), l.length)));
        let (held, length) = (card.checkpoints.iter().map(|c| (c.lane.as_str(), c.id.as_str(), c.length)))
            .chain(lanes)
            .find(|(_, id, _)| *id == checkpoint)
            .map(|(lane, _, length)| (lane.to_owned(), length))
            .ok_or_else(|| Refusal(Error::not_found("commit_not_found", format!("Checkpoint {checkpoint} is unavailable in {from}"))))?;
        if held != ask.lane {
            return Err(Refusal(
                Error::new(Kind::Usage, "commit_lane_mismatch", format!("{from}@{checkpoint} is lane {held}, not {}", ask.lane))
                    .next(format!("cozy model upload {from}@{checkpoint} {to} --lane {held}")),
            ));
        }
        Ok(Self { ask, from, checkpoint, length, said, mirror })
    }

    /// Records it, the mirror before it, and sets its trigger and strength.
    pub(super) async fn run(self, api: &Api, hub: &Origin) -> Result<RecordedCheckpoint, Refusal> {
        let Self { ask, from, checkpoint, length, said, mirror } = self;
        let to = ModelId::hub(&ask.destination).map_err(Refusal)?;
        let mut warnings: Vec<String> = said.into_iter().collect();
        if let Some(mirror) = mirror {
            warnings.extend(mirror.record(api, hub).await?);
        }
        let client = signed_in(api, hub)?;
        let made = (client.create_model_checkpoint(&to.org, &to.name, &checkpoint, length).await).map_err(|e| refusal(hub, e))?;
        let display = match (ask.trigger, ask.strength) {
            (None, None) => None,
            (trigger, strength) => Some(display(&api.machines, hub, to.to_string(), trigger, strength).await),
        };
        Ok(RecordedCheckpoint {
            next: vec![format!("cozy model publish {to} <version> {}", made.id)],
            checkpoint: made.id,
            from: format!("{from}@{checkpoint}"),
            lane: made.lane,
            model: to.to_string(),
            display,
            warnings,
        })
    }
}
