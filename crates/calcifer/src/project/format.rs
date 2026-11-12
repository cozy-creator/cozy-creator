//! The project's boxes as Calcifer reads them (design/tools.md): each box a name, the tool that makes it, its fields, the file you approved,
//! and words for whatever mentions it. The platform reads `tool`, `approved`, `description`, `models`, `lora` and `<name>` in any text, and
//! knows no field's meaning, but for the reserved package `cozy`, which it runs itself. Every other key is a field: the binder gives it to
//! the tool by its type, and it counts in the key. What one box gets wrong is said, and that box does the best it can.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use api::{
    error::{Error, Kind},
    calcifer::project::{Read, Timeline, Track, Warning},
    calcifer::tool::EDIT,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    written::Written,
    folder::{self, Files, Folder, Lock, Version},
    graph::{self, Graph, SELF, as_name, can_take, edits, is_name, mentions, mentions_in, mentions_replaced, name_key, version_of},
};

/// The name used to render every asset.
pub(crate) const ALL: &str = "all";
/// What the platform reads of an asset. Any other key is an input.
const PLATFORM: [&str; 8] = ["tool", "approved", "description", "models", "lora", NODE_KIND, SELECTED_VERSION, STARRED];
/// What goes on a function's first call, beside its input.
const MADE: [&str; 2] = ["models", "lora"];
pub(super) const KINDS: [&str; 3] = ["image", "video", "audio"];
/// What a text asset holds, and what it is.
pub(super) const TEXT: &str = "text";
/// Persistent container identity; this is project metadata, never a tool input.
pub(super) const NODE_KIND: &str = "node_kind";
/// The exact current media version, independently of whether two versions contain identical bytes.
pub(super) const SELECTED_VERSION: &str = "selected_version";
/// A person's star on an asset, to find it again (cl-892): no input, so it puts nothing out of date.
pub(super) const STARRED: &str = "starred";

pub(super) fn node_kind(value: Option<&Value>) -> Result<Option<&str>, Error> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(kind)) if kind == "video_edit" => Ok(Some("video")),
        Some(Value::String(kind)) if matches!(kind.as_str(), "text" | "image" | "video" | "audio") => Ok(Some(kind)),
        _ => Err(invalid("node_kind", "node_kind must be text, image, video or audio".into())),
    }
}

pub(super) fn selected_version(value: Option<&Value>) -> Result<Option<u32>, Error> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_u64().and_then(|number| u32::try_from(number).ok()).filter(|number| *number > 0)
            .map(Some).ok_or_else(|| invalid(SELECTED_VERSION, "selected_version must be a positive version number".into())),
    }
}

pub(super) fn node_media(kind: &str) -> &str {
    if kind == "video_edit" { "video" } else { kind }
}
/// The reserved package Calcifer runs itself.
const COZY: &str = "cozy";
const FRAME: &str = "cozy/frame";
/// A box's words for its tool.
pub(super) const INSTRUCTIONS: &str = "instructions";
/// What opens an asset's words.
pub(super) const LEADS: [&str; 2] = ["summary", INSTRUCTIONS];
/// What `cozy` reads of an edit or a frame.
const TOOL_INPUTS: [&str; 6] = ["tracks", "fps", "aspect_ratio", "resolution", "video", "at"];

/// Whether `tool` names what Calcifer makes itself: a function of the package `cozy`.
pub(crate) fn made_here(tool: &str) -> bool {
    let tool = tool.trim();
    let path = api::target::written_as_path(tool) || tool.ends_with(".py");
    !path && tool.split(['@', '/']).find(|part| *part != "_") == Some(COZY)
}

/// What `plain` makes, if Calcifer makes it: `cozy/edit` a video, `cozy/frame` a picture.
pub(super) fn kind_made_here(plain: &str) -> Option<&'static str> {
    match plain {
        EDIT => Some("video"),
        FRAME => Some("image"),
        _ => None,
    }
}

/// A new project's boxes: a short film that renders as it stands.
pub(crate) fn starter() -> Written {
    const SHEET: &str = "The whole body from the front on the left, with nothing added to the body described and no ground under it. On the right, the same character has been spun a half turn, at the same size, so its back faces the viewer and nothing on its front can be seen. Two separate views, with no text.";
    let boxed = |name: &str, fields: Vec<(&str, Value)>| super::written::Boxed { name: name.to_owned(), fields: fields.into_iter().map(|(field, value)| (field.to_owned(), value)).collect(), ..Default::default() };
    let still = |name: &str, description: &str, instructions: String| boxed(name, vec![
        ("tool", json!("qwen-image-2/image")), ("aspect_ratio", json!("16:9")), ("instructions", json!(instructions)), ("description", json!(description)),
    ]);
    let character = |name: &str, description: &str| still(name, description, format!("<style> {description} {SHEET}"));
    let shot = |name: &str, summary: &str, soundscape: &str, beats: Value| boxed(name, vec![
        ("tool", json!("minimax-h3/generate")), ("aspect_ratio", json!("16:9")), ("summary", json!(format!("<style>\n{summary}"))), ("soundscape", json!(soundscape)), ("beats", beats),
    ]);
    let coast = "A volcanic coast under a storm sky, with a dark sea on the left, lava cliffs on the right, and a small white crater city far away at the center.";
    let boxes = vec![
        boxed("style", vec![("text", json!("High-quality Unreal Engine graphics in a bright, hand-painted, cel-shaded anime style."))]),
        character("kyogre", "Kyogre, the colossal blue whale-like legendary Pokémon with huge fins, no legs, and glowing red lines."),
        character("groudon", "Groudon, the colossal red legendary Pokémon with grey stone plates and huge claws."),
        still("coast", coast, format!("<style> {coast}")),
        boxed("edit", vec![("tool", json!("cozy/edit")), ("fps", json!(24)), ("tracks", json!([{"clips": ["<awaken>", "<erupt>", "<clash>"]}]))]),
        shot("awaken", "<kyogre> bursts out of the stormy sea of <coast>.", "Howling wind, crashing waves, and rolling thunder.", json!([
            {"duration": 6, "description": "A wide shot from the water's edge looks out over the stormy sea of <coast>, with lightning flickering behind the clouds. The water bulges at the center of the frame, and <kyogre> bursts out of it, facing right, with sheets of white water pouring off its fins. The camera tilts up at slow speed as heavy rain starts to fall."},
        ])),
        shot("erupt", "<groudon> claws its way out of the lava cliffs of <coast> and roars.", "Cracking rock, hissing lava, heavy footfalls, and a deep roar.", json!([
            {"duration": 6, "description": "A wide shot from the rocks looks up at the lava cliffs of <coast>. The rock face cracks at the center of the frame, and <groudon> claws its way out, lava sliding off its stone plates in glowing strings. It rears up and roars toward the left. The camera pushes in at slow speed and shakes with each step."},
        ])),
        shot("clash", "<kyogre> and <groudon> fight on <coast> in one continuous shot with no cuts.", "Crashing surf, roaring fire, bursts of steam, and deep impacts.", json!([
            {"duration": 14, "description": "A wide shot shows <kyogre> in the surf on the left, facing right, and <groudon> on the rocks on the right, facing left. <kyogre> surges forward and fires a thick column of water across the frame. It bursts into white steam against the stone plates of <groudon>."},
            {"duration": 10, "description": "<groudon> breathes a wide torrent of orange fire across the surf, and <kyogre> erupts from the water at the shoreline. The two crash together chest to chest, and a wall of water and steam rushes toward the camera."},
        ])),
    ];
    Written { head: vec![], boxes }
}

/// One asset as its box says it, after defaults.
pub(crate) struct Asset {
    pub name: String,
    /// What its file is: what it holds or last made, else what its tool's interface last said, else a picture. No more than what to call it.
    pub kind: String,
    /// Explicit container identity, preserved independently of its selected tool.
    pub node_kind: Option<String>,
    /// The exact version selected by navigation; `pin` retains compatibility with existing file pins.
    pub selected_version: Option<u32>,
    /// Whether anything said that.
    pub tool_makes: bool,
    /// What makes it, as the memo holds it: no release, no root namespace, and local code by its directory's name.
    pub tool: Option<String>,
    /// What makes it, as its box says it.
    pub target: String,
    /// Its `models` and `lora`, after defaults.
    pub made: Map<String, Value>,
    /// Its inputs, after defaults, each mention of a text asset replaced by its words: what its function is given.
    pub inputs: Map<String, Value>,
    /// Its inputs as its box says them, when that isn't `inputs`: what the page shows, and what its key holds.
    pub written: Option<Map<String, Value>>,
    /// The text assets `inputs` took in, those they mention too: each is a part of its key, as a read is.
    pub inlined: Vec<usize>,
    /// A text asset's words: what a mention of it says, in place.
    pub text: Option<String>,
    /// The inputs it wrote itself.
    pub own: BTreeSet<String>,
    /// The words for whatever mentions it.
    pub description: String,
    /// `approved`: the sha256 it holds. Its tool and inputs stay in its box, and can show it out of date.
    pub pin: Option<String>,
    /// `starred: true`.
    pub starred: bool,
    /// It has no operation: no `tool` that can make it, or its box says nothing.
    pub hollow: bool,
    /// What it reads, in the order its text mentions them.
    pub reads: Vec<Reading>,
}

/// One thing a text reads.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Reading {
    pub asset: usize,
    /// One version of it, which nothing moves: `<abr.2>`, `<self.1>`. None: whatever it stands on, and what reads it waits for that.
    pub number: Option<u32>,
    /// What the mention says, the name a function hears it by.
    pub said: String,
}

/// A function Calcifer runs itself, free, with no machine.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Tool {
    Cut,
    Frame,
}

/// The project as Calcifer reads it: its boxes, what each reads, and in what order they can be made.
pub(crate) struct Doc {
    pub assets: Vec<Asset>,
    /// What each reads.
    pub upstream: Vec<Vec<usize>>,
    /// Each asset after what it reads.
    pub order: Vec<usize>,
    pub warnings: Vec<Warning>,
    index: HashMap<String, usize>,
    /// Each package's own tags, as the lock last read them. A package not here hasn't been read yet.
    reserved: BTreeMap<String, Vec<String>>,
    /// What each tool makes, as the lock last read it.
    makes: BTreeMap<String, String>,
    /// Mentions naming nothing and no tag its package is known to have. Until a package is read its tags may be among them, so
    /// nothing is made of these before then.
    unread: Vec<(usize, String)>,
    /// The project's folder, which says what is on this computer.
    dir: PathBuf,
    /// Boxes written with a dotted name, read as their `_` form: by the dotted name's key, as a mention may still say it.
    dotted: HashMap<String, usize>,
}

/// Why an asset has the state it has, or none.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum State {
    /// A hash its box approves: the version it is, if one, with its whole file set, and what changed since it was made.
    Pinned { version: Option<u32>, changed: Vec<String> },
    /// This version has these inputs.
    Current(u32),
    /// Only words, for what mentions it: no file, and nothing to make.
    Words,
    /// Something it reads has no state.
    Waits,
    /// It has versions, none with these inputs, and why.
    Stale(String),
    #[default]
    Missing,
}

/// An asset's state now, and the memo key it stands on.
#[derive(Clone, Default)]
pub(crate) struct Standing {
    pub state: State,
    /// Its files; None is no state.
    pub files: Option<Files>,
    /// Its key; None while something it reads has no state.
    pub inputs: Option<String>,
    pub reads: Vec<ReadNow>,
}

impl Standing {
    /// The version that is its state, if one is.
    pub(crate) fn version(&self) -> Option<u32> {
        match self.state {
            State::Pinned { version, .. } => version,
            State::Current(version) => Some(version),
            _ => None,
        }
    }
}

/// What an operation reads now.
#[derive(Clone)]
pub(crate) struct ReadNow {
    pub name: String,
    pub asset: usize,
    pub files: Files,
    /// A tool reading a newest version out of date: its number.
    pub newest: Option<u32>,
    /// The version a mention fixes: `<abr.2>`.
    pub number: Option<u32>,
}

impl Doc {
    /// The project as its rows say now.
    pub(crate) fn load(folder: &Folder) -> Result<Self, Error> {
        let lock = folder.lock()?;
        folder.reading_boxes(|written| Self::of(folder, written, &lock))?
    }

    /// The project a render was asked with: its snapshot, of the files and tags the project holds now.
    pub(crate) fn asked(folder: &Folder, written: &Written) -> Result<Self, Error> {
        Self::of(folder, written, &folder.lock()?)
    }

    /// The project `written` says, of the files and tags `lock` holds.
    pub(super) fn of(folder: &Folder, written: &Written, lock: &Lock) -> Result<Self, Error> {
        let (top, listed) = written.reading();
        let mut warnings = vec![];
        let empty = Map::new();
        let mut defaults = Map::new();
        for (section, value) in top.get("defaults").and_then(Value::as_object).unwrap_or(&empty) {
            if !KINDS.contains(&section.as_str()) {
                warnings.push(Warning::of_project(format!("defaults.{section}: the only keys are image, video and audio; it is ignored")));
                continue;
            }
            let mut filled = value.as_object().cloned().unwrap_or_default();
            for word in PLATFORM.iter().filter(|word| !MADE.contains(word)) {
                if filled.remove(*word).is_some() {
                    warnings.push(Warning::of_project(format!("defaults.{section}.{word}: defaults fill a node's inputs: it is ignored")));
                }
            }
            defaults.insert(section.clone(), Value::Object(filled));
        }
        let declared = top.get("assets").and_then(Value::as_object).unwrap_or(&empty);
        for body in declared.values().filter_map(Value::as_object) {
            node_kind(body.get(NODE_KIND))?;
        }
        // A name holds no `.` (decision 4): one written with one reads as its `_` form, said.
        let read_as = |name: &String| if name.contains('.') { name.replace('.', "_") } else { name.clone() };
        for name in &listed {
            named(&read_as(name))?;
        }
        let mut seen: HashMap<String, String> = HashMap::new();
        for name in &listed {
            if let Some(first) = seen.insert(name_key(&read_as(name)), name.clone()) {
                return Err(one_name_twice(&first, name));
            }
        }
        let mut doc = Self { assets: vec![], upstream: vec![], order: vec![], warnings, index: HashMap::new(), reserved: lock.reserved.clone(), makes: lock.makes.clone(), unread: vec![], dir: folder.dir.clone(), dotted: HashMap::new() };
        for name in &listed {
            let Some(written) = declared.get(name) else { continue };
            let shown = read_as(name);
            if shown != *name {
                doc.dotted.insert(name_key(name), doc.assets.len());
                doc.warn(&shown, format!("{name}: a name holds no `.`, so it reads as {shown}: rename it {shown}"));
            }
            doc.declare(&shown, written, &defaults, folder, lock);
        }
        for i in 0..doc.assets.len() {
            if doc.assets[i].hollow && doc.assets[i].pin.is_none() && doc.assets[i].description.is_empty() && doc.assets[i].text.is_none() && lock.asset(&doc.assets[i].name).is_none_or(|held| held.versions.is_empty()) {
                let (name, said) = (doc.assets[i].name.clone(), doc.nothing_to_make(i));
                doc.warn(&name, said);
            }
        }
        doc.inline();
        doc.resolve();
        Ok(doc)
    }

    /// An asset of the project, after defaults, what it approved read. A name taken: the first stands.
    fn declare(&mut self, name: &str, written: &Value, defaults: &Map<String, Value>, folder: &Folder, lock: &Lock) {
        let key = name_key(name);
        if key == ALL {
            self.warn(name, format!("{name} is named `{ALL}`: rendering `{ALL}` means everything, and makes it with the rest"));
        }
        let mut body = match written {
            Value::Object(body) => body.clone(),
            Value::Null => Map::new(),
            _ => {
                self.warn(name, format!("{name} is no mapping of keys: it is empty"));
                Map::new()
            }
        };
        let target = text_of(body.get("tool")).trim().to_owned();
        let tool = (!target.is_empty()).then(|| plain_tool(&target, folder));
        let node_kind = if tool.as_deref() == Some(EDIT) { Some("video".to_owned()) }
            else { node_kind(body.get(NODE_KIND)).expect("node kind was validated").map(str::to_owned) };
        let selected_version = selected_version(body.get(SELECTED_VERSION)).ok().flatten();
        let mut pin = None;
        match body.get("approved") {
            None | Some(Value::Null) => {}
            Some(value) => match file_named(&text_of(Some(value)), lock) {
                Ok(sha256) => {
                    if !lock.files.get(&sha256).is_some_and(|file| file.here) {
                        self.warn(name, format!("{name}'s approved: {} is not on this computer; download it in the browser opened by cozy open", folder::prefix_of(&sha256)));
                    }
                    pin = Some(sha256);
                }
                Err(why) => self.warn(name, format!("{name}'s approved{why}")),
            },
        }
        let held = lock.asset(name).and_then(|held| held.versions.last());
        let made = held.and_then(|version| KINDS.iter().copied().find(|kind| version.files.contains_key(*kind)));
        let of_file = pin.as_ref().and_then(|sha256| lock.files.get(sha256)).and_then(folder::File::media);
        let makes = tool.as_ref().and_then(|tool| lock.makes.get(tool)).and_then(|kind| KINDS.iter().copied().find(|known| known == kind));
        // No tool and no images: it's words, in whichever field they stayed.
        let text = (tool.is_none() && pin.is_none() && held.is_none() && node_kind.as_deref().is_none_or(|kind| kind == TEXT)).then(|| body.get(TEXT).or_else(|| LEADS.iter().find_map(|field| body.get(*field)))).flatten().map(|words| text_of(Some(words)).trim_end().to_owned());
        let said = tool.as_deref().and_then(kind_made_here).or(node_kind.as_deref().map(node_media)).or(of_file).or(made).or(makes).or(text.as_ref().map(|_| TEXT));
        let kind = said.unwrap_or("image").to_owned();
        let description = text_of(body.get("description")).trim().to_owned();
        for mention in mentions(&description) {
            self.warn(name, format!("{name}'s description says <{mention}>: a description is words, so it stays as text and nothing is read"));
        }
        let own: BTreeSet<String> = body.keys().filter(|key| !PLATFORM.contains(&key.as_str())).cloned().collect();
        let (mut made, mut inputs) = (Map::new(), Map::new());
        // What Calcifer makes reads no style, and an edit's shape is its own to say.
        // By what its tool makes, never by what a version or an approved file is, or its key would move when it had one.
        if let Some(makes) = makes.filter(|_| tool.as_deref().is_some_and(|tool| !made_here(tool))) {
            for (field, value) in defaults.get(makes).and_then(Value::as_object).unwrap_or(&Map::new()) {
                match MADE.contains(&field.as_str()) {
                    true => drop(made.insert(field.clone(), value.clone())),
                    false => drop(inputs.insert(field.clone(), value.clone())),
                }
            }
        }
        for (field, value) in &body {
            match field.as_str() {
                field if MADE.contains(&field) => drop(made.insert(field.to_owned(), value.clone())),
                field if PLATFORM.contains(&field) || (field == TEXT && text.is_some()) => {}
                field => drop(inputs.insert(field.to_owned(), value.clone())),
            }
        }
        // `null` is no value, and takes back what defaults gave.
        inputs.retain(|_, value| !value.is_null());
        made.retain(|_, value| !value.is_null());
        if tool.as_deref() == Some(EDIT) && inputs.get("tracks").and_then(Value::as_array).is_some_and(|tracks| tracks.len() > 1) {
            self.warn(name, format!("{name}: an edit has one track for now: the others are left out"));
        }
        let hollow = body.is_empty() || tool.as_deref().is_none_or(|tool| made_here(tool) && kind_made_here(tool).is_none());
        self.index.insert(key, self.assets.len());
        let starred = body.get(STARRED).and_then(Value::as_bool).unwrap_or(false);
        self.assets.push(Asset { name: name.to_owned(), kind, node_kind, selected_version, tool_makes: makes.is_some(), tool, target, made, inputs, written: None, inlined: vec![], text, own, description, pin, starred, hollow, reads: vec![] });
    }

    /// Each mention of a text asset in what a tool is given replaced by its words, and theirs in turn. Only here: what's read, what's sent and
    /// the key all come from this. A mention that closes a cycle stays plain text, and a tag of its package stays that tag.
    fn inline(&mut self) {
        let texts: HashMap<String, usize> = self.assets.iter().enumerate().filter(|(_, asset)| asset.text.is_some()).map(|(i, asset)| (name_key(&asset.name), i)).collect();
        if texts.is_empty() {
            return;
        }
        let mut said = vec![];
        for i in 0..self.assets.len() {
            if self.assets[i].tool.as_deref().is_none_or(made_here) {
                continue;
            }
            let tags = self.tags(i).cloned().unwrap_or_default();
            let (mut inputs, mut took) = (self.assets[i].inputs.clone(), vec![]);
            inputs.values_mut().for_each(|value| self.inline_in(value, &tags, &texts, &mut took, &mut said));
            if !took.is_empty() {
                let asset = &mut self.assets[i];
                asset.written = Some(std::mem::replace(&mut asset.inputs, inputs));
                asset.inlined = took;
            }
        }
        self.warnings.extend(said);
    }

    /// `fields` with each text box they mention put in as its words, as a box's are: `cozy run`'s, written outside the project.
    pub(super) fn worded(&self, fields: &mut Map<String, Value>, tags: &[String]) {
        let texts: HashMap<String, usize> = self.assets.iter().enumerate().filter(|(_, asset)| asset.text.is_some()).map(|(i, asset)| (name_key(&asset.name), i)).collect();
        let (mut took, mut said) = (vec![], vec![]);
        fields.values_mut().for_each(|value| self.inline_in(value, tags, &texts, &mut took, &mut said));
    }

    fn inline_in(&self, value: &mut Value, tags: &[String], texts: &HashMap<String, usize>, took: &mut Vec<usize>, said: &mut Vec<Warning>) {
        match value {
            Value::String(text) => *text = self.with_words(text, tags, texts, &mut vec![], took, said),
            Value::Array(items) => items.iter_mut().for_each(|item| self.inline_in(item, tags, texts, took, said)),
            Value::Object(fields) => fields.values_mut().for_each(|item| self.inline_in(item, tags, texts, took, said)),
            _ => {}
        }
    }

    fn with_words(&self, text: &str, tags: &[String], texts: &HashMap<String, usize>, inside: &mut Vec<usize>, took: &mut Vec<usize>, said: &mut Vec<Warning>) -> String {
        mentions_replaced(text, &mut |mention| {
            let &j = texts.get(&name_key(mention)).filter(|_| !is_tag(tags, mention))?;
            let name = &self.assets[j].name;
            if inside.contains(&j) {
                let line = Warning::of(name, format!("{name} mentions itself, through what it mentions: that mention stays plain text"));
                if !said.contains(&line) {
                    said.push(line);
                }
                return None;
            }
            if !took.contains(&j) {
                took.push(j);
            }
            inside.push(j);
            let words = self.with_words(self.assets[j].text.as_deref().unwrap_or_default(), tags, texts, inside, took, said);
            inside.pop();
            Some(words)
        })
    }

    /// Each asset's reads, and the order. A mention of nothing stays text; one that closes a cycle too.
    fn resolve(&mut self) {
        let n = self.assets.len();
        let (mut upstream, mut said) = (vec![vec![]; n], vec![]);
        let mut reads = vec![vec![]; n];
        let mut unread = vec![];
        for asset in self.assets.iter().filter(|asset| asset.text.is_some()) {
            if let Some((tool, _)) = self.reserved.iter().find(|(_, tags)| is_tag(tags, &asset.name)) {
                said.push(Warning::of(&asset.name, format!("{0} is a tag of {tool}, so a text asset can't go by it: rename {0} in the browser opened by cozy open", asset.name)));
            }
        }
        for i in 0..n {
            let asset = &self.assets[i];
            let tags = self.tags(i);
            for tag in tags.into_iter().flatten().filter(|tag| self.find(tag).is_some()) {
                let line = Warning::of(&asset.name, format!("{tag} is also a tag of {}: <{tag}> in its text is that tag, not this: rename {tag} in the browser opened by cozy open", self.target(i)));
                if !said.contains(&line) {
                    said.push(line);
                }
            }
            for mention in mentions_in(&Value::Object(asset.inputs.clone())) {
                // A tag of its package, in any case, stays text as written, whatever the project names.
                if tags.is_some_and(|tags| is_tag(tags, &mention)) {
                    continue;
                }
                match self.mentioned(i, &mention) {
                    None => {
                        let line = Warning::of(&asset.name, no_bare_self(&mention));
                        if name_key(&mention) == SELF && !said.contains(&line) {
                            said.push(line);
                        }
                        unread.push((i, mention));
                    }
                    // A version is a file that is already there, so it holds nothing up and closes no cycle.
                    Some((j, Some(number))) => {
                        if !reads[i].iter().any(|read: &Reading| read.asset == j && read.number == Some(number)) {
                            reads[i].push(Reading { asset: j, number: Some(number), said: mention });
                        }
                    }
                    // Left by a cycle of text assets. What has no tool sends its words nowhere, so it reads nothing by them.
                    Some((j, None)) if self.assets[j].text.is_some() || asset.tool.is_none() => {}
                    Some((j, None)) => {
                        if j != i && !upstream[i].contains(&j) {
                            upstream[i].push(j);
                            reads[i].push(Reading { asset: j, number: None, said: self.assets[j].name.clone() });
                        }
                    }
                }
            }
        }
        let fixed = vec![vec![]; n];
        let graph = Graph::new(upstream, &fixed);
        for &(reader, read) in &graph.closing {
            reads[reader].retain(|held| held.asset != read || held.number.is_some());
            said.push(Warning::of(&self.assets[reader].name, format!("{} mentions {}, which is made from it: that mention stays plain text", self.assets[reader].name, self.assets[read].name)));
        }
        for (asset, reads) in self.assets.iter_mut().zip(reads) {
            asset.reads = reads;
        }
        self.order = graph.order;
        self.upstream = graph.upstream;
        self.unread = unread;
        self.warnings.extend(said);
    }

    /// The box a mention names by the dotted name it was written with, read as its `_` form.
    pub(super) fn dotted_box(&self, mention: &str) -> Option<usize> {
        self.dotted.get(&name_key(mention)).copied()
    }

    pub(crate) fn find(&self, name: &str) -> Option<usize> {
        self.index.get(&name_key(name)).copied()
    }

    /// What `mention` in asset `i`'s text names: a box, and a version of it.
    fn mentioned(&self, i: usize, mention: &str) -> Option<(usize, Option<u32>)> {
        if let Some(j) = self.dotted_box(mention) {
            return Some((j, None));
        }
        let read = graph::mention(mention)?;
        let j = self.find(read.name).or((name_key(read.name) == SELF && read.version.is_some()).then_some(i))?;
        Some((j, read.version))
    }

    fn warn(&mut self, name: &str, message: String) {
        self.warnings.push(Warning::of(name, message));
    }

    /// Asset `i`'s inputs as its function is given them: a text's last newline is no one's words.
    pub(crate) fn inputs_of(&self, i: usize) -> Map<String, Value> {
        let mut inputs = self.assets[i].inputs.clone();
        inputs.values_mut().for_each(end_trimmed);
        inputs
    }

    /// Editable inputs from this frozen document, before binding or text-asset expansion.
    pub(crate) fn authoring_fields(&self, i: usize) -> Map<String, Value> {
        let asset = &self.assets[i];
        let mut fields = asset.written.clone().unwrap_or_else(|| self.inputs_of(i));
        fields.values_mut().for_each(end_trimmed);
        fields.extend(asset.made.iter().map(|(field, value)| (field.clone(), value.clone())));
        fields
    }

    /// What asset `i` says, flat, by each control's `field`: its inputs, defaults too, and the platform's words. An edit's tracks are the wire's `timeline`.
    pub(crate) fn flat_fields(&self, i: usize) -> Map<String, Value> {
        let asset = &self.assets[i];
        let mut flat = asset.written.clone().unwrap_or_else(|| self.inputs_of(i));
        flat.values_mut().for_each(end_trimmed);
        flat.extend(asset.made.iter().map(|(field, value)| (field.clone(), value.clone())));
        flat.extend(asset.text.iter().map(|text| (TEXT.to_owned(), json!(text))));
        if let Some(kind) = &asset.node_kind { flat.insert(NODE_KIND.to_owned(), json!(kind)); }
        if !asset.description.is_empty() {
            flat.insert("description".to_owned(), json!(asset.description));
        }
        if let Some(tracks) = flat.remove("tracks") {
            let timeline = Timeline { tracks: vec![Track { clips: super::tools::clips_of(&tracks) }] };
            flat.insert("timeline".to_owned(), serde_json::to_value(timeline).expect("clips serialize"));
        }
        flat
    }

    /// What asset `i` reads by a mention, each with the places its text says so and what it sends now (`now`, as `stamped` read it).
    pub(crate) fn reads_of(&self, i: usize, now: &[ReadNow]) -> Vec<Read> {
        let asset = &self.assets[i];
        let mut said = vec![];
        for (field, value) in asset.inputs.iter().chain(asset.written.iter().flatten()) {
            said_in(value, field.clone(), &mut said);
        }
        said.sort();
        said.dedup();
        let at = |key: &str| said.iter().filter(|(mention, _)| name_key(mention) == key).map(|(_, place)| place.clone()).collect();
        let tool = self.tool(i).is_some();
        let sent = |read: &Reading| {
            let held = now.iter().find(|held| held.asset == read.asset && held.number == read.number)?;
            let files: Vec<_> = held.files.values().collect();
            let sends = if tool || read.asset == i { json!({"files": files}) } else { json!({"description": super::binder::OFF, "files": files}) };
            Some(digest(&sends)[..READ_PART].to_owned())
        };
        let read = |read: &Reading| {
            let key = name_key(if read.number.is_some() { &read.said } else { &self.assets[read.asset].name });
            Read { name: self.assets[read.asset].name.clone(), version: read.number, at: at(&key), key: sent(read) }
        };
        let words = asset.inlined.iter().map(|&j| {
            let key = Some(digest(&json!({"text": self.assets[j].text}))[..READ_PART].to_owned());
            Read { name: self.assets[j].name.clone(), version: None, at: at(&name_key(&self.assets[j].name)), key }
        });
        asset.reads.iter().map(read).chain(words).collect()
    }

    /// The assets whose text fixes version `number` of asset `i`, itself aside: what a person deleting it is told.
    pub(crate) fn read_by(&self, i: usize, number: u32) -> Vec<String> {
        let names = (0..self.assets.len()).filter(|&j| j != i && self.assets[j].reads.iter().any(|read| read.asset == i && read.number == Some(number)));
        names.map(|j| self.assets[j].name.clone()).collect()
    }

    /// Each version a text names, by its asset's name key: a deleted asset's too, as `reclaim` keeps what they'd bring back.
    pub(crate) fn versions_named(&self) -> BTreeMap<String, BTreeSet<u32>> {
        let fixed = self.assets.iter().flat_map(|asset| &asset.reads).filter_map(|read| Some((name_key(&self.assets[read.asset].name), read.number?)));
        let gone = self.unread.iter().filter_map(|(_, mention)| version_of(mention).map(|(name, number)| (name_key(name), number)));
        let mut named: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
        fixed.chain(gone).for_each(|(key, number)| {
            named.entry(key).or_default().insert(number);
        });
        named
    }

    /// The tags of the function that makes asset `i`, once it has been read.
    pub(crate) fn tags(&self, i: usize) -> Option<&Vec<String>> {
        self.reserved.get(self.assets[i].tool.as_deref().unwrap_or_default())
    }

    /// What is said of a name that is nothing in the project, and no tag of `tags`.
    /// What's said of `mention` in a box's text, which names nothing and is no tag of `tags`.
    pub(super) fn not_there(&self, mention: &str, tags: &[String], lock: &Lock) -> String {
        self.no_longer(mention, tags, lock)
    }

    fn no_longer(&self, name: &str, tags: &[String], lock: &Lock) -> String {
        if name_key(name) == SELF {
            return no_bare_self(name);
        }
        let named = graph::mention(name).map_or(name, |read| read.name);
        // A box deleted is named as deleted until Undo brings it back (cl-919): no other box stands in for it.
        if lock.was_deleted(named, super::now_ms()) {
            return format!("<{name}> was deleted");
        }
        match self.near(named, tags).map(|close| format!("{close}{}", &name[named.len()..])) {
            Some(close) => format!("<{name}> is no longer in the project: did you mean <{close}>?"),
            None => format!("<{name}> is no longer in the project"),
        }
    }

    /// Why asset `i` can't run, given its package's `tags`: each `<name>` in its text that names nothing and is no tag. Running it would
    /// send its words to a model with no picture, and charge for that. None: it can.
    pub(crate) fn missing_inputs(&self, i: usize, tags: &[String], lock: &Lock) -> Option<String> {
        let mut said: Vec<String> = vec![];
        let nothing = self.mentions_of_nothing(i, tags).map(|mention| self.no_longer(mention, tags, lock));
        nothing.chain(self.versions_gone(i, lock).into_iter().map(|(_, why)| why)).for_each(|line| if !said.contains(&line) { said.push(line) });
        (!said.is_empty()).then(|| format!("{}: {}", self.assets[i].name, said.join("; ")))
    }

    /// The names in `missing_inputs`, each once.
    pub(crate) fn missing_names(&self, i: usize, tags: &[String], lock: &Lock) -> Vec<String> {
        let mut names: Vec<String> = vec![];
        let gone = self.versions_gone(i, lock);
        self.mentions_of_nothing(i, tags).chain(gone.iter().map(|(mention, _)| *mention)).for_each(|name| if !names.iter().any(|there| there == name) { names.push(name.to_owned()) });
        names
    }

    /// The versions asset `i`'s text fixes that a render can't read: known with no package read.
    pub(crate) fn versions_missing(&self, i: usize, lock: &Lock) -> Vec<String> {
        self.versions_gone(i, lock).into_iter().map(|(mention, _)| mention.to_owned()).collect()
    }

    /// The same, each with why: deleted, or never made.
    fn versions_gone(&self, i: usize, lock: &Lock) -> Vec<(&str, String)> {
        let mut gone = vec![];
        for read in &self.assets[i].reads {
            let Some(number) = read.number else { continue };
            let of = &self.assets[read.asset];
            let held = lock.asset(&of.name);
            let deleted = held.is_some_and(|held| held.discarded.iter().any(|version| version.number == number));
            if !deleted && self.version_files(read.asset, number, lock).is_some() {
                continue;
            }
            let listed: Vec<String> = held.into_iter().flat_map(|held| &held.versions).map(|version| version.number.to_string()).collect();
            let said = &read.said;
            let why = match (deleted, listed.as_slice()) {
                (true, _) => format!("<{said}> was deleted: the browser history lists it, and Undo brings {}'s {number} back", of.name),
                (_, []) => format!("<{said}>: {} has no versions yet", of.name),
                (_, listed) => format!("<{said}>: {} has no version {number}, only {}", of.name, listed.join(", ")),
            };
            gone.push((said.as_str(), why));
        }
        gone
    }

    fn mentions_of_nothing<'a>(&'a self, i: usize, tags: &'a [String]) -> impl Iterator<Item = &'a str> {
        self.unread.iter().filter(move |(asset, mention)| *asset == i && !is_tag(tags, mention)).map(|(_, mention)| mention.as_str())
    }

    /// What is said ahead of a run of asset `i`, given its package's `tags`. A pin runs nothing, so it says nothing, whatever its text says.
    pub(crate) fn said_once_read(&self, i: usize, tags: &[String], lock: &Lock) -> Option<Warning> {
        let said = self.missing_inputs(i, tags, lock).filter(|_| self.assets[i].pin.is_none())?;
        Some(Warning::of(&self.assets[i].name, said))
    }

    /// `said_once_read` of each asset whose package the lock knows.
    pub(crate) fn said_of_known(&self, lock: &Lock) -> Vec<Warning> {
        (0..self.assets.len()).filter(|&i| self.makes(i) && self.tool(i).is_none()).filter_map(|i| self.said_once_read(i, self.tags(i)?, lock)).collect()
    }

    /// The name `mention`, which names nothing, is likely a typo of: within two edits, neither of them short, and no tag of `tags`.
    fn near(&self, mention: &str, tags: &[String]) -> Option<&str> {
        const SHORTEST: usize = 4;
        let key = name_key(mention);
        let names = self.assets.iter().filter(|asset| !is_tag(tags, &asset.name)).map(|asset| (name_key(&asset.name), asset.name.as_str()));
        let close = names.filter(|(other, _)| key.chars().count() >= SHORTEST && other.chars().count() >= SHORTEST).map(|(other, name)| (edits(&key, &other), name));
        close.filter(|(apart, _)| *apart <= 2).min_by_key(|(apart, _)| *apart).map(|(_, name)| name)
    }

    /// The function that makes asset `i`, as its box says it.
    pub(crate) fn target(&self, i: usize) -> String {
        self.assets[i].target.clone()
    }

    /// Asset `i`'s operation when Calcifer runs it itself: a function of the reserved package `cozy`.
    pub(crate) fn tool(&self, i: usize) -> Option<Tool> {
        match self.assets[i].tool.as_deref()? {
            EDIT => Some(Tool::Cut),
            FRAME => Some(Tool::Frame),
            _ => None,
        }
    }

    /// The one line that says why asset `i` has no operation.
    pub(crate) fn nothing_to_make(&self, i: usize) -> String {
        let asset = &self.assets[i];
        match asset.tool.as_deref() {
            None if asset.own.is_empty() => format!("{} says nothing: it has nothing to make", asset.name),
            None => format!("{} has no `tool`: it has nothing to make", asset.name),
            Some(tool) => format!("{}: {COZY} makes {EDIT} and {FRAME}, not {tool}: it has nothing to make", asset.name),
        }
    }

    /// Whether asset `i` has an operation that can make it.
    pub(crate) fn makes(&self, i: usize) -> bool {
        !self.assets[i].hollow
    }

    /// Every asset's state, in the project's terms.
    /// Whether this was read with the package tags `lock` knows now.
    pub(crate) fn knows_tags_of(&self, lock: &Lock) -> bool {
        self.reserved == lock.reserved && self.makes == lock.makes
    }

    pub(crate) fn stand(&self, lock: &Lock) -> Vec<Standing> {
        let mut done = vec![Standing::default(); self.assets.len()];
        for &i in &self.order {
            done[i] = self.standing(i, lock, &done);
        }
        done
    }

    /// The hash its box approves, else its newest version for these inputs, else none. A hash that is a version's main file brings
    /// the version's whole file set; one that is none's is just that file.
    pub(crate) fn standing(&self, i: usize, lock: &Lock, done: &[Standing]) -> Standing {
        let asset = &self.assets[i];
        let held = lock.asset(&asset.name);
        let reads = self.reading(i, lock, done);
        let inputs = reads.as_ref().map(|reads| self.key(i, reads));
        let reads = reads.unwrap_or_default();
        let stand = |state, files| Standing { state, files, inputs: inputs.clone(), reads: reads.clone() };
        let versions = held.map(|held| held.versions.as_slice()).unwrap_or_default();
        let stands = |version: &Version, now: &str| !version.inputs.is_empty() && self.moved(i, &version.inputs, now, &reads).is_empty();
        if let Some(sha256) = &asset.pin {
            return match asset.selected_version.and_then(|number| versions.iter().find(|version| version.number == number && version.main() == Some(sha256)))
                .or_else(|| versions.iter().rev().find(|version| version.main() == Some(sha256))) {
                Some(version) => {
                    let changed = match &inputs {
                        Some(now) if !version.inputs.is_empty() && !asset.hollow => self.moved(i, &version.inputs, now, &reads),
                        _ => vec![],
                    };
                    stand(State::Pinned { version: Some(version.number), changed }, Some(version.files.clone()))
                }
                None => stand(State::Pinned { version: None, changed: vec![] }, Some(Files::from([(asset.kind.clone(), sha256.clone())]))),
            };
        }
        // With no tool nothing can put it out of date: it stands on its newest, else its words.
        if asset.hollow {
            if let Some(version) = versions.last() {
                return stand(State::Current(version.number), Some(version.files.clone()));
            }
            if !asset.description.is_empty() || asset.text.is_some() {
                return stand(State::Words, Some(Files::new()));
            }
        }
        let Some(now) = &inputs else { return stand(State::Waits, None) };
        // A film short of its pieces, or that went without one now here, isn't its state, though the memo would say so: it cuts again.
        let off = |version: &folder::Version| -> Option<String> {
            if self.tool(i) != Some(Tool::Cut) {
                return None;
            }
            let here = |piece: &str| {
                let read = reads.iter().find(|read| name_key(&read.name) == name_key(piece));
                read.and_then(|read| lock.files.get(folder::main_of(&read.files)?)).is_some_and(|file| self.dir.join(&file.path).is_file())
            };
            match version.without.iter().find(|left| left.absent && here(&left.piece)) {
                Some(left) => Some(format!("{} is here now", left.piece)),
                None => super::tools::film_off(&asset.inputs, &reads, lock, version),
            }
        };
        let holding = || versions.iter().rev().filter(|version| version.inputs == *now);
        if let Some(why) = holding().next().filter(|_| holding().all(|version| off(version).is_some())).and_then(off) {
            return stand(State::Stale(why), None);
        }
        match (holding().find(|version| off(version).is_none()), versions.last()) {
            (Some(version), _) => stand(State::Current(version.number), Some(version.files.clone())),
            (None, None) => stand(State::Missing, None),
            (None, Some(_)) => {
                if let Some(version) = versions.iter().rev().find(|version| self.tool(i).is_none() && stands(version, now)) {
                    return stand(State::Current(version.number), Some(version.files.clone()));
                }
                stand(State::Stale("its inputs changed".to_owned()), None)
            }
        }
    }

    /// What asset `i` reads now; None while something has no state. A tool reads state, else the newest version: what has
    /// neither it skips, and with nothing to read at all, it waits.
    fn reading(&self, i: usize, lock: &Lock, done: &[Standing]) -> Option<Vec<ReadNow>> {
        let tool = self.tool(i).is_some();
        let mut reads = vec![];
        for read in &self.assets[i].reads {
            let j = read.asset;
            // A deleted one is still what was read, so nothing it made goes out of date; `versions_gone` stops a render.
            if let Some(number) = read.number {
                reads.extend(self.version_files(j, number, lock).map(|files| ReadNow { name: read.said.clone(), asset: j, files, newest: None, number: Some(number) }));
                continue;
            }
            let name = self.assets[j].name.clone();
            match &done[j].files {
                Some(files) => reads.push(ReadNow { name, asset: j, files: files.clone(), newest: None, number: None }),
                None if tool => {
                    let newest = lock.asset(&self.assets[j].name).and_then(|held| held.versions.last());
                    reads.extend(newest.map(|newest| ReadNow { name, asset: j, files: newest.files.clone(), newest: Some(newest.number), number: None }));
                }
                None => return None,
            }
        }
        (!tool || !reads.is_empty()).then_some(reads)
    }

    /// Version `number` of asset `j`, on record though deleted. A file it holds with no version of any yet is its first.
    pub(super) fn version_files(&self, j: usize, number: u32, lock: &Lock) -> Option<Files> {
        let asset = &self.assets[j];
        let held = lock.asset(&asset.name);
        if let Some(version) = held.and_then(|held| held.on_record().find(|version| version.number == number)) {
            return Some(version.files.clone());
        }
        let none_yet = number == 1 && held.is_none_or(|held| held.on_record().next().is_none());
        asset.pin.as_ref().filter(|_| none_yet).map(|sha256| Files::from([(asset.kind.clone(), sha256.clone())]))
    }

    /// Its key, blind to names: eight hex digits of what makes it and its written fields, each box they mention numbered by first mention,
    /// then twelve for each of those boxes in that order (`parts`). So a rename puts nothing out of date.
    fn key(&self, i: usize, reads: &[ReadNow]) -> String {
        let asset = &self.assets[i];
        let mut own = asset.made.clone();
        if let Some(tool) = &asset.tool {
            own.insert("tool".into(), json!(tool));
        }
        // A tool's key holds what it reads of its inputs.
        let tool = self.tool(i).is_some();
        let mut inputs: Value = Value::Object(
            (asset.written.as_ref().unwrap_or(&asset.inputs).iter())
                .filter(|(field, _)| !tool || TOOL_INPUTS.contains(&field.as_str()))
                .map(|(field, value)| (field.clone(), value.clone()))
                .collect(),
        );
        let mut order = vec![];
        self.numbered(i, &mut inputs, &mut order);
        own.insert("inputs".into(), inputs);
        let parts: String = self.parts(i, reads, &mut order).into_iter().map(|(_, part)| part).collect();
        format!("{}{parts}", &digest(&Value::Object(own))[..OWN_PART])
    }

    /// Each mention under `value` with its box by number, first mentioned first, as `order` holds them: `<Amy.2>` is `<#0.2>`. A mention of
    /// nothing is numbered too, by its name, so a read that's gone moves nothing else. A tag stays as written.
    fn numbered(&self, i: usize, value: &mut Value, order: &mut Vec<Result<usize, String>>) {
        let tags = self.tags(i).cloned().unwrap_or_default();
        let mut walk = |text: &mut String| {
            *text = mentions_replaced(text, &mut |inner| {
                if is_tag(&tags, inner) {
                    return None;
                }
                let (named, rest) = match (self.mentioned(i, inner), self.dotted_box(inner)) {
                    (Some((j, _)), Some(_)) => (Ok(j), ""),
                    (Some((j, _)), None) => (Ok(j), &inner[graph::mention(inner)?.name.len()..]),
                    (None, _) => {
                        let read = graph::mention(inner)?;
                        (Err(name_key(read.name)), &inner[read.name.len()..])
                    }
                };
                let k = order.iter().position(|held| *held == named).unwrap_or_else(|| {
                    order.push(named);
                    order.len() - 1
                });
                Some(format!("<#{k}{rest}>"))
            });
        };
        each_text(value, &mut walk);
    }

    /// What each box asset `i` mentions sends, in `order`, by name: a text box its words, numbered on, and a box its files and (empty) description,
    /// of each version read. Calcifer's own tools read files alone.
    fn parts(&self, i: usize, reads: &[ReadNow], order: &mut Vec<Result<usize, String>>) -> Vec<(String, String)> {
        let tool = self.tool(i).is_some();
        let mut parts = vec![];
        let mut k = 0;
        while k < order.len() {
            let j = match &order[k] {
                Ok(j) => *j,
                Err(name) => {
                    parts.push((name.clone(), GONE.to_owned()));
                    k += 1;
                    continue;
                }
            };
            let of = &self.assets[j];
            let held = match of.text.as_ref().filter(|_| self.assets[i].inlined.contains(&j)) {
                Some(text) => {
                    let mut words = json!(text);
                    self.numbered(i, &mut words, order);
                    json!({"text": words})
                }
                None => {
                    let mut read: Vec<(Option<u32>, Value)> = reads.iter().filter(|read| read.asset == j).map(|read| {
                        let files: Vec<_> = read.files.values().collect();
                        (read.number, if tool || j == i { json!({"files": files}) } else { json!({"description": super::binder::OFF, "files": files}) })
                    }).collect();
                    read.sort_by_key(|(number, _)| *number);
                    json!(read)
                }
            };
            parts.push((of.name.clone(), digest(&held)[..READ_PART].to_owned()));
            k += 1;
        }
        parts
    }

    /// What moved since a version was made at key `then`: its inputs, or each box it mentions whose part changed. One `then` holds that's gone
    /// is no change, as nothing could make it again.
    fn moved(&self, i: usize, then: &str, now: &str, reads: &[ReadNow]) -> Vec<String> {
        if then == now {
            return vec![];
        }
        if own_part(then) != own_part(now) {
            return vec!["its inputs".to_owned()];
        }
        let held: Vec<&[u8]> = then.as_bytes().get(OWN_PART..).unwrap_or_default().chunks_exact(READ_PART).collect();
        let mut order = vec![];
        let mut inputs = Value::Object(self.assets[i].written.clone().unwrap_or_else(|| self.assets[i].inputs.clone()));
        self.numbered(i, &mut inputs, &mut order);
        let parts = self.parts(i, reads, &mut order);
        // A read that's gone is no change: nothing could make it again.
        let changed = |k: usize, part: &str| part != GONE && held.get(k).is_some_and(|was| *was != part.as_bytes());
        parts.into_iter().enumerate().filter(|(k, (_, part))| changed(*k, part)).map(|(_, (name, _))| format!("<{name}>")).collect()
    }

    /// The words asset `i` is made from, its text assets' in them, what leads first, as a version keeps them. Calcifer's own tools have none.
    pub(crate) fn text(&self, i: usize) -> Option<String> {
        if self.tool(i).is_some() {
            return None;
        }
        let inputs = &self.assets[i].inputs;
        let lead = LEADS.iter().filter_map(|field| inputs.get(*field));
        let rest = inputs.iter().filter(|(field, _)| !LEADS.contains(&field.as_str())).map(|(_, value)| value);
        let mut words = vec![];
        lead.chain(rest).for_each(|value| words_in(value, &mut words));
        (!words.is_empty()).then(|| words.join("\n\n"))
    }

}

/// Every string of `value` that holds more than a word, in order: what's said, not a shape or a mention.
fn words_in<'a>(value: &'a Value, words: &mut Vec<&'a str>) {
    match value {
        Value::String(text) if text.trim().contains(char::is_whitespace) => words.push(text.trim()),
        Value::Array(items) => items.iter().for_each(|item| words_in(item, words)),
        Value::Object(fields) => fields.values().for_each(|field| words_in(field, words)),
        _ => {}
    }
}

/// `tool` as the key holds it: no release, no root namespace, and local code by its directory's name, with the function it names, so moving
/// from a checkout to the published package puts nothing out of date.
pub(crate) fn plain_tool(tool: &str, folder: &Folder) -> String {
    let with = tool.trim();
    if api::target::written_as_path(with) || with.ends_with(".py") {
        let dir_name = |path: &str| {
            let name = path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(path).to_owned();
            name.strip_suffix(".py").map(str::to_owned).unwrap_or(name)
        };
        let home = platform::paths::login_home().unwrap_or_default();
        return match crate::local_code::target(with, Some(&folder.dir), &home) {
            Some(Ok((path, Some(function)))) => format!("{}/{function}", dir_name(&path.to_string_lossy())),
            _ => dir_name(with),
        };
    }
    let path = with.split('@').next().unwrap_or(with);
    path.strip_prefix("_/").unwrap_or(path).to_owned()
}

/// Each mention under `value`, with where: `summary`, `beats.0.description`, `tracks.0.clips.1`.
fn said_in(value: &Value, place: String, found: &mut Vec<(String, String)>) {
    match value {
        Value::String(text) => found.extend(mentions(text).into_iter().map(|mention| (mention.to_owned(), place.clone()))),
        Value::Array(items) => items.iter().enumerate().for_each(|(n, item)| said_in(item, format!("{place}.{n}"), found)),
        Value::Object(fields) => fields.iter().for_each(|(field, item)| said_in(item, format!("{place}.{field}"), found)),
        _ => {}
    }
}

/// Each text under `value` with nothing after its last word.
pub(super) fn end_trimmed(value: &mut Value) {
    match value {
        Value::String(text) => text.truncate(text.trim_end().len()),
        Value::Array(items) => items.iter_mut().for_each(end_trimmed),
        Value::Object(fields) => fields.values_mut().for_each(end_trimmed),
        _ => {}
    }
}

fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

/// `name`, refused unless it can be a name: the project is wrong, and nothing is rendered from it.
pub(crate) fn named(name: &str) -> Result<(), Error> {
    if is_name(name) {
        return Ok(());
    }
    let why = match name.contains([' ', ',']) {
        true => "names can't hold spaces or commas",
        false if name.contains('.') => "names hold no `.`: `<name.2>` is a version of a box, and `<name.word>` an extra of it",
        false => "names hold only letters, digits, `_` and `-`",
    };
    Err(invalid("project_name", format!("`{name}`: {why}; use `{}`", as_name(name))))
}

/// Whether something new can go by `name`, a person having typed it.
pub(crate) fn new_name(name: &str) -> Result<(), Error> {
    named(name)?;
    match can_take(name) {
        true => Ok(()),
        false => Err(invalid("project_name", format!("`{name}` is what a box calls itself in its own text: use `{name}_2`"))),
    }
}

fn no_bare_self(said: &str) -> String {
    format!("<{said}> would read its own newest version, which every render changes: write <self.1>, the version to start from")
}

/// The sha256 a box's `approved` names, as its Doc reads it: none where it names none, or none that reads.
pub(super) fn approved_file(approved: Option<&Value>, lock: &Lock) -> Option<String> {
    match approved {
        None | Some(Value::Null) => None,
        Some(value) => file_named(&text_of(Some(value)), lock).ok(),
    }
}

/// Only its first media may initialize an approval. An explicit unpin stays unpinned.
pub(super) fn can_initialize_approval(folder: &Folder, name: &str) -> Result<bool, Error> {
    let approved = folder.reading_boxes(|written| written.get(name).and_then(|held| held.get("approved")).cloned())?;
    folder.reading_lock(|lock| approved_file(approved.as_ref(), lock).is_none() && lock.asset(name).is_none_or(|held| held.on_record().next().is_none()))
}

/// The sha256 a file pin's value names, or how it fails, to follow "{name}'s {field}".
fn file_named(written: &str, lock: &Lock) -> Result<String, String> {
    if written.is_empty() || !written.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(" is not a hash: {written} is not read: import {} in the browser opened by `cozy open`", quoted(written)));
    }
    match lock.matching(&written.to_ascii_lowercase()).as_slice() {
        [file] => Ok(file.sha256.clone()),
        [] => Err(format!(": no file's hash starts with {written}: import a file in the browser opened by `cozy open`")),
        several => Err(format!(": {written} fits {} files: approve the version you mean again", several.len())),
    }
}

/// `text` as one word on a command line.
fn quoted(text: &str) -> String {
    match text.bytes().any(|byte| byte.is_ascii_whitespace() || b"'\"$&;|<>()*?#`".contains(&byte)) {
        true => format!("'{}'", text.replace('\'', r"'\''")),
        false => text.to_owned(),
    }
}

/// `text` as it calls something new, if it can: through `as_name`, so a `.` is `_`.
pub(crate) fn plain_name(text: &str) -> Option<String> {
    let name = as_name(text);
    (!name_key(&name).is_empty()).then_some(name)
}

/// `wanted`, or counting up if something in the project goes by it, or versions wait in the lock for it: `hero_2`, and `shot_1` is `shot_2`.
pub(crate) fn free_name(doc: &Doc, lock: &Lock, wanted: &str) -> String {
    free_of(|name| doc.find(name).is_some(), lock, wanted)
}

/// `free_name`, of what `holds` says the project goes by.
pub(super) fn free_of(holds: impl Fn(&str) -> bool, lock: &Lock, wanted: &str) -> String {
    let taken = |name: &str| [ALL, SELF].contains(&name_key(name).as_str()) || holds(name) || lock.asset(name).is_some_and(|held| !held.versions.is_empty());
    if !taken(wanted) {
        return wanted.to_owned();
    }
    let counted = wanted.rsplit_once('_').filter(|(stem, _)| !stem.is_empty()).and_then(|(stem, number)| Some((stem, number.parse::<u32>().ok()?)));
    let (stem, from) = counted.map_or((wanted, 2), |(stem, number)| (stem, u64::from(number) + 1));
    (from..).map(|n| format!("{stem}_{n}")).find(|name| !taken(name)).expect("a free name")
}

/// What a person who typed `name` is told when the project has it.
pub(crate) fn name_taken(doc: &Doc, name: &str) -> Error {
    let there = doc.find(name).map_or(name, |i| doc.assets[i].name.as_str());
    Error::new(Kind::Conflict, "asset_exists", format!("{there} is in the project already")).remedy(format!("use another name, or edit {there} in the browser opened by cozy open"))
}

/// Whether a text asset can go by `name`: not a tag of a package this project uses.
pub(crate) fn refuse_tag(lock: &Lock, name: &str) -> Result<(), Error> {
    match lock.reserved.iter().find(|(_, tags)| is_tag(tags, name)) {
        Some((tool, _)) => Err(Error::new(Kind::Validation, "asset_name_reserved", format!("{name} is a tag of {tool}: <{name}> in a prompt means that")).remedy("call it something else")),
        None => Ok(()),
    }
}

/// Text trimmed, numbers as floats, keys sorted: a comment, a reformat or key order changes nothing.
pub(super) fn canonical(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(text.trim().to_owned()),
        Value::Number(number) => number.as_f64().map_or_else(|| value.clone(), |float| json!(float)),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        Value::Object(fields) => {
            let sorted: BTreeMap<&String, Value> = fields.iter().map(|(field, value)| (field, canonical(value))).collect();
            Value::Object(sorted.into_iter().map(|(field, value)| (field.clone(), value)).collect())
        }
        _ => value.clone(),
    }
}

fn digest(value: &Value) -> String {
    hex::encode(Sha256::digest(canonical(value).to_string().as_bytes()))
}

/// How much of a key says what makes it and its inputs, and how much says each read.
const OWN_PART: usize = 8;
const READ_PART: usize = 12;
/// A part for a box a mention names that's gone.
const GONE: &str = "------------";

/// Each text under `value`, changed by `with`.
fn each_text(value: &mut Value, with: &mut dyn FnMut(&mut String)) {
    match value {
        Value::String(text) => with(text),
        Value::Array(items) => items.iter_mut().for_each(|item| each_text(item, with)),
        Value::Object(fields) => fields.values_mut().for_each(|item| each_text(item, with)),
        _ => {}
    }
}

/// `first` and `second` as the project spells them: one name, as names match by `name_key`.
fn one_name_twice(first: &str, second: &str) -> Error {
    let said = format!("{first} and {second} are one name: a name matches in any case, with `_` and `-` alike");
    invalid("project_name_twice", said).remedy(format!("rename {second}"))
}

fn own_part(key: &str) -> &str {
    key.get(..OWN_PART).unwrap_or(key)
}

pub(super) fn invalid(code: &str, message: String) -> Error {
    Error::new(Kind::Validation, code, message)
}

/// Whether `name` is one of a package's `tags`, in any case.
fn is_tag(tags: &[String], name: &str) -> bool {
    tags.iter().any(|tag| name_key(tag) == name_key(name))
}
