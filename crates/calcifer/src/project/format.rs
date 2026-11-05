//! project.yaml, format 3 (design/project-format-ui.md): each asset a name, its file's type, the function that makes it, its inputs, the
//! file you approved, and words for whatever mentions it. The platform reads `kind`, `tool`, `approved`, `description`, `asset_type`,
//! `voice`, `models`, `lora` and `<name>` in any text, and knows no input's meaning, but for the reserved package `cozy`, which it runs itself.
//! Every other key is an input: it passes through to the function and counts in the key. Nothing refuses a project but a file of no
//! format or another: what one line gets wrong is said, and that line does the best it can. `assetfile` writes the file.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use protocol::{
    error::{Error, Kind},
    project::{Read, Timeline, Track, Warning},
    tool::EDIT,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    folder::{self, Files, Folder, Keeps, Lock, Version},
    graph::{Graph, as_name, edits, is_name, mentions, mentions_in, mentions_replaced, name_key},
};

/// The name `cozy project render all` means.
pub const ALL: &str = "all";
pub(super) const FORMAT: u64 = 3;
/// What the platform reads of an asset. Any other key is an input.
const PLATFORM: [&str; 7] = ["tool", "approved", "description", "asset_type", "voice", "models", "lora"];
/// What goes on a function's first call, beside its input.
const MADE: [&str; 2] = ["models", "lora"];
const KINDS: [&str; 3] = ["image", "video", "audio"];
const ASSET_TYPES: [&str; 3] = ["character", "prop", "environment"];
const SECTIONS: [&str; 3] = ["version", "defaults", "assets"];
/// What a text asset holds, and what it is.
pub(super) const TEXT: &str = "text";
/// What once said an asset's type: its tool does now, and a file with none is what it is.
const NOT_READ: &str = "kind";
/// The reserved package Calcifer runs itself.
const COZY: &str = "cozy";
const FRAME: &str = "cozy/frame";
const FOR_READERS: [&str; 2] = ["use_audio", "follow"];
/// What `cozy` reads of an edit or a frame.
const TOOL_INPUTS: [&str; 6] = ["tracks", "fps", "aspect_ratio", "resolution", "video", "at"];

/// Whether `tool` names what Calcifer makes itself: a function of the package `cozy`.
pub fn made_here(tool: &str) -> bool {
    let tool = tool.trim();
    let path = protocol::target::written_as_path(tool) || tool.ends_with(".py");
    !path && tool.split(['@', '/']).find(|part| *part != "_") == Some(COZY)
}

/// What `plain` makes, if Calcifer makes it: `cozy/edit` a video, `cozy/frame` a picture.
fn kind_made_here(plain: &str) -> Option<&'static str> {
    match plain {
        EDIT => Some("video"),
        FRAME => Some("image"),
        _ => None,
    }
}

/// A new project's project.yaml: a short film that renders as it stands.
pub fn starter() -> String {
    r#"# A short film, ready to render: cozy project render all --on <machine>
# Change anything. Each asset says what it is; `<name>` in any text reads that asset.
version: 3

assets:
  style:
    text: High-quality Unreal Engine graphics in a bright, hand-painted, cel-shaded anime style.

  kyogre:
    tool: qwen-image-2/generate
    aspect_ratio: "16:9"
    instructions: |
      <style> Kyogre, the colossal blue whale-like legendary Pokémon with huge fins, no legs, and glowing red lines. The whole body from the front on the left, with nothing added to the body described and no ground under it. On the right, the same character has been spun a half turn, at the same size, so its back faces the viewer and nothing on its front can be seen. Two separate views, with no text.
    description: Kyogre, the colossal blue whale-like legendary Pokémon with huge fins, no legs, and glowing red lines.
    asset_type: character

  groudon:
    tool: qwen-image-2/generate
    aspect_ratio: "16:9"
    instructions: |
      <style> Groudon, the colossal red legendary Pokémon with grey stone plates and huge claws. The whole body from the front on the left, with nothing added to the body described and no ground under it. On the right, the same character has been spun a half turn, at the same size, so its back faces the viewer and nothing on its front can be seen. Two separate views, with no text.
    description: Groudon, the colossal red legendary Pokémon with grey stone plates and huge claws.
    asset_type: character

  coast:
    tool: qwen-image-2/generate
    aspect_ratio: "16:9"
    instructions: <style> A volcanic coast under a storm sky, with a dark sea on the left, lava cliffs on the right, and a small white crater city far away at the center.
    description: A volcanic coast under a storm sky, with a dark sea on the left, lava cliffs on the right, and a small white crater city far away at the center.
    asset_type: environment

  edit:
    tool: cozy/edit
    fps: 24
    tracks:
      - clips:
          - <awaken>
          - <erupt>
          - <clash>

  awaken:
    tool: minimax-h3/generate
    aspect_ratio: "16:9"
    summary: |
      <style>
      <kyogre> bursts out of the stormy sea of <coast>.
    soundscape: Howling wind, crashing waves, and rolling thunder.
    beats:
      - duration: 6
        description: |
          A wide shot from the water's edge looks out over the stormy sea of <coast>, with lightning flickering behind the clouds. The water bulges at the center of the frame, and <kyogre> bursts out of it, facing right, with sheets of white water pouring off its fins. The camera tilts up at slow speed as heavy rain starts to fall.

  erupt:
    tool: minimax-h3/generate
    aspect_ratio: "16:9"
    summary: |
      <style>
      <groudon> claws its way out of the lava cliffs of <coast> and roars.
    soundscape: Cracking rock, hissing lava, heavy footfalls, and a deep roar.
    beats:
      - duration: 6
        description: |
          A wide shot from the rocks looks up at the lava cliffs of <coast>. The rock face cracks at the center of the frame, and <groudon> claws its way out, lava sliding off its stone plates in glowing strings. It rears up and roars toward the left. The camera pushes in at slow speed and shakes with each step.

  clash:
    tool: minimax-h3/generate
    aspect_ratio: "16:9"
    summary: |
      <style>
      <kyogre> and <groudon> fight on <coast> in one continuous shot with no cuts.
    soundscape: Crashing surf, roaring fire, bursts of steam, and deep impacts.
    beats:
      - duration: 14
        description: |
          A wide shot shows <kyogre> in the surf on the left, facing right, and <groudon> on the rocks on the right, facing left. <kyogre> surges forward and fires a thick column of water across the frame. It bursts into white steam against the stone plates of <groudon>.
      - duration: 10
        description: |
          <groudon> breathes a wide torrent of orange fire across the surf, and <kyogre> erupts from the water at the shoreline. The two crash together chest to chest, and a wall of water and steam rushes toward the camera.
"#
    .into()
}

/// One asset as the file says it, after defaults.
pub struct Asset {
    pub name: String,
    /// What its file is: what it holds or last made, else what its tool's interface last said, else a picture. No more than what to call it.
    pub kind: String,
    /// Whether anything said that.
    pub tool_makes: bool,
    /// What makes it, as the memo holds it: no release, no root namespace, and local code by its directory's name.
    pub tool: Option<String>,
    /// What makes it, as the file writes it.
    pub target: String,
    /// Its `models` and `lora`, after defaults.
    pub made: Map<String, Value>,
    /// Its inputs, after defaults, each mention of a text asset replaced by its words: what its function is given.
    pub inputs: Map<String, Value>,
    /// Its inputs as the file says them, when that isn't `inputs`: what the page shows, and what its key holds.
    pub written: Option<Map<String, Value>>,
    /// The text assets `inputs` took in, those they mention too: each is a part of its key, as a read is.
    pub inlined: Vec<usize>,
    /// A text asset's words: what a mention of it says, in place.
    pub text: Option<String>,
    /// The inputs it wrote itself.
    pub own: BTreeSet<String>,
    /// The words for whatever mentions it.
    pub description: String,
    pub asset_type: String,
    /// The asset a read of this hears as its voice.
    pub voice: Option<String>,
    /// `approved`: the sha256 it holds. Its tool and inputs stay in the file, and can show it out of date.
    pub pin: Option<String>,
    /// It has no operation: no `tool` that can make it, or the file says nothing of it.
    pub hollow: bool,
    /// What it reads, in the order its text mentions them.
    pub reads: Vec<usize>,
}

/// A function Calcifer runs itself, free, with no machine.
#[derive(Clone, Copy, PartialEq)]
pub enum Tool {
    Cut,
    Frame,
}

/// project.yaml as Calcifer reads it.
pub struct Doc {
    pub assets: Vec<Asset>,
    /// What each reads.
    pub upstream: Vec<Vec<usize>>,
    /// Each asset after what it reads.
    pub order: Vec<usize>,
    pub warnings: Vec<Warning>,
    index: HashMap<String, usize>,
    /// Each package's own tags, as project.lock last read them. A package not here hasn't been read yet.
    reserved: BTreeMap<String, Vec<String>>,
    /// What each tool makes, as project.lock last read it.
    makes: BTreeMap<String, String>,
    /// Mentions naming nothing and no tag its package is known to have. Until a package is read its tags may be among them, so
    /// nothing is made of these before then.
    unread: Vec<(usize, String)>,
    /// The project's folder, which says what is on this computer.
    dir: PathBuf,
}

/// Why an asset has the state it has, or none.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    /// A hash in the file: the version it is, if one, with its whole file set, and what changed since it was made.
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
pub struct Standing {
    pub state: State,
    /// Its files; None is no state.
    pub files: Option<Files>,
    /// Its key; None while something it reads has no state.
    pub inputs: Option<String>,
    pub reads: Vec<ReadNow>,
}

impl Standing {
    /// The version that is its state, if one is.
    pub fn version(&self) -> Option<u32> {
        match self.state {
            State::Pinned { version, .. } => version,
            State::Current(version) => Some(version),
            _ => None,
        }
    }
}

/// What an operation reads now.
#[derive(Clone)]
pub struct ReadNow {
    /// As it reaches the adapter.
    pub name: String,
    pub asset: usize,
    pub files: Files,
    /// A tool reading a newest version out of date: its number.
    pub newest: Option<u32>,
}

impl ReadNow {
    /// What it reads as: what this version holds.
    fn kind(&self) -> &'static str {
        KINDS.iter().copied().find(|kind| self.files.contains_key(*kind)).unwrap_or_default()
    }
}

impl Doc {
    /// The project as project.yaml says now.
    pub fn load(folder: &Folder) -> Result<Self, Error> {
        Self::read(folder, &folder.text()?)
    }

    /// The project `text` says. Only a file of no format, or another, is refused.
    pub fn read(folder: &Folder, text: &str) -> Result<Self, Error> {
        Self::of(folder, text, &folder.lock()?)
    }

    /// The project `text` says, of the files and tags `lock` holds.
    pub(super) fn of(folder: &Folder, text: &str, lock: &Lock) -> Result<Self, Error> {
        let (value, listed) = protocol::yaml::to_json_and_keys(text, &["assets"], names_a_file).map_err(|why| {
            let (line, written) = at_line(text, &why);
            let refused = invalid("project_yaml", format!("{}: {why}{line}", folder::PROJECT));
            match written.trim_start().split_once(':').is_some_and(|(_, value)| value.trim_start().starts_with('[')) {
                true => refused.remedy("text that starts with `[` goes on the lines under `summary: |` or `description: |`"),
                false => refused,
            }
        })?;
        let top = match value {
            Value::Object(top) => top,
            Value::Null => Map::new(),
            _ => return Err(invalid("project_yaml", format!("{} is no mapping of sections", folder::PROJECT))),
        };
        let version = top.get("version").and_then(Value::as_u64);
        if version.is_some_and(|version| version != FORMAT) || (version.is_none() && top.contains_key("entities")) {
            let said = format!("{} is format {}, and cozy reads format {FORMAT}: it is left as it is", folder::PROJECT, version.unwrap_or(2));
            return Err(invalid("project_format", said).next("cozy project upgrade"));
        }
        let mut warnings = vec![];
        if folder.lock_gone() {
            warnings.push(Warning::of_file(format!("{0} is gone and files/ holds files: their versions aren't read, so only what is approved stands. Put {0} back from git or a backup", folder::LOCK)));
        }
        if folder.lock_set_aside() {
            warnings.push(Warning::of_file(format!("{0} could not be read and is kept as {0}.broken: its versions, seeds and takes aren't read here, so only what is approved stands. Fix it and put it back, or delete it", folder::LOCK)));
        }
        if version.is_none() {
            warnings.push(Warning::of_file(format!("{} has no `version: {FORMAT}`, so it reads as version {FORMAT}: add it", folder::PROJECT)));
        }
        warnings.extend(top.keys().filter(|key| !SECTIONS.contains(&key.as_str())).map(|key| Warning::of_file(format!("{}: cozy reads no section {key}; it is ignored", folder::PROJECT))));
        let empty = Map::new();
        let mut defaults = Map::new();
        for (section, value) in top.get("defaults").and_then(Value::as_object).unwrap_or(&empty) {
            if !KINDS.contains(&section.as_str()) {
                warnings.push(Warning::of_file(format!("defaults.{section}: the only keys are image, video and audio; it is ignored")));
                continue;
            }
            let mut filled = value.as_object().cloned().unwrap_or_default();
            for word in PLATFORM.iter().filter(|word| !MADE.contains(word)) {
                if filled.remove(*word).is_some() {
                    warnings.push(Warning::of_file(format!("defaults.{section}.{word}: defaults fill a node's inputs: it is ignored")));
                }
            }
            defaults.insert(section.clone(), Value::Object(filled));
        }
        let declared = top.get("assets").and_then(Value::as_object).unwrap_or(&empty);
        for name in &listed {
            named(name)?;
        }
        let mut seen: HashMap<String, &String> = HashMap::new();
        for name in &listed {
            if let Some(first) = seen.insert(name_key(name), name) {
                return Err(one_name_twice(text, first, name));
            }
        }
        let mut doc = Self { assets: vec![], upstream: vec![], order: vec![], warnings, index: HashMap::new(), reserved: lock.reserved.clone(), makes: lock.makes.clone(), unread: vec![], dir: folder.dir.clone() };
        for name in &listed {
            let Some(written) = declared.get(name) else { continue };
            doc.declare(name, written, &defaults, folder, lock);
        }
        for i in 0..doc.assets.len() {
            if doc.assets[i].hollow && doc.assets[i].pin.is_none() && doc.assets[i].description.is_empty() && doc.assets[i].text.is_none() {
                let (name, said) = (doc.assets[i].name.clone(), doc.nothing_to_make(i));
                doc.warn(&name, said);
            }
        }
        doc.inline();
        doc.resolve();
        Ok(doc)
    }

    /// An asset of the file, after defaults, what it approved read. A name taken: the first stands.
    fn declare(&mut self, name: &str, written: &Value, defaults: &Map<String, Value>, folder: &Folder, lock: &Lock) {
        let key = name_key(name);
        if key == ALL {
            self.warn(name, format!("{name} is named `{ALL}`: `cozy project render {ALL}` means everything, and makes it with the rest"));
        }
        let body = match written {
            Value::Object(body) => body.clone(),
            Value::Null => Map::new(),
            _ => {
                self.warn(name, format!("{name} is no mapping of keys: it is empty"));
                Map::new()
            }
        };
        let target = text_of(body.get("tool")).trim().to_owned();
        let tool = (!target.is_empty()).then(|| plain_tool(&target, folder));
        let mut pin = None;
        match body.get("approved") {
            None | Some(Value::Null) => {}
            Some(value) => match file_named(&text_of(Some(value)), lock) {
                Ok(sha256) => {
                    if !lock.files.get(&sha256).is_some_and(|file| file.here) {
                        self.warn(name, format!("{name}'s approved: {} is not on this computer: cozy project pull", folder::prefix_of(&sha256)));
                    }
                    pin = Some(sha256);
                }
                Err(why) => self.warn(name, format!("{name}'s approved{why}")),
            },
        }
        if body.contains_key(NOT_READ) {
            self.warn(name, format!("{name}'s kind: what an asset is comes from its tool, or its file: it is ignored"));
        }
        let held = lock.asset(name).and_then(|held| held.versions.last());
        let made = held.and_then(|version| KINDS.iter().copied().find(|kind| version.files.contains_key(*kind)));
        let of_file = pin.as_ref().and_then(|sha256| lock.files.get(sha256)).and_then(folder::File::media);
        let makes = tool.as_ref().and_then(|tool| lock.makes.get(tool)).and_then(|kind| KINDS.iter().copied().find(|known| known == kind));
        let text = (tool.is_none() && pin.is_none() && body.contains_key(TEXT)).then(|| text_of(body.get(TEXT)).trim_end().to_owned());
        let said = tool.as_deref().and_then(kind_made_here).or(of_file).or(made).or(makes).or(text.as_ref().map(|_| TEXT));
        let kind = said.unwrap_or("image").to_owned();
        let mut asset_type = text_of(body.get("asset_type"));
        if !asset_type.is_empty() && !ASSET_TYPES.contains(&asset_type.as_str()) {
            self.warn(name, format!("{name}'s asset_type: {asset_type} isn't {}: it is ignored", ASSET_TYPES.join(", ")));
            asset_type.clear();
        }
        let description = text_of(body.get("description")).trim().to_owned();
        for mention in mentions(&description) {
            self.warn(name, format!("{name}'s description says <{mention}>: a description is words, so it stays as text and nothing is read"));
        }
        let voice = match body.get("voice") {
            None | Some(Value::Null) => None,
            Some(said) => {
                let text = text_of(Some(said));
                match mentions(&text).as_slice() {
                    [one] if text.trim() == format!("<{one}>") => Some((*one).to_owned()),
                    _ => {
                        self.warn(name, format!("{name}'s voice: write `voice: <name>`, an asset: it is ignored"));
                        None
                    }
                }
            }
        };
        let own: BTreeSet<String> = body.keys().filter(|key| !PLATFORM.contains(&key.as_str()) && *key != NOT_READ).cloned().collect();
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
                field if PLATFORM.contains(&field) || field == NOT_READ || (field == TEXT && text.is_some()) => {}
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
        self.assets.push(Asset { name: name.to_owned(), kind, tool_makes: makes.is_some(), tool, target, made, inputs, written: None, inlined: vec![], text, own, description, asset_type, voice, pin, hollow, reads: vec![] });
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
                said.push(Warning::of(&asset.name, format!("{0} is a tag of {tool}, so a text asset can't go by it: cozy project rename {0} <another name>", asset.name)));
            }
        }
        for i in 0..n {
            let asset = &self.assets[i];
            let tags = self.tags(i);
            for tag in tags.into_iter().flatten().filter(|tag| self.find(tag).is_some()) {
                let line = Warning::of(&asset.name, format!("{tag} is also a tag of {}: <{tag}> in its text is that tag, not this: cozy project rename {tag} <another name>", self.target(i)));
                if !said.contains(&line) {
                    said.push(line);
                }
            }
            if let Some(voice) = asset.voice.as_deref().filter(|voice| self.find(voice).is_none()) {
                said.push(Warning::of(&asset.name, format!("{}'s voice: <{voice}> is nothing in the project: add an asset named {voice}", asset.name)));
            }
            for mention in mentions_in(&Value::Object(asset.inputs.clone())) {
                // A tag of its package, in any case, stays text as written, whatever the project names.
                if tags.is_some_and(|tags| is_tag(tags, &mention)) {
                    continue;
                }
                let Some(j) = self.find(&mention) else {
                    unread.push((i, mention));
                    continue;
                };
                // Left by a cycle of text assets.
                if self.assets[j].text.is_some() {
                    continue;
                }
                if j != i && !upstream[i].contains(&j) {
                    upstream[i].push(j);
                    reads[i].push(j);
                }
            }
        }
        // What it reads brings its voice, which isn't what it is made of, so it is the last edge to go in a cycle.
        let mut fixed = vec![vec![]; n];
        for i in 0..n {
            for &j in &reads[i] {
                if let Some(voice) = self.voice_of(j).filter(|voice| *voice != i && !upstream[i].contains(voice)) {
                    upstream[i].push(voice);
                    fixed[i].push(voice);
                }
            }
        }
        let graph = Graph::new(upstream, &fixed);
        for &(reader, read) in &graph.closing {
            reads[reader].retain(|j| *j != read);
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

    pub fn find(&self, name: &str) -> Option<usize> {
        self.index.get(&name_key(name)).copied()
    }

    fn voice_of(&self, i: usize) -> Option<usize> {
        self.find(self.assets[i].voice.as_deref()?)
    }

    fn warn(&mut self, name: &str, message: String) {
        self.warnings.push(Warning::of(name, message));
    }

    /// Asset `i`'s inputs as its function is given them: a `|` text's last newline is the file's, not anyone's words.
    pub fn inputs_of(&self, i: usize) -> Map<String, Value> {
        let mut inputs = self.assets[i].inputs.clone();
        inputs.values_mut().for_each(end_trimmed);
        inputs
    }

    /// What asset `i` says, flat, by each control's `field`: its inputs, defaults too, and the platform's words. An edit's tracks are the wire's `timeline`.
    pub fn flat_fields(&self, i: usize) -> Map<String, Value> {
        let asset = &self.assets[i];
        let mut flat = asset.written.clone().unwrap_or_else(|| self.inputs_of(i));
        flat.values_mut().for_each(end_trimmed);
        flat.extend(asset.made.iter().map(|(field, value)| (field.clone(), value.clone())));
        flat.extend(asset.text.iter().map(|text| (TEXT.to_owned(), json!(text))));
        let words = [("description", asset.description.clone()), ("asset_type", asset.asset_type.clone()), ("voice", asset.voice.as_ref().map_or(String::new(), |voice| format!("<{voice}>")))];
        flat.extend(words.into_iter().filter(|(_, text)| !text.is_empty()).map(|(field, text)| (field.to_owned(), json!(text))));
        if let Some(tracks) = flat.remove("tracks") {
            let timeline = Timeline { tracks: vec![Track { clips: super::tools::clips_of(&tracks) }] };
            flat.insert("timeline".to_owned(), serde_json::to_value(timeline).expect("clips serialize"));
        }
        flat
    }

    /// What asset `i` reads by a mention, each with the places its text says so.
    pub fn reads_of(&self, i: usize) -> Vec<Read> {
        let asset = &self.assets[i];
        let mut said = vec![];
        for (field, value) in asset.inputs.iter().chain(asset.written.iter().flatten()) {
            said_in(value, field.clone(), &mut said);
        }
        said.sort();
        said.dedup();
        let read = |&j: &usize| {
            let key = name_key(&self.assets[j].name);
            let at = said.iter().filter(|(mention, _)| name_key(mention) == key).map(|(_, place)| place.clone());
            Read { name: self.assets[j].name.clone(), at: at.collect() }
        };
        asset.reads.iter().chain(&asset.inlined).map(read).collect()
    }

    /// The tags of the function that makes asset `i`, once it has been read.
    pub fn tags(&self, i: usize) -> Option<&Vec<String>> {
        self.reserved.get(self.assets[i].tool.as_deref().unwrap_or_default())
    }

    /// What is said of a name that is nothing in the project, and no tag of `tags`.
    fn no_longer(&self, name: &str, tags: &[String]) -> String {
        match self.near(name, tags) {
            Some(close) => format!("<{name}> is no longer in the project: did you mean <{close}>?"),
            None => format!("<{name}> is no longer in the project"),
        }
    }

    /// Why asset `i` can't run, given its package's `tags`: each `<name>` in its text that names nothing and is no tag. Running it would
    /// send its words to a model with no picture, and charge for that. None: it can.
    pub fn missing_inputs(&self, i: usize, tags: &[String]) -> Option<String> {
        let mut said: Vec<String> = vec![];
        self.mentions_of_nothing(i, tags).map(|mention| self.no_longer(mention, tags)).for_each(|line| if !said.contains(&line) { said.push(line) });
        (!said.is_empty()).then(|| format!("{}: {}", self.assets[i].name, said.join("; ")))
    }

    /// The names in `missing_inputs`, each once.
    pub fn missing_names(&self, i: usize, tags: &[String]) -> Vec<String> {
        let mut names: Vec<String> = vec![];
        self.mentions_of_nothing(i, tags).for_each(|name| if !names.iter().any(|there| there == name) { names.push(name.to_owned()) });
        names
    }

    fn mentions_of_nothing<'a>(&'a self, i: usize, tags: &'a [String]) -> impl Iterator<Item = &'a str> {
        self.unread.iter().filter(move |(asset, mention)| *asset == i && !is_tag(tags, mention)).map(|(_, mention)| mention.as_str())
    }

    /// What is said ahead of a run of asset `i`, given its package's `tags`. A pin runs nothing, so it says nothing, whatever its text says.
    pub fn said_once_read(&self, i: usize, tags: &[String]) -> Option<Warning> {
        let said = self.missing_inputs(i, tags).filter(|_| self.assets[i].pin.is_none())?;
        Some(Warning::of(&self.assets[i].name, said))
    }

    /// `said_once_read` of each asset whose package project.lock knows.
    pub fn said_of_known(&self) -> Vec<Warning> {
        (0..self.assets.len()).filter(|&i| self.makes(i) && self.tool(i).is_none()).filter_map(|i| self.said_once_read(i, self.tags(i)?)).collect()
    }

    /// The name `mention`, which names nothing, is likely a typo of: within two edits, neither of them short, and no tag of `tags`.
    fn near(&self, mention: &str, tags: &[String]) -> Option<&str> {
        const SHORTEST: usize = 4;
        let key = name_key(mention);
        let names = self.assets.iter().filter(|asset| !is_tag(tags, &asset.name)).map(|asset| (name_key(&asset.name), asset.name.as_str()));
        let close = names.filter(|(other, _)| key.chars().count() >= SHORTEST && other.chars().count() >= SHORTEST).map(|(other, name)| (edits(&key, &other), name));
        close.filter(|(apart, _)| *apart <= 2).min_by_key(|(apart, _)| *apart).map(|(_, name)| name)
    }

    /// The function that makes asset `i`, as the file writes it.
    pub fn target(&self, i: usize) -> String {
        self.assets[i].target.clone()
    }

    /// Asset `i`'s operation when Calcifer runs it itself: a function of the reserved package `cozy`.
    pub fn tool(&self, i: usize) -> Option<Tool> {
        match self.assets[i].tool.as_deref()? {
            EDIT => Some(Tool::Cut),
            FRAME => Some(Tool::Frame),
            _ => None,
        }
    }

    /// The one line that says why asset `i` has no operation.
    pub fn nothing_to_make(&self, i: usize) -> String {
        let asset = &self.assets[i];
        match asset.tool.as_deref() {
            None if asset.own.is_empty() => format!("{} says nothing: it has nothing to make", asset.name),
            None => format!("{} has no `tool`: it has nothing to make", asset.name),
            Some(tool) => format!("{}: {COZY} makes {EDIT} and {FRAME}, not {tool}: it has nothing to make", asset.name),
        }
    }

    /// Whether asset `i` has an operation that can make it.
    pub fn makes(&self, i: usize) -> bool {
        !self.assets[i].hollow
    }

    /// Every asset's state, in the file's terms.
    /// Whether this was read with the package tags `lock` knows now.
    pub fn knows_tags_of(&self, lock: &Lock) -> bool {
        self.reserved == lock.reserved && self.makes == lock.makes
    }

    pub fn stand(&self, lock: &Lock) -> Vec<Standing> {
        let mut done = vec![Standing::default(); self.assets.len()];
        for &i in &self.order {
            done[i] = self.standing(i, lock, &done);
        }
        done
    }

    /// Each asset's state with these versions, by name key, made its state: the key they stand on now written on each. Only a change of format does this.
    pub(super) fn stamped(&self, lock: &mut Lock, current: &BTreeMap<String, u32>) -> Vec<Standing> {
        let mut done = vec![Standing::default(); self.assets.len()];
        for &i in &self.order {
            let name = &self.assets[i].name;
            let now = self.standing(i, lock, &done);
            if let Some(number) = current.get(&name_key(name)).copied()
                && let Some(version) = lock.asset_mut(name, &self.assets[i].kind).versions.iter_mut().find(|version| version.number == number)
            {
                version.inputs = now.inputs.unwrap_or_default();
            }
            done[i] = self.standing(i, lock, &done);
        }
        done
    }

    /// The hash in the file, else its newest version for these inputs, else none. A hash that is a version's main file brings
    /// the version's whole file set; one that is none's is just that file.
    pub fn standing(&self, i: usize, lock: &Lock, done: &[Standing]) -> Standing {
        let asset = &self.assets[i];
        let held = lock.asset(&asset.name);
        let reads = self.reading(i, lock, done);
        let inputs = reads.as_ref().map(|reads| self.key(i, reads));
        let reads = reads.unwrap_or_default();
        let stand = |state, files| Standing { state, files, inputs: inputs.clone(), reads: reads.clone() };
        let versions = held.map(|held| held.versions.as_slice()).unwrap_or_default();
        let stands = |version: &Version, now: &str| !version.inputs.is_empty() && self.moved(i, &version.inputs, now, &reads).is_empty();
        if let Some(sha256) = &asset.pin {
            return match versions.iter().rev().find(|version| version.main() == Some(sha256)) {
                Some(version) => {
                    let changed = match &inputs {
                        Some(now) if !version.inputs.is_empty() => self.moved(i, &version.inputs, now, &reads),
                        _ => vec![],
                    };
                    stand(State::Pinned { version: Some(version.number), changed }, Some(version.files.clone()))
                }
                None => stand(State::Pinned { version: None, changed: vec![] }, Some(Files::from([(asset.kind.clone(), sha256.clone())]))),
            };
        }
        if asset.hollow && (!asset.description.is_empty() || asset.text.is_some()) {
            return stand(State::Words, Some(Files::new()));
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
    /// neither it skips, and with nothing to read at all, it waits. Each read holds its voice, as `voice`.
    fn reading(&self, i: usize, lock: &Lock, done: &[Standing]) -> Option<Vec<ReadNow>> {
        let tool = self.tool(i).is_some();
        let mut reads = vec![];
        for &j in &self.assets[i].reads {
            let name = self.assets[j].name.clone();
            match &done[j].files {
                Some(files) => {
                    let mut files = files.clone();
                    if let Some(voice) = self.voice_of(j).filter(|voice| !tool && self.upstream[i].contains(voice)) {
                        files.extend(folder::main_of(done[voice].files.as_ref()?).map(|main| ("voice".to_owned(), main.clone())));
                    }
                    reads.push(ReadNow { name, asset: j, files, newest: None });
                }
                None if tool => {
                    let newest = lock.asset(&self.assets[j].name).and_then(|held| held.versions.last());
                    reads.extend(newest.map(|newest| ReadNow { name, asset: j, files: newest.files.clone(), newest: Some(newest.number) }));
                }
                None => return None,
            }
        }
        (!tool || !reads.is_empty()).then_some(reads)
    }

    /// Its key: eight hex digits of what makes it and its inputs, then twelve for each thing it reads (`read_part`), in name order, so that a
    /// read gone hides no change in the others.
    fn key(&self, i: usize, reads: &[ReadNow]) -> String {
        let asset = &self.assets[i];
        let mut own = asset.made.clone();
        if let Some(tool) = &asset.tool {
            own.insert("tool".into(), json!(tool));
        }
        // A tool's key holds what it reads of its inputs.
        let tool = self.tool(i).is_some();
        let inputs: Map<String, Value> = (asset.written.as_ref().unwrap_or(&asset.inputs).iter())
            .filter(|(field, _)| !tool || TOOL_INPUTS.contains(&field.as_str()))
            .map(|(field, value)| (field.clone(), value.clone()))
            .collect();
        own.insert("inputs".into(), Value::Object(inputs));
        let parts: String = self.parts(i, reads).into_iter().map(|(_, part)| part).collect();
        format!("{}{parts}", &digest(&Value::Object(own))[..8])
    }

    /// Each thing asset `i` takes in, by name, as its key holds it: what it reads, and each text asset's words.
    fn parts(&self, i: usize, reads: &[ReadNow]) -> Vec<(String, String)> {
        let tool = self.tool(i).is_some();
        let read = reads.iter().map(|read| (read.name.clone(), self.read_part(tool, read)));
        let text = self.assets[i].inlined.iter().map(|&j| &self.assets[j]).map(|of| (of.name.clone(), part(&of.name, &json!({"text": of.text}))));
        let mut parts: Vec<(String, String)> = read.chain(text).collect();
        parts.sort();
        parts
    }

    /// A read in a key: four hex digits of its name and eight of what reaches a reader, so editing an approved asset's instructions leaves what
    /// reads it as it is. A tool's holds only files: same takes, same key, whatever their text says.
    fn read_part(&self, tool: bool, read: &ReadNow) -> String {
        let of = &self.assets[read.asset];
        let files: Vec<_> = read.files.values().collect();
        let held = match tool {
            true => json!({"files": files}),
            false => json!({"asset_type": of.asset_type, "description": of.description, "files": files}),
        };
        part(&read.name, &held)
    }

    /// What moved since a version was made at key `then`: its inputs, or each thing it takes in that changed. One `then` holds that's gone is no
    /// change, as nothing could make it again.
    fn moved(&self, i: usize, then: &str, now: &str, reads: &[ReadNow]) -> Vec<String> {
        if then == now {
            return vec![];
        }
        if own_part(then) != own_part(now) {
            return vec!["its inputs".to_owned()];
        }
        let held: Vec<&[u8]> = then.as_bytes().get(OWN_PART..).unwrap_or_default().chunks_exact(READ_PART).collect();
        self.parts(i, reads).into_iter().filter(|(_, part)| !held.contains(&part.as_bytes())).map(|(name, _)| format!("<{name}>")).collect()
    }

    /// The operation asset `i` is, as its adapter takes it: the function its tool names, its inputs, and what it reads with what is known of
    /// each file.
    pub fn operation(&self, i: usize, function: &str, reads: &[ReadNow], lock: &Lock, keep: Option<Value>, stand: &[Standing]) -> Value {
        let asset = &self.assets[i];
        let file = |sha256: &String| lock.files.get(sha256).map_or_else(|| json!({"sha256": sha256}), folder::File::record);
        let reads: Vec<Value> = reads
            .iter()
            .map(|read| {
                let of = &self.assets[read.asset];
                let files: Map<String, Value> = read.files.iter().map(|(name, sha256)| (name.clone(), file(sha256))).collect();
                let made = stand[read.asset].version().or(read.newest).and_then(|number| lock.asset(&of.name)?.version(number));
                let made_by = made.map_or("", |made| made.tool.as_deref().unwrap_or(&of.target));
                json!({"name": read.name, "kind": read.kind(), "asset_type": of.asset_type, "description": of.description, "files": files, "inputs": for_readers(&of.inputs), "made_by": made_by})
            })
            .collect();
        json!({"version": FORMAT, "asset": asset.name, "kind": asset.kind, "function": function, "inputs": self.inputs_of(i), "reads": reads, "keep": keep})
    }

    /// `--take`, `--in`: what its adapter is told of the take a new one keeps, its files as records; None if it holds no video.
    pub fn keep(&self, i: usize, lock: &Lock, keeps: Keeps) -> Option<Value> {
        let held = lock.asset(&self.assets[i].name)?.version(keeps.take)?;
        let video = lock.files.get(held.files.get("video")?)?;
        let latents = held.files.get("latents").and_then(|sha256| lock.files.get(sha256));
        let ends_s = if video.fps > 0.0 { video.frames as f64 / video.fps } else { 0.0 };
        let made_by = held.tool.as_deref().unwrap_or(&self.assets[i].target);
        Some(json!({"take": keeps.take, "video": video.record(), "latents": latents.map(folder::File::record), "at_s": keeps.in_s, "ends_s": ends_s, "made_by": made_by}))
    }
}

/// What of a read crosses its mention beside its description: how to take the file. What made it is for its maker alone.
fn for_readers(inputs: &Map<String, Value>) -> Map<String, Value> {
    inputs.iter().filter(|(field, _)| FOR_READERS.contains(&field.as_str())).map(|(field, value)| (field.clone(), value.clone())).collect()
}

/// `tool` as the key holds it: no release, no root namespace, and local code by its directory's name, with the function it names, so moving
/// from a checkout to the published package puts nothing out of date.
pub fn plain_tool(tool: &str, folder: &Folder) -> String {
    let with = tool.trim();
    if protocol::target::written_as_path(with) || with.ends_with(".py") {
        let dir_name = |path: &str| {
            let name = path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(path).to_owned();
            name.strip_suffix(".py").map(str::to_owned).unwrap_or(name)
        };
        let home = platform::paths::login_home().unwrap_or_default();
        return match crate::local_code::target(with, &folder.dir, &home) {
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
fn end_trimmed(value: &mut Value) {
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

/// `name`, refused unless it can be a name: the file is wrong, and nothing is rendered from it.
pub fn named(name: &str) -> Result<(), Error> {
    if is_name(name) {
        return Ok(());
    }
    let why = match name.contains([' ', ',']) {
        true => "names can't hold spaces or commas",
        false => "names hold only letters, digits, `_`, `-` and `.`",
    };
    Err(invalid("project_name", format!("`{name}`: {why}; use `{}`", as_name(name))))
}

/// The sha256 a file pin's value names, or how it fails, to follow "{name}'s {field}".
fn file_named(written: &str, lock: &Lock) -> Result<String, String> {
    if written.is_empty() || !written.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(" is not a hash: {written} is not read: `cozy project import {}` adds it to the project", quoted(written)));
    }
    match lock.matching(&written.to_ascii_lowercase()).as_slice() {
        [file] => Ok(file.sha256.clone()),
        [] => Err(format!(": no file's hash starts with {written}: `cozy project import <file>` adds one to the project")),
        several => Err(format!(": {written} fits {} files: write more of its hash", several.len())),
    }
}

/// `text` as one word on a command line.
fn quoted(text: &str) -> String {
    match text.bytes().any(|byte| byte.is_ascii_whitespace() || b"'\"$&;|<>()*?#`".contains(&byte)) {
        true => format!("'{}'", text.replace('\'', r"'\''")),
        false => text.to_owned(),
    }
}

/// Whether a scalar at this path names a file, so that a number's shape is a hash's prefix by its text: an asset's `approved`, and a file
/// input it writes. `12345678e012` isn't 1.2345678e19, and `012345678901` isn't 12345678901, which is another file's prefix.
pub(super) fn names_a_file(path: &[&str]) -> bool {
    matches!(path, ["assets", .., "approved" | "image" | "video" | "audio"])
}

/// A new empty project's project.yaml.
pub fn blank() -> String {
    format!("version: {FORMAT}\n")
}

/// `text` as it calls something new, if it can: through `as_name`. A `.` is a character like any other.
pub fn plain_name(text: &str) -> Option<String> {
    let name = as_name(text);
    (!name_key(&name).is_empty()).then_some(name)
}

/// `wanted`, or with `_2`, `_3`, … if something in the project goes by it, or versions wait in project.lock for it.
pub fn free_name(doc: &Doc, lock: &Lock, wanted: &str) -> String {
    let taken = |name: &str| name_key(name) == ALL || doc.find(name).is_some() || lock.asset(name).is_some_and(|held| !held.versions.is_empty());
    if !taken(wanted) {
        return wanted.to_owned();
    }
    (2..).map(|n| format!("{wanted}_{n}")).find(|name| !taken(name)).expect("a free name")
}

/// What a person who typed `name` is told when the project has it.
pub fn name_taken(doc: &Doc, name: &str) -> Error {
    let there = doc.find(name).map_or(name, |i| doc.assets[i].name.as_str());
    Error::new(Kind::Conflict, "asset_exists", format!("{there} is in the project already")).remedy(format!("use another name, or change it: cozy project set {there}"))
}

/// Whether a text asset can go by `name`: not a tag of a package this project uses.
pub fn refuse_tag(lock: &Lock, name: &str) -> Result<(), Error> {
    match lock.reserved.iter().find(|(_, tags)| is_tag(tags, name)) {
        Some((tool, _)) => Err(Error::new(Kind::Validation, "asset_name_reserved", format!("{name} is a tag of {tool}: <{name}> in a prompt means that")).remedy("call it something else")),
        None => Ok(()),
    }
}

/// `name` as a key of a mapping: quoted where YAML in general would read it as something other than this text.
pub fn yaml_key(name: &str) -> String {
    match protocol::yaml::to_json(name) {
        Ok(Value::String(read)) if read == name && name.parse::<f64>().is_err() => name.to_owned(),
        _ => format!("\"{name}\""),
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

/// One thing a key holds: four hex digits of its name and eight of `held`.
fn part(name: &str, held: &Value) -> String {
    format!("{}{}", &digest(&json!(name_key(name)))[..4], &digest(held)[..8])
}

/// `first` and `second` as the file spells them: one name, as names match by `name_key`, with where each is written.
fn one_name_twice(text: &str, first: &str, second: &str) -> Error {
    let lines = |name: &str| -> Vec<usize> {
        let quoted = format!("\"{name}\"");
        let writes = |line: &str| [name, quoted.as_str()].iter().any(|key| line.trim_start().strip_prefix(*key).is_some_and(|rest| rest.trim_start().starts_with(':')));
        let at = |line: &str| line.len() - line.trim_start().len();
        let under = text.lines().skip_while(|line| !line.starts_with("assets:")).skip(1).find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#')).map_or(2, at);
        text.lines().enumerate().filter(|(_, line)| at(line) == under && writes(line)).map(|(n, _)| n + 1).collect()
    };
    let (at, then) = (lines(first), lines(second));
    let one = at.first().copied().unwrap_or(0);
    let other = then.iter().copied().find(|line| *line != one).unwrap_or(one);
    let said = format!("{first}, on line {one}, and {second}, on line {other}, are one name: a name matches in any case, with `_` and `-` alike");
    invalid("project_name_twice", said).remedy(format!("rename one of them in {}", folder::PROJECT))
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

/// The line a YAML error names (`… line 4 column 4 …`) as it is written: said after the error, and alone.
fn at_line<'a>(text: &'a str, why: &str) -> (String, &'a str) {
    let number = why.split("line ").nth(1).and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next()).and_then(|n| n.parse::<usize>().ok());
    let Some(written) = number.and_then(|number| text.lines().nth(number.saturating_sub(1))) else { return (String::new(), "") };
    (format!(": `{}`", written.trim()), written)
}
