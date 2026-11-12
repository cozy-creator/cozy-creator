//! The one binder: a box's written fields, by its tool's types, as the call its function gets. The browser's Render and `cozy run`
//! all use it. A prompt goes as its text, each mention spelled as its box is, with what that text mentions; a slot takes only what's
//! written in it; a pickup comes only from the gesture. What's past a bound is clamped and said. It knows no model. A mention its tool
//! can't use where it's written is never sent: `unusable` is the one rule, and the page strikes it through with the same reason (cl-944).
use std::collections::BTreeSet;

use api::{
    error::{Error, Kind},
    interface::{Callable, Field, Snap, Struct, Type, Wire},
    calcifer::project::{Pickup, Unused},
    calcifer::tool::{self, MEDIA, PLATFORM},
};
use serde_json::{Map, Value, json};

use super::{
    folder::{self, Files, Lock},
    format::{Doc, Standing},
    graph::{self, SELF, Span, mentions, mentions_replaced, name_key},
};
use crate::runs::clip::Range;

/// What a payload holds in place of a part of a video, until it's cut: `cut:<n>`, the `n`th of `Bound::cuts`.
pub(crate) const CUT: &str = "cut:";

/// A part of a video, cut on this computer before it's sent.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cut {
    pub source: String,
    pub range: Range,
}

/// One box's call, as it's asked for.
pub(crate) struct Call<'a> {
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
    /// A direct run's own named files, which its prompts' mentions read after the project's boxes.
    pub inline: &'a [Inline],
}

/// A file a direct run names in its own `references` (`[{name, image|video|audio}]`): `<name>` in its prompts sends it, as a
/// box's file would go.
#[derive(Clone, Debug)]
pub(crate) struct Inline {
    pub name: String,
    /// `image`, `video` or `audio`.
    pub kind: String,
    /// Its digest, `sha256:<hex>`.
    pub file: String,
}

/// What a call is.
#[derive(Debug, Default)]
pub(crate) struct Bound {
    pub payload: Map<String, Value>,
    pub warnings: Vec<String>,
    pub cuts: Vec<Cut>,
    /// The mentions its tool can't use, left out.
    pub unused: Vec<Unused>,
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
    /// What its version's file says of itself (`orbit`).
    metadata: Map<String, Value>,
    made_by: String,
    own: bool,
}

/// A box's description, as every reader gets it: descriptions are off for now (cl-900, tools §9). What's stored stays in project.db.
pub(super) const OFF: &str = "";

struct Binder<'a> {
    call: &'a Call<'a>,
    bound: Bound,
    /// Mentions that name nothing, as written.
    nothing: Vec<String>,
    /// Mentions of a video by angle, where the video isn't an orbit.
    unturned: Vec<String>,
}

/// What `call`'s text mentions that its tool can't use, where, and why: what a box shows struck through. It binds as a run would, sending nothing.
pub(crate) fn unused(call: &Call) -> Vec<Unused> {
    let mut binder = Binder { call, bound: Bound::default(), nothing: vec![], unturned: vec![] };
    // Only a pickup refuses here, and a box at rest has none.
    let _ = binder.fields();
    binder.bound.unused
}

/// `call` bound. Err: nothing sensible can be sent, said free before any machine is involved.
pub(crate) fn bind(call: &Call) -> Result<Bound, Error> {
    let mut binder = Binder { call, bound: Bound::default(), nothing: vec![], unturned: vec![] };
    binder.fields()?;
    let declared = &call.callable.request;
    if call.pickup.is_some() && !declared.fields.iter().any(|field| matches!(field.ty.plain(), Type::Pickup(_))) {
        binder.warn(format!("{}: {} picks up from no take: it renders whole", call.name, call.tool));
    }
    for written in call.fields.keys().filter(|written| !PLATFORM.contains(&written.as_str()) && declared.fields.iter().all(|field| field.name != **written)) {
        binder.warn(format!("{}: {} reads no input {written}", call.name, call.tool));
    }
    if let Some(first) = binder.nothing.first() {
        let said: Vec<String> = binder.nothing.iter().map(|mention| call.doc.not_there(mention, call.reserved, call.lock)).collect();
        let refused = Error::new(Kind::Validation, "input_missing", format!("{}: {}", call.name, said.join("; ")));
        let deleted = graph::mention(first).is_some_and(|mention| call.lock.was_deleted(mention.name, super::now_ms()));
        return Err(match call.own {
            Some(_) => refused.remedy(format!("bring it back, or edit {}'s text", call.name)),
            None if deleted => refused.remedy(format!("undo its delete in the browser opened by cozy open, or leave <{first}> out")).next("cozy open"),
            // A run's own: a box of its project, or one of its own references.
            None => refused
                .remedy(format!("make <{first}> a box of the run's project, or name it in the run: references:='[{{\"name\": \"{first}\", \"image\": \"<file>\"}}]'"))
                .next("cozy open"),
        });
    }
    if let Some(first) = binder.unturned.first() {
        let name = graph::mention(first).map_or(first.as_str(), |mention| mention.name);
        let said: Vec<String> = binder.unturned.iter().map(|key| format!("<{key}>")).collect();
        let said = format!("{}: {} names an angle, and {name} isn't an orbit", call.name, said.join(", "));
        return Err(Error::new(Kind::Validation, "mention_not_orbit", said).remedy(format!("turn on Orbit on {name}'s box, or name its frames, as <{name}:48-120>")));
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
    /// Each declared field, as it's written.
    fn fields(&mut self) -> Result<(), Error> {
        let call = self.call;
        for field in &call.callable.request.fields {
            let at = field.name.as_str();
            if let Type::Pickup(members) = field.ty.plain() {
                if call.fields.contains_key(at) {
                    self.warn(format!("{}'s {at}: a pickup isn't written; use Pick up from here", call.name));
                }
                if let Some(pickup) = call.pickup {
                    let value = self.pickup(field, members, pickup)?;
                    self.bound.payload.insert(at.to_owned(), value);
                }
                continue;
            }
            let Some(value) = call.fields.get(at) else { continue };
            if let Some(value) = self.typed(at, &field.ty, value)? {
                self.bound.payload.insert(at.to_owned(), value);
            }
        }
        Ok(())
    }

    /// Why a mention of `kind`, written where `takes` is what's taken, isn't used: None, it is. Words take a text box's words; a slot
    /// of `file` takes any file. What's said names the tool, and `here` when another of its fields would take it.
    fn unusable(&self, takes: &[&str], kind: &str) -> Option<String> {
        if takes.contains(&kind) || (takes.contains(&"file") && MEDIA.contains(&kind)) {
            return None;
        }
        let here = if declares(&self.call.callable.request, kind) { " here" } else { "" };
        let kinds = match kind {
            "image" => "images",
            "video" => "videos",
            other => other,
        };
        Some(format!("{} can't use {kinds}{here}", tool::title(self.call.package)))
    }

    /// `inner`, written at `at`, left out for `why`.
    fn left_out(&mut self, at: &str, inner: &str, why: String) {
        let unused = Unused { mention: inner.to_owned(), at: at.to_owned(), why };
        if !self.bound.unused.contains(&unused) {
            self.bound.unused.push(unused);
        }
    }

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

    /// Words: each text box is in them already, a tag stays, and any other box goes as its name, left out.
    fn words(&mut self, at: &str, text: &str) -> String {
        mentions_replaced(text, &mut |inner| {
            if self.tag(inner) {
                return None;
            }
            let mention = self.mention_of(inner)?;
            let kind = self.kind_of(&mention)?;
            if let Some(why) = self.unusable(&["text"], &kind) {
                self.left_out(at, inner, why);
                return Some(mention.name.to_owned());
            }
            None
        })
    }

    /// A prompt: its text, each mention spelled as its box spells itself, and each box it mentions, once, in the order first mentioned. What
    /// it can't use goes as its name, left out.
    fn prompt(&mut self, at: &str, kinds: &[String], text: &str) -> Value {
        let takes: Vec<&str> = kinds.iter().map(String::as_str).chain(["text"]).collect();
        let (mut references, mut keys, mut said, mut nothing) = (vec![], BTreeSet::new(), vec![], vec![]);
        let text = mentions_replaced(text, &mut |inner| {
            if self.tag(inner) {
                return None;
            }
            let mention = self.mention_of(inner)?;
            let Some(kind) = self.kind_of(&mention) else {
                nothing.push(inner.to_owned());
                return None;
            };
            if kind == "text" {
                return None;
            }
            if let Some(why) = self.unusable(&takes, &kind) {
                self.left_out(at, inner, why);
                return Some(mention.name.to_owned());
            }
            let Some(named) = self.named(&mention).flatten() else {
                said.push(format!("{}'s {at}: <{inner}> has no file here, so it goes as its name", self.call.name));
                return Some(mention.name.to_owned());
            };
            // A part that couldn't be cut is sent whole, which may be a kind this can't use.
            if let Some(why) = self.unusable(&takes, &named.kind) {
                self.left_out(at, inner, why);
                return Some(mention.name.to_owned());
            }
            if keys.insert(named.key.clone()) {
                let mut reference = json!({"key": named.key, "type": named.kind, "file": named.file, "description": OFF});
                if !named.metadata.is_empty() {
                    reference["metadata"] = Value::Object(named.metadata.clone());
                }
                references.push(reference);
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
            Some(_) => Some(graph::Mention { name: inner, version: None, part: None, span: None }),
            None => graph::mention(inner),
        }
    }

    /// Whether `mention` is a text box's: one left in by a cycle stays as written.
    fn text_box(&self, mention: &graph::Mention) -> bool {
        self.box_of(mention).is_some_and(|j| self.call.doc.assets[j].text.is_some())
    }

    /// What `mention` is, as far as it's known with nothing cut: `image`, `video`, `audio`, `text`, or an extra's name. An orbit's angle is
    /// a picture, and a box with no file yet is the kind it makes. None: it names nothing.
    fn kind_of(&self, mention: &graph::Mention) -> Option<String> {
        if self.text_box(mention) {
            return Some("text".to_owned());
        }
        let kind = match self.found(mention)? {
            Some(named) => named.kind,
            None => self.call.doc.assets[self.box_of(mention)?].kind.clone(),
        };
        Some(match mention.span {
            Some(Span::Angle(_)) if kind == "video" => "image".to_owned(),
            _ => kind,
        })
    }

    /// What `mention` sends, a part of it cut. None: it names nothing; Some(None): its box has no file.
    fn named(&mut self, mention: &graph::Mention) -> Option<Option<Named>> {
        let Some(mut named) = self.found(mention)? else { return Some(None) };
        let cut = match mention.span {
            None => return Some(Some(named)),
            Some(Span::Frames(from, to)) => self.range(&named, from, to).map(|cut| Some((cut, named.kind.clone()))),
            Some(span) => self.turned(&named, span),
        };
        match cut {
            // A cut is a file of its own: what its source's file says of itself, and a picture's version's other files, aren't its.
            Ok(Some((file, kind))) => {
                if kind != named.kind {
                    named.files.clear();
                }
                named.metadata.clear();
                (named.file, named.kind) = (file, kind);
            }
            Ok(None) => self.unturned.push(named.key.clone()),
            Err(why) => {
                self.warn(format!("{}: <{}> {why}: it's sent whole", self.call.name, named.key));
                if let Some(j) = self.box_of(mention) {
                    named.key = graph::Mention { span: None, ..*mention }.spelled(&self.call.doc.assets[j].name);
                }
            }
        }
        Some(Some(named))
    }

    /// What `mention` names, before any part of it is cut. None: it names nothing; Some(None): its box has no file.
    fn found(&self, mention: &graph::Mention) -> Option<Option<Named>> {
        let Call { doc, lock, stand, gone, .. } = self.call;
        let Some(j) = self.box_of(mention) else { return self.inline(mention).map(Some) };
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
        let version = number.and_then(|number| lock.asset(&asset.name)?.version(number));
        let made_by = version.and_then(|version| version.tool.clone()).unwrap_or_default();
        let metadata = version.map(|version| version.metadata_of(&file)).unwrap_or_default();
        Some(Some(Named { key: mention.spelled(&asset.name), kind, file, files, metadata, made_by, own: Some(j) == self.call.own }))
    }

    /// The run's own reference `mention` names, whole: no version, part or range of it.
    fn inline(&self, mention: &graph::Mention) -> Option<Named> {
        if mention.version.is_some() || mention.part.is_some() || mention.span.is_some() {
            return None;
        }
        let own = self.call.inline.iter().find(|inline| name_key(&inline.name) == name_key(mention.name))?;
        let files = Files::from([(own.kind.clone(), own.file.clone())]);
        Some(Named { key: own.name.clone(), kind: own.kind.clone(), file: own.file.clone(), files, metadata: Map::new(), made_by: String::new(), own: false })
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
        Ok(self.cut(&named.file, Range::Frames { from, to }))
    }

    /// An orbit's frame at an angle, as a picture, or its turn from one angle to another, as a clip, to be cut, with the kind it is. An angle
    /// is the frame `floor(angle / degrees × frames) mod frames`, as the page shows it, and a turn the frames shown while turning. A looping
    /// orbit's angles wrap, and its turn may cross 0°; another's are held to its turn, said. None: the video isn't an orbit.
    fn turned(&mut self, named: &Named, span: Span) -> Result<Option<(String, String)>, String> {
        /// Below a frame's rounding, so 120° of 48 frames is frame 16.
        const EVEN: f64 = 1e-9;
        if named.kind != "video" {
            return Err(format!("is {}, and only an orbit video takes an angle", a(&named.kind)));
        }
        let Some(orbit) = named.metadata.get("orbit").and_then(Value::as_object) else { return Ok(None) };
        let degrees = orbit.get("degrees").and_then(Value::as_f64).filter(|degrees| degrees.is_finite() && *degrees > 0.0).unwrap_or(360.0);
        let loops = orbit.get("loops").and_then(Value::as_bool) != Some(false);
        let measured = self.call.lock.files.get(&named.file).map_or(0, |file| match file.frames {
            0 => (file.lasts_ms() as f64 / 1000.0 * file.fps).round() as u64,
            frames => frames,
        });
        let frames = u32::try_from(measured).unwrap_or(u32::MAX);
        if frames == 0 {
            return Err("isn't measured yet, so no angle of it can be found".into());
        }
        let at = |angle: f64| angle * f64::from(frames) / degrees;
        let first = |angle: f64| (at(angle) + EVEN).floor() as u32;
        let end = |angle: f64| (at(angle) - EVEN).ceil() as u32;
        let range = match span {
            Span::Frames(..) => unreachable!("frames are a range"),
            Span::Angle(angle) if loops => Range::Frame(first(angle.rem_euclid(degrees)) % frames),
            Span::Angle(angle) => {
                if angle > degrees {
                    self.warn(format!("{}: <{}> is past its {degrees}° turn: its last frame", self.call.name, named.key));
                }
                Range::Frame(first(angle.min(degrees)).min(frames - 1))
            }
            Span::Turn(from, to) if loops => {
                let turn = if to - from >= degrees { degrees } else { (to - from).rem_euclid(degrees) };
                if turn <= 0.0 {
                    return Err(format!("names no part of its {degrees}° turn"));
                }
                let start = from.rem_euclid(degrees);
                let from = first(start) % frames;
                match end(start + turn).clamp(from + 1, from + frames) {
                    to if to <= frames => Range::Frames { from, to: (to < frames).then_some(to) },
                    to => Range::Wrapped { from, to: to - frames },
                }
            }
            Span::Turn(from, to) => {
                if to > degrees {
                    self.warn(format!("{}: <{}> runs past its {degrees}° turn: cut to its end", self.call.name, named.key));
                }
                let (start, stop) = (from.min(degrees), to.min(degrees));
                if stop <= start {
                    return Err(format!("names no part of its {degrees}° turn, which doesn't loop past 0°"));
                }
                let from = first(start).min(frames - 1);
                let to = end(stop).clamp(from + 1, frames);
                Range::Frames { from, to: (to < frames).then_some(to) }
            }
        };
        let kind = if matches!(range, Range::Frame(_)) { "image" } else { "video" };
        Ok(Some((self.cut(&named.file, range), kind.to_owned())))
    }

    /// `range` of `source`, cut once however often it's named: what the payload holds in its place.
    fn cut(&mut self, source: &str, range: Range) -> String {
        let cut = Cut { source: source.to_owned(), range };
        let n = self.bound.cuts.iter().position(|held| *held == cut).unwrap_or_else(|| {
            self.bound.cuts.push(cut);
            self.bound.cuts.len() - 1
        });
        format!("{CUT}{n}")
    }

    /// The files a slot takes from what's written in it: each mention, else a hash of the project's. A mention of the wrong kind is left
    /// out, as a prompt's is; a hash, with a warning (decision 8).
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
                    [file] => found.push(Named { key: String::new(), kind: file.media().unwrap_or(kind).to_owned(), file: file.sha256.clone(), files: Files::new(), metadata: Map::new(), made_by: String::new(), own: false }),
                    _ => found.push(Named { key: String::new(), kind: kind.to_owned(), file: text.to_owned(), files: Files::new(), metadata: Map::new(), made_by: String::new(), own: false }),
                }
                continue;
            }
            for inner in said {
                let Some(mention) = self.mention_of(inner) else { continue };
                let Some(held) = self.kind_of(&mention) else {
                    self.nothing.push(inner.to_owned());
                    continue;
                };
                if let Some(why) = self.unusable(&[kind], &held) {
                    self.left_out(at, inner, why);
                    continue;
                }
                match self.named(&mention).flatten() {
                    None => self.warn(format!("{}'s {at}: <{inner}> has no file here, so it's left empty", self.call.name)),
                    Some(named) if kind != "file" && named.kind != kind => {
                        let why = self.unusable(&[kind], &named.kind).unwrap_or_default();
                        self.left_out(at, inner, why);
                    }
                    Some(named) => found.push(named),
                }
            }
        }
        if !many && found.len() > 1 {
            self.warn(format!("{}'s {at} takes one file, and {} are written: it takes the first", self.call.name, found.len()));
            found.truncate(1);
        }
        Ok(found)
    }

    /// A file, and the extras its struct names: what its version holds by that name, when this package made it.
    fn media(&self, named: &Named, extras: &Struct) -> Value {
        let mut out = Map::from_iter([("type".to_owned(), json!(named.kind)), ("file".to_owned(), json!(named.file)), ("description".to_owned(), json!(OFF))]);
        if !named.metadata.is_empty() {
            out.insert("metadata".to_owned(), Value::Object(named.metadata.clone()));
        }
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

/// Whether any field of `request` takes a file of `kind`, at any depth.
fn declares(request: &Struct, kind: &str) -> bool {
    request.fields.iter().any(|field| takes(&field.ty, kind))
}

fn takes(ty: &Type, kind: &str) -> bool {
    match ty {
        Type::Prompt(kinds) => kinds.iter().any(|held| held == kind),
        Type::Asset(held) | Type::Media { kind: held, .. } => held == kind || (held == "file" && MEDIA.contains(&kind)),
        Type::Pickup(_) => kind == "video",
        Type::List(item) | Type::Map(item) => takes(item, kind),
        Type::Struct(members) => declares(members, kind),
        Type::Union { branches, .. } => branches.iter().any(|branch| takes(branch, kind)),
        _ => false,
    }
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
pub(crate) fn made_here(made_by: &str, package: &str) -> bool {
    let of = |path: &str| path.rsplit('/').next().unwrap_or(path).to_owned();
    let function = made_by.split('@').next().unwrap_or(made_by);
    function.rsplit_once('/').is_some_and(|(made, _)| of(made) == of(package))
}

/// `frame` on the frames `snap` allows: the nearest, and none before its first.
pub(crate) fn snapped(frame: u32, snap: Snap) -> u32 {
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
pub(crate) fn cuts_filled(value: &mut Value, files: &[String]) {
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
