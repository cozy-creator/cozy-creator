//! The one binder: a box's written fields, by its tool's types, as the call its function gets. Render, `cozy project render`, `cozy run` and
//! Caption all use it. A prompt goes as its text, each mention spelled as its box is, with what that text mentions; a slot takes only what's
//! written in it; a pickup comes only from the gesture. What's past a bound is clamped and said. It knows no model.
use std::collections::BTreeSet;

use api::{
    error::{Error, Kind},
    interface::{Callable, Field, Snap, Struct, Type, Wire},
    calcifer::project::Pickup,
    calcifer::tool::{MEDIA, PLATFORM},
};
use serde_json::{Map, Value, json};

use super::{
    folder::{self, Files, Lock},
    format::{Doc, Standing},
    graph::{self, SELF, mentions, mentions_replaced, name_key},
};

/// What a payload holds in place of a range of a video, until it's cut: `cut:<n>`, the `n`th of `Bound::cuts`.
pub const CUT: &str = "cut:";

/// A range of a video, cut on this computer before it's sent.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    pub source: String,
    pub from: u32,
    pub to: Option<u32>,
}

/// One box's call, as it's asked for.
pub struct Call<'a> {
    /// Who asks: the box, or `cozy run`'s run.
    pub name: &'a str,
    /// Its tool as written, for what's said.
    pub tool: &'a str,
    /// Its package, whose own extras it reads.
    pub package: &'a str,
    pub callable: &'a Callable,
    /// Its written fields, each text box put in as its words.
    pub fields: &'a Map<String, Value>,
    /// The model's own tags, which stay text.
    pub reserved: &'a [String],
    pub doc: &'a Doc,
    /// The box itself: what `<self.1>` and a take's `own` mean.
    pub own: Option<usize>,
    pub lock: &'a Lock,
    pub stand: &'a [Standing],
    /// Files kept by a machine that's gone: nothing reads them.
    pub gone: &'a BTreeSet<String>,
    /// Pick up from here, the one way a pickup is filled.
    pub pickup: Option<&'a Pickup>,
}

/// What a call is.
#[derive(Debug, Default)]
pub struct Bound {
    pub payload: Map<String, Value>,
    pub warnings: Vec<String>,
    pub cuts: Vec<Cut>,
}

/// A box a mention names, as a function is sent it.
struct Named {
    /// The mention as its box spells it: `Amy.2`, `walk:48-120`.
    key: String,
    /// What its file is: `image`, `video`, `audio`, or another role's name.
    kind: String,
    /// Its file's hash, or `cut:<n>` for a range.
    file: String,
    files: Files,
    description: String,
    made_by: String,
    own: bool,
}

struct Binder<'a> {
    call: &'a Call<'a>,
    bound: Bound,
    /// Mentions that name nothing, as written.
    nothing: Vec<String>,
}

/// `call` bound. Err: nothing sensible can be sent, said free before any machine is involved.
pub fn bind(call: &Call) -> Result<Bound, Error> {
    let mut binder = Binder { call, bound: Bound::default(), nothing: vec![] };
    let declared = &call.callable.request;
    for field in &declared.fields {
        let at = field.name.as_str();
        if let Type::Pickup(members) = field.ty.plain() {
            if call.fields.contains_key(at) {
                binder.warn(format!("{}'s {at}: a pickup isn't written; use Pick up from here", call.name));
            }
            if let Some(pickup) = call.pickup {
                let value = binder.pickup(field, members, pickup)?;
                binder.bound.payload.insert(at.to_owned(), value);
            }
            continue;
        }
        let Some(value) = call.fields.get(at) else { continue };
        if let Some(value) = binder.typed(at, &field.ty, value)? {
            binder.bound.payload.insert(at.to_owned(), value);
        }
    }
    if call.pickup.is_some() && !declared.fields.iter().any(|field| matches!(field.ty.plain(), Type::Pickup(_))) {
        binder.warn(format!("{}: {} picks up from no take: it renders whole", call.name, call.tool));
    }
    for written in call.fields.keys().filter(|written| !PLATFORM.contains(&written.as_str()) && declared.fields.iter().all(|field| field.name != **written)) {
        binder.warn(format!("{}: {} reads no input {written}", call.name, call.tool));
    }
    if !binder.nothing.is_empty() {
        let said: Vec<String> = binder.nothing.iter().map(|mention| call.doc.not_there(mention, call.reserved)).collect();
        return Err(Error::new(Kind::Validation, "input_missing", format!("{}: {}", call.name, said.join("; "))).remedy(format!("bring it back, or edit {}'s text", call.name)));
    }
    let mut payload = std::mem::take(&mut binder.bound.payload);
    binder.rows(declared, &mut payload, call.name);
    binder.bound.payload = payload;
    for field in declared.fields.iter().filter(|field| field.wire == Wire::Required && !binder.bound.payload.contains_key(&field.name)) {
        if matches!(field.ty.plain(), Type::Unknown) {
            let said = format!("{}: {} needs {}, of a type this cozy can't fill", call.name, call.tool, field.name);
            return Err(Error::new(Kind::Unavailable, "tool_input_unknown", said).next("cozy update"));
        }
        if !matches!(field.ty.plain(), Type::Model | Type::Tree) {
            let said = format!("{} can't run: {} needs its {}", call.name, call.tool, field.name);
            return Err(Error::new(Kind::Usage, "tool_input_missing", said).remedy(format!("write {}'s {}", call.name, field.name)));
        }
    }
    binder.bound.warnings.dedup();
    Ok(binder.bound)
}

impl Binder<'_> {
    fn warn(&mut self, said: String) {
        if !self.bound.warnings.contains(&said) {
            self.bound.warnings.push(said);
        }
    }

    fn tag(&self, inner: &str) -> bool {
        self.call.reserved.iter().any(|tag| name_key(tag) == name_key(inner))
    }

    /// `value`, written at `at`, as `ty` takes it. None: it's empty, and absent is its default.
    fn typed(&mut self, at: &str, ty: &Type, value: &Value) -> Result<Option<Value>, Error> {
        if value.is_null() {
            return Ok(None);
        }
        Ok(Some(match (ty.plain(), value) {
            (Type::Str, Value::String(text)) => json!(self.words(at, text)),
            (Type::Prompt(kinds), Value::String(text)) => self.prompt(at, kinds, text),
            (Type::Prompt(_), Value::Object(_)) => value.clone(),
            (Type::Asset(kind), _) if is_file(kind) => match self.slot(at, kind, value, false)?.into_iter().next() {
                Some(named) => json!(named.file),
                None => return Ok(None),
            },
            (Type::Media { kind, extras }, _) => match self.slot(at, kind, value, false)?.into_iter().next() {
                Some(named) => self.media(&named, extras),
                None => return Ok(None),
            },
            (Type::List(item), _) if matches!(item.plain(), Type::Asset(kind) if is_file(kind)) => {
                let Type::Asset(kind) = item.plain() else { unreachable!() };
                Value::Array(self.slot(at, kind, value, true)?.into_iter().map(|named| json!(named.file)).collect())
            }
            (Type::List(item), _) if matches!(item.plain(), Type::Media { .. }) => {
                let Type::Media { kind, extras } = item.plain() else { unreachable!() };
                Value::Array(self.slot(at, kind, value, true)?.iter().map(|named| self.media(named, extras)).collect())
            }
            (Type::List(item), Value::Array(rows)) => {
                let mut out = vec![];
                for (n, row) in rows.iter().enumerate() {
                    out.extend(self.typed(&format!("{at}.{n}"), item, row)?);
                }
                Value::Array(out)
            }
            (Type::Struct(members), Value::Object(written)) => Value::Object(self.members(at, members, written)?),
            (Type::Shape, Value::String(ratio)) => json!(ratio.trim()),
            _ => value.clone(),
        }))
    }

    fn members(&mut self, at: &str, members: &Struct, written: &Map<String, Value>) -> Result<Map<String, Value>, Error> {
        let mut out = Map::new();
        for (key, value) in written {
            match members.fields.iter().find(|field| field.name == *key) {
                Some(field) => {
                    if let Some(value) = self.typed(&format!("{at}.{key}"), &field.ty, value)? {
                        out.insert(key.clone(), value);
                    }
                }
                None => self.warn(format!("{}'s {at}: {} reads no {key}", self.call.name, self.call.tool)),
            }
        }
        Ok(out)
    }

    /// Words: each text box is in them already, a tag stays, and a box with a file goes as its name.
    fn words(&mut self, at: &str, text: &str) -> String {
        let mut said = vec![];
        let out = mentions_replaced(text, &mut |inner| {
            if self.tag(inner) {
                return None;
            }
            let mention = self.mention_of(inner)?;
            if self.text_box(&mention) {
                return None;
            }
            self.box_of(&mention)?;
            said.push(inner.to_owned());
            Some(mention.name.to_owned())
        });
        for inner in said {
            self.warn(format!("{}'s {at} takes words, so <{inner}> goes as its name", self.call.name));
        }
        out
    }

    /// A prompt: its text, each mention spelled as its box spells itself, and each box it mentions, once, in the order first mentioned. What
    /// it doesn't take goes as its name.
    fn prompt(&mut self, at: &str, kinds: &[String], text: &str) -> Value {
        let (mut references, mut keys, mut said, mut nothing) = (vec![], BTreeSet::new(), vec![], vec![]);
        let text = mentions_replaced(text, &mut |inner| {
            if self.tag(inner) {
                return None;
            }
            let mention = self.mention_of(inner)?;
            if self.text_box(&mention) {
                return None;
            }
            let Some(named) = self.named(&mention) else {
                nothing.push(inner.to_owned());
                return None;
            };
            let Some(named) = named else {
                said.push(format!("{}'s {at}: <{inner}> has no file here, so it goes as its name", self.call.name));
                return Some(mention.name.to_owned());
            };
            if !kinds.contains(&named.kind) {
                said.push(format!("{}'s {at} takes {}, so <{inner}>, {}, goes as its name", self.call.name, kinds.join(" or "), a(&named.kind)));
                return Some(mention.name.to_owned());
            }
            if keys.insert(named.key.clone()) {
                references.push(json!({"key": named.key, "type": named.kind, "file": named.file, "description": named.description}));
            }
            Some(format!("<{}>", named.key))
        });
        said.into_iter().for_each(|line| self.warn(line));
        self.nothing.extend(nothing);
        json!({"text": text, "references": references})
    }

    /// The box `mention` names: None, nothing.
    fn box_of(&self, mention: &graph::Mention) -> Option<usize> {
        match name_key(mention.name) == SELF {
            true => self.call.own.filter(|_| mention.version.is_some()),
            false => self.call.doc.find(mention.name).or_else(|| self.call.doc.dotted_box(mention.name)),
        }
    }

    /// `inner` as a mention: a box written with a dotted name, whole, else by the grammar.
    fn mention_of<'m>(&self, inner: &'m str) -> Option<graph::Mention<'m>> {
        match self.call.doc.dotted_box(inner) {
            Some(_) => Some(graph::Mention { name: inner, version: None, part: None, range: None }),
            None => graph::mention(inner),
        }
    }

    /// Whether `mention` is a text box's: one left in by a cycle stays as written.
    fn text_box(&self, mention: &graph::Mention) -> bool {
        self.box_of(mention).is_some_and(|j| self.call.doc.assets[j].text.is_some())
    }

    /// What `mention` sends. None: it names nothing; Some(None): its box has no file.
    fn named(&mut self, mention: &graph::Mention) -> Option<Option<Named>> {
        let Call { doc, lock, stand, gone, .. } = self.call;
        let j = self.box_of(mention)?;
        let asset = &doc.assets[j];
        let (files, number) = match mention.version {
            Some(number) => (doc.version_files(j, number, lock)?, Some(number)),
            None => match stand[j].files.as_ref().filter(|files| !files.is_empty()) {
                Some(files) => (files.clone(), stand[j].version()),
                None => match lock.asset(&asset.name).and_then(|held| held.versions.last()) {
                    Some(newest) => (newest.files.clone(), Some(newest.number)),
                    None => return Some(None),
                },
            },
        };
        let files: Files = files.into_iter().filter(|(_, sha256)| !gone.contains(sha256)).collect();
        // All it has stays on a machine that's gone: the function does without it, as with a box that has no version.
        if files.is_empty() {
            return Some(None);
        }
        let (kind, file) = match mention.part {
            Some(part) => (part.to_owned(), files.get(part)?.clone()),
            None => {
                let main = folder::main_of(&files)?.clone();
                let role = files.iter().find(|(_, sha256)| **sha256 == main).map(|(role, _)| role.clone()).unwrap_or_default();
                let kind = MEDIA.iter().find(|media| **media == role).map(|media| (*media).to_owned());
                (kind.or_else(|| lock.files.get(&main).and_then(folder::File::media).map(str::to_owned)).unwrap_or(role), main)
            }
        };
        let made_by = number.and_then(|number| lock.asset(&asset.name)?.version(number)?.tool.clone()).unwrap_or_default();
        let mut named = Named { key: mention.spelled(&asset.name), kind, file, files, description: asset.description.clone(), made_by, own: Some(j) == self.call.own };
        if let Some((from, to)) = mention.range {
            match self.range(&named, from, to) {
                Ok(cut) => named.file = cut,
                Err(why) => {
                    self.warn(format!("{}: <{}> {why}: it's sent whole", self.call.name, named.key));
                    named.key = graph::Mention { range: None, ..*mention }.spelled(&asset.name);
                }
            }
        }
        Some(Some(named))
    }

    /// Frames `from` to `to` of a video, to be cut: past its end, to its end.
    fn range(&mut self, named: &Named, from: u32, to: Option<u32>) -> Result<String, String> {
        if named.kind != "video" {
            return Err(format!("is {}, and only a video takes a range", a(&named.kind)));
        }
        let frames = self.call.lock.files.get(&named.file).map_or(0, |file| file.frames as u32);
        let to = match (to, frames) {
            (Some(to), frames) if frames > 0 && to > frames => {
                self.warn(format!("{}: <{}> runs past its {frames} frames: cut to its end", self.call.name, named.key));
                None
            }
            (to, _) => to,
        };
        if to.is_some_and(|to| to <= from) || (frames > 0 && from >= frames) {
            return Err(format!("names no frames of its {frames}"));
        }
        let cut = Cut { source: named.file.clone(), from, to };
        let n = self.bound.cuts.iter().position(|held| *held == cut).unwrap_or_else(|| {
            self.bound.cuts.push(cut);
            self.bound.cuts.len() - 1
        });
        Ok(format!("{CUT}{n}"))
    }

    /// The files a slot takes from what's written in it: each mention, else a hash of the project's. One of the wrong kind is left out,
    /// with a warning (decision 8).
    fn slot(&mut self, at: &str, kind: &str, value: &Value, many: bool) -> Result<Vec<Named>, Error> {
        let written: Vec<&str> = match value {
            Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
            Value::String(text) => vec![text.as_str()],
            _ => vec![],
        };
        let mut found = vec![];
        for text in written {
            let said = mentions(text);
            if said.is_empty() {
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                match self.call.lock.matching(&text.trim_start_matches("sha256:").to_ascii_lowercase()).as_slice() {
                    [file] if kind != "file" && file.media().is_some_and(|media| media != kind) => {
                        self.warn(format!("{}'s {at} takes {}, and {text} is {}: it's left empty", self.call.name, a(kind), a(file.media().unwrap_or_default())));
                    }
                    [file] => found.push(Named { key: String::new(), kind: file.media().unwrap_or(kind).to_owned(), file: file.sha256.clone(), files: Files::new(), description: String::new(), made_by: String::new(), own: false }),
                    _ => found.push(Named { key: String::new(), kind: kind.to_owned(), file: text.to_owned(), files: Files::new(), description: String::new(), made_by: String::new(), own: false }),
                }
                continue;
            }
            for inner in said {
                let Some(mention) = self.mention_of(inner) else { continue };
                match self.named(&mention) {
                    None => self.nothing.push(inner.to_owned()),
                    Some(None) => self.warn(format!("{}'s {at}: <{inner}> has no file here, so it's left empty", self.call.name)),
                    Some(Some(named)) if kind != "file" && named.kind != kind => {
                        self.warn(format!("{}'s {at} takes {}, and <{inner}> is {}: it's left empty", self.call.name, a(kind), a(&named.kind)));
                    }
                    Some(Some(named)) => found.push(named),
                }
            }
        }
        if !many && found.len() > 1 {
            self.warn(format!("{}'s {at} takes one file, and {} are written: it takes the first", self.call.name, found.len()));
            found.truncate(1);
        }
        Ok(found)
    }

    /// A file with its description, and the extras its struct names: what its version holds by that name, when this package made it.
    fn media(&self, named: &Named, extras: &Struct) -> Value {
        let mut out = Map::from_iter([("type".to_owned(), json!(named.kind)), ("file".to_owned(), json!(named.file)), ("description".to_owned(), json!(named.description))]);
        let ours = made_here(&named.made_by, self.call.package);
        for extra in &extras.fields {
            let value = match extra.name.as_str() {
                "own" => json!(named.own),
                role => named.files.get(role).filter(|_| ours).map_or(Value::Null, |sha256| json!(sha256)),
            };
            out.insert(extra.name.clone(), value);
        }
        Value::Object(out)
    }

    /// Pick up from here: the take, as the pickup's `take` takes it, and the frame it's picked up at, on the frames it may land on.
    fn pickup(&mut self, field: &Field, members: &Struct, pickup: &Pickup) -> Result<Value, Error> {
        let name = self.call.name;
        let take = match graph::mentions(&pickup.take).first().and_then(|inner| graph::mention(inner)) {
            Some(mention) => self.named(&mention),
            None => None,
        };
        let Some(Some(take)) = take.filter(|take| take.as_ref().is_some_and(|take| take.kind == "video")) else {
            let said = format!("{name}: {} is no take to pick up from", pickup.take);
            return Err(Error::new(Kind::Validation, "pickup_no_take", said).remedy("pick up from one of a video box's takes, as <shot_1.3>"));
        };
        let snap: Snap = field.extra.get("snap").and_then(|snap| serde_json::from_value(snap.clone()).ok()).unwrap_or_default();
        let out_frame = snapped(pickup.out_frame, snap);
        if out_frame != pickup.out_frame {
            self.warn(format!("{name}: frame {} isn't one a pickup lands on: it picks up at {out_frame}", pickup.out_frame));
        }
        let mut out = Map::new();
        for member in &members.fields {
            match (member.name.as_str(), member.ty.plain()) {
                ("take", Type::Media { extras, .. }) => drop(out.insert("take".into(), self.media(&take, extras))),
                ("take", _) => drop(out.insert("take".into(), json!(take.file))),
                ("out_frame", _) => drop(out.insert("out_frame".into(), json!(out_frame))),
                ("beats_from", _) => drop(out.insert("beats_from".into(), pickup.beats_from.map_or(Value::Null, |seconds| json!(seconds)))),
                _ => {}
            }
        }
        Ok(Value::Object(out))
    }

    /// Each list held to its count and each row to its own bounds, and each number, choice and shape to what it takes, with a line for each
    /// that moved.
    fn rows(&mut self, declared: &Struct, fields: &mut Map<String, Value>, of: &str) {
        for field in &declared.fields {
            let key = field.name.as_str();
            let Some(value) = fields.get_mut(key) else {
                // A list of rows a box opens with: as many as it takes, each its defaults.
                if let (Type::List(item), Some(least)) = (field.ty.plain(), field.constraints.min_length)
                    && matches!(item.plain(), Type::Struct(_))
                    && field.wire == Wire::Required
                {
                    fields.insert(key.to_owned(), Value::Array(vec![json!({}); least as usize]));
                }
                continue;
            };
            match field.ty.plain() {
                Type::Int | Type::Float if value.is_number() => {
                    let (low, high) = limits(field);
                    let at = value.as_f64().unwrap_or_default();
                    let held = at.clamp(low.unwrap_or(f64::NEG_INFINITY), high.unwrap_or(f64::INFINITY));
                    if held != at {
                        self.warn(format!("{of}'s {key}: {at} is past what it takes, so it's {held}"));
                        *value = if *field.ty.plain() == Type::Int { json!(held as i64) } else { json!(held) };
                    }
                }
                Type::Literal(members) if !value.is_null() && !members.contains(value) => {
                    let offered: Vec<String> = members.iter().filter(|member| !member.is_null()).map(|member| member.as_str().map_or_else(|| member.to_string(), str::to_owned)).collect();
                    self.warn(format!("{of}'s {key}: {value} isn't one of {}, so it's its default", offered.join(", ")));
                    fields.remove(key);
                }
                Type::Shape => {
                    if let Some(held) = value.as_str().and_then(|ratio| shape(field, ratio))
                        && Some(held.as_str()) != value.as_str()
                    {
                        self.warn(format!("{of}'s {key}: {value} isn't a shape it takes, so it's {held}"));
                        *value = json!(held);
                    }
                }
                Type::List(item) => {
                    let Value::Array(rows) = value else { continue };
                    if let Some(most) = field.constraints.max_length.filter(|most| rows.len() as u64 > *most) {
                        self.warn(format!("{of}'s {key}: {} given and it takes {most}, so it uses the first {most}", rows.len()));
                        rows.truncate(most as usize);
                    }
                    if let Some(least) = field.constraints.min_length.filter(|least| (rows.len() as u64) < *least) {
                        rows.resize_with(least as usize, || json!({}));
                    }
                    if let Type::Struct(row) = item.plain() {
                        for (n, one) in rows.iter_mut().enumerate() {
                            if let Value::Object(one) = one {
                                self.rows(row, one, &format!("{of}'s {key} {}", n + 1));
                            }
                        }
                    }
                }
                Type::Struct(members) => {
                    if let Value::Object(one) = value {
                        self.rows(members, one, &format!("{of}'s {key}"));
                    }
                }
                _ => {}
            }
        }
    }
}

/// Whether `kind` is a file a slot takes.
fn is_file(kind: &str) -> bool {
    MEDIA.contains(&kind) || kind == "file"
}

/// `a picture`, `a video`, `a sound`.
fn a(kind: &str) -> String {
    match kind {
        "image" => "a picture".into(),
        "video" => "a video".into(),
        "audio" => "a sound".into(),
        other => format!("its {other}"),
    }
}

/// Whether `made_by`, `org/package/function@release`, is of `package`: a package reads only its own extras.
pub fn made_here(made_by: &str, package: &str) -> bool {
    let of = |path: &str| path.rsplit('/').next().unwrap_or(path).to_owned();
    let function = made_by.split('@').next().unwrap_or(made_by);
    function.rsplit_once('/').is_some_and(|(made, _)| of(made) == of(package))
}

/// `frame` on the frames `snap` allows: the nearest, and none before its first.
pub fn snapped(frame: u32, snap: Snap) -> u32 {
    let every = snap.every.max(1);
    let base = snap.offset % every;
    let first = snap.first.max(snap.offset);
    let lowest = first + (every + base - first % every) % every;
    if frame <= lowest {
        return lowest;
    }
    let below = frame - (frame + every - base) % every;
    let above = below + every;
    if frame - below <= above - frame { below.max(lowest) } else { above }
}

/// A number's ends: what it declares, else what a box is told.
fn limits(field: &Field) -> (Option<f64>, Option<f64>) {
    let extra = |key: &str| field.extra.get(key).and_then(Value::as_f64);
    let c = &field.constraints;
    (c.ge.or(c.gt).or_else(|| extra("min")), c.le.or(c.lt).or_else(|| extra("max")))
}

/// `ratio` held to what a shape takes: the nearest allowed, else within its ends. None: it's no ratio, and goes as written.
fn shape(field: &Field, ratio: &str) -> Option<String> {
    let value = |text: &str| -> Option<f64> {
        let (w, h) = text.split_once(':')?;
        let (w, h) = (w.trim().parse::<f64>().ok()?, h.trim().parse::<f64>().ok()?);
        (w > 0.0 && h > 0.0).then_some(w / h)
    };
    let at = value(ratio)?;
    let ratios = field.extra.get("ratios");
    let allowed: Vec<&str> = ratios.and_then(|ratios| ratios["allowed"].as_array()).into_iter().flatten().filter_map(Value::as_str).collect();
    if !allowed.is_empty() {
        let near = allowed.iter().filter_map(|text| Some((*text, value(text)?))).min_by(|a, b| (a.1.ln() - at.ln()).abs().total_cmp(&(b.1.ln() - at.ln()).abs()));
        return near.map(|(text, _)| text.to_owned());
    }
    let (low, high) = (ratios.and_then(|ratios| ratios["min"].as_f64()), ratios.and_then(|ratios| ratios["max"].as_f64()));
    let held = at.clamp(low.unwrap_or(0.0), high.unwrap_or(f64::INFINITY));
    Some(match held == at {
        true => ratio.trim().to_owned(),
        false if held >= 1.0 => format!("{}:1", trimmed(held)),
        false => format!("1:{}", trimmed(1.0 / held)),
    })
}

fn trimmed(number: f64) -> String {
    let text = format!("{number:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Each `cut:<n>` in `payload` as the `n`th of `files`, the cuts' hashes.
pub fn cuts_filled(value: &mut Value, files: &[String]) {
    match value {
        Value::String(text) => {
            if let Some(file) = text.strip_prefix(CUT).and_then(|n| n.parse::<usize>().ok()).and_then(|n| files.get(n)) {
                *text = file.clone();
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| cuts_filled(item, files)),
        Value::Object(fields) => fields.values_mut().for_each(|item| cuts_filled(item, files)),
        _ => {}
    }
}
