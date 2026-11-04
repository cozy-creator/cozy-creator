//! project.yaml, format 2 (design/project-graph.md): assets, the operation that makes each (a package, by `with`, and a `kind`,
//! which its adapter turns into calls), the memo key, and what each asset's state is. The platform reads `kind`, `render` (`with`, `prompt`,
//! `models`, `lora`), a hash in `image`, `video` or `audio`, an entity's links, and `<name>` in any text. It knows
//! no kind's meaning and no field's shape, but for the reserved package `cozy`, which it runs itself. Every other field passes through to the adapter and counts in the
//! key. Nothing refuses a project but a file of no format: what one line gets wrong is said, and that line does the best it can.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use protocol::{
    error::{Error, Kind},
    project::{Read, Warning},
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    folder::{self, Files, Folder, Keeps, Lock},
    graph::{Graph, as_name, edits, is_name, mentions, mentions_in, name_key},
};

/// The name `cozy project render all` means.
pub const ALL: &str = "all";
const FORMAT: u64 = 2;
/// What `render` unfolds to: how to make it, never at an asset's root.
const RENDERED: [&str; 5] = ["with", "instructions", "options", "models", "lora"];
/// The fields that name an asset's main file. A hash there pins it; a `<mention>` is an input.
const PINS: [&str; 3] = ["image", "video", "audio"];
/// The links of an entity that a read of it gets as files. Its other links go by name, `<kyogre.primordial>`.
const ROLES: [&str; 4] = ["image", "video", "audio", "voice"];
const RESERVED: [&str; 9] = ["render", "with", "kind", "models", "lora", "options", "image", "video", "audio"];
const SECTIONS: [&str; 4] = ["version", "defaults", "entities", "assets"];
/// The reserved package Calcifer runs itself.
const COZY: &str = "cozy";
/// What crosses a mention. A node's prompt, and whatever else went into making it, is for its maker alone (Paul, 11-04): a reader gets
/// the words meant for it, and how to take the file.
const FOR_READERS: [&str; 3] = ["description", "use_audio", "follow"];
/// What `cozy` reads of a cut or a frame.
const TOOL_FIELDS: [&str; 6] = ["pieces", "fps", "aspect_ratio", "resolution", "video", "at"];

/// A new project's project.yaml: a short film that renders as it stands. Cozy never writes it again.
pub fn starter() -> String {
    r#"# A short film, ready to render: cozy project render all --on <machine>
# Change anything. Each asset says what it is; `<name>` in any text reads that asset.
version: 2
defaults:
  image:
    with: qwen-image-2
    style: High-quality Unreal Engine graphics in a bright, hand-painted, cel-shaded anime style.
    aspect_ratio: "16:9"
  video:
    with: minimax-h3
    style: High-quality Unreal Engine graphics in a bright, hand-painted, cel-shaded anime style.
    aspect_ratio: "16:9"

entities:
  kyogre:
    kind: character
    description: |
      Kyogre, the colossal blue whale-like legendary Pokémon with huge fins, no legs, and glowing red lines.
    image: <kyogre_sheet>

  groudon:
    kind: character
    description: |
      Groudon, the colossal red legendary Pokémon with grey stone plates and huge claws.
    image: <groudon_sheet>

  coast:
    kind: environment
    description: A volcanic coast under a storm sky, with a dark sea on the left, lava cliffs on the right, and a small white crater city far away at the center.
    image: <coast_plate>

assets:
  kyogre_sheet:
    render: |
      Kyogre, the colossal blue whale-like legendary Pokémon with huge fins, no legs, and glowing red lines. The whole body from the front on the left, with nothing added to the body described and no ground under it. On the right, the same character has been spun a half turn, at the same size, so its back faces the viewer and nothing on its front can be seen. Two separate views, with no text.

  groudon_sheet:
    render: |
      Groudon, the colossal red legendary Pokémon with grey stone plates and huge claws. The whole body from the front on the left, with nothing added to the body described and no ground under it. On the right, the same character has been spun a half turn, at the same size, so its back faces the viewer and nothing on its front can be seen. Two separate views, with no text.

  coast_plate:
    render: A volcanic coast under a storm sky, with a dark sea on the left, lava cliffs on the right, and a small white crater city far away at the center.

  edit:
    kind: video
    render: {with: cozy}
    fps: 24
    pieces:
      - <awaken>
      - <erupt>
      - <clash>

  awaken:
    kind: video
    summary: |
      <kyogre> bursts out of the stormy sea of <coast>.
    soundscape: Howling wind, crashing waves, and rolling thunder.
    beats:
      - duration: 6
        description: |
          A wide shot from the water's edge looks out over the stormy sea of <coast>, with lightning flickering behind the clouds. The water bulges at the center of the frame, and <kyogre> bursts out of it, facing right, with sheets of white water pouring off its fins. The camera tilts up at slow speed as heavy rain starts to fall.

  erupt:
    kind: video
    summary: |
      <groudon> claws its way out of the lava cliffs of <coast> and roars.
    soundscape: Cracking rock, hissing lava, heavy footfalls, and a deep roar.
    beats:
      - duration: 6
        description: |
          A wide shot from the rocks looks up at the lava cliffs of <coast>. The rock face cracks at the center of the frame, and <groudon> claws its way out, lava sliding off its stone plates in glowing strings. It rears up and roars toward the left. The camera pushes in at slow speed and shakes with each step.

  clash:
    kind: video
    summary: |
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
    pub kind: String,
    /// What makes it, as the memo holds it: no release, no root namespace, and local code by its directory's name.
    pub tool: Option<String>,
    pub body: Map<String, Value>,
    /// What passes to the adapter: every field but the platform's, and a file field holding a mention.
    pub fields: Map<String, Value>,
    /// The fields it wrote itself.
    pub own: BTreeSet<String>,
    /// A pin, the line `approve` writes: its field and sha256. Its `render` and text stay in the file, ignored while it stands.
    pub pin: Option<(&'static str, String)>,
    /// It has no operation: no `with`, of its own or from defaults, or no `kind` for its package's adapter.
    pub hollow: bool,
    /// What it reads, in the order its text mentions them.
    pub reads: Vec<usize>,
    /// A hash written in an entity's link: the entity or sub-entity, and the field, whose line holds it.
    pub line: Option<(String, String)>,
    /// It is no asset but an entity, a sub-entity or a link of one: no work and no versions, its files what its links hold now.
    pub links: Option<Links>,
}

/// What an entity, a sub-entity or a link of one points at. Each is a node of the graph, so a read of it is like any read.
#[derive(Clone, Default)]
pub struct Links {
    /// Its files: role to the asset whose main file that is. A link of an entity holds one, under "", and reads that asset's whole set.
    pub roles: BTreeMap<String, String>,
    /// Its own links as the file writes them, field to asset. A link of an entity has none.
    pub own: BTreeMap<String, String>,
    /// Its sub-entities' names.
    pub subs: Vec<String>,
    /// The entity it is a sub-entity or link of.
    pub parent: Option<String>,
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
    /// Mentions naming nothing and no tag its package is known to have. Until a package is read its tags may be among them, so
    /// nothing is made of these before then.
    unread: Vec<(usize, String)>,
    /// How many the file declares, in order, and those after them are what its entities make: an entity, and a link or a hash in one.
    declared: usize,
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
    /// An entity, whose links all stand.
    Linked,
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

impl Doc {
    /// The project as project.yaml says now.
    pub fn load(folder: &Folder) -> Result<Self, Error> {
        Self::read(folder, &folder.text()?)
    }

    /// The project `text` says. Only a file of no format, or an older one, is refused.
    pub fn read(folder: &Folder, text: &str) -> Result<Self, Error> {
        let value = protocol::yaml::to_json_keeping(text, names_a_file).map_err(|why| {
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
        let old = ["assets", "shots", "entities"].iter().any(|key| top.get(*key).is_some_and(Value::is_array))
            || top.contains_key("world_states")
            || top.get("version").is_some_and(|version| version.as_u64() == Some(1));
        if old {
            let said = format!("{} is format 1, and cozy reads format 2: it is left as it is", folder::PROJECT);
            return Err(invalid("project_format", said).remedy("move it to `assets:` (design/project-graph), or start from `cozy project new`"));
        }
        let mut warnings = vec![];
        if top.get("version").and_then(Value::as_u64) != Some(FORMAT) {
            warnings.push(Warning::of_file(format!("{} has no `version: {FORMAT}`, so it reads as version {FORMAT}: add it", folder::PROJECT)));
        }
        warnings.extend(top.keys().filter(|key| !SECTIONS.contains(&key.as_str())).map(|key| Warning::of_file(format!("{}: cozy reads no section {key}; it is ignored", folder::PROJECT))));
        let empty = Map::new();
        let mut defaults = Map::new();
        for (section, value) in top.get("defaults").and_then(Value::as_object).unwrap_or(&empty) {
            if !PINS.contains(&section.as_str()) {
                warnings.push(Warning::of_file(format!("defaults.{section}: the only keys are image, video and audio; it is ignored")));
                continue;
            }
            // Each is a default `render` block.
            let block = Map::from_iter([("render".to_owned(), value.clone())]);
            defaults.insert(section.clone(), Value::Object(unfolded(&format!("defaults.{section}"), block, &mut warnings)));
        }
        let declared = top.get("assets").and_then(Value::as_object).unwrap_or(&empty);
        let lock = folder.lock()?;
        let entities = top.get("entities").and_then(Value::as_object).unwrap_or(&empty);
        for name in protocol::yaml::keys(text, &["assets"]) {
            named(&name)?;
        }
        for name in protocol::yaml::keys(text, &["entities"]) {
            named(&name)?;
            let fields = entities.get(&name).and_then(Value::as_object).into_iter().flat_map(|body| body.keys());
            for field in fields.filter(|field| !["kind", "description", "render"].contains(&field.as_str())) {
                named(&format!("{name}.{field}"))?;
                for inner in entities[&name][field].as_object().into_iter().flat_map(|sub| sub.keys()).filter(|inner| !["kind", "description", "render"].contains(&inner.as_str())) {
                    named(&format!("{name}.{field}.{inner}"))?;
                }
            }
        }
        let mut doc = Self { assets: vec![], upstream: vec![], order: vec![], warnings, index: HashMap::new(), reserved: lock.reserved.clone(), unread: vec![], declared: 0, dir: folder.dir.clone() };
        for name in protocol::yaml::keys(text, &["assets"]) {
            let Some(written) = declared.get(&name) else { continue };
            doc.declare(text, &name, written, &defaults, folder, &lock);
        }
        doc.declared = doc.assets.len();
        for name in protocol::yaml::keys(text, &["entities"]) {
            let Some(written) = entities.get(&name) else { continue };
            doc.entity(text, &name, written, &defaults, folder, &lock);
        }
        for i in 0..doc.assets.len() {
            if doc.assets[i].hollow && doc.assets[i].pin.is_none() && doc.assets[i].links.is_none() {
                let (name, said) = (doc.assets[i].name.clone(), doc.nothing_to_make(i));
                doc.warn(&name, said);
            }
        }
        doc.resolve();
        Ok(doc)
    }

    /// An asset of the file, after defaults, its pin read. A name taken: the first stands.
    fn declare(&mut self, text: &str, name: &str, written: &Value, defaults: &Map<String, Value>, folder: &Folder, lock: &Lock) -> Option<usize> {
        let key = name_key(name);
        if self.index.contains_key(&key) {
            self.warn(name, format!("two assets are named {name}: the first stands"));
            return None;
        }
        if key == ALL {
            self.warn(name, format!("{name} is named `{ALL}`: `cozy project render {ALL}` means everything, and makes it with the rest"));
        }
        let body = match written {
            Value::Object(body) => body.clone(),
            Value::Null => Map::new(),
            _ => {
                self.warn(name, format!("{name} is no mapping of fields: it is empty"));
                Map::new()
            }
        };
        let mut body = unfolded(name, body, &mut self.warnings);
        if without_writes(&mut body) {
            self.warn(name, format!("{name}'s writes: a shot no longer makes pictures, so these are ignored: one frame of a video is its own asset, `{{video: <{name}>, at: 3, render: {{with: cozy}}}}`"));
        }
        let own: BTreeSet<String> = body.keys().cloned().collect();
        // An asset's kind is its file's type: what it writes, else the field holding its hash, else `image`.
        let hash = |field: &&str| body.get(*field).and_then(Value::as_str).is_some_and(|text| mentions(text).is_empty());
        let held = PINS.iter().copied().find(hash);
        let written_kind = text_of(body.get("kind"));
        let kind = if PINS.contains(&written_kind.as_str()) { written_kind.clone() } else { held.unwrap_or("image").to_owned() };
        if !written_kind.is_empty() && written_kind != kind {
            self.warn(name, format!("{name}'s kind: an asset is an image, a video or audio, and anything else belongs on an entity: {written_kind} is ignored"));
        }
        // A hash and nothing to make it from: a constant, which no operation or adapter makes, whatever its kind.
        let constant = held.is_some() && body.keys().all(|key| key == "kind" || key == "description" || PINS.contains(&key.as_str()));
        // `defaults` fills what its `render` doesn't write, but one naming its own `with` takes nothing from there.
        let (mut merged, mut options) = (Map::new(), Map::new());
        if let Some(section) = defaults.get(&kind).and_then(Value::as_object).filter(|_| !constant && !body.contains_key("with")) {
            options.extend(section.get("options").and_then(Value::as_object).cloned().unwrap_or_default());
            merged.extend(section.clone());
        }
        // `options` is the adapter's own map: the asset's values over its defaults', key by key.
        options.extend(body.get("options").and_then(Value::as_object).cloned().unwrap_or_default());
        merged.extend(body);
        if !options.is_empty() {
            merged.insert("options".into(), Value::Object(options));
        }
        let mut pin = None;
        for field in PINS {
            let Some(value) = merged.get(field) else { continue };
            let written = match value {
                Value::String(text) if !mentions(text).is_empty() => continue,
                Value::String(text) => text.clone(),
                Value::Array(_) if mentions_in(value).is_empty() => {
                    self.warn(name, format!("{name}'s {field}: a list pins nothing: an asset has one main file, so make an asset for each; it is ignored"));
                    merged.remove(field);
                    continue;
                }
                _ => continue,
            };
            if field != kind {
                self.warn(name, format!("{name}'s {field}: an asset has one main file, its {kind}: make an asset for this one, and link both from an entity; it is ignored"));
                merged.remove(field);
                continue;
            }
            match file_named(&written, lock) {
                Ok(sha256) => {
                    if !lock.files.get(&sha256).is_some_and(|file| folder.path(&file.path).is_file()) {
                        self.warn(name, format!("{name}'s {field}: {} is not on this computer: cozy project pull", folder::prefix_of(&sha256)));
                    }
                    pin = Some((field, sha256));
                }
                Err(why) => {
                    self.warn(name, format!("{name}'s {field}{why}"));
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
        let with = merged.get("with").and_then(Value::as_str).map(|with| plain_with(with, folder));
        let hollow = constant || with.is_none() || (with.as_deref() == Some(COZY) && !["video", "image"].contains(&kind.as_str()));
        self.index.insert(key, self.assets.len());
        self.assets.push(Asset { name: name.to_owned(), kind, tool: with, body: merged, fields, own, pin, hollow, reads: vec![], line: None, links: None });
        Some(self.assets.len() - 1)
    }

    /// An entity: identity and links, no work. It, each sub-entity and each link are nodes of the graph, so `<kyogre>`,
    /// `<groudon.primordial>` and `<kyogre.primordial>` read like any mention. A sub-entity takes from its parent what it doesn't have.
    fn entity(&mut self, text: &str, name: &str, written: &Value, defaults: &Map<String, Value>, folder: &Folder, lock: &Lock) {
        let Some(body) = written.as_object() else {
            self.warn(name, format!("{name} is no mapping of fields: it is ignored"));
            return;
        };
        let (own, subs) = self.links_of(text, name, body, true, defaults, folder, lock);
        let roles = |own: &BTreeMap<String, String>| -> BTreeMap<String, String> {
            own.iter().filter(|(field, _)| ROLES.contains(&field.as_str())).map(|(field, asset)| (field.clone(), asset.clone())).collect()
        };
        let kind = match text_of(body.get("kind")) {
            kind if kind.is_empty() => "image".to_owned(),
            kind => kind,
        };
        let description = body.get("description").cloned();
        let mut named = vec![];
        for (field, sub) in &subs {
            let full = format!("{name}.{field}");
            let (mine, _) = self.links_of(text, &full, sub, false, defaults, folder, lock);
            let mut files = roles(&own);
            files.extend(roles(&mine));
            let its_kind = match text_of(sub.get("kind")) {
                its if its.is_empty() => kind.clone(),
                its => its,
            };
            let its_description = sub.get("description").cloned().or_else(|| description.clone());
            let links = Links { roles: files, own: mine, subs: vec![], parent: Some(name.to_owned()) };
            if self.node(&full, its_kind, its_description, links) {
                named.push(full);
            }
        }
        let links = Links { roles: roles(&own), own: own.clone(), subs: named, parent: None };
        self.node(name, kind.clone(), description.clone(), links);
        for (field, asset) in own {
            let full = format!("{name}.{field}");
            // A hash in the link made the asset of this name, so `<kyogre.image>` reads that.
            if asset != full {
                let links = Links { roles: BTreeMap::from([(String::new(), asset)]), parent: Some(name.to_owned()), ..Links::default() };
                self.node(&full, kind.clone(), description.clone(), links);
            }
        }
    }

    /// An entity's links and sub-entities. A link names an asset, or holds a hash, shorthand for a constant asset named for the link.
    #[allow(clippy::too_many_arguments)]
    fn links_of(&mut self, text: &str, name: &str, body: &Map<String, Value>, nests: bool, defaults: &Map<String, Value>, folder: &Folder, lock: &Lock) -> (BTreeMap<String, String>, Vec<(String, Map<String, Value>)>) {
        let (mut links, mut subs) = (BTreeMap::new(), vec![]);
        for (field, value) in body {
            match (field.as_str(), value) {
                ("kind" | "description", _) => {}
                ("render", _) => self.warn(name, format!("{name}'s render: an entity holds no way of making anything: put it on an asset it links; it is ignored")),
                (_, Value::Object(sub)) if nests => subs.push((field.clone(), sub.clone())),
                (_, Value::Object(_)) => self.warn(name, format!("{name}'s {field}: sub-entities go one level; it is ignored")),
                (_, Value::String(said)) if matches!(mentions(said).as_slice(), [one] if said.trim() == format!("<{one}>")) => {
                    links.insert(field.clone(), mentions(said)[0].to_owned());
                }
                (_, Value::String(written)) if !written.is_empty() && written.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
                    match file_named(written, lock) {
                        Ok(sha256) => {
                            let role = lock.files.get(&sha256).map_or("image", folder::File::field);
                            let full = format!("{name}.{field}");
                            let constant = Value::Object(Map::from_iter([(role.to_owned(), Value::String(written.clone()))]));
                            if let Some(i) = self.declare(text, &full, &constant, defaults, folder, lock) {
                                self.assets[i].line = Some((name.to_owned(), field.clone()));
                                links.insert(field.clone(), full);
                            }
                        }
                        Err(why) => self.warn(name, format!("{name}'s {field}{why}")),
                    }
                }
                _ => self.warn(name, format!("{name}'s {field}: an entity links to an asset, as `{field}: <its_name>`, or a file's hash; it is ignored")),
            }
        }
        (links, subs)
    }

    /// A node that is no work, unless the name is taken: names are shared across assets and entities, and the asset stands.
    fn node(&mut self, name: &str, kind: String, description: Option<Value>, links: Links) -> bool {
        let key = name_key(name);
        if self.index.contains_key(&key) {
            self.warn(name, format!("{name} is an asset and an entity, and they share names: the asset stands, so rename one"));
            return false;
        }
        let fields: Map<String, Value> = description.into_iter().map(|text| ("description".to_owned(), text)).collect();
        self.index.insert(key, self.assets.len());
        self.assets.push(Asset {
            name: name.to_owned(),
            kind,
            tool: None,
            body: Map::new(),
            fields,
            own: BTreeSet::new(),
            pin: None,
            hollow: true,
            reads: vec![],
            line: None,
            links: Some(links),
        });
        true
    }

    /// Each asset's reads, and the order. A mention of nothing stays text; one that closes a cycle too.
    fn resolve(&mut self) {
        let n = self.assets.len();
        let (mut upstream, mut fixed, mut said) = (vec![vec![]; n], vec![vec![]; n], vec![]);
        let mut reads = vec![vec![]; n];
        let mut unread = vec![];
        // A link of an entity says what the asset says of itself, else what the entity says.
        for i in 0..n {
            let target = self.assets[i].links.as_ref().and_then(|links| self.find(links.roles.get("")?));
            if let Some(description) = target.and_then(|j| self.assets[j].fields.get("description").filter(|_| self.assets[j].links.is_none()).cloned()) {
                self.assets[i].fields.insert("description".into(), description);
            }
        }
        for i in 0..n {
            let asset = &self.assets[i];
            if let Some(links) = &asset.links {
                for (field, target) in &links.own {
                    match self.find(target) {
                        None => said.push(Warning::of(&asset.name, format!("{}'s {field}: <{target}> is nothing in the project: add an asset named {target}", asset.name))),
                        Some(j) if self.assets[j].links.is_some() => said.push(Warning::of(&asset.name, format!("{}'s {field}: <{target}> is an entity, and an entity links to assets: it is ignored", asset.name))),
                        Some(_) => {}
                    }
                }
                for target in links.roles.values() {
                    if let Some(j) = self.find(target).filter(|&j| self.assets[j].links.is_none() && !upstream[i].contains(&j)) {
                        upstream[i].push(j);
                        fixed[i].push(j);
                    }
                }
                continue;
            }
            let tags = self.tags(i);
            for tag in tags.into_iter().flatten().filter(|tag| self.find(tag).is_some()) {
                let line = Warning::of(&asset.name, format!("{tag} is also a tag of {}: <{tag}> in its text is that tag, not this, so rename it", self.target(i)));
                if !said.contains(&line) {
                    said.push(line);
                }
            }
            for mention in mentions_in(&Value::Object(asset.fields.clone())) {
                // A tag of its package, in any case, stays text as written, whatever the project names.
                if tags.is_some_and(|tags| is_tag(tags, &mention)) {
                    continue;
                }
                let Some(j) = self.find(&mention) else {
                    unread.push((i, mention));
                    continue;
                };
                // An entity's links are edges too, through its sub-entities as well, so what it links to can't read it.
                if self.links_to(j, i) {
                    let line = Warning::of(&asset.name, format!("{} mentions {}, which is made from it: that mention stays plain text", asset.name, self.assets[j].name));
                    if !said.contains(&line) {
                        said.push(line);
                    }
                    continue;
                }
                if j != i && !upstream[i].contains(&j) {
                    upstream[i].push(j);
                    reads[i].push(j);
                }
            }
        }
        said.dedup();
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

    fn warn(&mut self, name: &str, message: String) {
        self.warnings.push(Warning::of(name, message));
    }

    /// Asset `i`'s inputs, flat: what it passes to its function, whichever part of the file says it, defaults too.
    pub fn flat_fields(&self, i: usize) -> Map<String, Value> {
        let asset = &self.assets[i];
        let passes = |(field, value): &(&String, &Value)| !RESERVED.contains(&field.as_str()) || (PINS.contains(&field.as_str()) && !mentions_in(value).is_empty());
        let mut flat: Map<String, Value> = asset.body.iter().filter(passes).map(|(field, value)| (field.clone(), value.clone())).collect();
        flat.extend(asset.body.get("options").and_then(Value::as_object).into_iter().flatten().map(|(field, value)| (field.clone(), value.clone())));
        // A pin over an input keeps it, and it is what the box shows.
        flat.extend(PINS.iter().filter_map(|field| Some(((*field).to_owned(), asset.fields.get(*field)?.clone()))));
        flat.extend(["models", "lora"].iter().filter_map(|field| Some(((*field).to_owned(), asset.body.get(*field)?.clone()))));
        flat
    }

    /// What asset `i` reads by a mention, each with the places its text says so.
    pub fn reads_of(&self, i: usize) -> Vec<Read> {
        let asset = &self.assets[i];
        let mut said = vec![];
        for (field, value) in &asset.fields {
            said_in(value, field.clone(), &mut said);
        }
        let read = |&j: &usize| {
            let key = name_key(&self.assets[j].name);
            let at = said.iter().filter(|(mention, _)| name_key(mention) == key).map(|(_, place)| place.clone());
            Read { name: self.assets[j].name.clone(), at: at.collect() }
        };
        asset.reads.iter().map(read).collect()
    }

    /// Whether asset `i` is what node `j`'s entity links to, directly or through a sub-entity.
    fn links_to(&self, j: usize, i: usize) -> bool {
        let Some(links) = &self.assets[j].links else { return false };
        let root = links.parent.as_deref().and_then(|parent| self.find(parent)).and_then(|root| self.assets[root].links.as_ref()).unwrap_or(links);
        let subs = root.subs.iter().filter_map(|sub| self.assets[self.find(sub)?].links.as_ref());
        root.own.values().chain(subs.flat_map(|sub| sub.own.values())).any(|target| self.find(target) == Some(i))
    }

    /// The tags of the package that makes asset `i`, once it has been read.
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

    /// Why asset `i` can't run, given its package's `tags`: each `<name>` in its text that names nothing and is no tag, and each file
    /// link to nothing of an entity it reads. Running it would send its words to a model with no picture, and charge for that. None: it can.
    pub fn missing_inputs(&self, i: usize, tags: &[String]) -> Option<String> {
        let own = self.mentions_of_nothing(i, tags).map(|mention| self.no_longer(mention, tags));
        let linked = self.links_to_nothing(i).map(|(j, role, target)| match role {
            "" => format!("{}: {}", self.assets[j].name, self.no_longer(target, tags)),
            role => format!("{}'s {role}: {}", self.assets[j].name, self.no_longer(target, tags)),
        });
        let mut said: Vec<String> = vec![];
        own.chain(linked).for_each(|line| if !said.contains(&line) { said.push(line) });
        (!said.is_empty()).then(|| format!("{}: {}", self.assets[i].name, said.join("; ")))
    }

    /// The names in `missing_inputs`, each once.
    pub fn missing_names(&self, i: usize, tags: &[String]) -> Vec<String> {
        let mut names: Vec<String> = vec![];
        let all = self.mentions_of_nothing(i, tags).chain(self.links_to_nothing(i).map(|(_, _, target)| target));
        all.for_each(|name| if !names.iter().any(|there| there == name) { names.push(name.to_owned()) });
        names
    }

    /// The name keys of what asset `i` read that is gone: mentions of nothing, and each entity with a file link to nothing.
    fn reads_gone(&self, i: usize) -> BTreeSet<String> {
        let mentions = self.unread.iter().filter(|(asset, _)| *asset == i).map(|(_, mention)| name_key(mention));
        mentions.chain(self.links_to_nothing(i).map(|(j, ..)| name_key(&self.assets[j].name))).collect()
    }

    fn mentions_of_nothing<'a>(&'a self, i: usize, tags: &'a [String]) -> impl Iterator<Item = &'a str> {
        self.unread.iter().filter(move |(asset, mention)| *asset == i && !is_tag(tags, mention)).map(|(_, mention)| mention.as_str())
    }

    /// Each entity asset `i` reads, a file link of it that goes nowhere, and where it says it goes.
    fn links_to_nothing(&self, i: usize) -> impl Iterator<Item = (usize, &str, &str)> {
        self.assets[i].reads.iter().flat_map(move |&j| {
            let links = self.assets[j].links.iter().flat_map(|links| &links.roles);
            links.filter(|(_, target)| self.find(target).is_none()).map(move |(role, target)| (j, role.as_str(), target.as_str()))
        })
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

    /// The asset named `name`: what can hold versions, as an entity can't.
    pub fn holds(&self, name: &str) -> Option<usize> {
        self.find(name).filter(|&i| self.assets[i].links.is_none())
    }

    /// The assets node `i` links, and its sub-entities': what rendering an entity renders.
    pub fn linked(&self, i: usize) -> Vec<usize> {
        let Some(links) = &self.assets[i].links else { return vec![] };
        let subs = links.subs.iter().filter_map(|sub| self.assets[self.find(sub)?].links.as_ref());
        let names = links.own.values().chain(links.roles.get("")).chain(subs.flat_map(|sub| sub.own.values()));
        let mut found: Vec<usize> = names.filter_map(|name| self.holds(name)).collect();
        found.sort_unstable();
        found.dedup();
        found
    }

    /// The package that makes asset `i`: its own `with`, else its kind's default. The platform names no package.
    pub fn target(&self, i: usize) -> String {
        self.assets[i].body.get("with").and_then(Value::as_str).unwrap_or_default().to_owned()
    }

    /// Asset `i`'s operation when Calcifer runs it itself: the reserved package `cozy`.
    pub fn tool(&self, i: usize) -> Option<Tool> {
        let asset = &self.assets[i];
        (asset.tool.as_deref()? == COZY).then_some(())?;
        match asset.kind.as_str() {
            "video" => Some(Tool::Cut),
            "image" => Some(Tool::Frame),
            _ => None,
        }
    }

    /// The one line that says why asset `i` has no operation.
    pub fn nothing_to_make(&self, i: usize) -> String {
        let asset = &self.assets[i];
        match asset.body.contains_key("with") {
            true => format!("{}: the package {COZY} makes a video (a cut) or an image (a frame), not {}: it has nothing to make", asset.name, asset.kind),
            false => format!("{} has no `render.with`, and no `defaults.{}.with`: it has nothing to make", asset.name, asset.kind),
        }
    }

    /// Whether asset `i` has an operation that can make it.
    pub fn makes(&self, i: usize) -> bool {
        !self.assets[i].hollow
    }

    /// Whether the file itself declares asset `i`, as what an entity makes is not.
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
        if let Some(links) = &asset.links {
            let mut files = Files::new();
            for (role, target) in &links.roles {
                let Some(j) = self.holds(target) else { continue };
                let Some(held) = &done[j].files else { return Standing { state: State::Waits, ..Standing::default() } };
                match role.as_str() {
                    "" => files.extend(held.clone()),
                    role => files.extend(folder::main_of(held).map(|main| (role.to_owned(), main.clone()))),
                }
            }
            return Standing { state: State::Linked, files: Some(files), ..Standing::default() };
        }
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
                    // What is gone from the project is no reason to make it again: nothing could be, and the pin is what stands.
                    let changed = match &inputs {
                        Some(now) if !version.inputs.is_empty() && *now != version.inputs => {
                            let gone = self.reads_gone(i);
                            changed(&version.digests, &digests).into_iter().filter(|name| !gone.contains(&name_key(name.trim_matches(['<', '>'])))).collect()
                        }
                        _ => vec![],
                    };
                    stand(State::Pinned { version: Some(version.number), changed }, Some(version.files.clone()))
                }
                None => stand(State::Pinned { version: None, changed: vec![] }, Some(Files::from([((*field).to_owned(), sha256.clone())]))),
            };
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
                None => super::tools::film_off(&asset.fields, &reads, lock, version),
            }
        };
        let holding = || versions.iter().rev().filter(|version| version.inputs == *now);
        if let Some(why) = holding().next().filter(|_| holding().all(|version| off(version).is_some())).and_then(off) {
            return stand(State::Stale(why), None);
        }
        match (holding().find(|version| off(version).is_none()), versions.last()) {
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
        if let Some(tool) = &asset.tool {
            top.insert("with".into(), json!(tool));
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
        // Every key made so far holds this: without it, every version of every project would go out of date.
        top.insert("writes".into(), json!([]));
        (digest(&Value::Object(top)), digests)
    }

    /// What a node says, flat, by the names its box and its function use: the file's split between what it is and how it's made ends
    /// here. The root wins where both say. `render`'s text is `instructions`, for adapters of this format.
    pub fn node_fields(&self, i: usize) -> Map<String, Value> {
        let asset = &self.assets[i];
        let mut flat = asset.body.get("options").and_then(Value::as_object).cloned().unwrap_or_default();
        flat.extend(asset.fields.clone());
        flat
    }

    /// The operation asset `i` is, as its adapter takes it: the function its tool names, its fields, and what it reads with what is known of each file.
    pub fn operation(&self, i: usize, function: Option<&str>, reads: &[ReadNow], lock: &Lock, keep: Option<Value>, stand: &[Standing]) -> Value {
        let asset = &self.assets[i];
        let file = |sha256: &String| lock.files.get(sha256).map_or_else(|| json!({"sha256": sha256}), folder::File::record);
        let reads: Vec<Value> = reads
            .iter()
            .map(|read| {
                let of = &self.assets[read.asset];
                let files: Map<String, Value> = read.files.iter().map(|(name, sha256)| (name.clone(), file(sha256))).collect();
                // What an entity shows was made by what made the asset it links.
                let links = of.links.as_ref();
                let shows = links.and_then(|links| links.roles.get("image").or(links.roles.get("")).or(links.roles.values().next()));
                let shown = shows.and_then(|asset| self.holds(asset)).unwrap_or(read.asset);
                let made = stand[shown].version().or(read.newest).and_then(|number| lock.asset(&self.assets[shown].name)?.version(number));
                let made_by = made.and_then(|made| made.made_by.as_ref()).map_or("", |by| by.with.as_str());
                json!({"name": read.name, "kind": of.kind, "fields": for_readers(&of.fields), "files": files, "made_by": made_by})
            })
            .collect();
        let options = asset.body.get("options").cloned().unwrap_or_else(|| json!({}));
        json!({"version": FORMAT, "asset": asset.name, "kind": asset.kind, "function": function.unwrap_or_default(), "fields": self.node_fields(i), "options": options, "reads": reads, "keep": keep})
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

/// What of a read crosses its mention.
fn for_readers(fields: &Map<String, Value>) -> Map<String, Value> {
    fields.iter().filter(|(field, _)| FOR_READERS.contains(&field.as_str())).map(|(field, value)| (field.clone(), value.clone())).collect()
}

/// `with` as the key holds it: no release, no root namespace, and local code by its directory's name, with the function it names, so moving
/// from a checkout to the published package puts nothing out of date.
pub fn plain_with(with: &str, folder: &Folder) -> String {
    let with = with.trim();
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

/// Each mention under `value`, with where: `summary`, `beats.0.description`, `pieces.1`.
fn said_in(value: &Value, place: String, found: &mut Vec<(String, String)>) {
    match value {
        Value::String(text) => found.extend(mentions(text).into_iter().map(|mention| (mention.to_owned(), place.clone()))),
        Value::Array(items) => items.iter().enumerate().for_each(|(n, item)| said_in(item, format!("{place}.{n}"), found)),
        Value::Object(fields) => fields.iter().for_each(|(field, item)| said_in(item, format!("{place}.{field}"), found)),
        _ => {}
    }
}

/// What passes to the adapter.
fn fields_of(body: &Map<String, Value>) -> Map<String, Value> {
    (body.iter())
        .filter(|(field, value)| !RESERVED.contains(&field.as_str()) || (PINS.contains(&field.as_str()) && !mentions_in(value).is_empty()))
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect()
}

/// `body` with no `writes`, its own or a beat's, so a file written when a shot made pictures keeps its key. Whether it had any.
fn without_writes(body: &mut Map<String, Value>) -> bool {
    let own = body.remove("writes").is_some();
    let beats = body.get_mut("beats").and_then(Value::as_array_mut).into_iter().flatten();
    beats.filter_map(Value::as_object_mut).fold(own, |had, beat| beat.remove("writes").is_some() || had)
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

/// An asset's lines, or a section of `defaults`, as the platform reads them: at its root what it is, in `render` how to make it.
/// `render` is its text, or a map where `with`, `prompt`, `models` and `lora` are the platform's and every other key is the adapter's
/// (`op.options`). On the wire the text is `instructions`, as it always was.
fn unfolded(name: &str, mut body: Map<String, Value>, warnings: &mut Vec<Warning>) -> Map<String, Value> {
    for field in RENDERED {
        if body.remove(field).is_some() {
            warnings.push(Warning::of(name, format!("{name}'s {field}: it goes under `render:`; it is ignored")));
        }
    }
    match body.remove("render") {
        Some(Value::Object(render)) => {
            let mut options = Map::new();
            for (field, value) in render {
                match field.as_str() {
                    "prompt" => drop(body.insert("instructions".into(), value)),
                    "with" | "models" | "lora" => drop(body.insert(field, value)),
                    _ => drop(options.insert(field, value)),
                }
            }
            if !options.is_empty() {
                body.insert("options".into(), Value::Object(options));
            }
        }
        Some(Value::Null) | None => {}
        Some(text) => drop(body.insert("instructions".into(), text)),
    }
    body
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

/// Whether a scalar at this path names a file, so that a number's shape is a hash's prefix by its text: an entity's fields, and an
/// asset's `image`, `video` or `audio`, wherever in it. `12345678e012` isn't 1.2345678e19, and `012345678901` isn't 12345678901,
/// which is another file's prefix.
pub(super) fn names_a_file(path: &[&str]) -> bool {
    match path {
        ["entities", ..] => true,
        ["assets", .., field] => PINS.contains(field),
        _ => false,
    }
}

/// A new empty project's project.yaml.
pub fn blank() -> String {
    format!("version: {FORMAT}\n")
}

/// `text` as it calls something new, if it can: through `as_name`, a `.` as `_`, since a `.` means a sub-entity or a numbered write.
pub fn plain_name(text: &str) -> Option<String> {
    let name = as_name(&text.replace('.', "_"));
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

/// `name` as a key of a mapping: quoted where YAML in general would read it as something other than this text.
pub fn yaml_key(name: &str) -> String {
    match protocol::yaml::to_json(name) {
        Ok(Value::String(read)) if read == name && name.parse::<f64>().is_err() => name.to_owned(),
        _ => format!("\"{name}\""),
    }
}

/// `text` with one constant more under `assets:`, on a line of its own after what is there, the section made if it isn't. Nothing
/// else moves, and what it made is checked to read as this project with that one asset more. Err: why not, and what to write by hand.
pub fn constant(text: &str, name: &str, field: &str, value: &str) -> Result<String, Error> {
    let one = format!("{}: {{{field}: {value}}}", yaml_key(name));
    let by_hand = |why: &str| invalid("project_edit", format!("{name} can't be added to {}: {why}", folder::PROJECT)).remedy(format!("write `{one}` under `assets:` yourself"));
    let read = |text: &str| protocol::yaml::to_json(text);
    let mut wanted = read(text).map_err(|why| invalid("project_yaml", format!("{}: {why}", folder::PROJECT)))?;
    if wanted.is_null() {
        wanted = json!({});
    }
    let top = wanted.as_object_mut().ok_or_else(|| by_hand("it is no mapping of sections"))?;
    let assets = top.entry("assets").or_insert(Value::Null);
    if assets.is_null() {
        *assets = json!({});
    }
    let written = read(value).unwrap_or_else(|_| json!(value));
    assets.as_object_mut().ok_or_else(|| by_hand("its `assets:` is no mapping"))?.insert(name.to_owned(), json!({field: written}));
    under_section(text, "assets", &format!("{one}\n")).into_iter().find(|edited| read(edited).is_ok_and(|read| read == wanted)).ok_or_else(|| by_hand("the edit would change something else"))
}

/// The ways to add `block`, a key's lines written from column 0, to `section`, keeping what is there, to try in turn. A mapping all on one
/// line takes only a single line, and a block of several needs lines of its own.
pub(super) fn under_section(text: &str, section: &str, block: &str) -> Vec<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let indented = |indent: usize| block.split_inclusive('\n').map(|line| if line.trim().is_empty() { line.to_owned() } else { format!("{}{line}", " ".repeat(indent)) }).collect::<String>();
    let Some(at) = lines.iter().position(|line| spaces(line) == 0 && key_of(line).as_deref() == Some(section)) else {
        let glue = if text.is_empty() { "" } else if text.ends_with('\n') { "\n" } else { "\n\n" };
        return vec![format!("{text}{glue}{section}:\n{}", indented(2))];
    };
    let (written, comment) = split_comment(&lines[at][lines[at].find(':').expect("a key") + 1..]);
    let comment = comment.map_or(String::new(), |comment| format!("  {comment}"));
    match written {
        "" => {
            let end = ((at + 1)..lines.len()).find(|&n| significant(lines[n]) && spaces(lines[n]) == 0).unwrap_or(lines.len());
            let indent = ((at + 1)..end).find(|&n| significant(lines[n])).map_or(2, |n| spaces(lines[n]));
            let last = ((at + 1)..end).rev().find(|&n| significant(lines[n]) || (!lines[n].trim().is_empty() && spaces(lines[n]) > indent));
            // Past a text's last line, and what comments on it, unless that isn't the text's end, then at the section's end.
            [last.unwrap_or(at), end - 1].iter().map(|&after| insert_line(&lines, after, &indented(indent))).collect()
        }
        "{}" | "~" | "null" => vec![replace_line(&lines, at, &format!("{section}:{comment}\n{}", indented(2)))],
        written => match written.strip_prefix('{').and_then(|inner| inner.strip_suffix('}')) {
            Some(inner) if block.trim_end().lines().count() == 1 => vec![replace_line(&lines, at, &format!("{section}: {{{}, {}}}{comment}\n", block.trim_end(), inner.trim_start()))],
            _ => vec![],
        },
    }
}

/// `text` with asset `from` called `to`: its key, and every `<from>`. Comments and formatting stay.
pub fn renamed_in(text: &str, from: &str, to: &str) -> Result<String, Error> {
    let by_hand = || invalid("project_edit", format!("{from} can't be renamed in {}: it has no line of its own under `assets:`", folder::PROJECT)).remedy(format!("rename it there yourself, then `cozy project rename {from} {to}`"));
    let (mut said, mut rest) = (String::with_capacity(text.len()), text);
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let inner = after.find(['<', '>', '\n']).filter(|&end| after.as_bytes()[end] == b'>').map(|end| &after[..end]);
        match inner.filter(|inner| is_name(inner) && name_key(inner) == name_key(from)) {
            Some(inner) => {
                said.push_str(&rest[..=open]);
                said.push_str(to);
                rest = &after[inner.len()..];
            }
            None => {
                said.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    said.push_str(rest);
    let place = Place::find(&said, from).ok_or_else(by_hand)?;
    let written = key_of(place.lines[place.key]).ok_or_else(by_hand)?;
    Ok(replace_line(&place.lines, place.key, &place.lines[place.key].replacen(&written, &yaml_key(to), 1)))
}

/// `value` as a file field writes it: quoted when YAML would read it as a number.
pub fn yaml_scalar(value: &str) -> String {
    match value.parse::<f64>() {
        Ok(_) => format!("\"{value}\""),
        Err(_) => value.to_owned(),
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
    let place = Place::find(text, name).ok_or_else(|| by_hand("it has no line of its own in the file".into()))?;
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
    let place = Place::find(text, name).ok_or_else(|| by_hand("it has no line of its own in the file".into()))?;
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
    let (mut before, mut after) = (read(text)?, read(edited)?);
    let asset = slot(&mut before, name).ok_or("it isn't under `assets:`")?;
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
    if let Some(asset) = slot(&mut after, name).filter(|asset| asset.is_null()) {
        *asset = json!({});
    }
    (before == after).then_some(()).ok_or_else(|| "the edit would change something else".into())
}

/// The value `name` is in a project's JSON: an asset, or an entity or sub-entity.
pub(super) fn slot<'a>(doc: &'a mut Value, name: &str) -> Option<&'a mut Value> {
    let key_in = |map: &Value, name: &str| map.as_object()?.keys().find(|key| name_key(key) == name_key(name)).cloned();
    for section in ["assets", "entities"] {
        if let Some(key) = key_in(&doc[section], name) {
            return doc[section].get_mut(&key);
        }
    }
    let (entity, sub) = name.match_indices('.').map(|(at, _)| (&name[..at], &name[at + 1..])).find(|(entity, sub)| {
        key_in(&doc["entities"], entity).is_some_and(|key| key_in(&doc["entities"][&key], sub).is_some())
    })?;
    let entity = key_in(&doc["entities"], entity)?;
    let sub = key_in(&doc["entities"][&entity], sub)?;
    doc["entities"][&entity].get_mut(&sub)
}

/// The input a pin in `field` keeps in a comment on its line.
fn pinned_over(text: &str, name: &str, field: &str) -> Option<String> {
    let place = Place::find(text, name)?;
    let (_, rest) = field_rest(place.lines[place.field(field)?], field);
    let (_, was) = rest.split_once("# was ")?;
    Some(split_comment(was).0.to_owned())
}

/// Where an asset's lines are in the text.
pub(super) struct Place<'a> {
    pub lines: Vec<&'a str>,
    /// The line that names it.
    pub key: usize,
    /// The lines after it that are its body.
    pub body: std::ops::Range<usize>,
    /// How far its fields are indented.
    pub child: usize,
    /// All on one line, `name: {…}`: what is before `{`, what is inside, what follows `}`.
    pub flow: Option<(String, String, String)>,
}

impl<'a> Place<'a> {
    /// The lines of `name` under `assets:`, or of an entity or sub-entity (`kyogre.primordial`), whose links can hold hashes.
    pub fn find(text: &'a str, name: &str) -> Option<Self> {
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let section = |section: &str| {
            let start = lines.iter().position(|line| spaces(line) == 0 && split_comment(line).0 == section)? + 1;
            let end = (start..lines.len()).find(|&n| significant(lines[n]) && spaces(lines[n]) == 0).unwrap_or(lines.len());
            Some(start..end)
        };
        let of_entity = || {
            let entities = section("entities:")?;
            block(&lines, entities.clone(), name).or_else(|| {
                name.match_indices('.').find_map(|(at, _)| {
                    let (_, entity, ..) = block(&lines, entities.clone(), &name[..at])?;
                    block(&lines, entity, &name[at + 1..])
                })
            })
        };
        let (key, body, child, flow) = section("assets:").and_then(|assets| block(&lines, assets, name)).or_else(of_entity)?;
        Some(Self { lines, key, body, child, flow })
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

pub(super) fn significant(line: &str) -> bool {
    !line.trim().is_empty() && !line.trim_start().starts_with('#')
}

pub(super) fn spaces(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// The lines of the key `name` among the mapping that fills `within`: the line opening it, its body, how far its fields are
/// indented, and for one all on one line, what is before `{`, what is inside, and what follows `}`.
#[allow(clippy::type_complexity)]
fn block(lines: &[&str], within: std::ops::Range<usize>, name: &str) -> Option<(usize, std::ops::Range<usize>, usize, Option<(String, String, String)>)> {
    let (start, end) = (within.start, within.end);
    let at = spaces(lines[start..end].iter().find(|line| significant(line))?);
    let key = (start..end).find(|&n| significant(lines[n]) && spaces(lines[n]) == at && key_of(lines[n]).is_some_and(|key| name_key(&key) == name_key(name)))?;
    let finish = ((key + 1)..end).find(|&n| significant(lines[n]) && spaces(lines[n]) <= at).unwrap_or(end);
    let first = ((key + 1)..finish).find(|&n| significant(lines[n]));
    let child = first.map_or(at + 2, |n| spaces(lines[n]));
    // Where its value starts: past the colon that ends the key.
    let after = lines[key].find(": ").map(|colon| colon + 2);
    let flow = after.and_then(|after| {
        let (head, value) = lines[key].split_at(after);
        let (written, _) = split_comment(value);
        let inner = written.strip_prefix('{')?.strip_suffix('}')?;
        let close = value.find(written)? + written.len();
        Some((head.to_owned(), inner.to_owned(), value[close..].to_owned()))
    });
    Some((key, (key + 1)..finish, child, flow))
}

/// The key a line opens: `name:`, `"name":`, `'name':`.
pub(super) fn key_of(line: &str) -> Option<String> {
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
pub(super) fn field_rest<'a>(line: &'a str, field: &str) -> (&'a str, &'a str) {
    let spaces = line.len() - line.trim_start_matches(' ').len();
    (&line[..spaces], line[spaces + field.len()..].trim_start_matches(':').trim())
}

/// What a value writes, and its comment. A `#` in quotes is no comment, and neither is one with no space before it.
pub(super) fn split_comment(rest: &str) -> (&str, Option<&str>) {
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
