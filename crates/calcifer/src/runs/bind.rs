//! Binds `cozy run`'s terms to a callable's interface: the payload, the files it attaches, the
//! model choices and a conversion's destination.
//!
//! Typing is the interface's, never how a token looks: `steps=20` is an int because `steps` is
//! declared int. Types and required fields are strict, and every problem is named in one refusal.
//! An undeclared field, at any depth, is dropped with one warning and never refuses a run. A value
//! over a limit is no refusal either (Paul, 11-03): Runtime clamps a number and cuts text and lists,
//! each with an adjustment row, and a list of files is cut here, before its files go.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read as _,
    path::{Path, PathBuf},
};

use protocol::{
    calcifer::{FileTerm, Note, NoteLevel, Sent, StagedInput, path_key},
    contract as describe,
    error::{Error, Kind},
    ids::Digest,
    interface::{AssetsSlot, Callable, Constraints, Field, Interface, Struct, Type, Wire},
    target::written_as_path,
};
use serde_json::{Map, Number, Value};
use sha2::{Digest as _, Sha256};

use super::clip::{Clips, Cut};

/// The request field every callable takes beside its interface: how LoRAs apply.
const LORA_MODE: &str = "lora_mode";

/// What a run's request gave (`RunRequest`).
#[derive(Debug)]
pub struct Terms {
    /// The payload as an object; the terms override it.
    pub input: Option<Map<String, Value>>,
    /// The terms: `key=value`, `key:=json`, `model.<slot>=<ref>`, positionals.
    pub inputs: Vec<String>,
    /// The whole payload as JSON or YAML text.
    pub input_text: Option<String>,
    /// Attached files: `[field.path|label]` and a file.
    pub assets: Vec<FileTerm>,
    /// `--file-fidelity label-or-index=auto|low|medium|high`.
    pub fidelities: Vec<String>,
    /// `--lora [slot=]ref[,strength]`.
    pub loras: Vec<String>,
    /// The run's idempotency key, which an omitted integer `seed` derives from.
    pub key: String,
    /// The client's files and folders the request names by path, as written there, sent by content.
    pub paths: BTreeMap<String, Sent>,
    /// Where the digests of its project's files are remembered; None reads each every time.
    pub digests: Option<PathBuf>,
    /// Where segments of videos are cut and kept.
    pub clips: Clips,
    /// Where uploaded files are held (`PUT /v1/files`), by their digest's hex.
    pub uploads: PathBuf,
    /// The run's project, whose files a request may name by hash.
    pub project: Option<crate::project::Holding>,
}

impl Terms {
    /// The file Calcifer holds under `digest`.
    fn held(&self, digest: &Digest) -> Option<PathBuf> {
        let path = self.uploads.join(hex::encode(digest.bytes()));
        let held = path.is_file();
        if held {
            super::files::touch(&path);
        }
        held.then_some(path)
    }

    /// What the client sent under the path `text`.
    fn sent(&self, text: &str) -> Option<Sent> {
        self.paths.get(path_key(text)).copied()
    }

    /// The file the client sent under the path `text`, held here.
    fn sent_file(&self, text: &str) -> Option<PathBuf> {
        match self.sent(text)? {
            Sent::File(digest) => self.held(&digest),
            Sent::Tree(_) => None,
        }
    }

    /// The file `text` names by content or as sent, here now, and its digest; none when it names none.
    async fn file(&self, text: &str) -> Result<Option<(PathBuf, Digest)>, Error> {
        let held = |digest: Digest| self.held(&digest).map(|path| (path, digest));
        if let Some(found) = text.parse().ok().and_then(held) {
            return Ok(Some(found));
        }
        if let Some(path) = match &self.project {
            Some(project) => project.file(text).await?,
            None => None,
        } {
            let digest = protocol::digests::of(self.digests.as_deref(), &path)?;
            return Ok(Some((path, digest)));
        }
        Ok(match self.sent(text) {
            Some(Sent::File(digest)) => held(digest),
            _ => None,
        })
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Bound {
    pub payload: Map<String, Value>,
    pub files: Vec<InputFile>,
    pub models: Vec<Choice>,
    /// A conversion's `org/model`, where the machine publishes the weights it writes.
    pub destination: Option<String>,
    /// Undeclared fields that were dropped, as people read their paths.
    pub ignored: Vec<String>,
    /// The files of input trees, sent with the run; their manifests (in `files`) name them.
    pub tree_files: Vec<StagedInput>,
    /// Segments of videos sent in place of their sources.
    pub cuts: Vec<Cut>,
    /// What binding said, in order.
    pub notes: Vec<Note>,
    /// The lists of files over a limit that were cut, as people read them.
    pub cut: Vec<String>,
}

/// A file bound to a payload field, whose value is its digest: `path` holds it here, and `shown` is
/// how people named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputFile {
    pub field: String,
    pub path: PathBuf,
    pub shown: String,
    pub digest: Digest,
    pub length: u64,
    pub media_type: String,
    pub order: u32,
}

pub use protocol::choices::Choice;
use protocol::choices::Choices;

impl Bound {
    /// The warning for the fields that were dropped, if any.
    pub fn warning(&self, target: &str) -> Option<String> {
        ignored_warning(target, &self.ignored)
    }
}

impl Bound {
    /// The payload's canonical JSON: sorted keys, no whitespace, exact integers.
    pub fn canonical_payload(&self) -> protocol::machine::Json {
        let text = canonical(&Value::Object(self.payload.clone()));
        protocol::machine::Json::parse(text).expect("canonical JSON is JSON")
    }
}

/// JSON with sorted keys and no whitespace, whatever serde_json's map order.
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort();
            let members: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical(&object[k])))
                .collect();
            format!("{{{}}}", members.join(","))
        }
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        other => other.to_string(),
    }
}

/// A warm run's model choices: `model.<slot>=`, `--lora` and a conversion's input model. It
/// carries no payload, so any other term is refused.
pub async fn warm_choices(
    target: &str, release: Option<&str>, interface: &Interface, callable: &Callable,
    terms: &Terms, program: bool, notes: &mut Vec<Note>,
) -> Result<(Vec<Choice>, Option<protocol::machine::Json>), Error> {
    let mut binder = Binder {
        target: "",
        callable,
        terms,
        document: Map::new(),
        choices: Choices::new("", &callable.name, &callable.models, interface.call_slots(callable)),
        files: Vec::new(),
        named_files: Vec::new(),
        cuts: Vec::new(),
        notes: Vec::new(),
        cut: Vec::new(),
    };
    let mut inputs = terms.inputs.clone();
    let destination = binder.conversion(&mut inputs)?;
    if destination.is_some() {
        return Err(usage("warm_inputs", "warming takes no output destination"));
    }
    let positional = binder.read_terms(&inputs)?;
    binder.loras()?;
    if !positional.is_empty()
        || !binder.document.is_empty()
        || !binder.files.is_empty()
        || !binder.named_files.is_empty()
        || !terms.assets.is_empty()
        || !terms.fidelities.is_empty()
        || terms.input_text.is_some()
        || terms.input.is_some()
    {
        if program {
            if !binder.files.is_empty() || !binder.named_files.is_empty() || !terms.assets.is_empty() || !terms.fidelities.is_empty() {
                return Err(usage("warm_inputs", "GPU program warming takes scalar inputs, not media files"));
            }
            let bound = bind(target, release, interface, callable, terms).await?;
            if !bound.files.is_empty() || !bound.tree_files.is_empty() || bound.destination.is_some() {
                return Err(usage("warm_inputs", "GPU program warming takes scalar inputs, not files, trees or output destinations"));
            }
            let payload = bound.canonical_payload();
            return Ok((bound.models, Some(payload)));
        }
        notes.push(Note {
            level: NoteLevel::Warning,
            message: "--warm keeps a function ready and takes no inputs, only model.<slot>= and --lora choices: they are ignored".into(),
        });
    }
    Ok((binder.choices.into_choices(), None))
}

/// Binds `terms` for `callable`; `target` names it in messages (`org/pkg/fn`) and `release` the
/// one its interface is (none for local code).
pub async fn bind(
    target: &str,
    release: Option<&str>,
    interface: &Interface,
    callable: &Callable,
    terms: &Terms,
) -> Result<Bound, Error> {
    let mut binder = Binder {
        target,
        callable,
        terms,
        document: Map::new(),
        choices: Choices::new(target, &callable.name, &callable.models, interface.call_slots(callable)),
        files: Vec::new(),
        named_files: Vec::new(),
        cuts: Vec::new(),
        notes: Vec::new(),
        cut: Vec::new(),
    };
    let mut inputs = terms.inputs.clone();
    let destination = binder.conversion(&mut inputs)?;
    match (&terms.input_text, &terms.input) {
        (Some(text), None) => binder.read(text, "input_text")?,
        (None, Some(input)) => binder.take(input.clone())?,
        (None, None) => {}
        _ => return Err(usage("input_twice", "a request gives its payload as one of input or input_text")),
    }
    let positional = binder.read_terms(&inputs)?;
    binder.primary(positional)?;
    binder.loras()?;
    let mut files = binder.attach().await?;
    binder.fidelities()?;
    let tree_files = binder.trees(&mut files)?;
    binder.seed();
    if let Some(project) = &terms.project {
        let package = target.rsplit_once('/').map_or(target, |(package, _)| package);
        let (payload, named, said) = project.bound(target, package, callable, std::mem::take(&mut binder.document), &terms.clips).await?;
        binder.document = payload;
        files.retain(|file| !named.iter().any(|held| held.field == file.field));
        files.extend(named);
        said.into_iter().for_each(|message| binder.warn(message));
    }
    let mut ignored = validate(target, release, callable, &mut binder.document)?;
    ignored.extend(std::mem::take(&mut binder.choices.dropped));
    ignored.sort();
    let replaced = std::mem::take(&mut binder.choices.replaced);
    binder.notes.extend(replaced.into_iter().map(|message| Note { level: protocol::calcifer::NoteLevel::Warning, message }));
    Ok(Bound {
        payload: binder.document,
        files,
        models: binder.choices.into_choices(),
        destination,
        ignored,
        tree_files,
        cuts: binder.cuts,
        notes: binder.notes,
        cut: binder.cut,
    })
}

struct Binder<'a> {
    target: &'a str,
    callable: &'a Callable,
    terms: &'a Terms,
    document: Map<String, Value>,
    /// Its model choices; their `dropped` are warned of with the payload's.
    choices: Choices<'a>,
    /// Positional files, as written, bound to media inputs by kind.
    files: Vec<String>,
    /// `key=value` terms whose field is a file: `--file key=value`.
    named_files: Vec<FileTerm>,
    cuts: Vec<Cut>,
    notes: Vec<Note>,
    cut: Vec<String>,
}

impl Binder<'_> {
    /// A conversion reads `<input-model> [<org/model>]`: the first is its first slot's choice.
    fn conversion(&mut self, inputs: &mut Vec<String>) -> Result<Option<String>, Error> {
        if !self.callable.is_conversion() {
            return Ok(None);
        }
        let (bare, rest): (Vec<String>, Vec<String>) =
            inputs.drain(..).partition(|term| !term.contains('='));
        *inputs = rest;
        if bare.len() > 2 {
            return Err(usage(
                "conversion_positionals",
                format!(
                    "{} takes <{}> and an optional <org/model> destination; got {} positional values",
                    self.callable.name,
                    self.callable.models[0].param,
                    bare.len()
                ),
            )
            .remedy(describe::usage_line(self.target, self.callable)));
        }
        let mut bare = bare.into_iter();
        if let Some(input) = bare.next() {
            let slot = self.callable.models[0].param.clone();
            self.choices.choose(&slot, input)?;
        }
        bare.next()
            .map(|destination| {
                let mut parts = destination.split('/');
                match (parts.next(), parts.next(), parts.next()) {
                    (Some(org), Some(name), None) if !org.is_empty() && !name.is_empty() => {
                        Ok(destination.to_lowercase())
                    }
                    _ => Err(usage(
                        "destination_malformed",
                        format!("{destination:?} is not an org/model destination"),
                    )),
                }
            })
            .transpose()
    }

    /// A payload as text: JSON, or YAML when it is not JSON.
    fn read(&mut self, text: &str, what: &str) -> Result<(), Error> {
        let loaded: Value = match serde_json::from_str(text) {
            Ok(value) => value,
            _ => protocol::yaml::to_json(text).map_err(|e| invalid("input_invalid", format!("{what}: {e}")))?,
        };
        let Value::Object(loaded) = loaded else {
            return Err(invalid("input_invalid", format!("{what} does not hold one object")));
        };
        self.take(loaded)
    }

    /// The payload's fields and model choices.
    fn take(&mut self, mut loaded: Map<String, Value>) -> Result<(), Error> {
        self.choices.take(&mut loaded)?;
        for (key, value) in loaded {
            let key = self.fold(&key)?;
            self.document.insert(key, value);
        }
        Ok(())
    }

    /// The argv terms; answers the bare positionals.
    fn read_terms(&mut self, inputs: &[String]) -> Result<Vec<String>, Error> {
        let mut positional = Vec::new();
        for term in inputs {
            if self.positional_file(term)? {
                self.files.push(path_key(term).to_owned());
                continue;
            }
            let colon = term.find(":=");
            let equals = term.find('=');
            let (key, raw, structured) = match (colon, equals) {
                (Some(c), Some(e)) if e == c + 1 => (&term[..c], &term[c + 2..], true),
                (_, Some(e)) => (&term[..e], &term[e + 1..], false),
                _ => {
                    positional.push(term.clone());
                    continue;
                }
            };
            if let Some(slot) = key.strip_prefix("model.") {
                if structured {
                    return Err(usage(
                        "model_choice_malformed",
                        format!("{key} has no structured spelling"),
                    )
                    .remedy(format!("{key}=org/model[@release][/lane]")));
                }
                self.choices.choose(slot, raw.trim().to_owned())?;
                continue;
            }
            let key = self.fold(key)?;
            // Pick up from here: `pickup.take=<shot_1.3> pickup.out_frame=240`.
            if let Some((head, member)) = key.split_once('.')
                && self.callable.field(head).is_some_and(|field| matches!(field.ty.plain(), Type::Pickup(_)))
            {
                let value = match member {
                    "take" => Value::String(raw.to_owned()),
                    _ => serde_json::from_str(raw).map_err(|_| usage("pickup_malformed", format!("{key}={raw} is not a number")))?,
                };
                set_path(&mut self.document, &[head.to_owned(), member.to_owned()], value)?;
                continue;
            }
            // A mention is the project's box, which its binder fills.
            if !structured && !crate::project::graph::mentions(raw).is_empty() {
                self.document.insert(key, Value::String(raw.to_owned()));
                continue;
            }
            let spelled = self.callable.field(&key).and_then(|field| scalar(&field.ty, raw));
            if !structured && spelled.is_none() && self.names_a_file(&key) {
                self.named_files.push(FileTerm { name: Some(key), file: raw.strip_prefix('@').unwrap_or(raw).to_owned() });
                continue;
            }
            let value = if structured {
                serde_json::from_str(raw).map_err(|_| {
                    usage("json_malformed", format!("{key}:=… is not JSON: {raw}")).remedy(
                        "`key:=<json>` carries a nested value verbatim; `key=value` is the scalar form",
                    )
                })?
            } else {
                match self.callable.field(&key) {
                    Some(field) => self.typed(field, raw)?,
                    None => Value::String(raw.to_owned()),
                }
            };
            self.document.insert(key, value);
        }
        Ok(positional)
    }

    /// One positional value fills the first declared field a text value can fill.
    fn primary(&mut self, positional: Vec<String>) -> Result<(), Error> {
        if positional.is_empty() {
            return Ok(());
        }
        let Some(field) = self
            .callable
            .request
            .fields
            .iter()
            .find(|f| !self.callable.is_assets(&f.name) && f.ty.media().is_none())
        else {
            self.warn(format!("{} has no request field a value fills: {} ignored", self.callable.name, positional.join(" ")));
            return Ok(());
        };
        if positional.len() > 1 {
            let words = positional.len();
            self.warn(format!("{words} values given for {}: read as one (quote it, or `key=value`)", field.name));
        }
        let value = self.typed(field, &positional.join(" "))?;
        self.document.insert(field.name.clone(), value);
        Ok(())
    }

    /// `--lora [slot=]ref[,strength]`, in order. A `://` before the `=` is a source's URL.
    fn loras(&mut self) -> Result<(), Error> {
        for raw in &self.terms.loras {
            let (slot, rest) = match raw.split_once('=') {
                Some((slot, rest)) if !slot.contains("://") => (Some(slot), rest),
                _ => (None, raw.as_str()),
            };
            let (reference, strength) = rest.split_once(',').unwrap_or((rest, "1"));
            self.choices.lora(slot, reference, strength, raw)?;
        }
        Ok(())
    }

    /// Folds a field name onto its declared spelling; an exact spelling wins.
    fn fold(&self, key: &str) -> Result<String, Error> {
        if self.callable.field(key).is_some() {
            return Ok(key.to_owned());
        }
        let matches: Vec<&Field> = self
            .callable
            .request
            .fields
            .iter()
            .filter(|f| f.name.eq_ignore_ascii_case(key))
            .collect();
        match matches.as_slice() {
            [] => Ok(key.to_owned()),
            [one] => Ok(one.name.clone()),
            _ => Err(invalid(
                "request_field_case_ambiguous",
                format!(
                    "{key:?} folds onto more than one of {}'s fields",
                    self.callable.name
                ),
            )
            .remedy("spell the field exactly")),
        }
    }

    fn warn(&mut self, message: String) {
        self.notes.push(Note { level: NoteLevel::Warning, message });
    }

    /// Whether `key` is an input file, or a list of them (`image`, `pictures.0.image`, `plate.video`).
    fn names_a_file(&self, key: &str) -> bool {
        let list = self.callable.field(key).is_some_and(|field| field.ty.media().is_some());
        let parts: Vec<String> = key.split('.').map(str::to_owned).collect();
        list || self.accepts(&parts).is_some()
    }

    /// A scalar spelled as the field's declared type.
    fn typed(&self, field: &Field, raw: &str) -> Result<Value, Error> {
        scalar(&field.ty, raw).ok_or_else(|| {
            let name = format!("{}.{}", self.callable.name, field.name);
            match &field.ty {
                Type::List(_) | Type::Map(_) | Type::Struct(_) | Type::Model => invalid(
                    "value_not_scalar",
                    format!("{name} is not a scalar and `key=value` cannot spell one"),
                )
                .remedy(format!("carry it as {}:=<json>", field.name)),
                ty => invalid(
                    "value_type",
                    format!(
                        "{name} is declared {} and {raw:?} is not one",
                        describe::phrase(ty)
                    ),
                )
                .remedy(format!("structured values are {}:=<json>", field.name)),
            }
        })
    }

    /// Whether a bare term is a file for a media input: an explicit path (which must have been
    /// sent), or a file sent under it when the callable takes media. Never a `key=value` term.
    fn positional_file(&self, term: &str) -> Result<bool, Error> {
        let explicit = written_as_path(term) && !term.contains(char::is_whitespace);
        if !explicit && term.contains('=') || !self.takes_media() {
            return Ok(false);
        }
        if matches!(self.terms.sent(term), Some(Sent::File(_))) {
            return Ok(true);
        }
        if explicit {
            return Err(not_sent("input file", term));
        }
        Ok(false)
    }

    fn takes_media(&self) -> bool {
        self.callable.assets.is_some()
            || self
                .callable
                .request
                .fields
                .iter()
                .any(|f| f.ty.media().is_some())
    }

    /// Binds every file: those named in the payload, `--file` and positional files. Each value
    /// becomes the file's digest, and the file its InputFile. A range of a video is a mention of
    /// the project's, `<take3@48-336>`, which its binder cuts.
    async fn attach(&mut self) -> Result<Vec<InputFile>, Error> {
        self.cut_file_lists();
        let mut specs: Vec<Spec> = Vec::new();
        for field in &self.callable.request.fields {
            if let Some(value) = self.document.get(&field.name) {
                embedded_files(&field.ty, value, &field.name, self.terms, &mut specs)?;
            }
        }
        for term in self.terms.assets.iter().chain(&self.named_files) {
            let spec = self.asset_spec(term, &specs);
            specs.push(spec);
        }
        if let Some(assets) = &self.callable.assets {
            self.document
                .entry(assets.parameter.clone())
                .or_insert_with(|| Value::Array(vec![]));
        }
        let positional = std::mem::take(&mut self.files);
        self.bind_positional(&positional, &mut specs)?;
        self.cut_file_specs(&mut specs);
        self.preflight(&specs)?;

        let mut seen = BTreeSet::new();
        let mut files = Vec::new();
        for spec in specs {
            let parts = match spec.field {
                Some(field) => {
                    let mut parts: Vec<String> = field.split('.').map(str::to_owned).collect();
                    if parts.iter().any(|p| p.is_empty()) {
                        return Err(usage(
                            "asset_path_malformed",
                            format!("--file field path {field:?} is malformed"),
                        ));
                    }
                    parts[0] = self.fold(&parts[0])?;
                    parts
                }
                None => {
                    let Some(assets) = &self.callable.assets else {
                        return Err(usage(
                            "assets_undeclared",
                            format!(
                                "{} declares no Files input for {}",
                                self.callable.name,
                                spec.source
                            ),
                        )
                        .remedy("use --file <field-path>=<file> for a named input"));
                    };
                    let list =
                        self.document[&assets.parameter]
                            .as_array_mut()
                            .ok_or_else(|| {
                                invalid(
                                    "assets_not_list",
                                    format!("{} must be a list", assets.parameter),
                                )
                            })?;
                    let mut entry = Map::new();
                    if let Some(label) = &spec.label {
                        entry.insert("label".into(), label.clone().into());
                    }
                    list.push(Value::Object(entry));
                    vec![
                        assets.parameter.clone(),
                        (list.len() - 1).to_string(),
                        "asset".into(),
                    ]
                }
            };
            let field = parts.join(".");
            if !seen.insert(field.clone()) {
                return Err(invalid(
                    "asset_repeated",
                    format!("input file field {field:?} was given more than once"),
                ));
            }
            let accepts = self.accepts(&parts).ok_or_else(|| {
                invalid(
                    "asset_field_unknown",
                    format!(
                        "{}.{field} is not an input file field of this function",
                        self.callable.name
                    ),
                )
                .remedy(describe::usage_line(self.target, self.callable))
            })?;
            let named = path_key(&spec.source).to_owned();
            let (path, digest) = match (self.terms.file(&named).await?, self.terms.sent(&named)) {
                (Some(found), _) => found,
                (None, Some(Sent::Tree(_))) => return Err(invalid("asset_not_a_file", format!("{field}: {named} is a folder, not a file"))),
                (None, Some(Sent::File(_))) | (None, None) if named.starts_with("sha256:") || self.terms.sent(&named).is_some() => {
                    return Err(Error::new(Kind::NotFound, "file_not_held", format!("{field}: Calcifer holds no file {named}"))
                        .remedy("PUT its bytes to /v1/files/sha256:<hex> first"));
                }
                (None, _) => return Err(not_sent(&field, &named)),
            };
            let sniffed = sniff(&path)?;
            let (kind_max, kinds) = accepts;
            let max = kinds
                .iter()
                .find(|(types, _)| {
                    types
                        .iter()
                        .any(|t| t.eq_ignore_ascii_case(&sniffed.media_type))
                })
                .or_else(|| kinds.iter().find(|(types, _)| types.is_empty()))
                .map(|(_, max)| max.or(kind_max))
                .ok_or_else(|| {
                    let accepted: Vec<&str> = kinds
                        .iter()
                        .flat_map(|(t, _)| t.iter().map(String::as_str))
                        .collect();
                    invalid(
                        "asset_media_type",
                        format!(
                            "{field} accepts [{}], not {} ({})",
                            accepted.join(", "),
                            sniffed.media_type,
                            spec.source
                        ),
                    )
                })?;
            if let Some(max) = max.filter(|max| sniffed.length > *max) {
                return Err(invalid(
                    "input_asset_bound",
                    format!(
                        "{field} is {} bytes; it accepts at most {max}",
                        sniffed.length
                    ),
                ));
            }
            set_path(
                &mut self.document,
                &parts,
                Value::String(digest.to_string()),
            )?;
            let order = parts
                .iter()
                .filter_map(|p| p.parse::<u32>().ok())
                .next_back()
                .unwrap_or(0);
            files.push(InputFile {
                field,
                path,
                shown: spec.source.clone(),
                digest,
                length: sniffed.length,
                media_type: sniffed.media_type,
                order,
            });
        }
        self.cut_assets(&mut files);
        files.sort_by(|a, b| a.field.cmp(&b.field));
        Ok(files)
    }

    /// Each folder at a Tree input is its tree document, sent as a file, its files sent too: the field
    /// names the document by its digest, as the machine materializes the tree.
    fn trees(&mut self, files: &mut Vec<InputFile>) -> Result<Vec<StagedInput>, Error> {
        let mut found = vec![];
        for field in &self.callable.request.fields {
            if let Some(value) = self.document.get(&field.name) {
                tree_dirs(&field.ty, value, &field.name, &mut found);
            }
        }
        let mut members = vec![];
        for (field, source) in found {
            let digest = match (self.terms.sent(&source), source.parse::<Digest>()) {
                (Some(Sent::Tree(digest)), _) | (None, Ok(digest)) => digest,
                (Some(Sent::File(_)), _) => return Err(usage("tree_not_a_directory", format!("{field}: {source} is a file, not a folder"))),
                (None, Err(_)) => return Err(not_sent(&field, &source)),
            };
            let not_held = |what: String| {
                Error::new(Kind::NotFound, "file_not_held", format!("{field}: Calcifer holds no {what}")).remedy("PUT its bytes to /v1/files/sha256:<hex> first")
            };
            let path = self.terms.held(&digest).ok_or_else(|| not_held(format!("tree document {digest}")))?;
            let document = std::fs::read(&path).map_err(|e| Error::new(Kind::Internal, "tree_unreadable", format!("{field}: {e}")))?;
            let tree = protocol::tree::parse(&document).map_err(|e| usage("tree_invalid", format!("{field}: {source}: {e}")))?;
            for (name, member) in tree {
                let digest: Digest = format!("sha256:{}", member.sha256).parse().expect("a tree names sha256 hex");
                let held = self.terms.held(&digest).ok_or_else(|| not_held(format!("{name} of {source} ({digest})")))?;
                members.push(StagedInput { path: held, digest, length: member.length });
            }
            let parts: Vec<String> = field.split('.').map(str::to_owned).collect();
            set_path(
                &mut self.document,
                &parts,
                Value::String(digest.to_string()),
            )?;
            files.push(InputFile {
                field,
                path,
                shown: source,
                digest,
                length: document.len() as u64,
                media_type: protocol::tree::MEDIA.into(),
                order: 0,
            });
        }
        files.sort_by(|a, b| a.field.cmp(&b.field));
        Ok(members)
    }

    /// `--file-fidelity`: each hint goes on the Files occurrence its label or index names, for
    /// the package to read.
    fn fidelities(&mut self) -> Result<(), Error> {
        if self.terms.fidelities.is_empty() {
            return Ok(());
        }
        let Some(assets) = &self.callable.assets else {
            self.warn(format!("{} declares no Files input: --file-fidelity is ignored", self.callable.name));
            return Ok(());
        };
        let mut none = vec![];
        let list = match self.document.get_mut(&assets.parameter) {
            Some(Value::Array(list)) => list,
            _ => &mut none,
        };
        let (mut given, mut ignored) = (BTreeSet::new(), vec![]);
        for term in &self.terms.fidelities {
            let (key, fidelity) = term.rsplit_once('=').expect("clap checked its form");
            let index = list.iter().position(|e| e["label"] == key).or_else(|| {
                let index = key.parse().ok().filter(|i: &usize| i.to_string() == key)?;
                (index < list.len()).then_some(index)
            });
            let Some(index) = index.filter(|i| list[*i].is_object()) else {
                ignored.push(format!("--file-fidelity {key:?} names no attached file: ignored"));
                continue;
            };
            if !given.insert(index) {
                ignored.push(format!("file {key:?} was given a fidelity more than once: the last stands"));
            }
            list[index]["fidelity"] = fidelity.into();
        }
        ignored.into_iter().for_each(|message| self.warn(message));
        Ok(())
    }

    /// A file named `field.path` goes to that input (a list of files named whole takes its next
    /// slot); one named `label` labels an Assets occurrence; an unnamed one joins the Assets
    /// collection.
    fn asset_spec(&self, term: &FileTerm, specs: &[Spec]) -> Spec {
        let source = term.file.clone();
        let Some(name) = term.name.as_deref() else {
            return Spec { field: None, source, label: None };
        };
        let parts: Vec<String> = name.split('.').map(str::to_owned).collect();
        let first = self.fold(&parts[0]).ok();
        let list = first.as_deref().filter(|first| {
            let field = self.callable.field(first);
            parts.len() == 1
                && field
                    .and_then(|f| f.ty.media())
                    .is_some_and(|(_, list)| list)
        });
        if let Some(list) = list {
            let held = self
                .document
                .get(list)
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let prefix = format!("{list}.");
            let slot = |s: &Spec| -> Option<usize> {
                let rest = s.field.as_deref()?.strip_prefix(&prefix)?;
                rest.split('.').next()?.parse().ok()
            };
            let next = specs
                .iter()
                .filter_map(slot)
                .map(|i| i + 1)
                .max()
                .unwrap_or(0);
            return Spec {
                field: Some(format!("{list}.{}", held.max(next))),
                source,
                label: None,
            };
        }
        let named = self.callable.assets.is_none()
            || first.is_some_and(|first| {
                self.accepts(&[&[first][..], &parts[1..]].concat())
                    .is_some()
            });
        if named {
            Spec {
                field: Some(name.to_owned()),
                source,
                label: None,
            }
        } else {
            Spec {
                field: None,
                source,
                label: Some(name.to_owned()),
            }
        }
    }

    /// Each positional file fills the one free media input of its kind.
    fn bind_positional(&self, files: &[String], specs: &mut Vec<Spec>) -> Result<(), Error> {
        struct Input<'a> {
            name: &'a str,
            kind: &'a str,
            media_types: &'a [String],
            list: bool,
            assets: bool,
        }
        let mut inputs: Vec<Input> = Vec::new();
        if let Some(assets) = &self.callable.assets {
            for kind in &assets.kinds {
                inputs.push(Input {
                    name: &assets.parameter,
                    kind: &kind.kind,
                    media_types: &kind.media_types,
                    list: true,
                    assets: true,
                });
            }
        }
        for field in &self.callable.request.fields {
            if let (Some((kind, list)), false) =
                (field.ty.media(), self.callable.is_assets(&field.name))
            {
                inputs.push(Input {
                    name: &field.name,
                    kind,
                    media_types: &field.asset_bound.media_types,
                    list,
                    assets: false,
                });
            }
        }
        let mut taken: BTreeSet<String> = BTreeSet::new();
        let mut next: BTreeMap<String, usize> = BTreeMap::new();
        for (name, value) in &self.document {
            match value {
                Value::Array(list) => {
                    next.insert(name.clone(), list.len());
                }
                Value::Null => {}
                Value::String(s) if s.is_empty() => {}
                _ => {
                    taken.insert(name.clone());
                }
            }
        }
        for spec in specs.iter() {
            if let Some(field) = &spec.field {
                let mut parts = field.split('.');
                let first = self
                    .fold(parts.next().unwrap_or_default())
                    .unwrap_or_default();
                if let Some(index) = parts.next().and_then(|p| p.parse::<usize>().ok()) {
                    let at = next.entry(first.clone()).or_default();
                    *at = (*at).max(index + 1);
                }
                taken.insert(first);
            }
        }
        for file in files {
            let held = self.terms.sent_file(file).ok_or_else(|| not_sent("input file", file))?;
            let media_type = sniff(&held)?.media_type;
            let category = media_type.split('/').next().unwrap_or_default();
            // An input of its own kind first. An open one takes its kind's whole set, which reads
            // a GIF as video and a video's sound as audio, as the Runtime does.
            let rank = |input: &Input| -> u8 {
                let open = input.media_types.is_empty();
                let listed = input.media_types.iter().any(|t| t.eq_ignore_ascii_case(&media_type));
                let crosses = matches!((input.kind, category), ("file", _) | ("audio", "video"))
                    || (input.kind, media_type.as_str()) == ("video", "image/gif");
                match (open || listed, input.kind == category) {
                    (true, true) => 2,
                    (true, false) if listed || crosses => 1,
                    _ => 0,
                }
            };
            let best = inputs
                .iter()
                .filter(|i| i.list || !taken.contains(i.name))
                .map(rank)
                .max()
                .unwrap_or(0);
            let mut chosen: Vec<&Input> = inputs
                .iter()
                .filter(|i| (i.list || !taken.contains(i.name)) && best > 0 && rank(i) == best)
                .collect();
            chosen.dedup_by_key(|i| i.name);
            let shown = file;
            match chosen.as_slice() {
                [] => {
                    let names: Vec<String> = inputs
                        .iter()
                        .map(|i| format!("{} ({})", i.name, i.kind))
                        .collect();
                    return Err(invalid(
                        "positional_file_unbound",
                        format!(
                            "{shown} is {media_type}, which no free input of {} accepts",
                            self.callable.name
                        ),
                    )
                    .remedy(format!("its media inputs: {}", names.join(", "))));
                }
                [input] => {
                    let field = match (input.assets, input.list) {
                        (true, _) => None,
                        (false, true) => {
                            let at = next.entry(input.name.to_owned()).or_default();
                            *at += 1;
                            Some(format!("{}.{}", input.name, *at - 1))
                        }
                        (false, false) => {
                            taken.insert(input.name.to_owned());
                            Some(input.name.to_owned())
                        }
                    };
                    specs.push(Spec {
                        field,
                        source: file.clone(),
                        label: None,
                    });
                }
                several => {
                    let names: Vec<&str> = several.iter().map(|i| i.name).collect();
                    let choice = |i: &&Input| match i.assets {
                        true => format!("--file {shown} (joins {})", i.name),
                        false => format!("--file {}={shown}", i.name),
                    };
                    let choices: Vec<String> = several.iter().map(choice).collect();
                    return Err(usage(
                        "positional_file_ambiguous",
                        format!(
                            "{shown} is {media_type}, which inputs {} each accept",
                            names.join(" and ")
                        ),
                    )
                    .remedy(format!("name one: {}", choices.join(" or "))));
                }
            }
        }
        Ok(())
    }

    /// A list that holds files, cut to its limit in the payload as it stands, before a file is read.
    fn cut_file_lists(&mut self) {
        for field in &self.callable.request.fields {
            let Some(max) = field.constraints.max_length.filter(|_| holds_file(&field.ty)) else {
                continue;
            };
            let Some(Value::Array(list)) = self.document.get_mut(&field.name) else {
                continue;
            };
            if list.len() as u64 > max {
                let left = list.len() as u64 - max;
                list.truncate(max as usize);
                self.cut.push(cut_list(&self.callable.name, &field.name, max, format!("{left} items")));
            }
        }
    }

    /// The files named for a list past its limit: `pictures.11.image=` with `pictures` holding 10.
    fn cut_file_specs(&mut self, specs: &mut Vec<Spec>) {
        for field in &self.callable.request.fields {
            let Some(max) = field.constraints.max_length.filter(|_| holds_file(&field.ty)) else {
                continue;
            };
            let prefix = format!("{}.", field.name);
            let past = |spec: &Spec| {
                let index = spec.field.as_deref().and_then(|f| f.strip_prefix(&prefix)?.split('.').next());
                index.and_then(|index| index.parse::<u64>().ok()).is_some_and(|index| index >= max)
            };
            let left: Vec<String> = specs.iter().filter(|spec| past(spec)).map(|spec| spec.source.clone()).collect();
            if !left.is_empty() {
                specs.retain(|spec| !past(spec));
                self.cut.push(cut_list(&self.callable.name, &field.name, max, left.join(", ")));
            }
        }
    }

    /// Labels are unique, before any file is read.
    fn preflight(&self, specs: &[Spec]) -> Result<(), Error> {
        let Some(assets) = &self.callable.assets else {
            return Ok(());
        };
        let existing = self.document[&assets.parameter]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut labels = BTreeSet::new();
        let labelled = existing
            .iter()
            .filter_map(|e| e["label"].as_str().map(str::to_owned));
        for label in labelled.chain(
            specs
                .iter()
                .filter(|s| s.field.is_none())
                .filter_map(|s| s.label.clone()),
        ) {
            if !labels.insert(label.clone()) {
                return Err(invalid(
                    "asset_label_repeated",
                    format!("file label {label:?} was given more than once"),
                ));
            }
        }
        Ok(())
    }

    /// The Assets collection cut to what it declares, in all and of each kind: the first ones stay
    /// and the rest close up behind them.
    fn cut_assets(&mut self, files: &mut Vec<InputFile>) {
        let Some(assets) = &self.callable.assets else {
            return;
        };
        let prefix = format!("{}.", assets.parameter);
        let index = |file: &InputFile| -> Option<usize> { file.field.strip_prefix(&prefix)?.split('.').next()?.parse().ok() };
        let mut held: Vec<(usize, &InputFile)> = files.iter().filter_map(|file| Some((index(file)?, file))).collect();
        held.sort_by_key(|(at, _)| *at);
        let most = self.callable.field(&assets.parameter).and_then(|f| f.constraints.max_length);
        let (mut kept, mut each) = (0, vec![0; assets.kinds.len()]);
        let (mut gone, mut left) = (BTreeSet::new(), vec![]);
        for (at, file) in held {
            let kind = (assets.kinds.iter())
                .position(|kind| kind.media_types.iter().any(|t| t.eq_ignore_ascii_case(&file.media_type)));
            let over_kind = kind.is_some_and(|k| assets.kinds[k].max_count.is_some_and(|max| each[k] >= max));
            if over_kind || most.is_some_and(|max| kept >= max) {
                gone.insert(at);
                left.push(file.shown.clone());
                continue;
            }
            kept += 1;
            if let Some(k) = kind {
                each[k] += 1;
            }
        }
        if gone.is_empty() {
            return;
        }
        files.retain(|file| !index(file).is_some_and(|at| gone.contains(&at)));
        for file in files.iter_mut() {
            let Some(at) = index(file) else { continue };
            let now = at - gone.range(..at).count();
            file.field = file.field.replacen(&format!("{prefix}{at}"), &format!("{prefix}{now}"), 1);
            file.order = now as u32;
        }
        if let Some(Value::Array(list)) = self.document.get_mut(&assets.parameter) {
            let mut at = 0;
            list.retain(|_| {
                at += 1;
                !gone.contains(&(at - 1))
            });
        }
        self.cut.push(format!("{} takes {kept} of these files: {} left out", assets.parameter, left.join(", ")));
    }

    /// What the input at `parts` accepts: the byte bound it declares, and each media-type set with
    /// its own bound. None: no input file field is there.
    fn accepts(&self, parts: &[String]) -> Option<Accepts> {
        if let Some(AssetsSlot { parameter, kinds }) = &self.callable.assets
            && parts.len() == 3
            && &parts[0] == parameter
            && parts[2] == "asset"
            && parts[1].parse::<u32>().is_ok()
        {
            let kinds = kinds
                .iter()
                .map(|k| (k.media_types.clone(), k.max_bytes))
                .collect();
            return Some((None, kinds));
        }
        let field = self.callable.field(&parts[0])?;
        asset_at(
            &field.ty,
            &parts[1..],
            &field.asset_bound.media_types,
            field.asset_bound.max_bytes,
        )
        .map(|(types, max)| (max, vec![(types, None)]))
    }

    /// An omitted integer `seed` comes from the idempotency key, so a retried key rebuilds the
    /// same payload. An `int | None` seed is the package's to choose.
    fn seed(&mut self) {
        let declared = self
            .callable
            .field("seed")
            .is_some_and(|f| matches!(f.ty.plain(), Type::Int | Type::Seed));
        if declared && !self.document.contains_key("seed") {
            let digest = Sha256::digest(format!("cozy-seed\0{}", self.terms.key));
            let seed =
                u64::from_be_bytes(digest[..8].try_into().expect("8 bytes")) & ((1 << 53) - 1);
            self.document.insert("seed".into(), seed.into());
        }
    }
}

/// What an input file field accepts: the byte bound it declares, and each set of media types with
/// its own bound.
type Accepts = (Option<u64>, Vec<(Vec<String>, Option<u64>)>);

/// Whether a value of `ty` can name an input file.
fn holds_file(ty: &Type) -> bool {
    match ty {
        Type::Asset(_) | Type::Media { .. } | Type::Tree => true,
        Type::Union { branches, .. } => branches.iter().any(holds_file),
        Type::List(item) | Type::Map(item) => holds_file(item),
        Type::Struct(s) => s.fields.iter().any(|f| holds_file(&f.ty)),
        _ => false,
    }
}

/// The warning for a list of files cut to its limit.
fn cut_list(function: &str, list: &str, max: u64, left: String) -> String {
    match max {
        0 => format!("{function} takes no {list}: {left} left out"),
        max => format!("{list} takes at most {max}: {left} left out"),
    }
}

struct Spec {
    field: Option<String>,
    /// As written: a hash, or a path the client sent.
    source: String,
    label: Option<String>,
}

/// The input-file spec at `parts` below a field of type `ty`: its media types and byte bound.
fn asset_at(
    ty: &Type,
    parts: &[String],
    types: &[String],
    max: Option<u64>,
) -> Option<(Vec<String>, Option<u64>)> {
    match ty {
        Type::Asset(_) | Type::Media { .. } if parts.is_empty() => Some((types.to_vec(), max)),
        Type::Union { branches, .. } => {
            branches.iter().find_map(|b| asset_at(b, parts, types, max))
        }
        Type::List(item) if !parts.is_empty() && parts[0].parse::<u32>().is_ok() => {
            asset_at(item, &parts[1..], types, max)
        }
        Type::Struct(s) if !parts.is_empty() => {
            let field = s.fields.iter().find(|f| f.name == parts[0])?;
            let (types, max) = if field.asset_bound.media_types.is_empty() {
                (types, field.asset_bound.max_bytes.or(max))
            } else {
                (
                    field.asset_bound.media_types.as_slice(),
                    field.asset_bound.max_bytes.or(max),
                )
            };
            asset_at(&field.ty, &parts[1..], types, max)
        }
        _ => None,
    }
}

/// Files named inside the payload: any string at an input-file position.
fn embedded_files(ty: &Type, value: &Value, path: &str, terms: &Terms, out: &mut Vec<Spec>) -> Result<(), Error> {
    match (ty, value) {
        (Type::Asset(_) | Type::Media { .. }, Value::String(source)) if !source.is_empty() && crate::project::graph::mentions(source).is_empty() => {
            if source.contains("://") {
                return Err(invalid(
                    "asset_url",
                    format!("{path} needs a local file, not a URL"),
                ));
            }
            out.push(Spec {
                field: Some(path.to_owned()),
                source: source.clone(),
                label: None,
            });
        }
        (Type::Union { branches, .. }, Value::String(_))
            if branches.iter().any(|b| {
                *b == Type::Str || scalar(b, value.as_str().unwrap_or_default()).is_some()
            }) => {}
        (Type::Union { branches, .. }, _) => {
            let before = out.len();
            for branch in branches {
                embedded_files(branch, value, path, terms, out)?;
                if out.len() > before {
                    break;
                }
            }
        }
        (Type::List(item), Value::Array(values)) => {
            for (i, value) in values.iter().enumerate() {
                embedded_files(item, value, &format!("{path}.{i}"), terms, out)?;
            }
        }
        (Type::Struct(s), Value::Object(object)) => {
            for field in &s.fields {
                if let Some(value) = object.get(&field.name) {
                    embedded_files(&field.ty, value, &format!("{path}.{}", field.name), terms, out)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Local directories at Tree positions: each field's path and the directory as written.
fn tree_dirs(ty: &Type, value: &Value, path: &str, out: &mut Vec<(String, String)>) {
    match (ty, value) {
        (Type::Tree, Value::String(source)) if !source.is_empty() => {
            out.push((path.to_owned(), source.clone()));
        }
        (Type::Union { branches, .. }, value) => {
            for branch in branches {
                tree_dirs(branch, value, path, out);
            }
        }
        (Type::List(item), Value::Array(values)) => {
            for (i, value) in values.iter().enumerate() {
                tree_dirs(item, value, &format!("{path}.{i}"), out);
            }
        }
        (Type::Struct(s), Value::Object(object)) => {
            for field in &s.fields {
                if let Some(value) = object.get(&field.name) {
                    tree_dirs(&field.ty, value, &format!("{path}.{}", field.name), out);
                }
            }
        }
        _ => {}
    }
}

/// Writes `value` at `parts`, creating objects and list slots on the way.
fn set_path(
    document: &mut Map<String, Value>,
    parts: &[String],
    value: Value,
) -> Result<(), Error> {
    let mut node = document.entry(parts[0].clone()).or_insert(Value::Null);
    for part in &parts[1..] {
        if node.is_null() {
            *node = match part.parse::<usize>() {
                Ok(_) => Value::Array(vec![]),
                Err(_) => Value::Object(Map::new()),
            };
        }
        node = match node {
            Value::Array(list) => {
                let index: usize = part.parse().map_err(|_| {
                    invalid(
                        "asset_path_malformed",
                        format!("{part:?} is not a list position in {}", parts.join(".")),
                    )
                })?;
                if list.len() <= index {
                    list.resize(index + 1, Value::Null);
                }
                &mut list[index]
            }
            Value::Object(object) => object.entry(part.clone()).or_insert(Value::Null),
            _ => {
                return Err(invalid(
                    "asset_path_malformed",
                    format!(
                        "{} crosses a value that is not an object or list",
                        parts.join(".")
                    ),
                ));
            }
        };
    }
    *node = value;
    Ok(())
}

/// A scalar spelled for `ty`; None when the type has no such spelling.
pub(super) fn scalar(ty: &Type, raw: &str) -> Option<Value> {
    match ty {
        Type::Str | Type::Tree | Type::Unknown | Type::Prompt(_) | Type::Shape => Some(Value::String(raw.to_owned())),
        Type::Int | Type::Seed => integer(raw),
        Type::Float => raw
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .and_then(Number::from_f64)
            .map(Value::Number),
        Type::Bool => match raw.to_ascii_lowercase().as_str() {
            "true" | "yes" => Some(Value::Bool(true)),
            "false" | "no" => Some(Value::Bool(false)),
            _ => None,
        },
        Type::Null => (raw.eq_ignore_ascii_case("null") || raw.eq_ignore_ascii_case("none")).then_some(Value::Null),
        Type::Literal(members) => {
            let text = Value::String(raw.to_owned());
            if members.contains(&text) {
                return Some(text);
            }
            if let Some(member) = members.iter().find(|m| m.as_str().is_some_and(|m| m.eq_ignore_ascii_case(raw))) {
                return Some(member.clone());
            }
            if tiers(members, raw) {
                return Some(text);
            }
            let parsed: Value = serde_json::from_str(raw).ok()?;
            (members.iter().any(|m| same(m, &parsed)) || numbers(members, &parsed)).then_some(parsed)
        }
        Type::Union { branches, .. } => {
            // Every branch a bare field of its type spells: `16:9` for a shape|null, `7` for a seed|null.
            let scalars = || {
                branches.iter().filter(|b| {
                    matches!(
                        b,
                        Type::Str
                            | Type::Int
                            | Type::Float
                            | Type::Bool
                            | Type::Null
                            | Type::Literal(_)
                            | Type::Shape
                            | Type::Seed
                            | Type::Prompt(_)
                            | Type::Tree
                    )
                })
            };
            let text = |b: &Type| matches!(b, Type::Str | Type::Shape | Type::Prompt(_) | Type::Tree);
            // A branch with an unambiguous JSON spelling wins over a text branch.
            if let Ok(parsed) = serde_json::from_str::<Value>(raw)
                && !parsed.is_string()
                && scalars().any(|b| check(b, &mut parsed.clone(), "", &mut vec![]).is_empty())
            {
                return Some(match parsed {
                    Value::Number(_) => scalars()
                        .find_map(|b| (!text(b)).then(|| scalar(b, raw)).flatten())
                        .unwrap_or(parsed),
                    other => other,
                });
            }
            // `True` for a bool|null, `20.0` for an int|null: text is text wherever a text branch takes it.
            scalars()
                .filter(|b| text(b) || matches!(b, Type::Literal(_)))
                .find_map(|b| scalar(b, raw).filter(|v| check(b, &mut v.clone(), "", &mut vec![]).is_empty()))
                .or_else(|| scalars().find_map(|b| scalar(b, raw)))
        }
        Type::List(_) | Type::Map(_) | Type::Struct(_) | Type::Asset(_) | Type::Media { .. } | Type::Pickup(_) | Type::Model => None,
    }
}

fn integer(raw: &str) -> Option<Value> {
    let whole = || raw.parse::<f64>().ok().filter(|f| f.fract() == 0.0 && f.abs() < (1u64 << 53) as f64);
    raw.parse::<i64>()
        .map(Value::from)
        .or_else(|_| raw.parse::<u64>().map(Value::from))
        .ok()
        .or_else(|| whole().map(|f| Value::from(f as i64)))
}

/// A scalar a request file wrote as another spelling of the value `ty` takes: `"20"` or `20.0` for
/// an int, `"true"` for a bool, `Fast` for `fast`, a number for a string.
fn respelled(ty: &Type, value: &Value) -> Option<Value> {
    match (ty, value) {
        (Type::Int | Type::Float | Type::Bool, Value::String(s)) => scalar(ty, s),
        (Type::Int, Value::Number(n)) if !(n.is_i64() || n.is_u64()) => integer(&n.to_string()),
        (Type::Literal(_), Value::String(s)) => scalar(ty, s),
        (Type::Literal(members), Value::Number(n)) if tiers(members, &n.to_string()) => Some(Value::String(n.to_string())),
        (Type::Str, Value::Number(_) | Value::Bool(_)) => Some(Value::String(value.to_string())),
        _ => None,
    }
}

/// Whether `value` is a number for a Literal of numbers: Runtime takes the nearest member.
fn numbers(members: &[Value], value: &Value) -> bool {
    value.is_number() && members.iter().any(Value::is_number)
}

/// Whether `text` is a resolution (`4k`, `720p`, `1080`) for a Literal of them (`768p`, `1080p`, …):
/// Runtime takes the nearest. Any other text a Literal doesn't name stays refused.
fn tiers(members: &[Value], text: &str) -> bool {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    let tier = |member: &Value| member.as_str().and_then(|member| member.strip_suffix('p')).is_some_and(digits);
    let mut named = members.iter().filter(|member| !member.is_null()).peekable();
    let text = text.trim().to_ascii_lowercase();
    named.peek().is_some()
        && named.all(tier)
        && (digits(&text) || text.strip_suffix(['p', 'k']).is_some_and(digits))
}

/// JSON values equal as the interface means them: `1` and `1.0` are one number.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

/// Validates the payload against the request, dropping undeclared fields at any depth. Answers
/// the dropped paths; refuses with each problem at its path (five, then how many more), the
/// release it was checked against, and the dropped paths as a warning.
fn validate(
    target: &str,
    release: Option<&str>,
    callable: &Callable,
    document: &mut Map<String, Value>,
) -> Result<Vec<String>, Error> {
    let mut ignored = Vec::new();
    let mut problems = Vec::new();
    let mut missing = Vec::new();
    document.retain(|name, value| {
        if callable.field(name).is_some() {
            return true;
        }
        if name == LORA_MODE {
            if !matches!(value.as_str(), Some("bake" | "per_step")) {
                problems.push(format!("{LORA_MODE} must be \"bake\" or \"per_step\""));
            }
            return true;
        }
        ignored.push(name.clone());
        false
    });
    for field in &callable.request.fields {
        match document.get_mut(&field.name) {
            None if field.wire == Wire::Required => missing.push(describe::signature(field)),
            None => {}
            Some(value) => problems.extend(check_field(field, value, &field.name, &mut ignored)),
        }
    }
    ignored.sort();
    if !missing.is_empty() {
        problems.insert(
            0,
            format!("provide required arguments: [{}]", missing.join("; ")),
        );
    }
    if problems.is_empty() {
        return Ok(ignored);
    }
    let package = target
        .rsplit_once('/')
        .map_or(target, |(package, _)| package);
    let mut lines = vec![match release {
        Some(release) => {
            format!("{target} cannot take this request (checked against {package} {release}):")
        }
        None => format!("{target} cannot take this request:"),
    }];
    lines.extend(
        problems
            .iter()
            .take(5)
            .map(|problem| format!("  {problem}")),
    );
    if problems.len() > 5 {
        lines.push(format!("  and {} more", problems.len() - 5));
    }
    lines.extend(ignored_warning(target, &ignored).map(|w| format!("warning: {w}")));
    let refusal = invalid("request_payload_invalid", lines.join("\n"));
    Err(match release {
        Some(release) => refusal.next(format!("cozy run {target}@{release} --describe")),
        None => refusal,
    })
}

/// The warning for fields the payload named that the interface does not.
fn ignored_warning(target: &str, ignored: &[String]) -> Option<String> {
    let noun = if ignored.len() == 1 {
        "field"
    } else {
        "fields"
    };
    (!ignored.is_empty()).then(|| {
        format!(
            "ignored unknown {noun} {} — not in {target}'s interface",
            ignored.join(", ")
        )
    })
}

fn check_field(
    field: &Field,
    value: &mut Value,
    path: &str,
    ignored: &mut Vec<String>,
) -> Vec<String> {
    let problems = check(&field.ty, value, path, ignored);
    match problems.is_empty() {
        true => bounds(&field.constraints, value, path)
            .err()
            .into_iter()
            .collect(),
        false => problems,
    }
}

/// Checks `value` against `ty`: each problem at its path, every failing item and member. A union
/// no branch takes answers for its closest branch, the one the value's shape is (then the one
/// with fewest problems), never just that it matched none. Undeclared struct members are
/// dropped into `ignored`.
fn check(ty: &Type, value: &mut Value, path: &str, ignored: &mut Vec<String>) -> Vec<String> {
    if let Some(spelled) = respelled(ty, value) {
        *value = spelled;
    }
    match (ty, &mut *value) {
        (Type::Unknown | Type::Model, _)
        | (Type::Str, Value::String(_))
        | (Type::Bool, Value::Bool(_))
        | (Type::Null, Value::Null)
        | (Type::Float, Value::Number(_)) => vec![],
        (Type::Int, Value::Number(n)) if n.is_i64() || n.is_u64() => vec![],
        (Type::Asset(_) | Type::Tree, Value::String(s)) if !s.is_empty() => vec![],
        (Type::Shape, Value::String(s)) if ratio(s) => vec![],
        (Type::Shape, value) if value.is_string() => {
            vec![format!("{path}: {} names no ratio: write it as 16:9 or 2.39:1", shown(value))]
        }
        (Type::Seed, Value::Number(n)) if n.is_u64() => vec![],
        // What the project's binder made of a prompt, a described file and a pickup.
        (Type::Prompt(_) | Type::Media { .. } | Type::Pickup(_), Value::Object(_)) => vec![],
        (Type::Literal(members), value)
            if members.iter().any(|m| same(m, value))
                || numbers(members, value)
                || value.as_str().is_some_and(|text| tiers(members, text)) =>
        {
            vec![]
        }
        (Type::Literal(members), value) => {
            let allowed: Vec<String> = (members.iter())
                .map(|m| m.as_str().map_or_else(|| m.to_string(), str::to_owned))
                .collect();
            vec![format!(
                "{path}: {} is not one of {}",
                shown(value),
                allowed.join(", ")
            )]
        }
        (
            Type::Union {
                branches,
                tag_field,
            },
            value,
        ) => {
            let mut closest: Option<(u8, Vec<String>, Vec<String>)> = None;
            // The branch of the value's own kind first: 20 for str|int stays a number.
            let mut branches: Vec<&Type> = branches.iter().collect();
            branches.sort_by_key(|branch| std::cmp::Reverse(shape(branch, value)));
            for branch in branches {
                let branch = match (branch, tag_field) {
                    (Type::Struct(s), Some(tag)) if s.tag_field.is_none() => {
                        Type::Struct(Box::new(Struct {
                            tag_field: Some(tag.clone()),
                            ..(**s).clone()
                        }))
                    }
                    _ => branch.clone(),
                };
                let (mut tried, mut dropped) = (value.clone(), Vec::new());
                let problems = check(&branch, &mut tried, path, &mut dropped);
                if problems.is_empty() {
                    *value = tried;
                    ignored.extend(dropped);
                    return vec![];
                }
                let shape = shape(&branch, value);
                let closer = closest.as_ref().is_none_or(|(best, kept, _)| {
                    shape > *best || shape == *best && problems.len() < kept.len()
                });
                if shape > 0 && closer {
                    closest = Some((shape, problems, dropped));
                }
            }
            match closest {
                Some((_, problems, dropped)) => {
                    ignored.extend(dropped);
                    problems
                }
                None => vec![format!(
                    "{path}: {} is not {}",
                    shown(value),
                    describe::phrase(ty)
                )],
            }
        }
        (Type::List(item), Value::Array(values)) => {
            let mut problems = vec![];
            for (i, value) in values.iter_mut().enumerate() {
                problems.extend(check(item, value, &format!("{path}[{i}]"), ignored));
            }
            problems
        }
        (Type::Map(item), Value::Object(values)) => {
            let mut problems = vec![];
            for (key, value) in values.iter_mut() {
                problems.extend(check(item, value, &format!("{path}.{key}"), ignored));
            }
            problems
        }
        (Type::Struct(s), Value::Object(object)) => {
            if let Some(tag) = &s.tag_field
                && s.tag.is_some()
                && object.get(tag) != s.tag.as_ref()
            {
                return vec![format!("{path} has an absent or other {tag}")];
            }
            object.retain(|name, _| {
                let keep = s.fields.iter().any(|f| &f.name == name)
                    || s.tag_field.as_deref() == Some(name);
                if !keep {
                    ignored.push(format!("{path}.{name}"));
                }
                keep
            });
            let mut problems = vec![];
            for field in &s.fields {
                match object.get_mut(&field.name) {
                    Some(value) => problems.extend(check_field(
                        field,
                        value,
                        &format!("{path}.{}", field.name),
                        ignored,
                    )),
                    None if field.wire == Wire::Required => {
                        problems.push(format!("{path} omits required {}", field.name));
                    }
                    None => {}
                }
            }
            problems
        }
        _ => vec![format!("{path} is not {}", describe::phrase(ty))],
    }
}

/// Whether `text` names a ratio as Runtime reads one: `16:9`, `2.39:1`, `4/3`, `1920x1080`, `1.5`.
fn ratio(text: &str) -> bool {
    let side = |s: &str| s.trim().parse::<f64>().ok().filter(|v| v.is_finite());
    match text.split([':', '/', 'x', 'X', '×']).map(side).collect::<Vec<_>>()[..] {
        [Some(w)] => w > 0.0,
        [Some(w), Some(h)] => h != 0.0 && w / h > 0.0,
        _ => false,
    }
}

/// How closely `value`'s shape is `ty`'s: 2 the same kind (a list for a list, an object for a
/// struct whose tag it carries), 1 near (an object for a map), 0 another.
fn shape(ty: &Type, value: &Value) -> u8 {
    match (ty, value) {
        (Type::Union { branches, .. }, value) => {
            branches.iter().map(|b| shape(b, value)).max().unwrap_or(0)
        }
        (Type::Struct(s), Value::Object(object)) => match (&s.tag_field, &s.tag) {
            (Some(tag), Some(expected)) if object.get(tag) != Some(expected) => 0,
            _ => 2,
        },
        (Type::Map(_), Value::Object(_)) => 1,
        (Type::List(_), Value::Array(_))
        | (Type::Null, Value::Null)
        | (Type::Bool, Value::Bool(_))
        | (Type::Str | Type::Asset(_) | Type::Tree | Type::Shape, Value::String(_))
        | (Type::Prompt(_) | Type::Media { .. } | Type::Pickup(_), Value::Object(_))
        | (Type::Int | Type::Float | Type::Seed, Value::Number(_)) => 2,
        (Type::Literal(members), value) => {
            let kind = |v: &Value| std::mem::discriminant(v);
            u8::from(members.iter().any(|m| kind(m) == kind(value))) * 2
        }
        _ => 0,
    }
}

/// A value as a refusal quotes it: a scalar as written (a long string cut), else its kind.
fn shown(value: &Value) -> String {
    match value {
        Value::String(s) if s.chars().count() > 40 => {
            format!(
                "{:?}",
                format!("{}…", s.chars().take(39).collect::<String>())
            )
        }
        Value::Array(_) => "a list".into(),
        Value::Object(_) => "an object".into(),
        value => value.to_string(),
    }
}

/// What no value here can make good: too few. Over a limit goes on to Runtime, which clamps it.
fn bounds(c: &Constraints, value: &Value, path: &str) -> Result<(), String> {
    let length = match value {
        Value::String(s) => s.chars().count() as u64,
        Value::Array(a) => a.len() as u64,
        Value::Object(o) => o.len() as u64,
        _ => return Ok(()),
    };
    match c.min_length.filter(|min| length < *min) {
        Some(min) => Err(format!("{path} has length {length}; its minimum is {min}")),
        None => Ok(()),
    }
}

pub(crate) struct Sniffed {
    pub length: u64,
    pub media_type: String,
}

/// A file's size and media type, from its first bytes.
pub(crate) fn sniff(path: &Path) -> Result<Sniffed, Error> {
    let unreadable = |e: std::io::Error| {
        Error::new(
            Kind::NotFound,
            "input_file_unreadable",
            format!("input file {}: {e}", path.display()),
        )
    };
    let mut file = File::open(path).map_err(unreadable)?;
    let length = file.metadata().map_err(unreadable)?.len();
    let mut head = Vec::with_capacity(8192);
    file.by_ref()
        .take(8192)
        .read_to_end(&mut head)
        .map_err(unreadable)?;
    let media_type = match infer::get(&head).map(|t| t.mime_type()) {
        Some("audio/x-wav" | "audio/wave") => "audio/wav",
        Some("audio/x-flac") => "audio/flac",
        Some(other) => other,
        None => "application/octet-stream",
    };
    Ok(Sniffed {
        length,
        media_type: media_type.to_owned(),
    })
}

/// `what` names a path the request sent nothing under: Calcifer reads no client's disk.
fn not_sent(what: &str, path: &str) -> Error {
    Error::new(Kind::NotFound, "input_file_absent", format!("{what}: no file {path} came with the request"))
        .remedy("name a file that exists; a client sends each by content (PUT /v1/files) and names it in `paths`")
}

/// A slot's or callable's name.
fn usage(code: &str, message: impl Into<String>) -> Error {
    Error::new(Kind::Usage, code, message)
}

fn invalid(code: &str, message: impl Into<String>) -> Error {
    Error::new(Kind::Validation, code, message)
}
