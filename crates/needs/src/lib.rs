//! Which of a function's models a request needs (cl-1103; Paul, 11-16: "plan, without having to install packages and run
//! arbitrary code"). A package says it in its interface: on a model slot or a called function, `needs` is one CEL boolean
//! expression over `request`, the request as JSON with each field it leaves out at its declared default. Calcifer (a quote, what a
//! box prepares) and TensorD (what a call fetches before it starts) read it here, with one evaluator. A slot with no rule is always
//! needed, and one whose rule gives no answer for a request is as one with none. A slot may rule its components too
//! (`component_needs`, the Runtime's `needs={"model.vision": …}`): one whose rule doesn't hold isn't fetched for the call; one with
//! no rule, or whose rule gives no answer, comes with its slot.
//!
//! What a call then wants of its models is one walk here too (`wants`, `resolved`): its slots by the rules' answers and, for a
//! function that calls others, theirs, its own package's or another's (cl-1123: Orbit's `render_shot` is H3's). Calcifer walks it
//! to quote a call and to tell a machine what to fetch ahead of one (Paul, 11-17: "calcifer says 'okay, I'm going to send machine-A
//! this request, so I should tell it to download the required files'"); a machine walks it for a call sent with no such word. The
//! two can't disagree on what a request needs.
#![forbid(unsafe_code)]
#![deny(unreachable_pub)]

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};


use api::interface::{Field, Struct, Type};
use cel::common::ast::{operators, CallExpr, ComprehensionExpr, EntryExpr, Expr, IdedExpr, LiteralValue};
use serde::Deserialize as _;
use serde_json::{Map, Value};

/// The one name a rule reads.
const REQUEST: &str = "request";
/// The longest rule, as the Runtime's describer holds one (`MAX_NEEDS_RULE`).
const MAX_RULE: usize = 1024;
/// List tests (`exists`, `all`, …) one inside another: a rule takes at most a request's length squared.
const MAX_NESTED: usize = 2;
/// What a rule may do, said where it does something else.
const MAY: &str = "a rule has `&&`, `||`, `!`, comparisons, `in`, `size()`, `has()`, and `exists`, `all`, `exists_one` over a list";

/// One call's answers: each ruled model slot (by its parameter), component of one (`<parameter>.<component>`) or called function (by
/// its name) of its function, by what its rule says of the request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answers {
    /// Its rule holds: the call waits on its weights.
    pub needed: BTreeSet<String>,
    /// Its rule doesn't hold: nothing of it is fetched for the call.
    pub unneeded: BTreeSet<String>,
    /// Its rule gave no answer, and why: it is as one with no rule.
    pub unanswered: Vec<(String, String)>,
}

/// Each rule of an interface's entrypoint: a model slot's by its parameter, one of its components' by `<parameter>.<component>`, a
/// called function's by its name.
pub fn rules(entry: &Value) -> Vec<(String, String)> {
    let mut rules = vec![];
    for slot in api::interface::models(entry) {
        let Some(parameter) = api::interface::slot_path(slot).and_then(|path| path.rsplit_once(".models.")).map(|(_, parameter)| parameter) else {
            continue;
        };
        rules.extend(slot["needs"].as_str().map(|rule| (parameter.to_owned(), rule.to_owned())));
        for (component, rule) in slot["component_needs"].as_object().into_iter().flatten() {
            rules.extend(rule.as_str().map(|rule| (component_rule(parameter, component), rule.to_owned())));
        }
    }
    let calls = (entry["calls"].as_array().into_iter().flatten())
        .filter_map(|call| Some((call["function"].as_str()?.to_owned(), call["needs"].as_str()?.to_owned())));
    rules.extend(calls);
    rules
}

/// The name a component's rule answers under: `model.vision`.
fn component_rule(parameter: &str, component: &str) -> String {
    format!("{parameter}.{component}")
}

/// What `entry`'s rules say of the request `sent`, as its function gets it bound; one sent as nothing is its defaults.
pub fn answers(entry: &Value, sent: &Value) -> Answers {
    let mut answers = Answers::default();
    let rules = rules(entry);
    if rules.is_empty() {
        return answers;
    }
    let request = match Struct::deserialize(&entry["request"]) {
        Ok(request) => match sent {
            Value::Object(members) => Value::Object(fields(&request.fields, members)),
            _ => Value::Object(fields(&request.fields, &Map::new())),
        },
        Err(error) => {
            answers.unanswered = rules.into_iter().map(|(name, _)| (name, format!("its request can't be read: {error}"))).collect();
            return answers;
        }
    };
    for (name, rule) in rules {
        match holds(&rule, &request) {
            Ok(true) => answers.needed.insert(name),
            Ok(false) => answers.unneeded.insert(name),
            Err(why) => {
                answers.unanswered.push((name, why));
                continue;
            }
        };
    }
    answers
}

/// Another package a walk goes into: its distribution, as a call names it (`calls[].package`), and its interface (an environment's
/// callee App, cl-1123). Its slots and functions are named behind it: `<distribution>/<function>.models.<parameter>`.
pub type Other<'a> = (&'a str, &'a Value);

/// A call's package and the others it may call: `None` is its own, `Some(i)` the i-th other.
struct Packages<'a> {
    own: &'a Value,
    others: &'a [Other<'a>],
}

impl<'a> Packages<'a> {
    fn document(&self, package: Option<usize>) -> &'a Value {
        package.map_or(self.own, |at| self.others[at].1)
    }

    /// The package a call `from` a function of `from` goes to: another's when it is one of `others`, else `from`'s own.
    fn called(&self, from: Option<usize>, call: &Value) -> Option<usize> {
        let named = call["package"].as_str().map(normalized);
        named.and_then(|named| self.others.iter().position(|(distribution, _)| normalized(distribution) == named)).or(from)
    }

    /// What a walk names `function` of `package`: bare, its own; behind its distribution, another's.
    fn named(&self, package: Option<usize>, function: &str) -> String {
        match package {
            None => function.to_owned(),
            Some(at) => format!("{}/{function}", self.others[at].0),
        }
    }

    /// The function a walk's name names, with its package.
    fn entry(&self, name: &str) -> Option<(Option<usize>, &'a Value)> {
        match name.split_once('/') {
            None => api::interface::entrypoint(self.own, name).map(|entry| (None, entry)),
            Some((distribution, function)) => {
                let at = self.others.iter().position(|(held, _)| *held == distribution)?;
                api::interface::entrypoint(self.others[at].1, function).map(|entry| (Some(at), entry))
            }
        }
    }
}

/// A distribution's name as Python compares them (PEP 503): `minimax_h3` is `minimax-h3`.
fn normalized(name: &str) -> String {
    let mut said = String::with_capacity(name.len());
    for part in name.split(['-', '_', '.']).filter(|part| !part.is_empty()) {
        if !said.is_empty() {
            said.push('-');
        }
        said.push_str(&part.to_ascii_lowercase());
    }
    said
}

/// What a call wants of its models, by slot (`<function>.models.<parameter>`; another package's behind its distribution,
/// `<distribution>/<function>.models.<parameter>`), as its function's needs rules say of its request
/// (`wants`): the slots it waits for before it starts; for each function it may call that no rule answers for, the slots that one
/// waits for, of which what they all read is waited for; and what it may read besides, by group, fetched only in the background. A
/// slot named in none is never fetched for it. `unread`: the components of a slot its call reads none of, by the slot (its
/// `component_needs` rules don't hold), wherever the slot is named. Each side resolves a slot to a checkpoint itself (`resolved`):
/// Calcifer by its Hub card, to quote a call; a machine for its lane and store, to fetch one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wants {
    pub needed: Vec<String>,
    pub either: Vec<Vec<String>>,
    pub extras: Vec<(String, Vec<String>)>,
    pub unread: Vec<(String, Vec<String>)>,
}

impl Wants {
    /// Each function a slot of it belongs to, once, in the order first named; another package's behind its distribution.
    pub fn functions(&self) -> Vec<&str> {
        let slots = self.needed.iter().chain(self.either.iter().flatten()).chain(self.extras.iter().flat_map(|(_, slots)| slots));
        let mut functions: Vec<&str> = vec![];
        for function in slots.filter_map(|slot| slot.split_once(".models.").map(|(function, _)| function)) {
            if !functions.contains(&function) {
                functions.push(function);
            }
        }
        functions
    }
}

/// The extras of a function's own on-demand slots, as a group.
pub const ON_DEMAND: &str = "its on-demand models";

/// What a call of `entrypoint` in the interface `document` wants of its models, by slot, given what its rules say of the request
/// (`answers`; none, for a call sent with no request). A function that loads models waits for each slot but an on-demand one, and
/// for one whose rule holds; one whose rule doesn't hold is never fetched; the rest of its on-demand slots are its extras. One that
/// loads none and calls others (an orchestrator, H3's `generate`; Orbit, whose `render_shot` is H3's, one of `others`): for each
/// function it calls (`callees`), what that one waits for is waited for when its rule says this request calls it, and is `either`
/// when no rule answers, of which what they all read is waited for; the rest of each is an extra. A called function's own rules
/// read its own request, which only its call brings (Orbit builds H3's in code): none of them answers here.
pub fn wants(document: &Value, others: &[Other<'_>], entrypoint: &str, answers: &Answers) -> Wants {
    let mut wants = Wants::default();
    let Some(entry) = api::interface::entrypoint(document, entrypoint) else { return wants };
    if !api::interface::models(entry).is_empty() {
        let (needed, extras) = split(entry, answers, "");
        wants.needed = needed;
        if !extras.is_empty() {
            wants.extras.push((ON_DEMAND.to_owned(), extras));
        }
        wants.unread = unread(entry, answers);
        return wants;
    }
    let (packages, unasked) = (Packages { own: document, others }, Answers::default());
    for callee in callees(document, others, entrypoint, &answers.unneeded) {
        let Some((package, entry)) = packages.entry(&callee) else { continue };
        let behind = match package {
            None => String::new(),
            Some(at) => format!("{}/", others[at].0),
        };
        let (needed, extras) = split(entry, &unasked, &behind);
        match api::interface::name(entry).is_some_and(|name| answers.needed.contains(name)) {
            true => {
                wants.needed.extend(needed);
                if !extras.is_empty() {
                    wants.extras.push((callee, extras));
                }
            }
            false => {
                wants.either.push(needed.clone());
                wants.extras.push((callee, [needed, extras].concat()));
            }
        }
    }
    wants
}

/// `entry`'s own slots by `answers`, each path `behind` its package's name when it is another's: those it waits for (each but an
/// on-demand one, raw weights too, and one whose rule holds), and its extras (an on-demand one no rule answers). One whose rule
/// doesn't hold is in neither.
fn split(entry: &Value, answers: &Answers, behind: &str) -> (Vec<String>, Vec<String>) {
    let (mut needed, mut extras) = (vec![], vec![]);
    for slot in api::interface::models(entry) {
        let Some(path) = api::interface::slot_path(slot) else { continue };
        let parameter = path.rsplit_once(".models.").map_or(path, |(_, parameter)| parameter);
        if answers.unneeded.contains(parameter) {
            continue;
        }
        let raw = slot["weights"].as_bool() == Some(true);
        let on_demand = slot["on_demand"].as_bool() == Some(true);
        match !on_demand || raw || answers.needed.contains(parameter) {
            true => needed.push(format!("{behind}{path}")),
            false => extras.push(format!("{behind}{path}")),
        }
    }
    (needed, extras)
}

/// `entry`'s slots (by path) with the components its rules say the request doesn't read (`<parameter>.<component>` among `unneeded`).
fn unread(entry: &Value, answers: &Answers) -> Vec<(String, Vec<String>)> {
    let mut unread = vec![];
    for slot in api::interface::models(entry) {
        let Some(path) = api::interface::slot_path(slot) else { continue };
        let parameter = path.rsplit_once(".models.").map_or(path, |(_, parameter)| parameter);
        let components: Vec<String> = (slot["component_needs"].as_object().into_iter().flatten())
            .map(|(component, _)| component)
            .filter(|component| answers.unneeded.contains(&component_rule(parameter, component)))
            .cloned()
            .collect();
        if !components.is_empty() {
            unread.push((path.to_owned(), components));
        }
    }
    unread
}

/// The functions that construct a model and that `entrypoint` of `document`, loading none itself, calls: those its `calls` name (the
/// Runtime's static reading, through its own helpers), its package's own or one of `others`' (cl-1123: Orbit's `render_shot`, H3's),
/// and theirs in turn, in each package's order, its own first (cl-1077: H3's `edit` brought `upscale`'s upscaler). Another package's
/// is named behind its distribution (`minimax-h3/ref2va_segment`). One that declares no `calls` may call any internal one of its
/// package. One its rules say the request doesn't call (`uncalled`) is left out, and what only it reaches.
pub fn callees(document: &Value, others: &[Other<'_>], entrypoint: &str, uncalled: &BTreeSet<String>) -> Vec<String> {
    let packages = Packages { own: document, others };
    let constructs = |entry: &Value| api::interface::models(entry).iter().any(|slot| slot["weights"].as_bool() != Some(true));
    let reached = api::interface::entrypoint(document, entrypoint).filter(|entry| entry.get("calls").is_some()).map(|root| {
        let (mut reached, mut pending) = (BTreeSet::new(), vec![(None, root)]);
        while let Some((package, entry)) = pending.pop() {
            for call in entry["calls"].as_array().into_iter().flatten() {
                let Some(called) = call["function"].as_str() else { continue };
                if std::ptr::eq(entry, root) && uncalled.contains(called) {
                    continue;
                }
                let to = packages.called(package, call);
                if reached.insert((to, called.to_owned())) {
                    pending.extend(api::interface::entrypoint(packages.document(to), called).map(|entry| (to, entry)));
                }
            }
        }
        reached
    });
    let mut found = vec![];
    for package in std::iter::once(None).chain((0..others.len()).map(Some)) {
        for entry in api::interface::entrypoints(packages.document(package)).filter(|entry| constructs(entry)) {
            let Some(name) = api::interface::name(entry) else { continue };
            let called = match &reached {
                Some(reached) => reached.contains(&(package, name.to_owned())),
                None => package.is_none() && api::interface::internal(entry),
            };
            if called {
                found.push(packages.named(package, name));
            }
        }
    }
    found
}

/// Checkpoints by their id, each with the components read of it (none: all of it).
pub type Selected = Vec<(String, Vec<String>)>;

/// `Wants` as checkpoints: what the call waits for, and its extras by group. `unread`: of a checkpoint resolved whole (no components
/// named) in `needed`, the components the call reads none of, which only its header can take out (`wanted`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    pub needed: Selected,
    pub extras: Vec<(String, Selected)>,
    pub unread: Selected,
}

/// `wants` as checkpoints, each slot by `resolve` (its checkpoint and the components read of it; none: it reads nothing here), less
/// the components its call doesn't read (`Wants::unread`): what the call waits for is what every function of its `either` reads,
/// each one resolved whole (one that doesn't is left out), and each needed slot's; then each group of extras. Each checkpoint once in
/// each, with the components any of its slots reads.
pub fn resolved(wants: &Wants, resolve: impl Fn(&str) -> Option<(String, Vec<String>)>) -> Resolved {
    let unread = |slot: &str| wants.unread.iter().find(|(held, _)| held == slot).map_or(&[][..], |(_, components)| components.as_slice());
    // A slot whose components are named reads them less its unread ones (none left: nothing here); one read whole keeps them, said
    // in `Resolved::unread`.
    let resolve = |slot: &str| {
        resolve(slot).and_then(|(checkpoint, components)| match components.is_empty() {
            true => Some((checkpoint, components)),
            false => {
                let kept: Vec<String> = components.into_iter().filter(|component| !unread(slot).contains(component)).collect();
                (!kept.is_empty()).then_some((checkpoint, kept))
            }
        })
    };
    let selected = |slots: &[String]| {
        let mut selected = Selected::new();
        joined(&mut selected, slots.iter().filter_map(|slot| resolve(slot.as_str())).collect());
        selected
    };
    let each: Vec<Selected> = (wants.either.iter())
        .filter_map(|slots| slots.iter().map(|slot| resolve(slot.as_str())).collect::<Option<Selected>>())
        .map(|all| {
            let mut one = Selected::new();
            joined(&mut one, all);
            one
        })
        .collect();
    let mut needed = shared_of(&each);
    joined(&mut needed, selected(wants.needed.as_slice()));
    let extras = (wants.extras.iter())
        .map(|(group, slots)| (group.clone(), selected(slots.as_slice())))
        .filter(|(_, selected)| !selected.is_empty())
        .collect();
    // A whole checkpoint read stays whole when any of its slots reads all of it.
    let mut whole = Selected::new();
    for (slot, components) in &wants.unread {
        let Some((checkpoint, named)) = wants.needed.contains(slot).then(|| resolve(slot.as_str())).flatten() else { continue };
        let read_whole = (wants.needed.iter())
            .filter(|other| *other != slot)
            .filter_map(|other| resolve(other.as_str()))
            .any(|(held, named)| held == checkpoint && named.is_empty());
        if named.is_empty() && !read_whole && needed.iter().any(|(held, all)| *held == checkpoint && all.is_empty()) {
            joined(&mut whole, vec![(checkpoint, components.clone())]);
        }
    }
    Resolved { needed, extras, unread: whole }
}

/// What the checkpoints and components of `each` all share. No components names a whole checkpoint.
fn shared_of(each: &[Selected]) -> Selected {
    let holds = |list: &Selected, header: &str, component: Option<&str>| {
        list.iter().any(|(held, components)| held == header && (components.is_empty() || component.is_none_or(|component| components.iter().any(|c| c == component))))
    };
    let mut shared: Selected = vec![];
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

/// `more` added to `into`: each checkpoint once, with the components either reads (none: all of it).
fn joined(into: &mut Selected, more: Selected) {
    for (checkpoint, components) in more {
        match into.iter_mut().find(|(held, _)| *held == checkpoint) {
            Some((_, held)) if components.is_empty() => held.clear(),
            Some((_, held)) if held.is_empty() => {}
            Some((_, held)) => {
                let new: Vec<String> = components.into_iter().filter(|component| !held.contains(component)).collect();
                held.extend(new);
            }
            None => into.push((checkpoint, components)),
        }
    }
}

/// Each rule of the interface `document` that can't be read against its function's request, in words: a package with one isn't
/// published, installed or run. A document with no rules has none.
pub fn faults(document: &Value) -> Vec<String> {
    let mut faults = vec![];
    for entry in api::interface::entrypoints(document) {
        let rules = rules(entry);
        if rules.is_empty() {
            continue;
        }
        let function = api::interface::name(entry).unwrap_or_default();
        let request = match Struct::deserialize(&entry["request"]) {
            Ok(request) => request,
            Err(error) => {
                faults.push(format!("{function}: its request can't be read, so its rules can't either: {error}"));
                continue;
            }
        };
        for (name, rule) in rules {
            if let Err(why) = check(&rule, &request) {
                faults.push(format!("{function}: the rule for {name}, `{rule}`: {why}"));
            }
        }
    }
    faults
}

/// `sent` as a rule reads it: each field it leaves out at its declared default, at any depth, and a prompt sent as words alone as
/// its text with no references. A field with no default that isn't sent stays out.
fn fields(declared: &[Field], sent: &Map<String, Value>) -> Map<String, Value> {
    let mut filled = sent.clone();
    for field in declared {
        if let Some(held) = sent.get(&field.name).or(field.default.as_ref()) {
            filled.insert(field.name.clone(), typed(&field.ty, held));
        }
    }
    filled
}

fn typed(ty: &Type, held: &Value) -> Value {
    match (ty, held) {
        (Type::Struct(inner) | Type::Pickup(inner), Value::Object(members)) => Value::Object(fields(&inner.fields, members)),
        (Type::Media { extras, .. }, Value::Object(members)) => Value::Object(fields(&extras.fields, members)),
        (Type::List(item), Value::Array(items)) => Value::Array(items.iter().map(|held| typed(item, held)).collect()),
        (Type::Map(item), Value::Object(members)) => {
            Value::Object(members.iter().map(|(name, held)| (name.clone(), typed(item, held))).collect())
        }
        (Type::Prompt(_), Value::String(text)) => serde_json::json!({"text": text, "references": []}),
        (Type::Prompt(_), Value::Object(members)) => {
            let mut prompt = members.clone();
            prompt.entry("text").or_insert_with(|| Value::String(String::new()));
            prompt.entry("references").or_insert_with(|| Value::Array(vec![]));
            Value::Object(prompt)
        }
        (Type::Union { .. }, Value::Null) => Value::Null,
        // `T | None`, set: as `T`.
        (Type::Union { .. }, held) if !std::ptr::eq(ty.plain(), ty) => typed(ty.plain(), held),
        (_, held) => held.clone(),
    }
}

/// Whether `rule` holds for `request`, its defaults filled. Why not, when it gives no answer: it can't be read, it reads what the request
/// doesn't hold, or it gives something other than true or false.
pub fn holds(rule: &str, request: &Value) -> Result<bool, String> {
    let program = compiled(rule)?;
    let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut context = cel::Context::default();
        context.add_variable_from_value(REQUEST, value(request));
        program.execute(&context)
    }));
    match answer {
        Ok(Ok(cel::Value::Bool(holds))) => Ok(holds),
        Ok(Ok(other)) => Err(format!("it gives {}, not true or false", named(&other))),
        Ok(Err(error)) => Err(first_line(&error.to_string())),
        Err(_) => Err("it can't be evaluated".into()),
    }
}

/// Checks `rule` against the request it reads: it is CEL, it reads `request` alone, every field it names is one the request
/// declares, and it gives true or false. Why not, in words.
pub fn check(rule: &str, request: &Struct) -> Result<(), String> {
    let program = compiled(rule)?;
    match (Checker { request: &request.fields }).of(program.expression(), &[], 0)? {
        Kind::Bool | Kind::Any => Ok(()),
        other => Err(format!("it gives {}, not true or false", other.said())),
    }
}

fn compiled(rule: &str) -> Result<cel::Program, String> {
    if rule.len() > MAX_RULE {
        return Err(format!("it is longer than {MAX_RULE} characters"));
    }
    // The parser is a dependency's, reading a package's text: a panic in it is that text's fault, not this process's end.
    match std::panic::catch_unwind(|| cel::Program::compile(rule)) {
        Ok(Ok(program)) => Ok(program),
        Ok(Err(errors)) => Err(format!("it isn't CEL: {}", first_line(&errors.to_string()))),
        Err(_) => Err("it isn't CEL".into()),
    }
}

fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default().to_owned()
}

/// A request's JSON as CEL holds it: whole numbers as `int`, so `request.steps == 8` compares as written.
fn value(json: &Value) -> cel::Value {
    match json {
        Value::Null => cel::Value::Null,
        Value::Bool(held) => cel::Value::Bool(*held),
        Value::Number(number) => match (number.as_i64(), number.as_f64()) {
            (Some(whole), _) => cel::Value::Int(whole),
            (None, Some(real)) => cel::Value::Float(real),
            (None, None) => cel::Value::Null,
        },
        Value::String(text) => cel::Value::String(Arc::new(text.clone())),
        Value::Array(items) => cel::Value::List(Arc::new(items.iter().map(value).collect())),
        Value::Object(members) => {
            let held: HashMap<cel::objects::Key, cel::Value> =
                members.iter().map(|(name, member)| (cel::objects::Key::String(Arc::new(name.clone())), value(member))).collect();
            cel::Value::Map(cel::objects::Map { map: Arc::new(held) })
        }
    }
}

fn named(value: &cel::Value) -> &'static str {
    match value {
        cel::Value::List(_) => "a list",
        cel::Value::Map(_) => "a group of fields",
        cel::Value::Int(_) | cel::Value::UInt(_) | cel::Value::Float(_) => "a number",
        cel::Value::String(_) => "text",
        cel::Value::Null => "nothing",
        _ => "something else",
    }
}

/// What a part of a rule is, as far as the request's schema says.
#[derive(Clone, Debug)]
enum Kind<'a> {
    Bool,
    Int,
    Float,
    Text,
    Null,
    List(Box<Kind<'a>>),
    Map(Box<Kind<'a>>),
    Fields(&'a [Field]),
    /// A file with its description, and its version's own fields.
    Media(&'a [Field]),
    Prompt,
    /// It, or nothing.
    Optional(Box<Kind<'a>>),
    /// The schema doesn't say: its evaluation answers.
    Any,
}

impl Kind<'_> {
    fn said(&self) -> &'static str {
        match self {
            Kind::Bool => "true or false",
            Kind::Int => "a whole number",
            Kind::Float => "a number",
            Kind::Text => "text",
            Kind::Null => "nothing",
            Kind::List(_) => "a list",
            Kind::Map(_) => "a map",
            Kind::Fields(_) => "a group of fields",
            Kind::Media(_) => "a file",
            Kind::Prompt => "a prompt",
            Kind::Optional(_) => "a value that may be unset (compare it with `!= null`)",
            Kind::Any => "a value",
        }
    }
}

fn kind(ty: &Type) -> Kind<'_> {
    match ty {
        Type::Bool => Kind::Bool,
        Type::Int | Type::Seed => Kind::Int,
        Type::Float => Kind::Float,
        Type::Str | Type::Shape => Kind::Text,
        Type::Null => Kind::Null,
        Type::Literal(members) => literal(members),
        Type::List(item) => Kind::List(Box::new(kind(item))),
        Type::Map(item) => Kind::Map(Box::new(kind(item))),
        Type::Struct(inner) | Type::Pickup(inner) => Kind::Fields(&inner.fields),
        Type::Media { extras, .. } => Kind::Media(&extras.fields),
        Type::Prompt(_) => Kind::Prompt,
        Type::Union { branches, .. } => {
            let mut set = branches.iter().filter(|branch| **branch != Type::Null);
            match (set.next(), set.next()) {
                (Some(one), None) if branches.len() > 1 => Kind::Optional(Box::new(kind(one))),
                (Some(one), None) => kind(one),
                _ => Kind::Any,
            }
        }
        Type::Asset(_) | Type::Tree | Type::Model | Type::Unknown => Kind::Any,
    }
}

fn literal(members: &[Value]) -> Kind<'static> {
    if members.iter().all(Value::is_boolean) {
        Kind::Bool
    } else if members.iter().all(|member| member.is_i64() || member.is_u64()) {
        Kind::Int
    } else if members.iter().all(Value::is_number) {
        Kind::Float
    } else if members.iter().all(Value::is_string) {
        Kind::Text
    } else {
        Kind::Any
    }
}

/// `held`'s member `field`.
fn member<'a>(held: &Kind<'a>, field: &str) -> Result<Kind<'a>, String> {
    let among = |fields: &'a [Field]| fields.iter().find(|declared| declared.name == field).map(|declared| kind(&declared.ty));
    let absent = |has: Vec<&str>| format!("it has no field `{field}`; it has {}", has.join(", "));
    match held {
        Kind::Fields(fields) => {
            let fields: &'a [Field] = *fields;
            among(fields).ok_or_else(|| absent(fields.iter().map(|declared| declared.name.as_str()).collect()))
        }
        Kind::Media(extras) => {
            let extras: &'a [Field] = *extras;
            match field {
                "file" | "description" | "metadata" => Ok(Kind::Any),
                _ => among(extras).ok_or_else(|| {
                    absent(["file", "description", "metadata"].into_iter().chain(extras.iter().map(|declared| declared.name.as_str())).collect())
                }),
            }
        }
        Kind::Prompt => match field {
            "text" => Ok(Kind::Text),
            "references" => Ok(Kind::List(Box::new(Kind::Any))),
            _ => Err(absent(vec!["text", "references"])),
        },
        Kind::Optional(inner) => member(&**inner, field),
        Kind::Map(item) => Ok((**item).clone()),
        Kind::Any => Ok(Kind::Any),
        other => Err(format!("it is {}, which has no fields", other.said())),
    }
}

/// A name a list test binds, and what it holds.
type Name<'a> = (String, Kind<'a>);

struct Checker<'a> {
    request: &'a [Field],
}

impl<'a> Checker<'a> {
    /// What `node` is, with `names` bound, `nested` list tests deep.
    fn of(&self, node: &IdedExpr, names: &[Name<'a>], nested: usize) -> Result<Kind<'a>, String> {
        match &node.expr {
            Expr::Literal(literal) => Ok(match literal {
                LiteralValue::Boolean(_) => Kind::Bool,
                LiteralValue::Int(_) | LiteralValue::UInt(_) => Kind::Int,
                LiteralValue::Double(_) => Kind::Float,
                LiteralValue::String(_) => Kind::Text,
                LiteralValue::Null => Kind::Null,
                LiteralValue::Bytes(_) => Kind::Any,
            }),
            Expr::Ident(name) => match names.iter().rev().find(|(bound, _)| bound == name) {
                Some((_, held)) => Ok(held.clone()),
                None if name == REQUEST => Ok(Kind::Fields(self.request)),
                None => Err(format!("`{name}` isn't known: a rule reads `{REQUEST}` alone")),
            },
            Expr::Select(select) => {
                let held = self.of(&select.operand, names, nested)?;
                let found = member(&held, &select.field).map_err(|why| format!("`{}`: {why}", spelled(&select.operand)))?;
                Ok(if select.test { Kind::Bool } else { found })
            }
            Expr::Call(call) => self.call(call, names, nested),
            Expr::Comprehension(each) => self.each(each, names, nested),
            Expr::List(list) => {
                for element in &list.elements {
                    self.of(element, names, nested)?;
                }
                Ok(Kind::List(Box::new(Kind::Any)))
            }
            Expr::Map(map) => {
                for entry in &map.entries {
                    if let EntryExpr::MapEntry(entry) = &entry.expr {
                        self.of(&entry.key, names, nested)?;
                        self.of(&entry.value, names, nested)?;
                    }
                }
                Ok(Kind::Any)
            }
            Expr::Struct(_) | Expr::Unspecified => Err(format!("it builds a message; {MAY}")),
        }
    }

    fn call(&self, call: &CallExpr, names: &[Name<'a>], nested: usize) -> Result<Kind<'a>, String> {
        let target = match &call.target {
            Some(target) => Some(self.of(target, names, nested)?),
            None => None,
        };
        let mut args = vec![];
        for arg in &call.args {
            args.push(self.of(arg, names, nested)?);
        }
        let truth = |held: &Kind<'a>, sign: &str| match held {
            Kind::Bool | Kind::Any => Ok(()),
            other => Err(format!("{sign} takes true or false, and is given {}", other.said())),
        };
        let counted = |held: &Kind<'a>| matches!(held, Kind::List(_) | Kind::Map(_) | Kind::Text | Kind::Any);
        match call.func_name.as_str() {
            operators::LOGICAL_AND | operators::LOGICAL_OR | operators::LOGICAL_NOT | operators::NOT_STRICTLY_FALSE => {
                let sign = match call.func_name.as_str() {
                    operators::LOGICAL_AND => "`&&`",
                    operators::LOGICAL_OR => "`||`",
                    _ => "`!`",
                };
                for arg in &args {
                    truth(arg, sign)?;
                }
                Ok(Kind::Bool)
            }
            operators::EQUALS
            | operators::NOT_EQUALS
            | operators::LESS
            | operators::LESS_EQUALS
            | operators::GREATER
            | operators::GREATER_EQUALS
            | operators::IN => Ok(Kind::Bool),
            operators::CONDITIONAL => {
                if let Some(first) = args.first() {
                    truth(first, "`? :`")?;
                }
                Ok(match (args.get(1), args.get(2)) {
                    (Some(Kind::Bool), Some(Kind::Bool)) => Kind::Bool,
                    _ => Kind::Any,
                })
            }
            operators::INDEX | operators::OPT_INDEX => match args.first() {
                Some(Kind::List(item) | Kind::Map(item)) => Ok((**item).clone()),
                Some(Kind::Any | Kind::Optional(_)) | None => Ok(Kind::Any),
                Some(other) => Err(format!("`[ ]` reads a list or a map, and is given {}", other.said())),
            },
            operators::ADD | operators::SUBSTRACT | operators::MULTIPLY | operators::DIVIDE | operators::MODULO | operators::NEGATE => {
                Ok(match args.first() {
                    Some(Kind::Int) => Kind::Int,
                    Some(Kind::Float) => Kind::Float,
                    Some(Kind::Text) => Kind::Text,
                    _ => Kind::Any,
                })
            }
            "size" => match target.as_ref().or(args.first()) {
                Some(held) if counted(held) => Ok(Kind::Int),
                Some(Kind::Optional(inner)) if counted(&**inner) => Ok(Kind::Int),
                Some(other) => Err(format!("`size()` counts a list, a map or text, and is given {}", other.said())),
                None => Err("`size()` counts a list, a map or text".into()),
            },
            "contains" | "startsWith" | "endsWith" | "bool" => Ok(Kind::Bool),
            "int" | "uint" => Ok(Kind::Int),
            "double" => Ok(Kind::Float),
            "string" => Ok(Kind::Text),
            "min" | "max" => Ok(Kind::Any),
            other => Err(format!("`{other}` isn't something a rule may call; {MAY}")),
        }
    }

    /// A list test (`exists`, `all`, `exists_one`, `map`, `filter`), which the parser spells as a loop over its list.
    fn each(&self, each: &ComprehensionExpr, names: &[Name<'a>], nested: usize) -> Result<Kind<'a>, String> {
        if nested == MAX_NESTED {
            return Err(format!("it tests lists more than {MAX_NESTED} deep"));
        }
        let item = match self.of(&each.iter_range, names, nested)? {
            Kind::List(item) => *item,
            Kind::Map(_) => Kind::Text,
            Kind::Any | Kind::Optional(_) => Kind::Any,
            other => return Err(format!("`exists`, `all` and the like read a list, and are given {}", other.said())),
        };
        let mut inner = names.to_vec();
        match &each.iter_var2 {
            // Two names: the first is each item's place, the second the item.
            Some(second) => {
                inner.push((each.iter_var.clone(), Kind::Any));
                inner.push((second.clone(), item));
            }
            None => inner.push((each.iter_var.clone(), item)),
        }
        let kept = self.of(&each.accu_init, &inner, nested + 1)?;
        inner.push((each.accu_var.clone(), kept));
        self.of(&each.loop_cond, &inner, nested + 1)?;
        self.of(&each.loop_step, &inner, nested + 1)?;
        self.of(&each.result, &inner, nested + 1)
    }
}

/// A part of a rule as its author wrote it, where it is a name and its fields: `request.beats`. Else `it`.
fn spelled(node: &IdedExpr) -> String {
    match &node.expr {
        Expr::Ident(name) => name.clone(),
        Expr::Select(select) if !select.test => format!("{}.{}", spelled(&select.operand), select.field),
        _ => "it".into(),
    }
}
