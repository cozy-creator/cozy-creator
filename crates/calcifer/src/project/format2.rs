//! Format 2 as cozy 0.19 read it, for the one move to format 3: each asset's state, worked out from project.yaml and project.lock as
//! `cozy project status` did, so what was out of date stays so with no word from that cozy. Only what decides a state is here: the file
//! after `defaults`, an entity's nodes, a beat's writes, what each reads, and the memo key. It goes when format 2 does.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

use api::yaml::keys;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    folder::{Files, hex_of, main_of},
    format::canonical,
    graph::{Graph, mentions, mentions_in, name_key},
    tools::{Canvas, Piece, Shape, measure, past_end},
};

const PINS: [&str; 3] = ["image", "video", "audio"];
/// The links of an entity that a read of it gets as files.
const ROLES: [&str; 4] = ["image", "video", "audio", "voice"];
/// What `render` unfolds to, never at an asset's root.
const RENDERED: [&str; 5] = ["with", "instructions", "options", "models", "lora"];
const RESERVED: [&str; 10] = ["render", "with", "kind", "models", "lora", "options", "image", "video", "audio", "writes"];
const COZY: &str = "cozy";
/// What `cozy` read of a cut or a frame.
const TOOL_FIELDS: [&str; 6] = ["pieces", "fps", "aspect_ratio", "resolution", "video", "at"];
/// How long a beat with no `duration` lasted, where its writes are concerned.
const BEAT_S: f64 = 5.0;
const VIDEOS: [&str; 5] = ["mp4", "mov", "m4v", "webm", "mkv"];
const SOUNDS: [&str; 6] = ["wav", "mp3", "flac", "ogg", "m4a", "aac"];

/// project.lock of format 2: what decides a state.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct Lock {
    files: BTreeMap<String, File>,
    assets: BTreeMap<String, Held>,
    reserved: BTreeMap<String, Vec<String>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct File {
    path: String,
    width: u64,
    height: u64,
    fps: f64,
    frames: u64,
}

impl File {
    /// The role that pins it, by what it is.
    fn field(&self) -> &'static str {
        let extension = Path::new(&self.path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        match extension.as_str() {
            extension if VIDEOS.contains(&extension) => "video",
            extension if SOUNDS.contains(&extension) => "audio",
            _ => "image",
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Held {
    versions: Vec<Version>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Version {
    number: u32,
    inputs: String,
    files: Files,
    writes: BTreeMap<String, Files>,
    without: Vec<Without>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Without {
    piece: String,
    absent: bool,
}

impl Lock {
    fn asset(&self, name: &str) -> Option<&Held> {
        self.assets.get(&name_key(name))
    }

    /// The sha256 a pin's value names, if exactly one file's does.
    fn file_named(&self, written: &str) -> Option<String> {
        if written.is_empty() || !written.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let prefix = written.to_ascii_lowercase();
        match self.files.keys().filter(|sha256| hex_of(sha256).starts_with(&prefix)).collect::<Vec<_>>().as_slice() {
            [one] => Some((*one).clone()),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Asset {
    name: String,
    kind: String,
    body: Map<String, Value>,
    /// What passed to the adapter.
    fields: Map<String, Value>,
    pin: Option<(&'static str, String)>,
    /// A free frame a beat named.
    frame: bool,
    reads: Vec<usize>,
    writes: Vec<Write>,
    /// The shot whose function sent it, read after the beat that wrote it.
    writer: Option<usize>,
    /// The shot one of whose beats wrote it.
    beat_of: Option<usize>,
    /// An entity, a sub-entity or a link of one.
    links: Option<Links>,
}

#[derive(Clone, Default)]
struct Links {
    roles: BTreeMap<String, String>,
    own: BTreeMap<String, String>,
    subs: Vec<String>,
    parent: Option<String>,
}

/// A revision a shot's operation sent, as its key held it.
struct Write {
    key: String,
    record: Value,
}

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Cut,
    Frame,
}

#[derive(Clone, Default)]
enum State {
    Pinned(Option<u32>),
    Current(u32),
    /// The writer's take that wrote it.
    Written(u32),
    Linked,
    Stale,
    /// No version yet, nothing written, or something it reads has no state.
    #[default]
    Missing,
}

#[derive(Clone, Default)]
struct Stood {
    state: State,
    files: Option<Files>,
}

impl Stood {
    fn version(&self) -> Option<u32> {
        match self.state {
            State::Pinned(version) => version,
            State::Current(version) | State::Written(version) => Some(version),
            _ => None,
        }
    }
}

struct ReadNow {
    name: String,
    asset: usize,
    files: Files,
}

/// project.yaml of format 2 as 0.19 read it.
pub(crate) struct Doc {
    assets: Vec<Asset>,
    index: HashMap<String, usize>,
    order: Vec<usize>,
    reserved: BTreeMap<String, Vec<String>>,
    dir: PathBuf,
}

impl Doc {
    /// `top`, `text` read, with `lock`, in project folder `dir`.
    pub(crate) fn read(dir: &Path, text: &str, top: &Map<String, Value>, lock: &Lock) -> Self {
        let empty = Map::new();
        let defaults: Map<String, Value> = (top.get("defaults").and_then(Value::as_object).unwrap_or(&empty).iter())
            .filter(|(section, _)| PINS.contains(&section.as_str()))
            .map(|(section, value)| (section.clone(), Value::Object(unfolded(Map::from_iter([("render".to_owned(), value.clone())])))))
            .collect();
        let declared = top.get("assets").and_then(Value::as_object).unwrap_or(&empty);
        let entities = top.get("entities").and_then(Value::as_object).unwrap_or(&empty);
        let mut doc = Self { assets: vec![], index: HashMap::new(), order: vec![], reserved: lock.reserved.clone(), dir: dir.to_owned() };
        for name in keys(text, &["assets"]) {
            if let Some(written) = declared.get(&name) {
                doc.declare(text, &name, written, &defaults, lock);
            }
        }
        let declared = doc.assets.len();
        for i in 0..declared {
            let writes = if doc.assets[i].kind == "video" { beat_writes(&doc.assets[i].body) } else { vec![] };
            for (beat, image, at) in writes {
                doc.write(i, beat, &image, at);
            }
        }
        for name in keys(text, &["entities"]) {
            if let Some(written) = entities.get(&name) {
                doc.entity(text, &name, written, &defaults, lock);
            }
        }
        doc.resolve();
        doc
    }

    /// Each asset's row as `cozy project status --json` gave it: its name, `standing`, and the version that is its state.
    pub(crate) fn rows(&self, lock: &Lock) -> Vec<Value> {
        let stood = self.stand(lock);
        let rows = self.assets.iter().zip(&stood).filter(|(asset, _)| asset.links.is_none()).map(|(asset, stood)| {
            let (standing, version) = match stood.state {
                State::Stale => ("out_of_date", None),
                State::Missing => ("missing", None),
                // A revision stands on its writer's take, no version of its own.
                State::Written(_) => ("current", None),
                _ => ("current", stood.version()),
            };
            json!({"name": asset.name, "standing": standing, "version": version})
        });
        rows.collect()
    }

    /// The shot one of whose beats wrote what `mention` names: format 3 has no such thing.
    pub(crate) fn beat_of(&self, mention: &str) -> Option<&str> {
        let i = self.find(mention)?;
        Some(&self.assets[self.assets[i].beat_of?].name)
    }

    /// Each free frame a beat wrote: its name, the shot it is of, and when, in seconds into the shot.
    pub(crate) fn frames(&self) -> Vec<(String, String, f64)> {
        let frame = |asset: &Asset| Some((asset.name.clone(), self.assets[asset.beat_of?].name.clone(), asset.body.get("at")?.as_f64()?));
        self.assets.iter().filter(|asset| asset.frame).filter_map(frame).collect()
    }

    /// What `mention` meant.
    pub(crate) fn meant(&self, mention: &str) -> String {
        match (self.find(mention), self.beat_of(mention)) {
            (_, Some(shot)) => format!("was what a beat of {shot} wrote"),
            (Some(i), None) => format!("named {}", self.assets[i].name),
            (None, None) => "was plain text".to_owned(),
        }
    }

    fn declare(&mut self, text: &str, name: &str, written: &Value, defaults: &Map<String, Value>, lock: &Lock) -> Option<usize> {
        let key = name_key(name);
        if self.index.contains_key(&key) {
            return None;
        }
        let body = unfolded(written.as_object().cloned().unwrap_or_default());
        let hash = |field: &&str| body.get(*field).and_then(Value::as_str).is_some_and(|text| mentions(text).is_empty());
        let held = PINS.iter().copied().find(hash);
        let written_kind = text_of(body.get("kind"));
        let kind = if PINS.contains(&written_kind.as_str()) { written_kind } else { held.unwrap_or("image").to_owned() };
        let constant = held.is_some() && body.keys().all(|key| key == "kind" || PINS.contains(&key.as_str()));
        let (mut merged, mut options) = (Map::new(), Map::new());
        if let Some(section) = defaults.get(&kind).and_then(Value::as_object).filter(|_| !constant && !body.contains_key("with")) {
            options.extend(section.get("options").and_then(Value::as_object).cloned().unwrap_or_default());
            merged.extend(section.clone());
        }
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
                    merged.remove(field);
                    continue;
                }
                _ => continue,
            };
            match lock.file_named(&written).filter(|_| field == kind) {
                Some(sha256) => pin = Some((field, sha256)),
                None => drop(merged.remove(field)),
            }
        }
        let mut fields = fields_of(&merged);
        // A pin over an input left the input in a comment on its line, and while pinned it still counted.
        if let Some((field, _)) = &pin
            && let Some(was) = pinned_over(text, name, field)
        {
            fields.insert((*field).to_owned(), json!(was));
        }
        self.index.insert(key, self.assets.len());
        self.assets.push(Asset { name: name.to_owned(), kind, body: merged, fields, pin, ..Asset::default() });
        Some(self.assets.len() - 1)
    }

    /// An entity: it, each sub-entity and each link a node of the graph.
    fn entity(&mut self, text: &str, name: &str, written: &Value, defaults: &Map<String, Value>, lock: &Lock) {
        let Some(body) = written.as_object() else { return };
        let (own, subs) = self.links_of(text, name, body, true, defaults, lock);
        let roles = |own: &BTreeMap<String, String>| -> BTreeMap<String, String> { own.iter().filter(|(field, _)| ROLES.contains(&field.as_str())).map(|(field, asset)| (field.clone(), asset.clone())).collect() };
        let kind_of = |body: &Map<String, Value>, or: &str| Some(text_of(body.get("kind"))).filter(|kind| !kind.is_empty()).unwrap_or_else(|| or.to_owned());
        let kind = kind_of(body, "image");
        let description = body.get("description").cloned();
        let mut named = vec![];
        for (field, sub) in &subs {
            let full = format!("{name}.{field}");
            let (mine, _) = self.links_of(text, &full, sub, false, defaults, lock);
            let mut files = roles(&own);
            files.extend(roles(&mine));
            let links = Links { roles: files, own: mine, subs: vec![], parent: Some(name.to_owned()) };
            if self.node(&full, kind_of(sub, &kind), sub.get("description").cloned().or_else(|| description.clone()), links) {
                named.push(full);
            }
        }
        self.node(name, kind.clone(), description.clone(), Links { roles: roles(&own), own: own.clone(), subs: named, parent: None });
        for (field, asset) in own {
            let full = format!("{name}.{field}");
            if asset != full {
                let links = Links { roles: BTreeMap::from([(String::new(), asset)]), parent: Some(name.to_owned()), ..Links::default() };
                self.node(&full, kind.clone(), description.clone(), links);
            }
        }
    }

    /// An entity's links and sub-entities. A hash in a link made a constant asset named for the link.
    #[allow(clippy::too_many_arguments)]
    fn links_of(&mut self, text: &str, name: &str, body: &Map<String, Value>, nests: bool, defaults: &Map<String, Value>, lock: &Lock) -> (BTreeMap<String, String>, Vec<(String, Map<String, Value>)>) {
        let (mut links, mut subs) = (BTreeMap::new(), vec![]);
        for (field, value) in body {
            match (field.as_str(), value) {
                ("kind" | "description" | "render", _) => {}
                (_, Value::Object(sub)) if nests => subs.push((field.clone(), sub.clone())),
                (_, Value::String(said)) if matches!(mentions(said).as_slice(), [one] if said.trim() == format!("<{one}>")) => {
                    links.insert(field.clone(), mentions(said)[0].to_owned());
                }
                (_, Value::String(written)) => {
                    let Some(sha256) = lock.file_named(written) else { continue };
                    let role = lock.files.get(&sha256).map_or("image", File::field);
                    let full = format!("{name}.{field}");
                    let constant = Value::Object(Map::from_iter([(role.to_owned(), Value::String(written.clone()))]));
                    if self.declare(text, &full, &constant, defaults, lock).is_some() {
                        links.insert(field.clone(), full);
                    }
                }
                _ => {}
            }
        }
        (links, subs)
    }

    /// A node that is no work, unless an asset has the name.
    fn node(&mut self, name: &str, kind: String, description: Option<Value>, links: Links) -> bool {
        let key = name_key(name);
        if self.index.contains_key(&key) {
            return false;
        }
        let fields = description.into_iter().map(|text| ("description".to_owned(), text)).collect();
        self.index.insert(key, self.assets.len());
        self.assets.push(Asset { name: name.to_owned(), kind, fields, links: Some(links), ..Asset::default() });
        true
    }

    /// `image`, which beat `beat` of shot `i` writes `at` seconds into the shot: sent by its function if the shot reads it after that beat,
    /// else a free frame. A pinned image is read, never written; of two writes of one name the first stands.
    fn write(&mut self, i: usize, beat: usize, image: &str, at: f64) {
        let key = name_key(image);
        let revision = match self.index.get(&key) {
            Some(&revision) => revision,
            None => {
                self.index.insert(key.clone(), self.assets.len());
                self.assets.push(Asset { name: image.to_owned(), kind: "image".into(), ..Asset::default() });
                self.assets.len() - 1
            }
        };
        if self.assets[revision].writer.is_some() || self.assets[revision].frame || self.assets[revision].pin.is_some() {
            return;
        }
        let writer = self.assets[i].name.clone();
        let mut read_after = self.assets[i].fields.clone();
        if let Some(Value::Array(beats)) = read_after.get_mut("beats") {
            beats.drain(..(beat + 1).min(beats.len()));
        }
        if mentions_in(&Value::Object(read_after)).iter().any(|mention| name_key(mention) == key) {
            let record = json!({"name": self.assets[revision].name, "at_s": at, "fields": self.assets[revision].fields});
            self.assets[revision].writer = Some(i);
            self.assets[i].writes.push(Write { key, record });
        } else {
            let own = &mut self.assets[revision];
            own.frame = true;
            own.body.insert("with".into(), json!(COZY));
            own.body.insert("video".into(), json!(format!("<{writer}>")));
            own.body.insert("at".into(), json!(at));
            own.fields = fields_of(&own.body);
        }
        self.assets[revision].beat_of = Some(i);
    }

    /// Each asset's reads, and the order. A mention of nothing stays text; one that closes a cycle too.
    fn resolve(&mut self) {
        let n = self.assets.len();
        let (mut upstream, mut fixed, mut reads) = (vec![vec![]; n], vec![vec![]; n], vec![vec![]; n]);
        // A link of an entity says what the asset says of itself.
        for i in 0..n {
            let target = self.assets[i].links.as_ref().and_then(|links| self.find(links.roles.get("")?));
            if let Some(description) = target.and_then(|j| self.assets[j].fields.get("description").filter(|_| self.assets[j].links.is_none()).cloned()) {
                self.assets[i].fields.insert("description".into(), description);
            }
        }
        for i in 0..n {
            let asset = &self.assets[i];
            if let Some(writer) = asset.writer {
                upstream[i].push(writer);
                fixed[i].push(writer);
                continue;
            }
            if let Some(links) = &asset.links {
                for target in links.roles.values() {
                    if let Some(j) = self.find(target).filter(|&j| self.assets[j].links.is_none() && !upstream[i].contains(&j)) {
                        upstream[i].push(j);
                        fixed[i].push(j);
                    }
                }
                continue;
            }
            let tags = self.reserved.get(&plain_with(&self.target(i)));
            let own: BTreeSet<&str> = asset.writes.iter().map(|write| write.key.as_str()).collect();
            for mention in mentions_in(&Value::Object(asset.fields.clone())) {
                if tags.is_some_and(|tags| tags.iter().any(|tag| name_key(tag) == name_key(&mention))) {
                    continue;
                }
                let Some(j) = self.find(&mention) else { continue };
                if self.links_to(j, i) {
                    continue;
                }
                if j != i && !own.contains(name_key(&self.assets[j].name).as_str()) && !upstream[i].contains(&j) {
                    upstream[i].push(j);
                    reads[i].push(j);
                }
            }
        }
        let graph = Graph::new(upstream, &fixed);
        for &(reader, read) in &graph.closing {
            reads[reader].retain(|j| *j != read);
        }
        for (asset, reads) in self.assets.iter_mut().zip(reads) {
            asset.reads = reads;
        }
        self.order = graph.order;
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.index.get(&name_key(name)).copied()
    }

    fn holds(&self, name: &str) -> Option<usize> {
        self.find(name).filter(|&i| self.assets[i].links.is_none())
    }

    /// Whether asset `i` is what node `j`'s entity links to, directly or through a sub-entity.
    fn links_to(&self, j: usize, i: usize) -> bool {
        let Some(links) = &self.assets[j].links else { return false };
        let root = links.parent.as_deref().and_then(|parent| self.find(parent)).and_then(|root| self.assets[root].links.as_ref()).unwrap_or(links);
        let subs = root.subs.iter().filter_map(|sub| self.assets[self.find(sub)?].links.as_ref());
        root.own.values().chain(subs.flat_map(|sub| sub.own.values())).any(|target| self.find(target) == Some(i))
    }

    fn target(&self, i: usize) -> String {
        self.assets[i].body.get("with").and_then(Value::as_str).unwrap_or_default().to_owned()
    }

    fn tool(&self, i: usize) -> Option<Tool> {
        let asset = &self.assets[i];
        if asset.frame {
            return Some(Tool::Frame);
        }
        (plain_with(asset.body.get("with")?.as_str()?) == COZY).then_some(())?;
        match asset.kind.as_str() {
            "video" => Some(Tool::Cut),
            "image" => Some(Tool::Frame),
            _ => None,
        }
    }

    fn stand(&self, lock: &Lock) -> Vec<Stood> {
        let mut done = vec![Stood::default(); self.assets.len()];
        for &i in &self.order {
            done[i] = self.standing(i, lock, &done);
        }
        done
    }

    /// The hash in the file, else its newest version for these inputs, else none.
    fn standing(&self, i: usize, lock: &Lock, done: &[Stood]) -> Stood {
        let asset = &self.assets[i];
        if let Some(links) = &asset.links {
            let mut files = Files::new();
            for (role, target) in &links.roles {
                let Some(j) = self.holds(target) else { continue };
                let Some(held) = &done[j].files else { return Stood::default() };
                match role.as_str() {
                    "" => files.extend(held.clone()),
                    role => files.extend(main_of(held).map(|main| (role.to_owned(), main.clone()))),
                }
            }
            return Stood { state: State::Linked, files: Some(files) };
        }
        let versions = lock.asset(&asset.name).map(|held| held.versions.as_slice()).unwrap_or_default();
        if let Some((field, sha256)) = &asset.pin {
            return match versions.iter().rev().find(|version| main_of(&version.files) == Some(sha256)) {
                Some(version) => Stood { state: State::Pinned(Some(version.number)), files: Some(version.files.clone()) },
                None => Stood { state: State::Pinned(None), files: Some(Files::from([((*field).to_owned(), sha256.clone())])) },
            };
        }
        if let Some(writer) = asset.writer {
            let take = done[writer].version().and_then(|number| lock.asset(&self.assets[writer].name)?.versions.iter().find(|version| version.number == number));
            return match take.and_then(|take| Some((take.number, take.writes.get(&name_key(&asset.name))?))) {
                Some((version, files)) => Stood { state: State::Written(version), files: Some(files.clone()) },
                None => Stood::default(),
            };
        }
        let Some(reads) = self.reading(i, lock, done) else { return Stood::default() };
        let now = self.key(i, &reads);
        // A film short of its pieces, or that went without one now here, wasn't its state: it cut again.
        let off = |version: &Version| {
            if self.tool(i) != Some(Tool::Cut) {
                return false;
            }
            let here = |piece: &str| {
                let read = reads.iter().find(|read| name_key(&read.name) == name_key(piece));
                read.and_then(|read| lock.files.get(main_of(&read.files)?)).is_some_and(|file| self.dir.join(&file.path).is_file())
            };
            version.without.iter().any(|left| left.absent && here(&left.piece)) || self.film_off(i, &reads, lock, version)
        };
        let holding = || versions.iter().rev().filter(|version| version.inputs == now);
        if holding().next().is_some() && holding().all(off) {
            return Stood { state: State::Stale, files: None };
        }
        match (holding().find(|version| !off(version)), versions.last()) {
            (Some(version), _) => Stood { state: State::Current(version.number), files: Some(version.files.clone()) },
            (None, None) => Stood::default(),
            (None, Some(_)) => Stood { state: State::Stale, files: None },
        }
    }

    /// What asset `i` reads now; None while something has no state. A tool read state, else the newest version.
    fn reading(&self, i: usize, lock: &Lock, done: &[Stood]) -> Option<Vec<ReadNow>> {
        let tool = self.tool(i).is_some();
        let mut reads = vec![];
        for &j in &self.assets[i].reads {
            let name = self.assets[j].name.clone();
            match &done[j].files {
                Some(files) => reads.push(ReadNow { name, asset: j, files: files.clone() }),
                None if tool => {
                    let newest = lock.asset(&self.assets[j].name).and_then(|held| held.versions.last());
                    reads.extend(newest.map(|newest| ReadNow { name, asset: j, files: newest.files.clone() }));
                }
                None => return None,
            }
        }
        (!tool || !reads.is_empty()).then_some(reads)
    }

    /// The memo key: exactly what the adapter was given, hashed.
    fn key(&self, i: usize, reads: &[ReadNow]) -> String {
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
        let fields: Map<String, Value> = (asset.fields.iter()).filter(|(field, _)| !tool || TOOL_FIELDS.contains(&field.as_str())).map(|(field, value)| (field.clone(), value.clone())).collect();
        top.insert("fields".into(), Value::Object(fields));
        let mut sorted: Vec<&ReadNow> = reads.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        let reads: Vec<Value> = (sorted.into_iter())
            .map(|read| match tool {
                true => json!({"name": read.name, "files": read.files}),
                false => json!({"name": read.name, "kind": self.assets[read.asset].kind, "fields": self.assets[read.asset].fields, "files": read.files}),
            })
            .collect();
        top.insert("reads".into(), json!(reads));
        top.insert("writes".into(), json!(asset.writes.iter().map(|write| write.record.clone()).collect::<Vec<_>>()));
        hex::encode(Sha256::digest(canonical(&Value::Object(top)).to_string().as_bytes()))
    }

    /// Whether the film `version` holds isn't what cut `i`'s pieces add up to, one frame a piece being rounding.
    fn film_off(&self, i: usize, reads: &[ReadNow], lock: &Lock, version: &Version) -> bool {
        let Some(film) = version.files.get("video").and_then(|sha256| lock.files.get(sha256)).filter(|film| film.frames > 0) else { return false };
        let fields = &self.assets[i].fields;
        let fps = Canvas::of(fields, &mut vec![]).fps;
        let entries: Vec<(String, Option<u64>, Option<u64>)> = match fields.get("pieces").and_then(Value::as_array) {
            Some(items) => items.iter().filter_map(|item| Some((mentions_in(item).into_iter().next()?, item["start"].as_u64(), item["end"].as_u64()))).collect(),
            None => reads.iter().map(|read| (read.name.clone(), None, None)).collect(),
        };
        let piece = |(mention, start, end): (String, Option<u64>, Option<u64>)| {
            let read = reads.iter().find(|read| name_key(&read.name) == name_key(&mention))?;
            let file = lock.files.get(main_of(&read.files)?)?;
            let shape = (file.fps > 0.0).then_some(Shape { width: file.width, height: file.height, fps: file.fps, frames: file.frames });
            let (path, digest, file) = Default::default();
            Some(Piece { asset: read.name.clone(), path, digest, file, shape, still: false, start: start.unwrap_or(0), end: end.unwrap_or(u64::MAX) })
        };
        let mut pieces: Vec<Piece> = entries.into_iter().filter_map(piece).collect();
        pieces.retain(|piece| !version.without.iter().any(|left| name_key(&left.piece) == name_key(&piece.asset)));
        past_end(&mut pieces, fps);
        measure(&pieces, fps).is_some_and(|made| film.frames.abs_diff(made.frames) > pieces.len() as u64)
    }
}

/// What each beat writes, soonest first: the beat, the image, and when on the shot's timeline.
fn beat_writes(body: &Map<String, Value>) -> Vec<(usize, String, f64)> {
    let (mut start, mut found) = (0.0, vec![]);
    for (n, beat) in body.get("beats").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let length = beat["duration"].as_f64().filter(|duration| *duration > 0.0).unwrap_or(BEAT_S);
        let listed = match &beat["writes"] {
            Value::Array(items) => items.clone(),
            Value::Null => vec![],
            one => vec![one.clone()],
        };
        for item in listed {
            let (image, at) = match &item {
                Value::String(image) => (image.clone(), None),
                item => (text_of(item.get("image")), item["at"].as_f64()),
            };
            if !image.is_empty() {
                found.push((n, image, start + at.unwrap_or(length).clamp(0.0, length)));
            }
        }
        start += length;
    }
    found.sort_by(|a, b| a.2.total_cmp(&b.2));
    found
}

/// `with` as the key held it: no release, no root namespace, local code by its directory's name.
fn plain_with(with: &str) -> String {
    let with = with.trim();
    if api::target::written_as_path(with) || with.ends_with(".py") {
        let name = with.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(with);
        return name.strip_suffix(".py").unwrap_or(name).to_owned();
    }
    let path = with.split('@').next().unwrap_or(with);
    path.strip_prefix("_/").unwrap_or(path).to_owned()
}

/// What passed to the adapter. A beat's `writes` didn't.
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

/// An asset's lines as the platform read them: `render` its text, or `with`, `prompt`, `models`, `lora` and the adapter's `options`.
fn unfolded(mut body: Map<String, Value>) -> Map<String, Value> {
    for field in RENDERED {
        body.remove(field);
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

/// The input a pin in `field` of asset `name` keeps in a comment on its line: `video: <hash>  # was <clash>`.
fn pinned_over(text: &str, name: &str, field: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let significant = |line: &str| !line.trim().is_empty() && !line.trim_start().starts_with('#');
    let spaces = |line: &str| line.len() - line.trim_start_matches(' ').len();
    let start = lines.iter().position(|line| spaces(line) == 0 && super::upgrade::split_comment(line).0 == "assets:")? + 1;
    let end = (start..lines.len()).find(|&n| significant(lines[n]) && spaces(lines[n]) == 0).unwrap_or(lines.len());
    let at = spaces(lines[start..end].iter().find(|line| significant(line))?);
    let key = (start..end).find(|&n| significant(lines[n]) && spaces(lines[n]) == at && key_of(lines[n]).is_some_and(|key| name_key(&key) == name_key(name)))?;
    let finish = ((key + 1)..end).find(|&n| significant(lines[n]) && spaces(lines[n]) <= at).unwrap_or(end);
    let child = ((key + 1)..finish).find(|&n| significant(lines[n])).map(|n| spaces(lines[n]))?;
    let line = ((key + 1)..finish).map(|n| lines[n]).find(|line| spaces(line) == child && line[child..].strip_prefix(field).is_some_and(|rest| rest.trim_end().starts_with(':')))?;
    let rest = line[child + field.len()..].trim_start_matches(':').trim();
    let (_, was) = rest.split_once("# was ")?;
    Some(super::upgrade::split_comment(was).0.to_owned())
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
