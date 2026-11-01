//! The `video` project type: entities (characters, props and environments; an environment's camera
//! setups are nodes of their own), world states that shots write and read, shots of any length with
//! beats, an upscale per shot, and the edit, cut here with ffmpeg. Each node renders through its
//! render block: a function, cozy run's model choices and the function's options.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::{Path, PathBuf},
};

use protocol::error::{Error, Kind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{
    folder::{self, Call, Folder, Lock, Made, Upstream, digest_of},
    graph::{Edge, Graph, Node, mentions, name_key},
    host::{Accepts, Renderer},
};

pub const TYPE: &str = "video";
/// The name `cozy run all` means: every node, brought up to date.
pub const ALL: &str = "all";

/// Each node kind's function unless `defaults:` or the node's `render:` names another.
const RENDERERS: [(&str, &str); 6] = [
    ("entity", "qwen-image-2/generate"),
    ("setup", "qwen-image-2/generate"),
    ("world_state", "qwen-image-2/generate"),
    ("shot", "minimax-h3/generate"),
    ("upscale", "minimax-h3/upscale"),
    // Characters' and props' pictures before any use: the subject on white, so a shot renderer
    // never paints their backgrounds into the set.
    ("clean", "birefnet/remove_background"),
];

/// A new project's project.yaml: each kind shown, commented out. Cozy never writes it again: each
/// node's versions, and which is approved, are in project.lock.
pub fn skeleton() -> String {
    "# A video project (`cozy project --help`). Fill in the lists, then
#   cozy render all --project <this folder> --on <machine>
# makes everything missing or out of date, in order, and cuts edit.mp4 here.
type: video
style: \"\"   # the look, added to every picture and shot
# defaults:   # each node kind's render block; a node's own render: wins
#   shot: {with: minimax-h3/generate, lora: [{ref: org/lora, strength: 0.8}], options: {turbo: true}}
entities:   # characters, props, and environments with camera setups (camera text in screen terms)
  # - {name: theo, kind: character, description: \"A tall man in a green raincoat.\"}
  # - {name: dock, kind: environment, description: \"a foggy harbour at dawn\",
  #    setups: [{name: a, camera: \"wide: the red hut near on the left, the crates far on the right\"}]}
world_states:   # how a set stands; a shot that writes one changes it for the shots after
  # - {name: stage, init: \"<theo> stands on <dock>.\", blocking: \"theo by the hut.\"}
shots:   # in order; past its renderer's longest piece (15 s for minimax-h3) a shot needs beats
  # - {name: walk, setup: dock.a, prompt: \"<theo> crosses <dock>.\", duration_s: 8, writes: [{state: stage}],
  #    beats: [{at: 0, prompt: \"He walks to the edge.\"}, {at: 4, prompt: \"He looks out to sea.\"}],
  #    references: [{name: stride, kind: video, video: refs/walk.mp4}]}   # also plate: {video: …}, first_frame, last_frame
# a node's render: {with: <function>, models: {...}, lora: [...], options: {...}} renders it another way
# image: (or a shot's video:) attaches your own file, used until you approve a version cozy made
# resolution: 1440p   # the edit's short edge: shots below it are upscaled (their approved version), and
# aspect_ratio: \"16:9\"   # the canvas every shot fills; both are the project's alone
# edit: [{shot: walk, end: 6}]   # which shots and spans the edit cuts; absent: every shot, whole
"
    .into()
}

#[derive(Deserialize)]
struct Project {
    #[serde(rename = "type")]
    kind: String,
    /// Each node kind's render block, by `RENDERERS`' kinds.
    #[serde(default)]
    defaults: BTreeMap<String, Render>,
    #[serde(default)]
    style: String,
    /// The sound bed under every shot, for a renderer that makes sound.
    #[serde(default)]
    soundscape: String,
    /// The edit's short edge (`1440p`): a shot whose approved version is below it is upscaled.
    resolution: Option<String>,
    /// The canvas every still of a place and every shot fills (`16:9`, `9:16`, `2.39:1`).
    aspect_ratio: Option<String>,
    #[serde(default, deserialize_with = "listed")]
    entities: Vec<Entity>,
    #[serde(default, deserialize_with = "listed")]
    world_states: Vec<WorldState>,
    #[serde(default, deserialize_with = "listed")]
    shots: Vec<Shot>,
    /// None: every shot, in order, whole.
    edit: Option<Vec<Cut>>,
}

/// How a node is rendered: `with` a function (`[org/]package/function`), cozy run's model choices
/// (`models`, `lora`, as `cozy run --input` takes them) and the function's own `options`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Render {
    pub with: Option<String>,
    #[serde(default)]
    pub models: Value,
    #[serde(default)]
    pub lora: Value,
    #[serde(default)]
    pub options: Map<String, Value>,
}

impl Render {
    /// The function it names.
    pub fn target(&self) -> &str {
        self.with.as_deref().unwrap_or_default()
    }
}

/// A thing in the film's world.
#[derive(Deserialize)]
struct Entity {
    name: String,
    kind: EntityKind,
    #[serde(default)]
    description: String,
    #[serde(default)]
    setups: Vec<Setup>,
    /// A picture attached by hand: its version until one cozy made is approved.
    image: Option<String>,
    /// A character's voice, a sound file attached by hand.
    voice: Option<String>,
    render: Option<Render>,
}

#[derive(Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EntityKind {
    Character,
    Prop,
    Environment,
}

impl EntityKind {
    fn name(self) -> &'static str {
        match self {
            EntityKind::Character => "character",
            EntityKind::Prop => "prop",
            EntityKind::Environment => "environment",
        }
    }
}

/// A camera setup on its environment; its versions are setup plates.
#[derive(Deserialize)]
struct Setup {
    name: String,
    /// Where the camera is, in screen terms: what is near or far, left or right.
    camera: String,
    /// The plate from a shot's frame in place of the environment's picture (H3-Director's "place
    /// like shot N"): the place in it, with nobody, seen from this camera.
    frame: Option<FrameAt>,
    image: Option<String>,
    render: Option<Render>,
}

#[derive(Deserialize)]
struct FrameAt {
    shot: String,
    at: f64,
}

#[derive(Deserialize)]
struct WorldState {
    name: String,
    #[serde(default)]
    init: String,
    /// Who and what is where, as plain text; it reaches a shot that reads the state as its layout.
    #[serde(default)]
    blocking: String,
    image: Option<String>,
    render: Option<Render>,
}

#[derive(Deserialize)]
struct Shot {
    name: String,
    /// `Environment.Setup`.
    setup: Option<String>,
    prompt: String,
    duration_s: f64,
    #[serde(default)]
    beats: Vec<Beat>,
    /// World states its frame at `at` seconds (default: its end) becomes the next revision of.
    #[serde(default)]
    writes: Vec<Write>,
    /// Files that guide it, as its renderer's `references` list: `{name, kind, …}`, `kind` one of
    /// `character`, `prop`, `environment` (`image`), `video`, `audio`. After the ones the project
    /// compiles from what it reads.
    #[serde(default)]
    references: Vec<Map<String, Value>>,
    /// The video it edits: `{video, use_audio?}`.
    plate: Option<Map<String, Value>>,
    first_frame: Option<String>,
    last_frame: Option<String>,
    video: Option<String>,
    render: Option<Render>,
    #[serde(default)]
    upscale: Upscale,
}

#[derive(Clone, Deserialize)]
struct Write {
    state: String,
    at: Option<f64>,
}

/// The fields of a shot's reference that are files here.
const FILE_FIELDS: [&str; 4] = ["image", "video", "audio", "voice"];

#[derive(Clone, Serialize, Deserialize)]
struct Beat {
    at: f64,
    prompt: String,
}

#[derive(Default, Deserialize)]
struct Upscale {
    video: Option<String>,
    render: Option<Render>,
}

/// A piece of a shot in the edit; `start`/`end` in seconds (names pending Paul).
#[derive(Deserialize)]
struct Cut {
    shot: String,
    start: Option<f64>,
    end: Option<f64>,
}

/// Where a node is in project.yaml.
#[derive(Clone, Copy)]
enum Item {
    Entity(usize),
    Setup(usize, usize),
    State(usize),
    Shot(usize),
    Upscale(usize),
}

/// A piece of the edit: a shot's file and its digest, and from and to where in it, in seconds.
pub struct Piece {
    pub path: PathBuf,
    pub digest: String,
    pub start: Option<f64>,
    pub end: Option<f64>,
}

pub struct Video {
    project: Project,
    items: Vec<Item>,
    pub graph: Graph,
    pub warnings: Vec<String>,
    /// The project's canvas.
    pub canvas: Canvas,
    /// Each node's current video's width and height, when project.lock records them.
    sizes: Vec<Option<[u64; 2]>>,
}

/// The canvas the edit fills: its aspect, as W:H in whole numbers for a request and as a ratio,
/// and its short edge when `resolution:` sets one.
#[derive(Clone, Debug)]
pub struct Canvas {
    pub aspect: String,
    pub ratio: f64,
    pub short: Option<u64>,
}

impl Canvas {
    fn read(resolution: Option<&str>, aspect: Option<&str>) -> Result<Self, Error> {
        let short = match resolution {
            None => None,
            Some(text) => Some(
                (text.strip_suffix('p').and_then(|n| n.parse::<u64>().ok()).filter(|n| *n >= 16)).ok_or_else(|| {
                    invalid("resolution", format!("resolution {text} is not a short edge such as 1080p or 1440p"))
                })?,
            ),
        };
        let aspect = aspect.unwrap_or("16:9");
        let side = |text: &str| text.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0);
        let Some((w, h)) = aspect.split_once(':').and_then(|(w, h)| Some((side(w)?, side(h)?))) else {
            return Err(invalid("aspect_ratio", format!("aspect_ratio {aspect} is not W:H, such as 16:9 or 2.39:1")));
        };
        // Whole numbers for an image request's W:H: 2.39:1 is 239:100.
        let (mut a, mut b) = ((w * 100.0).round() as u64, (h * 100.0).round() as u64);
        let gcd = |mut x: u64, mut y: u64| {
            while y != 0 {
                (x, y) = (y, x % y);
            }
            x
        };
        let g = gcd(a, b).max(1);
        (a, b) = (a / g, b / g);
        Ok(Self { aspect: format!("{a}:{b}"), ratio: w / h, short })
    }

    /// Width and height on this aspect for `short` as its short edge, each even.
    pub fn size(&self, short: u64) -> (u64, u64) {
        let long = |s: f64| ((s / 2.0).round() as u64) * 2;
        match self.ratio >= 1.0 {
            true => (long(short as f64 * self.ratio), short),
            false => (short, long(short as f64 / self.ratio)),
        }
    }
}

/// A version's request, and what to record of it.
pub struct Request {
    /// Each file in it as its digest; `files` say where each one is here.
    pub payload: Value,
    pub made: Made,
    /// The result field holding the version.
    pub field: &'static str,
    /// A shot's call.
    pub call: Option<Call>,
    pub files: Vec<Input>,
    /// The function it goes to, and the run's model choices and options.
    pub render: Render,
    /// What it left out because its function does not take it.
    pub warnings: Vec<String>,
}

/// A file a request reads: its payload field (`assets.0.image`), its path here, and its digest; or
/// with `at`, that video's frame at `at` seconds, which the walk takes out before it sends.
pub struct Input {
    pub field: String,
    pub path: PathBuf,
    pub digest: String,
    pub at: Option<f64>,
}

/// What a request reads from upstream: the edge, and the file it reads now.
struct Read<'a> {
    edge: &'a Edge,
    path: PathBuf,
    digest: String,
}

impl Read<'_> {
    /// The file (its frame at `at`) as the payload's `field`: its digest, the file listed in `files`.
    fn input(&self, files: &mut Vec<Input>, field: String, at: Option<f64>) -> Value {
        files.push(Input { field, path: self.path.clone(), digest: self.digest.clone(), at });
        Value::String(self.digest.clone())
    }
}

impl Video {
    /// The project as project.yaml says now; a field cozy does not read, and a mention of nothing,
    /// are `warnings`.
    pub fn load(folder: &Folder) -> Result<Self, Error> {
        let value = protocol::yaml::to_json(&folder.text()?)
            .map_err(|why| invalid("project_yaml", format!("{}: {why}", folder::PROJECT)))?;
        let mut ignored: Vec<String> = vec![];
        let mut note = |path: serde_ignored::Path<'_>| ignored.push(path.to_string());
        let read = serde_ignored::Deserializer::new(&value, &mut note);
        let project: Project = serde_path_to_error::deserialize(read).map_err(|e| {
            let at = e.path().to_string();
            invalid("project_yaml", format!("{}: {at}: {}", folder::PROJECT, e.into_inner()))
                .remedy("each entity is {name, kind, …}: see `cozy project --help`")
        })?;
        let mut warnings: Vec<String> = ignored
            .iter()
            .map(|path| format!("{}: cozy reads no field {path}; it is ignored", folder::PROJECT))
            .collect();
        if project.kind != TYPE {
            return Err(invalid(
                "project_type",
                format!("{} is a {} project; this cozy makes {TYPE} projects", folder::PROJECT, project.kind),
            ));
        }
        warnings.extend(
            (project.defaults.keys())
                .filter(|kind| !RENDERERS.iter().any(|(k, _)| k == kind))
                .map(|kind| format!("{}: defaults.{kind} is no node kind; it is ignored", folder::PROJECT)),
        );
        let lock = folder.lock()?;
        // Its current file and its versions' files: the approved version, else the file attached by
        // hand (there, or the node is named with the fix), or that picture's clean.
        let versions = |id: &str, field: &str, attached: &Option<String>, cleans: bool| {
            current(folder, &lock, id, field, attached, cleans)
        };
        let mut nodes = vec![];
        let mut items = vec![];
        for (a, entity) in project.entities.iter().enumerate() {
            let kind = entity.kind.name();
            if !entity.setups.is_empty() && entity.kind != EntityKind::Environment {
                return Err(invalid(
                    "setup_owner",
                    format!("{} is a {kind}: only an environment has setups", entity.name),
                ));
            }
            let cleans = entity.kind != EntityKind::Environment;
            let (file, made) = versions(&entity.name, "image", &entity.image, cleans)?;
            let mut inputs = vec![];
            if let Some(voice) = &entity.voice {
                if entity.kind != EntityKind::Character {
                    return Err(invalid("voice_owner", format!("{} is a {kind}: only a character has a voice", entity.name)));
                }
                inputs.push(input_file(folder, &entity.name, "voice", voice)?);
            }
            nodes.push(Node {
                id: entity.name.clone(),
                kind,
                mentionable: true,
                state: false,
                order: None,
                texts: vec![entity.description.clone()],
                needs: vec![],
                writes: vec![],
                inputs,
                file,
                versions: made,
            });
            items.push(Item::Entity(a));
            for (s, setup) in entity.setups.iter().enumerate() {
                let id = format!("{}.{}", entity.name, setup.name);
                let (file, made) = versions(&id, "image", &setup.image, false)?;
                nodes.push(Node {
                    id,
                    kind: "setup",
                    mentionable: false,
                    state: false,
                    order: None,
                    texts: vec![setup.camera.clone()],
                    needs: vec![setup.frame.as_ref().map_or_else(|| entity.name.clone(), |frame| frame.shot.clone())],
                    writes: vec![],
                    inputs: vec![],
                    file,
                    versions: made,
                });
                items.push(Item::Setup(a, s));
            }
        }
        for entity in &project.entities {
            for (setup, frame) in entity.setups.iter().filter_map(|s| s.frame.as_ref().map(|f| (s, f))) {
                let id = format!("{}.{}", entity.name, setup.name);
                let fix = "write frame: {shot: <a shot's name>, at: <seconds into it>}";
                let shot = project.shots.iter().find(|shot| name_key(&shot.name) == name_key(&frame.shot));
                let Some(shot) = shot else {
                    return Err(invalid(
                        "frame_unknown",
                        format!("{id}'s frame names {}, which is no shot", frame.shot),
                    )
                    .remedy(fix));
                };
                if !(0.0..shot.duration_s).contains(&frame.at) {
                    let said =
                        format!("{id}'s frame is at {} s, outside {}'s 0–{} s", frame.at, shot.name, shot.duration_s);
                    return Err(invalid("frame_outside", said).remedy(fix));
                }
            }
        }
        for (w, state) in project.world_states.iter().enumerate() {
            let (file, made) = versions(&state.name, "image", &state.image, false)?;
            nodes.push(Node {
                id: state.name.clone(),
                kind: "world_state",
                mentionable: true,
                state: true,
                order: None,
                texts: vec![state.init.clone(), state.blocking.clone()],
                needs: vec![],
                writes: vec![],
                inputs: vec![],
                file,
                versions: made,
            });
            items.push(Item::State(w));
        }
        for (k, shot) in project.shots.iter().enumerate() {
            let texts: Vec<String> =
                std::iter::once(shot.prompt.clone()).chain(shot.beats.iter().map(|b| b.prompt.clone())).collect();
            let mut needs = vec![];
            if let Some(setup) = &shot.setup {
                if !nodes.iter().any(|n| n.kind == "setup" && name_key(&n.id) == name_key(setup)) {
                    return Err(invalid(
                        "setup_unknown",
                        format!("{}'s setup {setup} is no setup (write Environment.Setup)", shot.name),
                    ));
                }
                needs.push(setup.clone());
            }
            // An environment named with no setup of it is shot from its first setup.
            let framed = shot.setup.as_deref().and_then(|s| s.rsplit_once('.')).map(|(env, _)| name_key(env));
            for mention in texts.iter().flat_map(|text| mentions(text)) {
                let env = (project.entities.iter())
                    .find(|e| e.kind == EntityKind::Environment && name_key(&e.name) == name_key(mention));
                if let Some(env) = env.filter(|env| framed.as_deref() != Some(&name_key(&env.name)))
                    && let Some(first) = env.setups.first()
                {
                    needs.push(format!("{}.{}", env.name, first.name));
                }
            }
            let (file, made) = versions(&shot.name, "video", &shot.video, false)?;
            nodes.push(Node {
                id: shot.name.clone(),
                kind: "shot",
                mentionable: false,
                state: false,
                order: Some(k),
                texts,
                needs,
                writes: shot.writes.iter().map(|write| write.state.clone()).collect(),
                inputs: shot_inputs(folder, shot)?,
                file,
                versions: made,
            });
            items.push(Item::Shot(k));
            let id = format!("{}.upscale", shot.name);
            let (file, made) = versions(&id, "video", &shot.upscale.video, false)?;
            nodes.push(Node {
                id,
                kind: "upscale",
                mentionable: false,
                state: false,
                order: None,
                texts: vec![],
                needs: vec![shot.name.clone()],
                writes: vec![],
                inputs: vec![],
                file,
                versions: made,
            });
            items.push(Item::Upscale(k));
        }
        if let Some(all) = nodes.iter().find(|node| name_key(&node.id) == name_key(ALL)) {
            let said = format!("{} is named `{ALL}`, which `cozy render {ALL}` means", all.id);
            return Err(invalid("name_reserved", said).remedy("rename it"));
        }
        let graph = Graph::new(nodes)?;
        warnings.extend(graph.unknown.iter().map(|(i, mention)| {
            format!(
                "{} mentions {mention}, which is nothing in the project: it reaches the renderer as written",
                graph.nodes[*i].id
            )
        }));
        for shot in &project.shots {
            let mut written = BTreeSet::new();
            for write in &shot.writes {
                let at = write.at.unwrap_or(shot.duration_s);
                if !(at > 0.0 && at <= shot.duration_s) {
                    let said = format!("{} writes {} at {at} s, outside its 0–{} s", shot.name, write.state, shot.duration_s);
                    return Err(invalid("write_outside", said));
                }
                if !written.insert(name_key(&write.state)) {
                    return Err(invalid("write_twice", format!("{} writes {} twice", shot.name, write.state)));
                }
            }
            let mut named = BTreeSet::new();
            for entry in &shot.references {
                let name = entry.get("name").and_then(Value::as_str).unwrap_or_default();
                if graph.find(name).is_some() || !named.insert(name_key(name)) {
                    let said = format!("{}'s reference {name} has a name already taken", shot.name);
                    return Err(invalid("reference_name", said).remedy("each reference's name is its own: rename it"));
                }
            }
        }
        let canvas = Canvas::read(project.resolution.as_deref(), project.aspect_ratio.as_deref())?;
        let sizes = (graph.nodes.iter())
            .map(|node| {
                let (_, digest) = node.file.as_ref()?;
                lock.made(&node.id, digest)?.size
            })
            .collect();
        Ok(Self { project, items, graph, warnings, canvas, sizes })
    }

    /// Node `i`'s render block (`resolve`).
    pub fn render(&self, i: usize) -> Render {
        let (kind, own) = match self.items[i] {
            Item::Entity(a) => ("entity", self.project.entities[a].render.as_ref()),
            Item::Setup(a, s) => ("setup", self.project.entities[a].setups[s].render.as_ref()),
            Item::State(w) => ("world_state", self.project.world_states[w].render.as_ref()),
            Item::Shot(k) => ("shot", self.project.shots[k].render.as_ref()),
            Item::Upscale(k) => ("upscale", self.project.shots[k].upscale.render.as_ref()),
        };
        self.resolve(kind, own)
    }

    /// The render block that cleans a character's or prop's picture.
    pub fn clean_render(&self) -> Render {
        self.resolve("clean", None)
    }

    /// A node's own render block when it names a function; else its kind's default (`defaults:`)
    /// with the node's own fields over it. The function is the built-in one when neither names one.
    fn resolve(&self, kind: &str, own: Option<&Render>) -> Render {
        let default = self.project.defaults.get(kind).cloned().unwrap_or_default();
        let mut render = match own {
            Some(own) if own.with.is_some() => own.clone(),
            Some(own) => {
                let pick = |mine: &Value, theirs: Value| if mine.is_null() { theirs } else { mine.clone() };
                let mut options = default.options;
                options.extend(own.options.clone());
                Render { with: default.with, models: pick(&own.models, default.models), lora: pick(&own.lora, default.lora), options }
            }
            None => default,
        };
        if render.with.is_none() {
            render.with = RENDERERS.iter().find(|(k, _)| *k == kind).map(|(_, target)| (*target).to_owned());
        }
        render
    }

    /// The field that holds the node's file.
    pub fn field(&self, i: usize) -> &'static str {
        match self.items[i] {
            Item::Shot(_) | Item::Upscale(_) => "video",
            _ => "image",
        }
    }

    /// Whether `render all` makes the node: every node but an upscale the edit does not need. A
    /// shot is upscaled when `resolution:` is set and its current version's short edge is below it
    /// (a video attached by hand, whose size project.lock does not record, is upscaled).
    pub fn wanted(&self, i: usize) -> bool {
        let Item::Upscale(k) = self.items[i] else { return true };
        let Some(short) = self.canvas.short else { return false };
        let shot = self.graph.find(&self.project.shots[k].name).expect("a shot is a node");
        self.sizes[shot].is_none_or(|[w, h]| w.min(h) < short)
    }

    /// Whether node `i`'s pictures are cleaned before use: a character's or a prop's.
    pub fn cleans(&self, i: usize) -> bool {
        matches!(self.graph.nodes[i].kind, "character" | "prop")
    }

    /// Node `i`'s current picture and its digest when it still needs its clean: a character's or
    /// prop's attached by hand, with no clean in project.lock yet (every version cozy makes of one
    /// is its clean).
    pub fn unclean(&self, lock: &Lock, i: usize) -> Option<(String, String)> {
        let node = &self.graph.nodes[i];
        let file = node.file.as_ref().filter(|_| self.cleans(i))?;
        lock.made(&node.id, &file.1).is_none().then(|| file.clone())
    }

    /// Shots longer than their renderer's longest piece with no beats, as node index, name and
    /// length: each one's renderer decides.
    pub fn beatless(&self) -> Vec<(usize, &str, f64)> {
        (self.items.iter().enumerate())
            .filter_map(|(i, item)| match item {
                Item::Shot(k) if self.project.shots[*k].beats.is_empty() => {
                    Some((i, self.project.shots[*k].name.as_str(), self.project.shots[*k].duration_s))
                }
                _ => None,
            })
            .collect()
    }

    /// Every node's name key, which project.lock keeps versions for.
    pub fn keys(&self) -> BTreeSet<String> {
        self.graph.nodes.iter().map(|node| name_key(&node.id)).collect()
    }

    /// The digest of every file attached in project.yaml.
    pub fn attached(&self, folder: &Folder) -> Result<BTreeSet<String>, Error> {
        let mut files: Vec<&String> = vec![];
        for entity in &self.project.entities {
            files.extend(&entity.image);
            files.extend(entity.setups.iter().flat_map(|setup| &setup.image));
        }
        files.extend(self.project.world_states.iter().flat_map(|state| &state.image));
        for shot in &self.project.shots {
            files.extend(&shot.video);
            files.extend(&shot.upscale.video);
        }
        files.into_iter().filter(|file| folder.path(file).is_file()).map(|file| folder.digest(file)).collect()
    }

    /// The request that makes a version of node `i`, from what it reads now, as its function
    /// (`renderer`) takes it; a shot's is its first call.
    pub fn request(&self, folder: &Folder, lock: &Lock, i: usize, renderer: &Renderer) -> Result<Request, Error> {
        let call = match self.items[i] {
            Item::Shot(k) => Some(Call { ends: self.ends(k), ..Default::default() }),
            _ => None,
        };
        self.compile(folder, lock, i, renderer, call, &BTreeMap::new())
    }

    /// Shot `i`'s next call (`call`, which continues `call.from`), with the frames the shot wrote
    /// in the calls before it (`own`: state → file).
    pub fn next_call(
        &self,
        folder: &Folder,
        lock: &Lock,
        i: usize,
        renderer: &Renderer,
        call: Call,
        own: &BTreeMap<String, String>,
    ) -> Result<Request, Error> {
        self.compile(folder, lock, i, renderer, Some(call), own)
    }

    fn compile(
        &self,
        folder: &Folder,
        lock: &Lock,
        i: usize,
        renderer: &Renderer,
        call: Option<Call>,
        own: &BTreeMap<String, String>,
    ) -> Result<Request, Error> {
        let graph = &self.graph;
        let node = &graph.nodes[i];
        let mut upstream = vec![];
        let mut reads = vec![];
        for edge in &graph.upstream[i] {
            let up = &graph.nodes[edge.node];
            let digest = graph.read(lock, edge).ok_or_else(|| {
                Error::new(
                    Kind::Usage,
                    "upstream_empty",
                    format!("{} reads {}, which has no version yet", node.id, up.id),
                )
                .next(format!("cozy render {} --project …", up.id))
            })?;
            let path = match (&edge.state, &up.file) {
                (None, Some((file, _))) => folder.path(file),
                _ => folder.path(&folder.find(&digest)?),
            };
            upstream.push(Upstream { node: up.id.clone(), state: edge.state.clone(), file: digest.clone() });
            reads.push(Read { edge, path, digest });
        }
        upstream.extend(node.inputs.iter().map(|(key, file)| Upstream { node: key.clone(), state: None, file: file.clone() }));
        let made = Made { node: node.id.clone(), upstream: Some(upstream), ..Default::default() };
        let styled = |text: String| match self.project.style.trim() {
            "" => text,
            style => format!("{text}\n\nStyle: {style}."),
        };
        let mut files = vec![];
        let pictures = |files: &mut Vec<Input>| -> Vec<Value> {
            (reads.iter().enumerate())
                .map(|(n, read)| {
                    let up = &graph.nodes[read.edge.node];
                    let name = read.edge.state.as_deref().unwrap_or(&up.id);
                    json!({"name": name, "image": read.input(files, format!("pictures.{n}.image"), None)})
                })
                .collect()
        };
        let mut warnings = vec![];
        let (mut payload, call) = match self.items[i] {
            Item::Entity(a) => {
                let entity = &self.project.entities[a];
                // Only the entity itself, once: a picture it mentions is for likeness, never drawn beside it.
                let name = &entity.name;
                let (shown, ratio) = match entity.kind {
                    EntityKind::Character => (
                        format!("Character reference of {name} alone: a single front view of one whole figure, on a plain white background. Any other picture is for likeness only; draw no one else."),
                        "3:4",
                    ),
                    EntityKind::Prop => (
                        format!("Prop reference of {name} alone: one view of the object, on a plain white background. Any other picture is for likeness only; draw nothing else."),
                        "1:1",
                    ),
                    EntityKind::Environment => ("An establishing view of the empty set, no people.".to_owned(), self.canvas.aspect.as_str()),
                };
                let prompt = styled(format!("{}\n\n{shown}", entity.description.trim()));
                (json!({"prompt": prompt, "pictures": pictures(&mut files), "aspect_ratio": ratio}), None)
            }
            Item::Setup(a, s) => {
                // The environment's picture (or a shot's frame) first, and the camera in screen terms
                // as written (environment-angles REPORT.md): bare camera words return a mirror.
                let (env, setup) = (&self.project.entities[a], &self.project.entities[a].setups[s]);
                let camera = setup.camera.trim().trim_end_matches('.');
                let prompt = match setup.frame {
                    None => format!(
                        "Show this exact <{}> from another camera position: {camera}. Keep every object, material, the light and the weather exactly as in the image; add nothing new, no people.",
                        env.name
                    ),
                    Some(_) => format!(
                        "Show the place in this picture, <{}>, with nobody in it, from another camera position: {camera}. Keep every object, material, the light and the weather exactly as in the picture; add nothing new, no people.",
                        env.name
                    ),
                };
                let pictures: Vec<Value> = (reads.iter().enumerate())
                    .map(|(n, read)| {
                        let up = &graph.nodes[read.edge.node];
                        let (name, at) = match n {
                            0 => (env.name.as_str(), setup.frame.as_ref().map(|frame| frame.at)),
                            _ => (up.id.as_str(), None),
                        };
                        json!({"name": name, "image": read.input(&mut files, format!("pictures.{n}.image"), at)})
                    })
                    .collect();
                (json!({"prompt": prompt, "pictures": pictures, "aspect_ratio": self.canvas.aspect}), None)
            }
            Item::State(w) => {
                let state = &self.project.world_states[w];
                let prompt = styled([state.init.trim(), state.blocking.trim()].join("\n\n").trim().to_owned());
                (json!({"prompt": prompt, "pictures": pictures(&mut files), "aspect_ratio": self.canvas.aspect}), None)
            }
            Item::Shot(k) => {
                let call = call.expect("a shot is made in calls");
                let (payload, call) =
                    self.shot(folder, k, &reads, &mut files, call, own, &renderer.accepts, &mut warnings)?;
                (payload, Some(call))
            }
            Item::Upscale(k) => {
                // The text the shot was made from, for an upscaler that re-renders it (Paul, 10-31: the
                // upscaler gets the details of the original): its prompt and beats, the descriptions of
                // what it reads, and the style.
                let shot = &self.project.shots[k];
                let beats = shot.beats.iter().map(|beat| format!("Then, {}", beat.prompt.trim()));
                let mut text = std::iter::once(shot.prompt.trim().to_owned()).chain(beats).collect::<Vec<_>>().join(" ");
                let read = self.graph.find(&shot.name).map(|s| &self.graph.upstream[s]).into_iter().flatten();
                for edge in read {
                    let (name, description) = match (self.items[edge.node], &edge.state) {
                        (Item::Entity(a), _) => (&self.project.entities[a].name, self.project.entities[a].description.as_str()),
                        (Item::Setup(a, _), _) => (&self.project.entities[a].name, self.project.entities[a].description.as_str()),
                        (Item::State(w), _) => (&self.project.world_states[w].name, self.project.world_states[w].blocking.as_str()),
                        _ => continue,
                    };
                    if !description.trim().is_empty() {
                        let _ = write!(text, "\n{name}: {}", description.trim());
                    }
                }
                let mut payload = json!({"video": reads[0].input(&mut files, "video".into(), None), "prompt": styled(text.replace(['<', '>'], ""))});
                if let Some(resolution) = &self.project.resolution {
                    payload["resolution"] = Value::String(resolution.clone());
                }
                // The shot's latents, for an upscaler of its package (one that reads its format).
                let made = lock.made(&shot.name, &reads[0].digest);
                let same = made.and_then(|made| made.made_by.as_ref())
                    .is_some_and(|by| by.with.starts_with(&format!("{}/", renderer.package)));
                if let Some(latents) = made.and_then(|made| made.latents.as_ref()).filter(|_| same) {
                    let digest = digest_of(latents);
                    files.push(Input { field: "latents".into(), path: folder.path(latents), digest: digest.clone(), at: None });
                    payload["latents"] = Value::String(digest);
                }
                (payload, None)
            }
        };
        // What the function does not take is left out, said, and so are its files.
        let accepts = &renderer.accepts;
        let fields = payload.as_object_mut().expect("a request is an object");
        let left: Vec<String> = fields.keys().filter(|key| !accepts.fields.contains(*key)).cloned().collect();
        for key in &left {
            fields.remove(key);
            files.retain(|input| input.field.split('.').next() != Some(key.as_str()));
            warnings.push(format!("{} takes no {key}: {}'s is left out", self.render(i).target(), node.id));
        }
        Ok(Request { payload, made, field: self.field(i), call, files, render: self.render(i), warnings })
    }

    /// Where shot `k`'s consecutive calls end: at each time it writes a state a later beat of it
    /// mentions (that read is of its own write, so the call after it reads the frame), and at its
    /// end.
    fn ends(&self, k: usize) -> Vec<f64> {
        let shot = &self.project.shots[k];
        let read_after = |state: &str, at: f64| {
            (shot.beats.iter()).any(|beat| beat.at > at && mentions(&beat.prompt).iter().any(|m| name_key(m) == name_key(state)))
        };
        let mut ends: Vec<f64> = (shot.writes.iter())
            .map(|write| (write.state.as_str(), write.at.unwrap_or(shot.duration_s)))
            .filter(|&(state, at)| at < shot.duration_s && read_after(state, at))
            .map(|(_, at)| at)
            .collect();
        ends.sort_by(f64::total_cmp);
        ends.dedup();
        ends.push(shot.duration_s);
        ends
    }

    /// Shot `k`'s call `call` as its function takes it (`accepts`), on the shot's one timeline:
    /// - its text up to the call's end: `prompt`, the beats before it, the project's style,
    ///   soundscape and aspect ratio;
    /// - `references`: each entity it reads (its description, a character's voice), its setup's
    ///   picture as its environment's, each world state as a loose environment picture with its
    ///   blocking (the version current at the call's start: the shot's own frame once it wrote it,
    ///   `own`), then the shot's own references;
    /// - its plate, its first frame (first call), its last frame (last call), and the call before
    ///   it as `continue_from`.
    ///
    /// What the function does not take is left out and said (`said`), or refused where leaving it
    /// out would make another shot: a plate, or the calls a read of its own write needs.
    #[allow(clippy::too_many_arguments)]
    fn shot(
        &self,
        folder: &Folder,
        k: usize,
        reads: &[Read],
        files: &mut Vec<Input>,
        mut call: Call,
        own: &BTreeMap<String, String>,
        accepts: &Accepts,
        said: &mut Vec<String>,
    ) -> Result<(Value, Call), Error> {
        let shot = &self.project.shots[k];
        let (index, last) = (call.index, call.index + 1 == call.ends.len());
        let (start, end) = (if index == 0 { 0.0 } else { call.ends[index - 1] }, call.ends[index]);
        let takes = |field: &str| accepts.fields.contains(field);
        let refuse = |what: String| invalid("shot_unrenderable", format!("{}: {what}", shot.name));
        if call.ends.len() > 1 {
            if !takes("continue_from") {
                return Err(refuse("it reads a state it wrote itself, which needs its renderer to continue a video \
                                   (continue_from), and it takes none".into()));
            }
            let spans = call.ends.iter().scan(0.0, |from, &to| Some((std::mem::replace(from, to), to)));
            if let Some(min) = accepts.min_piece_s
                && let Some((from, to)) = spans.take(call.ends.len() - 1).find(|(from, to)| to - from < min)
            {
                return Err(refuse(format!("its call from {from} s to {to} s is under its renderer's {min} s")));
            }
        }
        let framed: BTreeSet<usize> = (reads.iter())
            .filter_map(|read| match self.items[read.edge.node] {
                Item::Setup(a, _) => Some(a),
                _ => None,
            })
            .collect();
        let blocking = |name: &str| {
            let state = self.project.world_states.iter().find(|s| name_key(&s.name) == name_key(name));
            state.map(|s| s.blocking.clone()).unwrap_or_default()
        };
        // Each reference the function takes, its fields cut to its kind's; its files listed.
        let mut references: Vec<Value> = vec![];
        let mut add = |mut entry: Map<String, Value>, sources: Vec<(&str, PathBuf, String)>, files: &mut Vec<Input>| {
            let kind = entry.get("kind").and_then(Value::as_str).unwrap_or_default().to_owned();
            let name = entry.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
            let Some(fields) = accepts.kinds.get(&kind) else {
                said.push(format!("{}'s renderer takes no {kind} reference: {name} is left out", shot.name));
                return;
            };
            let dropped: Vec<String> = entry.keys().filter(|key| *key != "kind" && !fields.contains(*key)).cloned().collect();
            for key in dropped {
                entry.remove(&key);
                said.push(format!("{}'s renderer takes no {key} on a {kind} reference: {name}'s is left out", shot.name));
            }
            let n = references.len();
            let kept: Vec<_> = sources.into_iter().filter(|(field, ..)| entry.contains_key(*field)).collect();
            for (field, path, digest) in kept {
                files.push(Input { field: format!("references.{n}.{field}"), path, digest: digest.clone(), at: None });
                entry.insert(field.into(), Value::String(digest));
            }
            references.push(Value::Object(entry));
        };
        let file = |path: &str| -> Result<(PathBuf, String), Error> { Ok((folder.path(path), folder.digest(path)?)) };
        for read in reads {
            let source = |field: &'static str| vec![(field, read.path.clone(), read.digest.clone())];
            match (self.items[read.edge.node], &read.edge.state) {
                (Item::Entity(a), _) if !framed.contains(&a) => {
                    let entity = &self.project.entities[a];
                    let mut entry = Map::new();
                    entry.insert("name".into(), json!(entity.name));
                    entry.insert("kind".into(), json!(entity.kind.name()));
                    entry.insert("image".into(), Value::Null);
                    entry.insert("description".into(), json!(entity.description));
                    let mut sources = source("image");
                    if let Some(voice) = &entity.voice {
                        let (path, digest) = file(voice)?;
                        entry.insert("voice".into(), Value::Null);
                        sources.push(("voice", path, digest));
                    }
                    add(entry, sources, files);
                }
                (Item::Setup(a, _), _) => {
                    let env = &self.project.entities[a];
                    let entry = json!({"name": env.name, "kind": "environment", "image": null, "description": env.description});
                    add(entry.as_object().expect("an object").clone(), source("image"), files);
                }
                (Item::State(_), _) | (Item::Shot(_), Some(_)) => {
                    let name = read.edge.state.clone().unwrap_or_else(|| self.graph.nodes[read.edge.node].id.clone());
                    let entry = json!({"name": name, "kind": "environment", "follow": "loose", "image": null, "description": blocking(&name)});
                    // Once this shot wrote it, its own frame.
                    let sources = match own.iter().find(|(state, _)| name_key(state) == name_key(&name)) {
                        Some((_, frame)) => vec![("image", folder.path(frame), digest_of(frame))],
                        None => source("image"),
                    };
                    add(entry.as_object().expect("an object").clone(), sources, files);
                }
                _ => {}
            }
        }
        for entry in &shot.references {
            let mut sources = vec![];
            for field in FILE_FIELDS {
                if let Some(path) = entry.get(field).and_then(Value::as_str) {
                    let (path, digest) = file(path)?;
                    sources.push((field, path, digest));
                }
            }
            add(entry.clone(), sources, files);
        }
        let mut payload = Map::new();
        let mut text = shot.prompt.clone();
        let beats: Vec<&Beat> = shot.beats.iter().filter(|beat| beat.at < end).collect();
        match takes("beats") {
            true => drop(payload.insert("beats".into(), json!(beats))),
            false => (beats.iter()).for_each(|beat| text = format!("{text} Then, {}", beat.prompt.trim())),
        }
        let style = self.project.style.trim();
        match (takes("style"), style.is_empty()) {
            (true, false) => drop(payload.insert("style".into(), json!(style))),
            (false, false) => text = format!("{text}\n\nStyle: {style}."),
            _ => {}
        }
        if !self.project.soundscape.trim().is_empty() {
            match takes("soundscape") {
                true => drop(payload.insert("soundscape".into(), json!(self.project.soundscape.trim()))),
                false => said.push(format!("{}'s renderer takes no soundscape: it is left out", shot.name)),
            }
        }
        if takes("references") {
            payload.insert("references".into(), Value::Array(references));
        } else {
            // With no references, a mention is the name itself.
            text = text.replace(['<', '>'], "");
        }
        payload.insert("prompt".into(), json!(text));
        payload.insert("duration_s".into(), json!(end));
        if takes("aspect_ratio") {
            payload.insert("aspect_ratio".into(), json!(self.canvas.aspect));
        }
        if let Some(plate) = &shot.plate {
            if !takes("plate") {
                return Err(refuse("its renderer takes no plate, the video it edits".into()));
            }
            let mut plate = plate.clone();
            if let Some(path) = plate.get("video").and_then(Value::as_str) {
                let (path, digest) = file(path)?;
                files.push(Input { field: "plate.video".into(), path, digest: digest.clone(), at: None });
                plate.insert("video".into(), Value::String(digest));
            }
            payload.insert("plate".into(), Value::Object(plate));
        }
        for (field, path, here) in [("first_frame", &shot.first_frame, index == 0), ("last_frame", &shot.last_frame, last)] {
            let Some(path) = path.as_ref().filter(|_| here) else { continue };
            if !takes(field) {
                said.push(format!("{}'s renderer takes no {field}: it is left out", shot.name));
                continue;
            }
            let (path, digest) = file(path)?;
            files.push(Input { field: field.into(), path, digest: digest.clone(), at: None });
            payload.insert(field.into(), Value::String(digest));
        }
        if let Some((video, latents)) = call.from.as_ref().filter(|_| index > 0) {
            let mut from = Map::new();
            for (field, file) in std::iter::once(("video", Some(video))).chain([("latents", latents.as_ref())]) {
                let Some(file) = file else { continue };
                let digest = digest_of(file);
                files.push(Input { field: format!("continue_from.{field}"), path: folder.path(file), digest: digest.clone(), at: None });
                from.insert(field.into(), Value::String(digest));
            }
            payload.insert("continue_from".into(), Value::Object(from));
        }
        call.writes = (shot.writes.iter())
            .map(|write| (write.state.clone(), write.at.unwrap_or(shot.duration_s)))
            .filter(|(_, at)| *at >= start && (*at < end || last))
            .collect();
        Ok((Value::Object(payload), call))
    }

    /// The pieces of the edit (`preview`: of its native cut): each cut's shot file and span, by
    /// default the shot's `duration_s` (a renderer may run past it). The edit takes each shot's
    /// upscale where it has one to make (`wanted`), else the shot; the preview takes the shot. And
    /// the nodes it would cut that have no file yet, skipped.
    pub fn cuts(&self, folder: &Folder, preview: bool) -> Result<(Vec<Piece>, Vec<String>), Error> {
        let cuts: Vec<(&str, Option<f64>, Option<f64>)> = match &self.project.edit {
            Some(cuts) => cuts.iter().map(|cut| (cut.shot.as_str(), cut.start, cut.end)).collect(),
            None => self.project.shots.iter().map(|shot| (shot.name.as_str(), None, None)).collect(),
        };
        let (mut pieces, mut skipped) = (vec![], vec![]);
        for (name, start, end) in cuts {
            let k = (self.project.shots.iter()).position(|s| name_key(&s.name) == name_key(name)).ok_or_else(|| {
                invalid("cut_unknown", format!("the edit cuts {name}, which is no shot"))
                    .remedy("each edit entry is {shot: <a shot's name>, start?, end?}")
            })?;
            let shot = &self.project.shots[k];
            let upscale = self.graph.find(&format!("{}.upscale", shot.name)).expect("a shot's upscale is a node");
            let i = match !preview && self.wanted(upscale) {
                true => upscale,
                false => self.graph.find(&shot.name).expect("a shot is a node"),
            };
            let node = &self.graph.nodes[i];
            let end = end.or(Some(shot.duration_s));
            match &node.file {
                Some((file, digest)) => {
                    pieces.push(Piece { path: folder.path(file), digest: digest.clone(), start, end })
                }
                None if !skipped.contains(&node.id) => skipped.push(node.id.clone()),
                None => {}
            }
        }
        Ok((pieces, skipped))
    }

    pub fn is_upscale(&self, i: usize) -> bool {
        matches!(self.items[i], Item::Upscale(_))
    }

    /// Whether a cut reads node `i`'s file: a shot's, or its upscale's.
    pub fn cut_reads(&self, i: usize) -> bool {
        matches!(self.items[i], Item::Shot(_) | Item::Upscale(_))
    }
}

/// Cuts `pieces` into `<project>/<name>` with cozy's ffmpeg, here, on `canvas` (its short edge, or
/// the first piece's): every piece fills it and is center-cropped, never letterboxed or stretched.
/// By fast join when every piece is already that size: each piece's span between keyframes is
/// copied, only the GOPs at its ends are encoded (kept in `<project>/.cuts` by file and span, so a
/// re-cut encodes only what changed), and the sound is encoded once. Pieces that differ in codec,
/// size or frame rate are encoded whole.
pub fn cut(
    ffmpeg: &Path,
    ffprobe: &Path,
    folder: &Folder,
    pieces: &[Piece],
    canvas: &Canvas,
    name: &str,
) -> Result<PathBuf, Error> {
    let probes = pieces.iter().map(|piece| probe(ffprobe, &piece.path)).collect::<Result<Vec<_>, _>>()?;
    let (width, height) = canvas.size(canvas.short.unwrap_or_else(|| probes[0].size.0.min(probes[0].size.1)));
    let ranges: Vec<(f64, f64)> = pieces
        .iter()
        .zip(&probes)
        .map(|(p, probe)| (p.start.unwrap_or(0.0), p.end.unwrap_or(probe.seconds).min(probe.seconds)))
        .collect();
    let io = |e: std::io::Error| failed(format!("{}: {e}", folder.dir.display()));
    let work = tempfile::tempdir_in(&folder.dir).map_err(io)?;
    let (cache, video) = (folder.dir.join(".cuts"), work.path().join("video.mp4"));
    std::fs::create_dir_all(&cache).map_err(io)?;
    let mut kept = BTreeSet::new();
    let fits = |probe: &Probe| probe.size == (width, height);
    if probes.iter().all(|probe| probe.video == probes[0].video && probe.video.starts_with("h264 ") && fits(probe)) {
        let mut list = String::new();
        for (k, ((piece, probe), &(start, end))) in pieces.iter().zip(&probes).zip(&ranges).enumerate() {
            for (n, (from, to, copy)) in spans(start, end, &probe.keys, probe.rate).into_iter().enumerate() {
                let (from_s, to_s) = (format!("{from:.6}"), format!("{:.6}", to - from));
                let part = match copy {
                    true => work.path().join(format!("{k}-{n}.ts")),
                    false => cache
                        .join(format!("{}-{from:.3}-{to:.3}.ts", &piece.digest.trim_start_matches("sha256:")[..16])),
                };
                let source = piece.path.as_os_str();
                if copy {
                    run(
                        ffmpeg,
                        &[
                            "-ss".as_ref(),
                            from_s.as_ref(),
                            "-i".as_ref(),
                            source,
                            "-t".as_ref(),
                            to_s.as_ref(),
                            "-map".as_ref(),
                            "0:v:0".as_ref(),
                            "-c".as_ref(),
                            "copy".as_ref(),
                            "-f".as_ref(),
                            "mpegts".as_ref(),
                            part.as_os_str(),
                        ],
                    )?;
                } else if !part.exists() {
                    let staged = work.path().join(format!("{k}-{n}.ts"));
                    run(
                        ffmpeg,
                        &[
                            "-ss".as_ref(),
                            from_s.as_ref(),
                            "-i".as_ref(),
                            source,
                            "-t".as_ref(),
                            to_s.as_ref(),
                            "-map".as_ref(),
                            "0:v:0".as_ref(),
                            "-c:v".as_ref(),
                            "libx264".as_ref(),
                            "-preset".as_ref(),
                            "veryfast".as_ref(),
                            "-crf".as_ref(),
                            "18".as_ref(),
                            "-pix_fmt".as_ref(),
                            probe.pix_fmt.as_ref(),
                            "-r".as_ref(),
                            probe.rate_text.as_ref(),
                            "-f".as_ref(),
                            "mpegts".as_ref(),
                            staged.as_os_str(),
                        ],
                    )?;
                    std::fs::rename(&staged, &part).map_err(io)?;
                }
                if !copy {
                    kept.insert(part.file_name().expect("a part has a name").to_owned());
                }
                let _ = writeln!(list, "file '{}'", part.display().to_string().replace('\'', r"'\''"));
            }
        }
        let listed = work.path().join("pieces.txt");
        std::fs::write(&listed, list).map_err(io)?;
        run(
            ffmpeg,
            &[
                "-f".as_ref(),
                "concat".as_ref(),
                "-safe".as_ref(),
                "0".as_ref(),
                "-i".as_ref(),
                listed.as_os_str(),
                "-c".as_ref(),
                "copy".as_ref(),
                "-f".as_ref(),
                "mp4".as_ref(),
                video.as_os_str(),
            ],
        )?;
    } else {
        let (mut filter, mut joined, mut inputs) = (String::new(), String::new(), vec![]);
        for (k, (piece, (start, end))) in pieces.iter().zip(&ranges).enumerate() {
            inputs.extend(["-i".into(), piece.path.clone().into_os_string()]);
            let _ = write!(
                filter,
                "[{k}:v]trim=start={start}:end={end},setpts=PTS-STARTPTS,\
                 scale={width}:{height}:force_original_aspect_ratio=increase,crop={width}:{height},setsar=1[v{k}];"
            );
            let _ = write!(joined, "[v{k}]");
        }
        let _ = write!(filter, "{joined}concat=n={}:v=1:a=0[v]", pieces.len());
        let mut args: Vec<std::ffi::OsString> = inputs;
        args.extend(
            [
                "-filter_complex",
                &filter,
                "-map",
                "[v]",
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "18",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "mp4",
            ]
            .map(Into::into),
        );
        args.push(video.clone().into_os_string());
        run(ffmpeg, &args.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
    }
    let mut mux: Vec<std::ffi::OsString> = vec!["-i".into(), video.into_os_string()];
    if probes.iter().all(|probe| probe.sound) {
        // The sound once, over the whole edit.
        let (mut filter, mut joined, mut inputs) = (String::new(), String::new(), vec![]);
        for (k, (piece, (start, end))) in pieces.iter().zip(&ranges).enumerate() {
            inputs.extend(["-i".into(), piece.path.clone().into_os_string()]);
            let _ = write!(filter, "[{k}:a]atrim=start={start}:end={end},asetpts=PTS-STARTPTS[a{k}];");
            let _ = write!(joined, "[a{k}]");
        }
        let _ = write!(filter, "{joined}concat=n={}:v=0:a=1[a]", pieces.len());
        let sound = work.path().join("sound.m4a");
        let mut args: Vec<std::ffi::OsString> = inputs;
        args.extend(["-filter_complex", &filter, "-map", "[a]", "-c:a", "aac", "-f", "mp4"].map(Into::into));
        args.push(sound.clone().into_os_string());
        run(ffmpeg, &args.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
        mux.extend(["-i".into(), sound.into_os_string(), "-map".into(), "1:a".into()]);
    }
    let part = work.path().join(name);
    mux.extend(["-map", "0:v", "-c", "copy", "-movflags", "+faststart", "-f", "mp4"].map(Into::into));
    mux.push(part.clone().into_os_string());
    run(ffmpeg, &mux.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
    let out = folder.dir.join(name);
    std::fs::rename(&part, &out).map_err(io)?;
    // The cache keeps what this edit used.
    for entry in std::fs::read_dir(&cache).map_err(io)?.flatten() {
        if !kept.contains(&entry.file_name()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(out)
}

/// A piece's spans from `start` to `end`: copied between its first and last keyframe inside them,
/// encoded before and after.
fn spans(start: f64, end: f64, keys: &[f64], rate: f64) -> Vec<(f64, f64, bool)> {
    let frame = 1.0 / rate;
    let inside: Vec<f64> = keys.iter().copied().filter(|&key| key >= start - 1e-6 && key <= end + 1e-6).collect();
    match (inside.first(), inside.last()) {
        (Some(&first), Some(&last)) if last - first >= frame => {
            let mut spans = vec![];
            if first - start >= frame / 2.0 {
                spans.push((start, first, false));
            }
            spans.push((first, last, true));
            if end - last >= frame / 2.0 {
                spans.push((last, end, false));
            }
            spans
        }
        _ => vec![(start, end, false)],
    }
}

/// What the edit needs of a file: its video (`<codec> <W>x<H> <pix_fmt> <rate>`), size, pixel
/// format and frame rate, whether it has sound, its length and its keyframes' times.
struct Probe {
    video: String,
    size: (u64, u64),
    pix_fmt: String,
    rate: f64,
    rate_text: String,
    sound: bool,
    seconds: f64,
    keys: Vec<f64>,
}

/// A video's width and height, when ffprobe reads them.
pub fn size(ffprobe: &Path, path: &Path) -> Option<[u64; 2]> {
    probe(ffprobe, path).ok().map(|probe| [probe.size.0, probe.size.1])
}

fn probe(ffprobe: &Path, path: &Path) -> Result<Probe, Error> {
    let read = |args: &[&str]| -> Result<Vec<u8>, Error> {
        let done = std::process::Command::new(ffprobe)
            .args(["-v", "error"])
            .args(args)
            .arg(path)
            .output()
            .map_err(|e| failed(format!("ffprobe did not start: {e}")))?;
        Ok(done.stdout)
    };
    let read_json: Value = serde_json::from_slice(&read(&[
        "-show_entries",
        "stream=codec_type,codec_name,width,height,pix_fmt,r_frame_rate:format=duration",
        "-of",
        "json",
    ])?)
    .unwrap_or_default();
    let streams = read_json["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|s| s["codec_type"] == "video").cloned().unwrap_or_default();
    let (Some(width), Some(height)) = (video["width"].as_u64(), video["height"].as_u64()) else {
        return Err(failed(format!("{} holds no video ffprobe reads", path.display())));
    };
    let rate_text = video["r_frame_rate"].as_str().unwrap_or("24/1").to_owned();
    let rate = match rate_text.split_once('/') {
        Some((n, d)) => n.parse::<f64>().unwrap_or(24.0) / d.parse::<f64>().unwrap_or(1.0).max(1.0),
        None => rate_text.parse().unwrap_or(24.0),
    };
    let pix_fmt = video["pix_fmt"].as_str().unwrap_or("yuv420p").to_owned();
    let codec = video["codec_name"].as_str().unwrap_or("");
    let keys = String::from_utf8_lossy(&read(&[
        "-select_streams",
        "v:0",
        "-skip_frame",
        "nokey",
        "-show_entries",
        "frame=pts_time",
        "-of",
        "csv=p=0",
    ])?)
    .lines()
    .filter_map(|line| line.trim().trim_end_matches(',').parse().ok())
    .collect();
    Ok(Probe {
        video: format!("{codec} {width}x{height} {pix_fmt} {rate_text}"),
        size: (width, height),
        pix_fmt,
        rate,
        rate_text,
        sound: streams.iter().any(|s| s["codec_type"] == "audio"),
        seconds: read_json["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0),
        keys,
    })
}

/// One ffmpeg run, refused with what it said when it fails.
fn run(ffmpeg: &Path, args: &[&std::ffi::OsStr]) -> Result<(), Error> {
    let done = std::process::Command::new(ffmpeg)
        .args(["-y", "-v", "error"])
        .args(args)
        .output()
        .map_err(|e| failed(format!("ffmpeg did not start: {e}")))?;
    match done.status.success() {
        true => Ok(()),
        false => {
            Err(failed(format!("ffmpeg could not cut the edit: {}", String::from_utf8_lossy(&done.stderr).trim())))
        }
    }
}

/// A file a node reads that is no node's, as its key (`<node>.<field>`) and digest; it must be there.
fn input_file(folder: &Folder, id: &str, field: &str, path: &str) -> Result<(String, String), Error> {
    if !folder.path(path).is_file() {
        return Err(invalid("file_missing", format!("{id}'s {field}: {path} is not there")).remedy("fix the path"));
    }
    Ok((format!("{id}.{field}"), folder.digest(path)?))
}

/// A shot's own files: its references', its plate's, its first and last frames.
fn shot_inputs(folder: &Folder, shot: &Shot) -> Result<Vec<(String, String)>, Error> {
    let mut found = vec![];
    for entry in &shot.references {
        let name = entry.get("name").and_then(Value::as_str).unwrap_or_default();
        for field in FILE_FIELDS {
            if let Some(path) = entry.get(field).and_then(Value::as_str) {
                found.push(input_file(folder, &shot.name, &format!("{name}.{field}"), path)?);
            }
        }
    }
    if let Some(path) = shot.plate.as_ref().and_then(|plate| plate.get("video")).and_then(Value::as_str) {
        found.push(input_file(folder, &shot.name, "plate", path)?);
    }
    for (field, path) in [("first_frame", &shot.first_frame), ("last_frame", &shot.last_frame)] {
        if let Some(path) = path {
            found.push(input_file(folder, &shot.name, field, path)?);
        }
    }
    Ok(found)
}

/// A node's current file (path and digest) and its versions' files.
type Versions = (Option<(String, String)>, Vec<String>);

/// A node's current file and its versions' files (`Node::file`, `Node::versions`): the version
/// project.lock approves; else the file attached in project.yaml as `field` (refused, with the fix,
/// when it is not there), or that picture's clean when the node `cleans`.
fn current(
    folder: &Folder,
    lock: &Lock,
    id: &str,
    field: &str,
    attached: &Option<String>,
    cleans: bool,
) -> Result<Versions, Error> {
    let node = lock.node(id);
    let versions: Vec<String> = node.map(|node| node.versions.iter().map(|made| made.file.clone()).collect()).unwrap_or_default();
    if let Some(approved) = node.and_then(|node| node.approved.clone()) {
        let digest = digest_of(&approved);
        return Ok((Some((approved, digest)), versions));
    }
    let Some(file) = attached else { return Ok((None, versions)) };
    if !folder.path(file).is_file() {
        return Err(invalid("file_missing", format!("{id}'s {field}: {file} is not there"))
            .remedy(format!("fix the path, or remove `{field}:` and `cozy render {id}` makes one")));
    }
    let digest = folder.digest(file)?;
    let clean = (node.into_iter().flat_map(|node| &node.versions))
        .find(|made| cleans && made.clean_of.as_deref() == Some(digest.as_str()));
    Ok((Some(clean.map_or((file.clone(), digest), |made| (made.file.clone(), digest_of(&made.file)))), versions))
}

/// A list a person may leave empty as `key:` alone (a null).
fn listed<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(read: D) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(read)?.unwrap_or_default())
}

fn invalid(code: &str, message: String) -> Error {
    Error::new(Kind::Validation, code, message)
}

fn failed(message: String) -> Error {
    Error::new(Kind::Unavailable, "edit_failed", message)
}
