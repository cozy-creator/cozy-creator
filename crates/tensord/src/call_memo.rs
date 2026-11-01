//! The machine memo of memoized callables (`memoize=True`). A run of one, top-level or a
//! child call, is answered from the success this machine holds for its signer under the same
//! computation, and starts nothing; a success is held (`Objects::hold_memo`). A computation is
//! the callable's operation identity (`operation`), the environment it runs in (its lock and
//! Runtime/TensorFS build), its request as given (files by content), its attention pin and lane
//! and the models it runs with. An author's condition (`memo_requires`) names the parameters a
//! request must give for it to be memoizable at all.
use crate::{api::domain::ModelChoice, catalog::Generation, gpu_service::Plan, journal::InputFile};
use serde_json::{json, Value};
use tensorfs::sha256;

pub enum Memo {
    /// Not memoizable: not declared so, or nothing identifies its code or environment.
    Off,
    /// Memoizable, but the request does not meet its author's condition: why.
    Unmet(String),
    /// Its computation (`sha256:<hex>`), and what keyed its code.
    Key(String, Basis),
}

/// What keys a memoized function's code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    /// Its own package's function: its module and the modules of its package it imports.
    Closure,
    /// Its package's whole source, which a closure could not be read for.
    Package,
    /// Another package's function: the operation identity its source declares.
    Identity,
}

impl Basis {
    pub fn name(self) -> &'static str {
        match self {
            Basis::Closure => "closure",
            Basis::Package => "package",
            Basis::Identity => "identity",
        }
    }
}

/// The memo of a call of `invocable` (its declaration) in App `application` of `generation`.
pub fn of(
    generation: &Generation,
    application: &str,
    invocable: &Value,
    request: &Value,
    files: &[InputFile],
    (attention, lane): (&str, &str),
    models: Value,
) -> Memo {
    if invocable["memoize"].as_bool() != Some(true) {
        return Memo::Off;
    }
    let missing: Vec<_> = invocable["memo_requires"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|name| request.get(*name).is_none_or(Value::is_null))
        .collect();
    if !missing.is_empty() {
        return Memo::Unmet(format!("{} not given", missing.join(", ")));
    }
    let Some((operation, basis)) = operation(generation, application, invocable) else {
        return Memo::Off;
    };
    if generation.environment.is_empty() {
        return Memo::Off;
    }
    let mut files: Vec<_> = files.iter().map(|f| json!([f.input_id, f.digest, f.media_type])).collect();
    files.sort_by_key(|file| file.to_string());
    let mut computation = json!({"operation": operation, "environment": generation.environment,
        "request": checkpoints(request), "files": files, "attention": attention, "models": models});
    // A lane asked is part of what ran, whatever the Hub would pick.
    if !lane.is_empty() {
        computation["lane"] = lane.into();
    }
    Memo::Key(format!("sha256:{}", sha256::hex_digest(&crate::boundary_json::exact(&computation))), basis)
}

/// The request with each model artifact it carries as its checkpoint's header: the run that
/// produced a checkpoint is provenance, never part of a consumer's computation.
fn checkpoints(value: &Value) -> Value {
    const ARTIFACT: [&str; 4] = ["checkpoint", "output_slot", "producer_request_id", "transaction"];
    match value {
        Value::Object(fields) if fields.len() == ARTIFACT.len() && ARTIFACT.iter().all(|k| fields.contains_key(*k)) => {
            json!({"checkpoint": fields["checkpoint"]})
        }
        Value::Object(fields) => Value::Object(fields.iter().map(|(k, v)| (k.clone(), checkpoints(v))).collect()),
        Value::Array(items) => Value::Array(items.iter().map(checkpoints).collect()),
        other => other.clone(),
    }
}

/// The models a call runs with: each slot's checkpoints and adapters once resolved (its raw
/// sources too), else the choices it carries.
pub fn models(plan: Option<&Plan>, choices: &[ModelChoice]) -> Value {
    match plan {
        Some(plan) => plan
            .slots
            .iter()
            .map(|slot| json!({"slot": slot.binding.model_binding_path,
                "snapshots": slot.binding.snapshots, "adapters": slot.binding.adapters}))
            .chain(plan.sources.iter().map(|source| json!({"source": source.parameter, "manifest": source.manifest})))
            .collect(),
        None => {
            let mut chosen: Vec<_> = choices
                .iter()
                .map(|choice| sha256::hex_digest(&crate::archive::encode_model_choice(choice)))
                .collect();
            chosen.sort();
            json!(chosen)
        }
    }
}

/// A memoized function's operation identity, the same on every machine, with the module and the
/// export: the root's own function by its package and the code closure this machine's Runtime
/// read from its source, a callee's by its package and the identity this machine read from its
/// installed source, else its package's files. The root's own description is its caller's, so
/// only this machine's reading keys it. A package with none of these memoizes nothing.
pub(crate) fn operation(generation: &Generation, application: &str, invocable: &Value) -> Option<(String, Basis)> {
    let (module, export) = (invocable["module"].as_str()?, invocable["export"].as_str()?);
    let identity = invocable["operation_identity"].as_str().filter(|identity| {
        identity
            .strip_prefix("sha256:")
            .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
    });
    let (base, basis) = match application.is_empty() || application == generation.application {
        true => match generation.closures.get(&format!("{module}:{export}")) {
            Some(closure) => (format!("{}\0{closure}", generation.package), Basis::Closure),
            None => (generation.source_digest.clone(), Basis::Package),
        },
        false => {
            let callee = generation.callees.iter().find(|c| c.application == application)?;
            match identity {
                Some(identity) => (format!("{}\0{identity}", callee.package), Basis::Identity),
                None => (callee.source_digest.clone(), Basis::Package),
            }
        }
    };
    let operation = format!("sha256:{}", sha256::hex_digest(format!("{base}\0{module}\0{export}").as_bytes()));
    (!base.is_empty()).then_some((operation, basis))
}
