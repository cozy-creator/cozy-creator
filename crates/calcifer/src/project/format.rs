//! project.yaml, format 2 (design/project-graph.md): assets, the operation that makes each (a package, by `with`, and a `kind`,
//! which its adapter turns into calls), the memo key, and what each asset's state is. The platform reads `with`, `kind`, `models`,
//! `lora`, `options`, a hash in `image`, `video` or `audio`, a beat's `writes` and `duration`, and `<name>` in any text. It knows
//! no kind's meaning and no field's shape, but for the reserved package `cozy`, which it runs itself. Every other field passes through to the adapter and counts in the
//! key. Nothing refuses a project but a file of no format: what one line gets wrong is said, and that line does the best it can.
use std::collections::{BTreeMap, BTreeSet, HashMap};

use protocol::error::{Error, Kind};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    folder::{self, Files, Folder, Keeps, Lock},
    graph::{Graph, mentions, mentions_in, name_key},
};

/// The name `cozy project render all` means.
pub const ALL: &str = "all";
const FORMAT: u64 = 2;
const PINS: [&str; 3] = ["image", "video", "audio"];
const RESERVED: [&str; 9] = ["with", "kind", "models", "lora", "options", "image", "video", "audio", "writes"];
const SECTIONS: [&str; 3] = ["version", "defaults", "assets"];
/// The reserved package Calcifer runs itself.
const COZY: &str = "cozy";
/// What `cozy` reads of a cut or a frame.
const TOOL_FIELDS: [&str; 6] = ["pieces", "fps", "aspect_ratio", "resolution", "video", "at"];
/// How long a beat with no `duration` lasts, where its writes are concerned; an adapter says the same.
const BEAT_S: f64 = 5.0;

/// A new project's project.yaml: a short film that renders as it stands. Cozy never writes it again.
pub fn starter() -> String {
    r#"# A short film, ready to render: cozy project render all --on <machine>
# Change anything. Each asset says what it is; `<name>` in any text reads that asset.
version: 2
defaults:
  all:
    style: High-quality Unreal Engine graphics in a bright, hand-painted, cel-shaded anime style.
    aspect_ratio: "16:9"
  character:
    with: qwen-image-2
  environment:
    with: qwen-image-2
  setup:
    with: qwen-image-2
  shot:
    with: minimax-h3
  edit:
    with: cozy

assets:
  kyogre:
    kind: character
    description: |
      Kyogre, the colossal blue whale-like legendary Pokémon with huge fins, no legs, and glowing red lines.
    instructions: |
      The whole body from the front on the left, with nothing added to the body described and no ground under it. On the right, the same character has been spun a half turn, at the same size, so its back faces the viewer and nothing on its front can be seen. Two separate views, with no text.

  groudon:
    kind: character
    description: |
      Groudon, the colossal red legendary Pokémon with grey stone plates and huge claws.
    instructions: |
      The whole body from the front on the left, with nothing added to the body described and no ground under it. On the right, the same character has been spun a half turn, at the same size, so its back faces the viewer and nothing on its front can be seen. Two separate views, with no text.

  coast:
    kind: environment
    description: A volcanic coast under a storm sky, with a dark sea on the left, lava cliffs on the right, and a small white crater city far away at the center.

  coast wide:
    kind: setup
    camera: <coast>. The camera stands high and far back, with the sea on the left, the cliffs on the right, and the crater city small at the center.

  awaken:
    kind: shot
    summary: |
      <kyogre> bursts out of the stormy sea of <coast>.
    soundscape: Howling wind, crashing waves, and rolling thunder.
    beats:
      - duration: 6
        description: |
          A wide shot from the water's edge looks out over the stormy sea of <coast>, with lightning flickering behind the clouds. The water bulges at the center of the frame, and <kyogre> bursts out of it, facing right, with sheets of white water pouring off its fins. The camera tilts up at slow speed as heavy rain starts to fall.

  erupt:
    kind: shot
    summary: |
      <groudon> claws its way out of the lava cliffs of <coast> and roars.
    soundscape: Cracking rock, hissing lava, heavy footfalls, and a deep roar.
    beats:
      - duration: 6
        description: |
          A wide shot from the rocks looks up at the lava cliffs of <coast>. The rock face cracks at the center of the frame, and <groudon> claws its way out, lava sliding off its stone plates in glowing strings. It rears up and roars toward the left. The camera pushes in at slow speed and shakes with each step.

  clash:
    kind: shot
    summary: |
      <kyogre> and <groudon> fight on <coast wide> in one continuous shot with no cuts.
    soundscape: Crashing surf, roaring fire, bursts of steam, and deep impacts.
    beats:
      - duration: 14
        description: |
          A wide shot shows <kyogre> in the surf on the left, facing right, and <groudon> on the rocks on the right, facing left. <kyogre> surges forward and fires a thick column of water across the frame. It bursts into white steam against the stone plates of <groudon>.
      - duration: 10
        description: |
          <groudon> breathes a wide torrent of orange fire across the surf, and <kyogre> erupts from the water at the shoreline. The two crash together chest to chest, and a wall of water and steam rushes toward the camera.

  edit:
    kind: edit
    fps: 24
    pieces:
      - <awaken>
      - <erupt>
      - <clash>
"#
    .into()
}

/// One asset as the file says it, after defaults.
pub struct Asset {
    pub name: String,
    pub kind: String,
    pub body: Map<String, Value>,
    /// What passes to the adapter: every field but the platform's, and a file field holding a mention.
    pub fields: Map<String, Value>,
    /// The fields it wrote itself.
    pub own: BTreeSet<String>,
    /// A pin, the line `approve` writes: its field and sha256. Its `with` and text stay in the file, ignored while it stands.
    pub pin: Option<(&'static str, String)>,
    /// A constant's other hashes (a character's `image` and `audio`): its files with the pin's, when the pin is no version's.
    pub also: Files,
    /// It has no operation: no `with`, of its own or from defaults, or no `kind` for its package's adapter.
    pub hollow: bool,
    /// A free frame a beat names: `cozy` makes it, and it stays the world state it is to what reads it.
    pub frame: bool,
    /// What it reads, in the order its text mentions them.
    pub reads: Vec<usize>,
    /// The revisions it sends to its function.
    pub writes: Vec<Write>,
    /// The asset whose function sends this revision: its state comes with that one's take.
    pub writer: Option<usize>,
}

/// A revision of a series an operation's text names.
#[derive(Clone)]
pub struct Write {
    pub key: String,
    pub name: String,
    /// Where on its shot's timeline: its beat's start, and `at` into it.
    pub at_s: f64,
    pub fields: Map<String, Value>,
}

impl Write {
    fn record(&self) -> Value {
        json!({"name": self.name, "at_s": self.at_s, "fields": self.fields})
    }
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
    /// What each reads, a revision its writer too.
    pub upstream: Vec<Vec<usize>>,
    /// Each asset after what it reads.
    pub order: Vec<usize>,
    pub warnings: Vec<String>,
    index: HashMap<String, usize>,
    /// How many the file declares, in order; those after them are revisions only a beat names.
    declared: usize,
}

/// Why an asset has the state it has, or none.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    /// A hash in the file: the version it is, if one, with its whole file set, and what changed since it was made.
    Pinned { version: Option<u32>, changed: Vec<String> },
    /// This version has these inputs.
    Current(u32),
    /// This take of its writer wrote it.
    Written { by: String, version: u32 },
    Unwritten { by: String },
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
    /// The key's inputs; None while something it reads has no state.
    pub inputs: Option<String>,
    pub digests: BTreeMap<String, String>,
    pub reads: Vec<ReadNow>,
}

impl Standing {
    /// The version that is its state, if one is.
    pub fn version(&self) -> Option<u32> {
        match self.state {
            State::Pinned { version, .. } => version,
            State::Current(version) | State::Written { version, .. } => Some(version),
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

impl Doc {
    /// The project as project.yaml says now.
    pub fn load(folder: &Folder) -> Result<Self, Error> {
        Self::read(folder, &folder.text()?)
    }

    /// The project `text` says. Only a file of no format, or an older one, is refused.
    pub fn read(folder: &Folder, text: &str) -> Result<Self, Error> {
        let value = protocol::yaml::to_json(text).map_err(|why| {
            invalid("project_yaml", format!("{}: {why}", folder::PROJECT))
                .remedy("text that starts with `[` goes on the lines under `summary: |` or `description: |`")
        })?;
        let top = match value {
            Value::Object(top) => top,
            Value::Null => Map::new(),
            _ => return Err(invalid("project_yaml", format!("{} is no mapping of sections", folder::PROJECT))),
        };
        let old = top.get("assets").is_some_and(Value::is_array)
            || ["entities", "shots", "world_states"].iter().any(|key| top.contains_key(*key))
            || top.get("version").is_some_and(|version| version.as_u64() == Some(1));
        if old {
            let said = format!("{} is format 1, and cozy reads format 2: it is left as it is", folder::PROJECT);
            return Err(invalid("project_format", said).remedy("move it to `assets:` (design/project-graph), or start from `cozy project new`"));
        }
        let mut warnings = vec![];
        if top.get("version").and_then(Value::as_u64) != Some(FORMAT) {
            warnings.push(format!("{} has no `version: {FORMAT}`, so it reads as version {FORMAT}: add it", folder::PROJECT));
        }
        warnings.extend(top.keys().filter(|key| !SECTIONS.contains(&key.as_str())).map(|key| format!("{}: cozy reads no section {key}; it is ignored", folder::PROJECT)));
        let empty = Map::new();
        let defaults = top.get("defaults").and_then(Value::as_object).unwrap_or(&empty);
        let declared = top.get("assets").and_then(Value::as_object).unwrap_or(&empty);
        let mut lock = folder.lock()?;
        folder.measure(&mut lock)?;
        let mut doc = Self { assets: vec![], upstream: vec![], order: vec![], warnings, index: HashMap::new(), declared: 0 };
        for name in protocol::yaml::keys(text, &["assets"]) {
            let Some(written) = declared.get(&name) else { continue };
            doc.declare(text, &name, written, defaults, folder, &lock);
        }
        doc.declared = doc.assets.len();
        for i in 0..doc.declared {
            if doc.assets[i].body.contains_key("writes") {
                doc.warnings.push(format!("{}'s writes: go on the beat that writes them, each `scene` with an `at` into that beat: these are ignored", doc.assets[i].name));
            }
            let mut said = vec![];
            let writes = beat_writes(&doc.assets[i].name, &doc.assets[i].body, &mut said);
            doc.warnings.append(&mut said);
            for (beat, scene, at) in writes {
                doc.write(i, beat, &scene, at);
            }
        }
        for i in 0..doc.assets.len() {
            if doc.assets[i].hollow && doc.assets[i].pin.is_none() {
                let said = doc.nothing_to_make(i);
                doc.warnings.push(said);
            }
        }
        doc.resolve();
        Ok(doc)
    }

    /// An asset of the file, after defaults, its pin read. A name taken: the first stands.
    fn declare(&mut self, text: &str, name: &str, written: &Value, defaults: &Map<String, Value>, folder: &Folder, lock: &Lock) {
        let key = name_key(name);
        if self.index.contains_key(&key) {
            self.warnings.push(format!("two assets are named {name}: the first stands"));
            return;
        }
        if key == ALL {
            self.warnings.push(format!("{name} is named `{ALL}`: `cozy project render {ALL}` means everything, and makes it with the rest"));
        }
        let body = match written {
            Value::Object(body) => body.clone(),
            Value::Null => Map::new(),
            _ => {
                self.warnings.push(format!("{name} is no mapping of fields: it is empty"));
                Map::new()
            }
        };
        let own: BTreeSet<String> = body.keys().cloned().collect();
        // An asset with an operation and no kind is an image. A constant, a hash and nothing to make, needs none.
        let constant = PINS.iter().any(|field| matches!(body.get(*field), Some(Value::Number(_))) || body.get(*field).and_then(Value::as_str).is_some_and(|text| mentions(text).is_empty()));
        let kind = match text_of(body.get("kind")) {
            kind if kind.is_empty() && !constant => "image".to_owned(),
            kind => kind,
        };
        // An asset naming its own function takes nothing from defaults, so a clean doesn't re-run over a style edit.
        let mut merged = Map::new();
        if !body.contains_key("with") {
            for section in ["all", kind.as_str()] {
                if let Some(section) = defaults.get(section).and_then(Value::as_object) {
                    merged.extend(section.clone());
                }
            }
        }
        merged.extend(body);
        let (mut pin, mut also) = (None, Files::new());
        for field in PINS {
            let Some(value) = merged.get(field) else { continue };
            let written = match value {
                Value::String(text) if !mentions(text).is_empty() => continue,
                Value::String(text) => text.clone(),
                Value::Number(number) => number.to_string(),
                _ => continue,
            };
            match file_named(&written, lock) {
                Ok(sha256) => {
                    let here = lock.files.get(&sha256).is_some_and(|file| folder.path(&file.path).is_file());
                    if !here {
                        self.warnings.push(format!("{name}'s {field}: {} is not on this computer: cozy project pull", folder::prefix_of(&sha256)));
                    }
                    match pin {
                        None => pin = Some((field, sha256)),
                        Some(_) => drop(also.insert(field.to_owned(), sha256)),
                    }
                }
                Err(why) => {
                    self.warnings.push(format!("{name}'s {field}{why}"));
                    merged.remove(field);
                }
            }
        }
        let mut fields = fields_of(&merged);
        // A pin over an input leaves the input in a comment on its line: while pinned it still counts, so the pin shows no false change.
        if let Some((field, _)) = &pin
            && let Some(was) = pinned_over(text, name, field)
        {
            fields.insert((*field).to_owned(), json!(was));
        }
        let with = merged.get("with").and_then(Value::as_str).map(plain_with);
        let hollow = with.is_none() || (with.as_deref() == Some(COZY) && !["edit", "frame"].contains(&kind.as_str()));
        self.index.insert(key, self.assets.len());
        self.assets.push(Asset { name: name.to_owned(), kind, body: merged, fields, own, pin, also, hollow, frame: false, reads: vec![], writes: vec![], writer: None });
    }

    /// `scene`, an asset beat `beat` of asset `i` writes `at` seconds into its shot: a screenshot, its picture. If the shot reads it
    /// after that beat, in a later beat or in text every segment holds, its function takes it, in its one take; otherwise it is a
    /// free frame, outside its key. A pinned scene is read, never written. Two assets writing one name: the first stands.
    fn write(&mut self, i: usize, beat: usize, scene: &str, at: f64) {
        let key = name_key(scene);
        let writer_name = self.assets[i].name.clone();
        let revision = match self.index.get(&key).copied() {
            Some(revision) => revision,
            None => {
                self.index.insert(key.clone(), self.assets.len());
                self.assets.push(Asset {
                    name: scene.to_owned(),
                    kind: "scene".into(),
                    body: Map::new(),
                    fields: Map::new(),
                    own: BTreeSet::new(),
                    pin: None,
                    also: Files::new(),
                    hollow: false,
                    frame: false,
                    reads: vec![],
                    writes: vec![],
                    writer: None,
                });
                self.assets.len() - 1
            }
        };
        let own = &mut self.assets[revision];
        if own.writer.is_some() || own.frame {
            self.warnings.push(format!("{writer_name} writes {scene}, which is written above: the first stands"));
            return;
        }
        own.hollow = false;
        if own.pin.is_some() {
            return;
        }
        let mut read_after = self.assets[i].fields.clone();
        if let Some(Value::Array(beats)) = read_after.get_mut("beats") {
            beats.drain(..(beat + 1).min(beats.len()));
        }
        let read = mentions_in(&Value::Object(read_after)).iter().any(|mention| name_key(mention) == key);
        if read {
            let (fields, name) = (self.assets[revision].fields.clone(), self.assets[revision].name.clone());
            self.assets[revision].writer = Some(i);
            self.assets[i].writes.push(Write { key, name, at_s: at, fields });
        } else {
            let own = &mut self.assets[revision];
            own.frame = true;
            own.body.insert("with".into(), json!("cozy"));
            own.body.insert("video".into(), json!(format!("<{writer_name}>")));
            own.body.insert("at".into(), json!(at));
            own.fields = fields_of(&own.body);
        }
    }

    /// Each asset's reads, and the order. A mention of nothing stays text; one that closes a cycle too.
    fn resolve(&mut self) {
        let n = self.assets.len();
        let (mut upstream, mut fixed, mut said) = (vec![vec![]; n], vec![vec![]; n], vec![]);
        let mut reads = vec![vec![]; n];
        for i in 0..n {
            let asset = &self.assets[i];
            if let Some(writer) = asset.writer {
                upstream[i].push(writer);
                fixed[i].push(writer);
                continue;
            }
            let own: BTreeSet<&str> = asset.writes.iter().map(|write| write.key.as_str()).collect();
            for mention in mentions_in(&Value::Object(asset.fields.clone())) {
                let Some(j) = self.find(&mention) else {
                    said.push(format!("{} mentions <{mention}>, which is nothing in the project: it reaches its function as written", asset.name));
                    continue;
                };
                let key = name_key(&self.assets[j].name);
                if j != i && !own.contains(key.as_str()) && !upstream[i].contains(&j) {
                    upstream[i].push(j);
                    reads[i].push(j);
                }
            }
        }
        said.dedup();
        let graph = Graph::new(upstream, &fixed);
        for &(reader, read) in &graph.closing {
            reads[reader].retain(|j| *j != read);
            said.push(format!("{} mentions {}, which is made from it: that mention stays plain text", self.assets[reader].name, self.assets[read].name));
        }
        for (asset, reads) in self.assets.iter_mut().zip(reads) {
            asset.reads = reads;
        }
        self.order = graph.order;
        self.upstream = graph.upstream;
        self.warnings.extend(said);
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.index.get(&name_key(name)).copied()
    }

    /// The package that makes asset `i`: its own `with`, else its kind's default. The platform names no package.
    pub fn target(&self, i: usize) -> String {
        self.assets[i].body.get("with").and_then(Value::as_str).unwrap_or_default().to_owned()
    }

    /// Asset `i`'s operation when Calcifer runs it itself: the reserved package `cozy`.
    pub fn tool(&self, i: usize) -> Option<Tool> {
        let asset = &self.assets[i];
        if asset.frame {
            return Some(Tool::Frame);
        }
        (plain_with(asset.body.get("with")?.as_str()?) == COZY).then_some(())?;
        match asset.kind.as_str() {
            "edit" => Some(Tool::Cut),
            "frame" => Some(Tool::Frame),
            _ => None,
        }
    }

    /// The one line that says why asset `i` has no operation.
    pub fn nothing_to_make(&self, i: usize) -> String {
        let asset = &self.assets[i];
        match asset.body.contains_key("with") {
            true => format!("{}: the package {COZY} makes `edit` and `frame`, not {}: it has nothing to make", asset.name, asset.kind),
            false => format!("{} has no `with`, and no `defaults.{}.with`: it has nothing to make", asset.name, asset.kind),
        }
    }

    /// Whether asset `i` has an operation that can make it.
    pub fn makes(&self, i: usize) -> bool {
        let asset = &self.assets[i];
        asset.writer.is_none() && !asset.hollow
    }

    /// Whether the file itself declares asset `i`, as a revision only a beat names is not.
    pub fn declared(&self, i: usize) -> bool {
        i < self.declared
    }

    /// Every asset's state, in the file's terms.
    pub fn stand(&self, lock: &Lock) -> Vec<Standing> {
        let mut done = vec![Standing::default(); self.assets.len()];
        for &i in &self.order {
            done[i] = self.standing(i, lock, &done);
        }
        done
    }

    /// The hash in the file, else its newest version for these inputs, else none. A hash that is a version's main file brings
    /// the version's whole file set; one that is none's is just that file.
    fn standing(&self, i: usize, lock: &Lock, done: &[Standing]) -> Standing {
        let asset = &self.assets[i];
        let held = lock.asset(&asset.name);
        let reads = self.reading(i, lock, done);
        let (inputs, digests) = match &reads {
            Some(reads) => {
                let (inputs, digests) = self.key(i, reads);
                (Some(inputs), digests)
            }
            None => (None, BTreeMap::new()),
        };
        let reads = reads.unwrap_or_default();
        let stand = |state, files| Standing { state, files, inputs: inputs.clone(), digests: digests.clone(), reads: reads.clone() };
        let versions = held.map(|held| held.versions.as_slice()).unwrap_or_default();
        if let Some((field, sha256)) = &asset.pin {
            return match versions.iter().rev().find(|version| version.main() == Some(sha256)) {
                Some(version) => {
                    let changed = match &inputs {
                        Some(now) if !version.inputs.is_empty() && *now != version.inputs => changed(&version.digests, &digests),
                        _ => vec![],
                    };
                    stand(State::Pinned { version: Some(version.number), changed }, Some(version.files.clone()))
                }
                None => {
                    let mut files = asset.also.clone();
                    files.insert((*field).to_owned(), sha256.clone());
                    stand(State::Pinned { version: None, changed: vec![] }, Some(files))
                }
            };
        }
        if let Some(writer) = asset.writer {
            let by = self.assets[writer].name.clone();
            let take = done[writer].version().and_then(|number| lock.asset(&by)?.version(number));
            let wrote = take.and_then(|take| Some((take.number, take.writes.get(&name_key(&asset.name))?)));
            return match wrote {
                Some((version, files)) => stand(State::Written { by, version }, Some(files.clone())),
                None => stand(State::Unwritten { by }, None),
            };
        }
        let Some(now) = &inputs else { return stand(State::Waits, None) };
        match (versions.iter().rev().find(|version| version.inputs == *now), versions.last()) {
            (Some(version), _) => stand(State::Current(version.number), Some(version.files.clone())),
            (None, None) => stand(State::Missing, None),
            (None, Some(last)) => {
                // A version from before digests (a migrated one) can't say which.
                let changed = if last.digests.is_empty() { vec![] } else { changed(&last.digests, &digests) };
                let why = if changed.is_empty() { "its inputs changed".to_owned() } else { format!("{} changed", changed.join(", ")) };
                stand(State::Stale(why), None)
            }
        }
    }

    /// What asset `i` reads now; None while something has no state. A tool reads state, else the newest version: what has
    /// neither it skips, and with nothing to read at all, it waits.
    fn reading(&self, i: usize, lock: &Lock, done: &[Standing]) -> Option<Vec<ReadNow>> {
        let tool = self.tool(i).is_some();
        let mut reads = vec![];
        for &j in &self.assets[i].reads {
            let name = self.assets[j].name.clone();
            match &done[j].files {
                Some(files) => reads.push(ReadNow { name, asset: j, files: files.clone(), newest: None }),
                None if tool => {
                    let newest = lock.asset(&self.assets[j].name).and_then(|held| held.versions.last());
                    reads.extend(newest.map(|newest| ReadNow { name, asset: j, files: newest.files.clone(), newest: Some(newest.number) }));
                }
                None => return None,
            }
        }
        (!tool || !reads.is_empty()).then_some(reads)
    }

    /// The memo key's inputs: exactly what the adapter is given, hashed, and a digest per field, to say which changed.
    fn key(&self, i: usize, reads: &[ReadNow]) -> (String, BTreeMap<String, String>) {
        let asset = &self.assets[i];
        let tool = self.tool(i).is_some();
        let mut top = Map::new();
        for field in ["models", "lora", "options"] {
            if let Some(value) = asset.body.get(field).filter(|value| !value.is_null()) {
                top.insert(field.into(), value.clone());
            }
        }
        if let Some(with) = asset.body.get("with").and_then(Value::as_str) {
            top.insert("with".into(), json!(plain_with(with)));
        }
        top.insert("kind".into(), json!(asset.kind));
        // A tool's key holds what it reads of its fields, so a style in defaults doesn't cut again.
        let fields: Map<String, Value> = (asset.fields.iter())
            .filter(|(field, _)| !tool || TOOL_FIELDS.contains(&field.as_str()))
            .map(|(field, value)| (field.clone(), value.clone()))
            .collect();
        let mut digests: BTreeMap<String, String> = top.iter().map(|(field, value)| (field.clone(), digest(value))).collect();
        digests.extend(fields.iter().map(|(field, value)| (field.clone(), digest(value))));
        top.insert("fields".into(), Value::Object(fields));
        let mut sorted: Vec<&ReadNow> = reads.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        let reads: Vec<Value> = sorted
            .into_iter()
            .map(|read| {
                let of = &self.assets[read.asset];
                // A tool's key holds only files: same takes, same key, whatever their text says.
                let record = match tool {
                    true => json!({"name": read.name, "files": read.files}),
                    false => json!({"name": read.name, "kind": of.kind, "fields": of.fields, "files": read.files}),
                };
                digests.insert(format!("<{}>", read.name), digest(&record));
                record
            })
            .collect();
        top.insert("reads".into(), json!(reads));
        let writes: Vec<Value> = asset.writes.iter().map(Write::record).collect();
        if !writes.is_empty() {
            digests.insert("writes".into(), digest(&json!(writes)));
        }
        top.insert("writes".into(), json!(writes));
        (digest(&Value::Object(top)), digests)
    }

    /// The operation asset `i` is, as its adapter takes it: its fields, and what it reads with what is known of each file.
    pub fn operation(&self, i: usize, reads: &[ReadNow], lock: &Lock, keep: Option<Value>, stand: &[Standing]) -> Value {
        let asset = &self.assets[i];
        let file = |sha256: &String| lock.files.get(sha256).map_or_else(|| json!({"sha256": sha256}), folder::File::record);
        let reads: Vec<Value> = reads
            .iter()
            .map(|read| {
                let of = &self.assets[read.asset];
                let files: Map<String, Value> = read.files.iter().map(|(name, sha256)| (name.clone(), file(sha256))).collect();
                let made = stand[read.asset].version().or(read.newest).and_then(|number| lock.asset(&of.name)?.version(number));
                let made_by = made.and_then(|made| made.made_by.as_ref()).map_or("", |by| by.with.as_str());
                json!({"name": read.name, "kind": of.kind, "fields": of.fields, "files": files, "made_by": made_by})
            })
            .collect();
        let writes: Vec<Value> = asset.writes.iter().map(Write::record).collect();
        json!({"version": FORMAT, "asset": asset.name, "kind": asset.kind, "fields": asset.fields, "reads": reads, "writes": writes, "keep": keep})
    }

    /// `--take`, `--in`: what its adapter is told of the take a new one keeps, its files as records; None if it holds no video.
    pub fn keep(&self, i: usize, lock: &Lock, keeps: Keeps) -> Option<Value> {
        let held = lock.asset(&self.assets[i].name)?.version(keeps.take)?;
        let video = lock.files.get(held.files.get("video")?)?;
        let latents = held.files.get("latents").and_then(|sha256| lock.files.get(sha256));
        let ends_s = if video.fps > 0.0 { video.frames as f64 / video.fps } else { 0.0 };
        let made_by = held.made_by.as_ref().map_or("", |by| by.with.as_str());
        Some(json!({"take": keeps.take, "video": video.record(), "latents": latents.map(folder::File::record), "at_s": keeps.in_s, "ends_s": ends_s, "made_by": made_by}))
    }
}

/// What each beat writes, soonest first: the beat, the scene, and when on the shot's timeline. A write is `{scene, at}`: `at` is
/// seconds into its beat, else its end; past the beat it clamps, which is said, as is any other field.
fn beat_writes(name: &str, body: &Map<String, Value>, said: &mut Vec<String>) -> Vec<(usize, String, f64)> {
    let (mut start, mut found) = (0.0, vec![]);
    for (n, beat) in body.get("beats").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let length = beat["duration"].as_f64().filter(|duration| *duration > 0.0).unwrap_or(BEAT_S);
        let listed = match &beat["writes"] {
            Value::Array(items) => items.clone(),
            Value::Null => vec![],
            one => vec![one.clone()],
        };
        for item in listed {
            let (scene, at) = match &item {
                Value::String(scene) => (scene.clone(), None),
                item => (text_of(item.get("scene")), item["at"].as_f64()),
            };
            if scene.is_empty() {
                said.push(format!("{name}'s beat {} writes nothing: each write names a `scene`", n + 1));
                continue;
            }
            for field in item.as_object().into_iter().flat_map(|write| write.keys()).filter(|field| !["scene", "at"].contains(&field.as_str())) {
                said.push(format!("{name}'s write of {scene}: cozy reads no field {field}; it is ignored"));
            }
            let within = at.unwrap_or(length).clamp(0.0, length);
            if let Some(at) = at.filter(|at| *at != within) {
                said.push(format!("{name} writes {scene} at {at} s into a beat {length} s long: it is taken at {within} s"));
            }
            found.push((n, scene, start + within));
        }
        start += length;
    }
    found.sort_by(|a, b| a.2.total_cmp(&b.2));
    found
}

/// `with` as the key holds it: no release, no root namespace, and local code by its directory's name, so moving from a checkout to
/// the published package puts nothing out of date.
fn plain_with(with: &str) -> String {
    let with = with.trim();
    if protocol::target::written_as_path(with) || with.ends_with(".py") {
        let name = with.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(with);
        return name.strip_suffix(".py").unwrap_or(name).to_owned();
    }
    let path = with.split('@').next().unwrap_or(with);
    path.strip_prefix("_/").unwrap_or(path).to_owned()
}

/// What passes to the adapter. A beat's `writes` doesn't: a write goes by `Operation.writes`, or is a free frame outside the key.
fn fields_of(body: &Map<String, Value>) -> Map<String, Value> {
    let mut fields: Map<String, Value> = (body.iter())
        .filter(|(field, value)| !RESERVED.contains(&field.as_str()) || (PINS.contains(&field.as_str()) && !mentions_in(value).is_empty()))
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect();
    for beat in fields.get_mut("beats").and_then(Value::as_array_mut).into_iter().flatten() {
        if let Some(beat) = beat.as_object_mut() {
            beat.remove("writes");
        }
    }
    fields
}

fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

/// The sha256 a file pin's value names, or how it fails, to follow "{name}'s {field}".
fn file_named(written: &str, lock: &Lock) -> Result<String, String> {
    if written.is_empty() || !written.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(" is not a hash: {written} is not read: `cozy project import {}` prints what to write", quoted(written)));
    }
    match lock.matching(&written.to_ascii_lowercase()).as_slice() {
        [file] => Ok(file.sha256.clone()),
        [] => Err(format!(": no file's hash starts with {written}: `cozy project import <file>` prints what to write")),
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

/// `value` as a file field writes it: quoted when YAML would read it as a number.
pub fn yaml_scalar(value: &str) -> String {
    match value.parse::<f64>() {
        Ok(_) => format!("\"{value}\""),
        Err(_) => value.to_owned(),
    }
}

/// Text trimmed, numbers as floats, keys sorted: a comment, a reformat or key order changes nothing.
fn canonical(value: &Value) -> Value {
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

/// The fields whose digests differ, by name.
fn changed(was: &BTreeMap<String, String>, now: &BTreeMap<String, String>) -> Vec<String> {
    let names: BTreeSet<&String> = was.keys().chain(now.keys()).collect();
    names.into_iter().filter(|name| was.get(*name) != now.get(*name)).cloned().collect()
}

/// `text` with asset `name` pinned: `field: value` its first line, or the one that field holds. Cozy edits only that one line, and
/// checks what it made reads as the same project with that one change. A field holding an input (`video: <clash>`) keeps it in a
/// comment on its line, which `unpinned` restores. Err: why not, and what to write by hand.
pub fn pinned(text: &str, name: &str, field: &str, value: &str) -> Result<String, Error> {
    let by_hand = |why: String| invalid("project_edit", format!("{name} can't be pinned in {}: {why}", folder::PROJECT)).remedy(format!("write `{field}: {value}` under {name} yourself"));
    let place = Place::find(text, name).ok_or_else(|| by_hand("it has no line of its own under `assets:`".into()))?;
    let lines = &place.lines;
    let edited = if let Some(flow) = &place.flow {
        let (head, inner) = (flow.0.as_str(), flow.1.as_str());
        if flow_field(inner, field).is_some_and(|at| !hex_value(&inner[at..])) {
            return Err(by_hand(format!("its {field} is an input on a line of many: write it on lines of its own")));
        }
        let inner = strip_flow_field(inner, field);
        let inner = if inner.trim().is_empty() { format!("{field}: {value}") } else { format!("{field}: {value}, {}", inner.trim_start()) };
        replace_line(lines, place.key, &format!("{head}{{{inner}}}{}", flow.2))
    } else {
        match place.field(field) {
            Some(at) => {
                let (indent, rest) = field_rest(lines[at], field);
                let (written, comment) = split_comment(rest);
                if written.is_empty() || written.starts_with(['|', '>']) {
                    return Err(by_hand(format!("its {field} runs over several lines")));
                }
                let line = match written.starts_with('<') {
                    true => format!("{indent}{field}: {value}  # was {}\n", rest.trim()),
                    false => format!("{indent}{field}: {value}{}\n", comment.map_or(String::new(), |comment| format!("  {comment}"))),
                };
                replace_line(lines, at, &line)
            }
            None => insert_line(lines, place.key, &format!("{}{field}: {value}\n", " ".repeat(place.child))),
        }
    };
    same_but(text, &edited, name, field, Some(value)).map_err(by_hand)?;
    Ok(edited)
}

/// `text` with asset `name`'s pin in `field` gone: the input it was over back on its line, else the line gone. Its `with` and text
/// were there all along.
pub fn unpinned(text: &str, name: &str, field: &str) -> Result<String, Error> {
    let by_hand = |why: String| invalid("project_edit", format!("{name} can't be unpinned in {}: {why}", folder::PROJECT)).remedy(format!("remove its `{field}:` yourself"));
    let place = Place::find(text, name).ok_or_else(|| by_hand("it has no line of its own under `assets:`".into()))?;
    let lines = &place.lines;
    let edited = if let Some(flow) = &place.flow {
        replace_line(lines, place.key, &format!("{}{{{}}}{}", flow.0, strip_flow_field(&flow.1, field), flow.2))
    } else {
        let at = place.field(field).ok_or_else(|| by_hand(format!("no `{field}:` line was found")))?;
        let (indent, rest) = field_rest(lines[at], field);
        match rest.split_once("# was ") {
            Some((_, was)) => replace_line(lines, at, &format!("{indent}{field}: {}\n", was.trim_end())),
            None => {
                let mut kept: Vec<&str> = lines.clone();
                kept.remove(at);
                kept.concat()
            }
        }
    };
    let was = pinned_over(text, name, field);
    same_but(text, &edited, name, field, was.as_deref()).map_err(by_hand)?;
    Ok(edited)
}

/// Whether `edited` reads as `text` with `name`'s `field` set to `value` (a quoted hash unquoted), or gone with None.
fn same_but(text: &str, edited: &str, name: &str, field: &str, value: Option<&str>) -> Result<(), String> {
    let read = |text: &str| protocol::yaml::to_json(text).map_err(|why| format!("the edit would not read: {why}"));
    let (mut before, after) = (read(text)?, read(edited)?);
    let key = |assets: &Map<String, Value>| assets.keys().find(|key| name_key(key) == name_key(name)).cloned();
    let at = before["assets"].as_object().and_then(key).ok_or("it isn't under `assets:`")?;
    let asset = &mut before["assets"][&at];
    if asset.is_null() {
        *asset = json!({});
    }
    let asset = asset.as_object_mut().ok_or("it is no mapping of fields")?;
    match value {
        Some(value) => {
            let written = protocol::yaml::to_json(value).unwrap_or_else(|_| json!(value));
            asset.insert(field.into(), written);
        }
        None => {
            asset.remove(field);
        }
    }
    let mut after = after;
    let emptied = after["assets"][&at].is_null();
    if emptied {
        after["assets"][&at] = json!({});
    }
    (before == after).then_some(()).ok_or_else(|| "the edit would change something else".into())
}

/// The input a pin in `field` keeps in a comment on its line.
fn pinned_over(text: &str, name: &str, field: &str) -> Option<String> {
    let place = Place::find(text, name)?;
    let (_, rest) = field_rest(place.lines[place.field(field)?], field);
    let (_, was) = rest.split_once("# was ")?;
    Some(split_comment(was).0.to_owned())
}

/// Where an asset's lines are in the text.
struct Place<'a> {
    lines: Vec<&'a str>,
    /// The line that names it.
    key: usize,
    /// The lines after it that are its body.
    body: std::ops::Range<usize>,
    /// How far its fields are indented.
    child: usize,
    /// All on one line, `name: {…}`: what is before `{`, what is inside, what follows `}`.
    flow: Option<(String, String, String)>,
}

impl<'a> Place<'a> {
    fn find(text: &'a str, name: &str) -> Option<Self> {
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let significant = |line: &str| !line.trim().is_empty() && !line.trim_start().starts_with('#');
        let indent = |line: &str| line.len() - line.trim_start_matches(' ').len();
        let start = lines.iter().position(|line| indent(line) == 0 && split_comment(line).0 == "assets:")? + 1;
        let end = (start..lines.len()).find(|&n| significant(lines[n]) && indent(lines[n]) == 0).unwrap_or(lines.len());
        let at = indent(lines[start..end].iter().find(|line| significant(line))?);
        let key = (start..end).find(|&n| significant(lines[n]) && indent(lines[n]) == at && key_of(lines[n]).is_some_and(|key| name_key(&key) == name_key(name)))?;
        let finish = ((key + 1)..end).find(|&n| significant(lines[n]) && indent(lines[n]) <= at).unwrap_or(end);
        let first = ((key + 1)..finish).find(|&n| significant(lines[n]));
        let child = first.map_or(at + 2, |n| indent(lines[n]));
        // Where its value starts: past the colon that ends the key.
        let after = lines[key].find(": ").map(|colon| colon + 2);
        let flow = after.and_then(|after| {
            let (head, value) = lines[key].split_at(after);
            let (written, _) = split_comment(value);
            let inner = written.strip_prefix('{')?.strip_suffix('}')?;
            let close = value.find(written)? + written.len();
            Some((head.to_owned(), inner.to_owned(), value[close..].to_owned()))
        });
        Some(Self { lines, key, body: (key + 1)..finish, child, flow })
    }

    /// The line holding `field`, among its own.
    fn field(&self, field: &str) -> Option<usize> {
        self.body.clone().find(|&n| {
            let line = self.lines[n];
            let spaces = line.len() - line.trim_start_matches(' ').len();
            spaces == self.child && line[spaces..].strip_prefix(field).is_some_and(|rest| rest.trim_end().starts_with(':'))
        })
    }
}

/// The key a line opens: `name:`, `"name":`, `'name':`.
fn key_of(line: &str) -> Option<String> {
    let line = line.trim();
    if let Some(quote @ ('"' | '\'')) = line.chars().next() {
        let rest = &line[1..];
        let end = rest.find(quote)?;
        return rest[end + 1..].trim_start().starts_with(':').then(|| rest[..end].to_owned());
    }
    let colon = line.find(": ").or_else(|| line.strip_suffix(':').map(str::len))?;
    Some(line[..colon].to_owned())
}

/// A field's line as its indent and what follows `field:`.
fn field_rest<'a>(line: &'a str, field: &str) -> (&'a str, &'a str) {
    let spaces = line.len() - line.trim_start_matches(' ').len();
    (&line[..spaces], line[spaces + field.len()..].trim_start_matches(':').trim())
}

/// What a value writes, and its comment. A `#` in quotes is no comment, and neither is one with no space before it.
fn split_comment(rest: &str) -> (&str, Option<&str>) {
    let (mut quote, bytes) = (None, rest.as_bytes());
    for (n, byte) in bytes.iter().enumerate() {
        match (quote, *byte) {
            (Some(open), byte) if byte == open => quote = None,
            (None, b'"' | b'\'') => quote = Some(*byte),
            (None, b'#') if n == 0 || bytes[n - 1].is_ascii_whitespace() => return (rest[..n].trim(), Some(rest[n..].trim())),
            _ => {}
        }
    }
    (rest.trim(), None)
}

/// Where `field:` starts among the comma-separated fields of a one-line mapping.
fn flow_field(inner: &str, field: &str) -> Option<usize> {
    let pattern = format!("{field}:");
    inner.match_indices(&pattern).find(|(at, _)| *at == 0 || inner[..*at].trim_end().ends_with([',', '{']) || inner[..*at].trim().is_empty()).map(|(at, _)| at)
}

/// Whether what starts at `field:` holds a hash, not an input.
fn hex_value(at_field: &str) -> bool {
    let value = at_field.split_once(':').map_or("", |(_, rest)| rest).split(',').next().unwrap_or_default().trim().trim_matches('"');
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// `inner` with `field: value` gone.
fn strip_flow_field(inner: &str, field: &str) -> String {
    let Some(at) = flow_field(inner, field) else { return inner.to_owned() };
    let rest = &inner[at..];
    let end = rest.find(',').map_or(rest.len(), |comma| comma + 1);
    format!("{}{}", &inner[..at], rest[end..].trim_start()).trim_end().trim_end_matches(',').to_owned()
}

fn replace_line(lines: &[&str], at: usize, line: &str) -> String {
    let mut kept: Vec<&str> = lines.to_vec();
    let ended = if line.ends_with('\n') || !lines[at].ends_with('\n') { line.to_owned() } else { format!("{line}\n") };
    kept[at] = &ended;
    kept.concat()
}

fn insert_line(lines: &[&str], after: usize, line: &str) -> String {
    let (head, tail) = lines.split_at(after + 1);
    let glue = if lines[after].ends_with('\n') { "" } else { "\n" };
    [head.concat().as_str(), glue, line, tail.concat().as_str()].concat()
}

fn invalid(code: &str, message: String) -> Error {
    Error::new(Kind::Validation, code, message)
}
