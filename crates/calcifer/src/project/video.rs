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
use sha2::{Digest as _, Sha256};

use super::{
    folder::{self, Asset, Call, Folder, Lock, Made, Upstream},
    graph::{Edge, Graph, Node, mentions, name_key},
    host::{Accepts, Renderer},
};

pub const TYPE: &str = "video";
/// The name `cozy project render all` means: every node, brought up to date.
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
    "# A video project (`cozy project --help`). Fill in the lists, then, inside this folder,
#   cozy project render all --on <machine>
# makes everything missing or out of date, in order, and cuts edit.mp4 (and its timeline, edit.otio) here. A file named here
# (relative to this folder, or absolute) is linked into assets/ on first use; everything cozy makes
# is kept there too, by content, and `cozy project status` prints each node's file.
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
  #    references: [{name: stride, kind: video, video: ~/footage/walk.mp4}]}   # also plate: {video: …}, first_frame, last_frame
# a node's render: {with: <function or ./dir/function>, models: {...}, lora: [...], options: {...}} renders it another way
# image: (or a shot's video:) attaches your own file (theo.png), used until you approve one cozy made
# resolution: 1440p   # the edit's short edge: shots below it are upscaled (their approved version), and
# aspect_ratio: \"16:9\"   # the canvas every shot fills; both are the project's alone
# fps: 24   # the edit's frame rate, fixed per project: edit: positions are frames at it
# edit: [{shot: walk, start: 12, end: 156}]   # shots and frames (end exclusive) the edit cuts; absent: every shot, whole
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
    /// The edit's frame rate, which its `edit:` positions count in (default 24, H3's).
    fps: Option<f64>,
    #[serde(default, deserialize_with = "listed")]
    entities: Vec<Entity>,
    #[serde(default, deserialize_with = "listed")]
    world_states: Vec<WorldState>,
    #[serde(default, deserialize_with = "listed")]
    shots: Vec<Shot>,
    /// None: every shot, in order, whole.
    edit: Option<Vec<Cut>>,
}

/// How a node is rendered: `with` a function as `cozy run` takes it (`[org/]package/function[@release]`,
/// or local code, `./dir/function`, which Calcifer makes ready), cozy run's model choices
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

/// A piece of a shot in the edit: from frame `start` (default its first) to frame `end` (exclusive;
/// default its `duration_s`), counted at the project's `fps`.
#[derive(Deserialize)]
struct Cut {
    shot: String,
    start: Option<u64>,
    end: Option<u64>,
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

/// A piece of the edit: a shot's node, its file (here, its digest, and its asset's path in the
/// project, `assets/<sha256>.<ext>`), and its frames from `start` to `end` (exclusive) at the
/// project's rate.
pub struct Piece {
    pub node: String,
    pub path: PathBuf,
    pub digest: String,
    pub asset: String,
    pub start: u64,
    pub end: u64,
}

pub struct Video {
    project: Project,
    /// Each file project.yaml names, by the path written there, as taken in.
    inputs: BTreeMap<String, folder::Input>,
    /// The files it names that are gone: what reads one cannot render.
    missing: BTreeSet<String>,
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
    /// Frames per second: the edit's rate, which its positions count in.
    pub fps: f64,
}

impl Canvas {
    fn read(resolution: Option<&str>, aspect: Option<&str>, fps: Option<f64>) -> Result<Self, Error> {
        let fps = fps.unwrap_or(24.0);
        if !(fps.is_finite() && fps > 0.0 && fps <= 240.0) {
            return Err(invalid("fps", format!("fps {fps} is not a frame rate such as 24, 25 or 30")));
        }
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
        Ok(Self { aspect: format!("{a}:{b}"), ratio: w / h, short, fps })
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

/// What a request reads from upstream: the edge, and the file it reads now (here, in the project,
/// and its sha256).
struct Read<'a> {
    edge: &'a Edge,
    path: PathBuf,
    file: String,
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
        // Every file it names, linked as an asset on first use (and again when it changed), and
        // recorded in project.lock with the path written. One that is gone keeps its record: what
        // reads it cannot render until it is linked again.
        let (mut inputs, mut missing) = (BTreeMap::new(), BTreeSet::new());
        for file in named(&project) {
            let known = lock.inputs.get(file);
            match folder.take_in(file, known)? {
                Some(input) => {
                    // A link is the original: it changing changes the project. A clone or copy is taken in again.
                    let changed = known.is_some_and(|known| known.asset.sha256 != input.asset.sha256);
                    if changed && known.is_some_and(|known| known.method.linked()) {
                        warnings.push(format!("reference changed: {file}; what reads it is out of date"));
                    }
                    if input.method == folder::Method::Copy && known.is_none_or(|known| known.method != folder::Method::Copy) {
                        warnings.push(format!("{file}: copied into {}/, as this computer allows it no link", folder::ASSETS));
                    }
                    inputs.insert(file.to_owned(), input);
                }
                None => {
                    warnings.push(format!("reference missing: {file}: relink (put it back, or fix its path in {})", folder::PROJECT));
                    missing.insert(file.to_owned());
                    inputs.extend(known.map(|known| (file.to_owned(), known.clone())));
                }
            }
        }
        if inputs != lock.inputs {
            folder.update_lock(|lock| lock.inputs = inputs.clone())?;
        }
        let input_file = |id: &str, field: &str, path: &str| input_file(&inputs, id, field, path);
        // Its current file and its versions' files: the approved version, else the file attached by
        // hand (there, or the node is named with the fix), or that picture's clean.
        let versions = |id: &str, attached: &Option<String>, cleans: bool| current(&inputs, &lock, id, attached, cleans);
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
            let (file, made) = versions(&entity.name, &entity.image, cleans)?;
            let mut reads = vec![];
            if let Some(voice) = &entity.voice {
                if entity.kind != EntityKind::Character {
                    return Err(invalid("voice_owner", format!("{} is a {kind}: only a character has a voice", entity.name)));
                }
                reads.push(input_file(&entity.name, "voice", voice)?);
            }
            nodes.push(Node {
                id: entity.name.clone(),
                kind,
                noun: "version",
                mentionable: true,
                state: false,
                order: None,
                texts: vec![entity.description.clone()],
                needs: vec![],
                writes: vec![],
                inputs: reads,
                file,
                versions: made,
            });
            items.push(Item::Entity(a));
            for (s, setup) in entity.setups.iter().enumerate() {
                let id = format!("{}.{}", entity.name, setup.name);
                let (file, made) = versions(&id, &setup.image, false)?;
                nodes.push(Node {
                    id,
                    kind: "setup",
                    noun: "version",
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
            let (file, made) = versions(&state.name, &state.image, false)?;
            nodes.push(Node {
                id: state.name.clone(),
                kind: "world_state",
                noun: "version",
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
            let (file, made) = versions(&shot.name, &shot.video, false)?;
            nodes.push(Node {
                id: shot.name.clone(),
                kind: "shot",
                noun: "take",
                mentionable: false,
                state: false,
                order: Some(k),
                texts,
                needs,
                writes: shot.writes.iter().map(|write| write.state.clone()).collect(),
                inputs: shot_inputs(&input_file, shot)?,
                file,
                versions: made,
            });
            items.push(Item::Shot(k));
            let id = format!("{}.upscale", shot.name);
            let (file, made) = versions(&id, &shot.upscale.video, false)?;
            nodes.push(Node {
                id,
                kind: "upscale",
                noun: "take",
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
            let said = format!("{} is named `{ALL}`, which `cozy project render {ALL}` means", all.id);
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
        let canvas = Canvas::read(project.resolution.as_deref(), project.aspect_ratio.as_deref(), project.fps)?;
        let sizes = (graph.nodes.iter())
            .map(|node| {
                let (path, _) = node.file.as_ref()?;
                lock.made(&node.id, path)?.size
            })
            .collect();
        Ok(Self { project, inputs, missing, items, graph, warnings, canvas, sizes })
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
        lock.made(&node.id, &file.0).is_none().then(|| file.clone())
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

    /// The files gone that node `i` reads, its own or through what it reads: it cannot render until
    /// they are linked again.
    pub fn missing(&self, i: usize) -> Vec<&str> {
        let mut gone: BTreeSet<&str> = self.own_missing(i).into_iter().collect();
        let mut seen = vec![false; self.graph.nodes.len()];
        let mut next: Vec<usize> = self.graph.upstream[i].iter().map(|edge| edge.node).collect();
        while let Some(j) = next.pop() {
            if !std::mem::replace(&mut seen[j], true) {
                gone.extend(self.own_missing(j));
                next.extend(self.graph.upstream[j].iter().map(|edge| edge.node));
            }
        }
        gone.into_iter().collect()
    }

    /// The files project.yaml names for node `i` itself that are gone.
    fn own_missing(&self, i: usize) -> Vec<&str> {
        let mut files: Vec<&str> = vec![];
        match self.items[i] {
            Item::Entity(a) => {
                let entity = &self.project.entities[a];
                files.extend(entity.image.iter().chain(&entity.voice).map(String::as_str));
            }
            Item::Setup(a, s) => files.extend(self.project.entities[a].setups[s].image.as_deref()),
            Item::State(w) => files.extend(self.project.world_states[w].image.as_deref()),
            Item::Shot(k) => {
                let shot = &self.project.shots[k];
                files.extend([&shot.video, &shot.first_frame, &shot.last_frame].into_iter().filter_map(Option::as_deref));
                files.extend(shot.plate.as_ref().and_then(|plate| plate.get("video")).and_then(Value::as_str));
                let references = shot.references.iter().flat_map(|entry| FILE_FIELDS.iter().filter_map(|field| entry.get(*field)));
                files.extend(references.filter_map(Value::as_str));
            }
            Item::Upscale(k) => files.extend(self.project.shots[k].upscale.video.as_deref()),
        }
        files.into_iter().filter(|file| self.missing.contains(*file)).collect()
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
        own: &BTreeMap<String, Asset>,
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
        own: &BTreeMap<String, Asset>,
    ) -> Result<Request, Error> {
        let graph = &self.graph;
        let node = &graph.nodes[i];
        let mut upstream = vec![];
        let mut reads = vec![];
        for edge in &graph.upstream[i] {
            let up = &graph.nodes[edge.node];
            let (file, digest) = graph.read(lock, edge).ok_or_else(|| {
                Error::new(
                    Kind::Usage,
                    "upstream_empty",
                    format!("{} reads {}, which has no version yet", node.id, up.id),
                )
                .next(format!("cozy project render {}", up.id))
            })?;
            upstream.push(Upstream { node: up.id.clone(), state: edge.state.clone(), asset: digest.clone() });
            reads.push(Read { edge, path: folder.path(&file), file, digest });
        }
        upstream.extend(node.inputs.iter().map(|(key, file)| Upstream { node: key.clone(), state: None, asset: file.clone() }));
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
                let made = lock.made(&shot.name, &reads[0].file);
                let same = made.and_then(|made| made.made_by.as_ref())
                    .is_some_and(|by| by.with.starts_with(&format!("{}/", renderer.package)));
                if let Some(latents) = made.and_then(|made| made.assets.get("latents")).filter(|_| same) {
                    let digest = latents.sha256.clone();
                    files.push(Input { field: "latents".into(), path: folder.path(&latents.path), digest: digest.clone(), at: None });
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
        own: &BTreeMap<String, Asset>,
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
        let file = |path: &str| -> Result<(PathBuf, String), Error> {
            let input = self.inputs.get(path).ok_or_else(|| invalid("file_missing", format!("{}: {path} is not there", shot.name)))?;
            Ok((folder.path(&input.asset.path), input.asset.sha256.clone()))
        };
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
                        Some((_, frame)) => vec![("image", folder.path(&frame.path), frame.sha256.clone())],
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
                let digest = file.sha256.clone();
                files.push(Input { field: format!("continue_from.{field}"), path: folder.path(&file.path), digest: digest.clone(), at: None });
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

    /// The pieces of the edit (`preview`: of its native cut): each cut's shot file and frames, by
    /// default the shot's `duration_s` (a renderer may run past it). The edit takes each shot's
    /// upscale where it has one to make (`wanted`), else the shot; the preview takes the shot. And
    /// the nodes it would cut that have no file yet, skipped.
    pub fn cuts(&self, folder: &Folder, preview: bool) -> Result<(Vec<Piece>, Vec<String>), Error> {
        let cuts: Vec<(&str, Option<u64>, Option<u64>)> = match &self.project.edit {
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
            let (start, end) = (start.unwrap_or(0), end.unwrap_or((shot.duration_s * self.canvas.fps).round() as u64));
            if start >= end {
                let said = format!("the edit cuts {name} from frame {start} to {end}: nothing");
                return Err(invalid("cut_empty", said).remedy("each edit entry is {shot, start?, end?} in frames, start before end"));
            }
            match &node.file {
                Some((file, digest)) => {
                    let (node, path, asset) = (node.id.clone(), folder.path(file), file.clone());
                    pieces.push(Piece { node, path, digest: digest.clone(), asset, start, end })
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

/// A cut made: its length in frames, and the pieces whose takes were retimed to the project's rate.
pub struct Cutted {
    pub frames: u64,
    pub retimed: Vec<String>,
}

/// Cuts `pieces` into `<project>/<name>` with cozy's ffmpeg, here, on `canvas` (its short edge, or
/// the first piece's) at its frame rate, and writes the same edit as OpenTimelineIO beside it
/// (`<name>.otio`, `otio`). Every in and out point is an exact frame. Every piece fills the canvas
/// and is center-cropped, never letterboxed or stretched. By fast join when every piece is already
/// that size and rate: each piece's frames between keyframes are copied, only the GOPs at its ends
/// are encoded (kept in `cache`, cozy's cache folder, by take, frames and encoding, so a re-cut
/// encodes only what changed; `prune` keeps it small), and the sound is encoded once. Pieces that
/// differ in codec, size or frame rate are encoded whole, a take at another rate retimed to the
/// project's. Nothing is encoded above or below its sources: an edge piece at its own take's video
/// bitrate, a whole encode and the sound at the highest of the takes cut in (bitrate, sample rate,
/// channels).
pub fn cut(
    ffmpeg: &Path,
    ffprobe: &Path,
    folder: &Folder,
    pieces: &[Piece],
    canvas: &Canvas,
    name: &str,
    cache: &Path,
) -> Result<Cutted, Error> {
    let probes = pieces.iter().map(|piece| probe(ffprobe, &piece.path)).collect::<Result<Vec<_>, _>>()?;
    let (fps, rate) = (canvas.fps, canvas.fps.to_string());
    let (width, height) = canvas.size(canvas.short.unwrap_or_else(|| probes[0].size.0.min(probes[0].size.1)));
    let native = |probe: &Probe| (probe.rate - fps).abs() <= 0.01;
    // Each piece's frames at the project's rate, within its take.
    let mut ranges: Vec<(u64, u64)> = vec![];
    for (piece, probe) in pieces.iter().zip(&probes) {
        let frames = available(probe, fps);
        if piece.start >= frames {
            let said = format!("the edit cuts {} from frame {}, and its take has {frames} frames at {fps} fps", piece.node, piece.start);
            return Err(invalid("cut_past_end", said).remedy("start its edit entry before its take's last frame"));
        }
        ranges.push((piece.start, piece.end.min(frames)));
    }
    let retimed: Vec<String> = (pieces.iter().zip(&probes))
        .filter(|(_, probe)| !native(probe))
        .map(|(piece, probe)| format!("{} ({} fps)", piece.node, probe.rate))
        .collect();
    let io = |e: std::io::Error| failed(format!("{}: {e}", cache.display()));
    std::fs::create_dir_all(cache).map_err(io)?;
    let work = tempfile::tempdir_in(cache).map_err(io)?;
    let video = work.path().join("video.mp4");
    let highest = probes.iter().filter_map(|probe| probe.video_bit_rate).max();
    let os = |args: &[&str]| args.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
    let fast = (probes.iter())
        .all(|p| p.video == probes[0].video && p.video.starts_with("h264 ") && p.size == (width, height) && native(p));
    if fast {
        let mut list = String::new();
        for (k, ((piece, probe), &(start, end))) in pieces.iter().zip(&probes).zip(&ranges).enumerate() {
            for (n, (from, to, copy)) in spans(start, end, &probe.keys).into_iter().enumerate() {
                let count = (to - from).to_string();
                // An edge piece is the same file for the same take, frames and encoding, whatever cuts it.
                let encoding = format!("libx264 veryfast {} {} {}", bit_rate(probe.video_bit_rate).join(" "), probe.pix_fmt, probe.rate_text);
                let encoding = &hex::encode(Sha256::digest(encoding.as_bytes()))[..8];
                let take = &piece.digest.trim_start_matches("sha256:")[..16];
                let part = match copy {
                    true => work.path().join(format!("{k}-{n}.ts")),
                    false => cache.join(format!("{take}-f{from}-f{to}-{encoding}.ts")),
                };
                if copy {
                    // From the keyframe at `from`: half a frame past it, so the seek lands on it.
                    let at = format!("{:.6}", (from as f64 + 0.5) / probe.rate);
                    let mut args = os(&["-ss", &at, "-i"]);
                    args.push(piece.path.clone().into_os_string());
                    args.extend(os(&["-frames:v", &count, "-map", "0:v:0", "-c", "copy", "-f", "mpegts"]));
                    args.push(part.clone().into_os_string());
                    run(ffmpeg, &args.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
                } else if !part.exists() {
                    // From the keyframe at or before `from`, its frames counted to `from`: no timestamp
                    // decides which frame comes first.
                    let staged = work.path().join(format!("{k}-{n}.ts"));
                    let key = probe.keys.iter().copied().filter(|&key| key <= from).max().unwrap_or(0);
                    let at = format!("{:.6}", (key as f64 + 0.5) / probe.rate);
                    let mut args = os(&["-noaccurate_seek", "-ss", &at, "-i"]);
                    args.push(piece.path.clone().into_os_string());
                    let trim = format!("trim=start_frame={},setpts=PTS-STARTPTS", from - key);
                    args.extend(os(&["-map", "0:v:0", "-vf", &trim, "-frames:v", &count, "-c:v", "libx264", "-preset", "veryfast"]));
                    args.extend(bit_rate(probe.video_bit_rate).iter().map(Into::into));
                    args.extend(os(&["-pix_fmt", &probe.pix_fmt, "-r", &probe.rate_text, "-f", "mpegts"]));
                    args.push(staged.clone().into_os_string());
                    run(ffmpeg, &args.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
                    std::fs::rename(&staged, &part).map_err(io)?;
                } else {
                    // Used now: its age starts again.
                    let _ = std::fs::File::options().write(true).open(&part).and_then(|file| file.set_modified(std::time::SystemTime::now()));
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
        for (k, ((piece, probe), (start, end))) in pieces.iter().zip(&probes).zip(&ranges).enumerate() {
            inputs.extend(["-i".into(), piece.path.clone().into_os_string()]);
            // A take at another rate is retimed first (each frame its source's nearest in time), then
            // cut by frame like any other.
            let retime = if native(probe) { String::new() } else { format!("fps={rate},") };
            let _ = write!(
                filter,
                "[{k}:v]{retime}trim=start_frame={start}:end_frame={end},setpts=PTS-STARTPTS,\
                 scale={width}:{height}:force_original_aspect_ratio=increase,crop={width}:{height},setsar=1[v{k}];"
            );
            let _ = write!(joined, "[v{k}]");
        }
        let _ = write!(filter, "{joined}concat=n={}:v=1:a=0[v]", pieces.len());
        let mut args: Vec<std::ffi::OsString> = inputs;
        let encode = ["-filter_complex", &filter, "-map", "[v]", "-c:v", "libx264", "-preset", "veryfast"];
        args.extend(encode.into_iter().chain(bit_rate(highest).iter().map(String::as_str)).map(Into::into));
        args.extend(["-pix_fmt", "yuv420p", "-r", &rate, "-f", "mp4"].map(Into::into));
        args.push(video.clone().into_os_string());
        run(ffmpeg, &args.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
    }
    let mut mux: Vec<std::ffi::OsString> = vec!["-i".into(), video.into_os_string()];
    let sounds: Vec<&Sound> = probes.iter().filter_map(|probe| probe.sound.as_ref()).collect();
    if sounds.len() == probes.len() {
        // The sound once, over the whole edit, each piece's at its frames' times.
        let (mut filter, mut joined, mut inputs) = (String::new(), String::new(), vec![]);
        for (k, (piece, (start, end))) in pieces.iter().zip(&ranges).enumerate() {
            inputs.extend(["-i".into(), piece.path.clone().into_os_string()]);
            let (start, end) = (*start as f64 / fps, *end as f64 / fps);
            let _ = write!(filter, "[{k}:a]atrim=start={start}:end={end},asetpts=PTS-STARTPTS[a{k}];");
            let _ = write!(joined, "[a{k}]");
        }
        let _ = write!(filter, "{joined}concat=n={}:v=0:a=1[a]", pieces.len());
        let sound = work.path().join("sound.m4a");
        let mut args: Vec<std::ffi::OsString> = inputs;
        let most = |of: fn(&Sound) -> Option<u64>| sounds.iter().filter_map(|sound| of(sound)).max();
        let mut encode = vec!["-filter_complex".to_owned(), filter, "-map".into(), "[a]".into(), "-c:a".into(), "aac".into()];
        for (flag, value) in [("-b:a", most(|s| s.bit_rate)), ("-ar", most(|s| s.sample_rate)), ("-ac", most(|s| s.channels))] {
            encode.extend(value.map(|value| [flag.to_owned(), value.to_string()]).into_iter().flatten());
        }
        args.extend(encode.into_iter().chain(["-f".into(), "mp4".into()]).map(Into::into));
        args.push(sound.clone().into_os_string());
        run(ffmpeg, &args.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
        mux.extend(["-i".into(), sound.into_os_string(), "-map".into(), "1:a".into()]);
    }
    // Made beside the cut, then named it: a reader never meets half a file.
    let (out, part) = (folder.dir.join(name), folder.dir.join(format!(".{name}.part")));
    mux.extend(["-map", "0:v", "-c", "copy", "-movflags", "+faststart", "-f", "mp4"].map(Into::into));
    mux.push(part.clone().into_os_string());
    run(ffmpeg, &mux.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
    std::fs::rename(&part, &out).map_err(io)?;
    let timeline = otio(name, pieces, &probes, &ranges, fps);
    let otio_path = folder.dir.join(Path::new(name).with_extension("otio"));
    let text = serde_json::to_string_pretty(&timeline).expect("a timeline serializes") + "\n";
    platform::durable::replace(&otio_path, text.as_bytes(), 0o644).map_err(|e| failed(format!("{}: {e}", otio_path.display())))?;
    prune(cache);
    Ok(Cutted { frames: ranges.iter().map(|(start, end)| end - start).sum(), retimed })
}

/// How many frames `probe`'s take has at the project's `fps`: its own, else as many as retiming
/// makes of them.
fn available(probe: &Probe, fps: f64) -> u64 {
    match (probe.rate - fps).abs() <= 0.01 {
        true => probe.frames,
        false => (probe.frames as f64 * fps / probe.rate).floor() as u64,
    }
}

/// The latest `cut` (`edit.mp4`, `preview.mp4`) as its timeline beside it says: how many frames
/// long, and its clips retimed (`<shot> (<fps> fps)`).
pub fn timeline(folder: &Folder, cut: &str) -> Option<Cutted> {
    let text = std::fs::read(folder.dir.join(Path::new(cut).with_extension("otio"))).ok()?;
    let timeline: Value = serde_json::from_slice(&text).ok()?;
    let video = timeline["tracks"]["children"].as_array()?.iter().find(|track| track["kind"] == "Video")?;
    let clips = video["children"].as_array()?;
    let frames = clips.iter().filter_map(|clip| clip["source_range"]["duration"]["value"].as_f64()).sum::<f64>();
    let retimed = (clips.iter())
        .filter_map(|clip| Some(format!("{} ({} fps)", clip["name"].as_str()?, clip["metadata"]["cozy"]["retimed_from_fps"].as_f64()?)))
        .collect();
    Some(Cutted { frames: frames.round() as u64, retimed })
}

/// The edit as OpenTimelineIO's own JSON (`Timeline.1`, as OpenTimelineIO 0.18 writes it), written
/// by hand: the project's rate, a video track and an audio track, one item per piece, each clip's
/// source range frame-exact at that rate and its reference that take's asset (`assets/<sha256>.<ext>`,
/// never whichever is approved, so the timeline keeps naming what it cut). A piece without sound is
/// a gap on the audio track. A take retimed to the project's rate says so in its clips' metadata.
fn otio(name: &str, pieces: &[Piece], probes: &[Probe], ranges: &[(u64, u64)], fps: f64) -> Value {
    let time = |value: u64| json!({"OTIO_SCHEMA": "RationalTime.1", "rate": fps, "value": value as f64});
    let range = |start: u64, frames: u64| json!({"OTIO_SCHEMA": "TimeRange.1", "start_time": time(start), "duration": time(frames)});
    let item = |schema: &str, name: &str, metadata: Value, source: Value| {
        json!({
            "OTIO_SCHEMA": schema,
            "metadata": metadata,
            "name": name,
            "source_range": source,
            "effects": [],
            "markers": [],
            "enabled": true,
            "color": null,
        })
    };
    let track = |kind: &str, sound: bool| {
        let children = (pieces.iter().zip(probes).zip(ranges)).map(|((piece, probe), &(start, end))| {
            if sound && probe.sound.is_none() {
                return item("Gap.1", "", json!({}), range(0, end - start));
            }
            let metadata = match (probe.rate - fps).abs() > 0.01 {
                true => json!({"cozy": {"retimed_from_fps": probe.rate}}),
                false => json!({}),
            };
            let mut clip = item("Clip.2", &piece.node, metadata, range(start, end - start));
            clip["media_references"] = json!({"DEFAULT_MEDIA": {
                "OTIO_SCHEMA": "ExternalReference.1",
                "metadata": {},
                "name": piece.node,
                "available_range": range(0, available(probe, fps)),
                "available_image_bounds": null,
                "target_url": piece.asset,
            }});
            clip["active_media_reference_key"] = json!("DEFAULT_MEDIA");
            clip
        });
        let mut track = item("Track.1", kind, json!({}), Value::Null);
        track["children"] = children.collect();
        track["kind"] = json!(kind);
        track
    };
    let mut tracks = item("Stack.1", "tracks", json!({}), Value::Null);
    tracks["children"] = json!([track("Video", false), track("Audio", true)]);
    json!({
        "OTIO_SCHEMA": "Timeline.1",
        "metadata": {"cozy": {"cut": name, "fps": fps}},
        "name": Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        "global_start_time": time(0),
        "tracks": tracks,
    })
}

/// How long an edge piece is kept unused, and how much the pieces may hold together.
const PIECES_KEPT_FOR: std::time::Duration = std::time::Duration::from_secs(14 * 24 * 3600);
const PIECES_CAP: u64 = 10 << 30;

/// The edge pieces in `cache` kept while used: one unused for `PIECES_KEPT_FOR` goes, and past
/// `PIECES_CAP` the least recently used go first.
fn prune(cache: &Path) {
    let now = std::time::SystemTime::now();
    let mut pieces: Vec<(std::time::SystemTime, u64, PathBuf)> = (std::fs::read_dir(cache).into_iter().flatten().flatten())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "ts"))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), entry.path()))
        })
        .collect();
    pieces.sort();
    let mut held: u64 = pieces.iter().map(|(_, len, _)| len).sum();
    for (used, len, path) in pieces {
        let old = now.duration_since(used).is_ok_and(|age| age > PIECES_KEPT_FOR);
        if (old || held > PIECES_CAP) && std::fs::remove_file(&path).is_ok() {
            held -= len;
        }
    }
}

/// A piece's frames from `start` to `end` (exclusive): copied between its first and last keyframe
/// inside them, encoded before and after.
fn spans(start: u64, end: u64, keys: &[u64]) -> Vec<(u64, u64, bool)> {
    let inside: Vec<u64> = keys.iter().copied().filter(|&key| key >= start && key <= end).collect();
    match (inside.first(), inside.last()) {
        (Some(&first), Some(&last)) if last > first => {
            let mut spans = vec![];
            if first > start {
                spans.push((start, first, false));
            }
            spans.push((first, last, true));
            if end > last {
                spans.push((last, end, false));
            }
            spans
        }
        _ => vec![(start, end, false)],
    }
}

/// What the edit needs of a file: its video (`<codec> <W>x<H> <pix_fmt> <rate>`), size, pixel
/// format, frame rate and frames, whether it has sound, and its keyframes' frames.
struct Probe {
    video: String,
    size: (u64, u64),
    pix_fmt: String,
    rate: f64,
    rate_text: String,
    frames: u64,
    /// Its video's bitrate (bit/s): its stream's, else its whole file's.
    video_bit_rate: Option<u64>,
    sound: Option<Sound>,
    keys: Vec<u64>,
}

/// A file's sound as ffprobe reads it.
struct Sound {
    bit_rate: Option<u64>,
    sample_rate: Option<u64>,
    channels: Option<u64>,
}

/// The encoder's bitrate flags for a source's `bit_rate`: the same average, capped at it over a
/// two-second buffer; without one, the encoder's own quality.
fn bit_rate(bit_rate: Option<u64>) -> Vec<String> {
    match bit_rate {
        Some(rate) => ["-b:v", &rate.to_string(), "-maxrate", &rate.to_string(), "-bufsize", &(2 * rate).to_string()]
            .map(str::to_owned)
            .into(),
        None => vec!["-crf".into(), "18".into()],
    }
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
        "stream=codec_type,codec_name,width,height,pix_fmt,r_frame_rate,nb_frames,bit_rate,sample_rate,channels:format=duration,bit_rate,start_time",
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
    let seconds = read_json["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    let start = read_json["format"]["start_time"].as_str().and_then(|d| d.parse::<f64>().ok()).unwrap_or(0.0);
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
    .filter_map(|line| line.trim().trim_end_matches(',').parse::<f64>().ok())
    .map(|at| ((at - start) * rate).round().max(0.0) as u64)
    .collect();
    Ok(Probe {
        video: format!("{codec} {width}x{height} {pix_fmt} {rate_text}"),
        size: (width, height),
        pix_fmt,
        rate,
        rate_text,
        frames: number(&video["nb_frames"]).unwrap_or_else(|| (seconds * rate).floor() as u64),
        video_bit_rate: number(&video["bit_rate"]).or_else(|| number(&read_json["format"]["bit_rate"])),
        sound: streams.iter().find(|s| s["codec_type"] == "audio").map(|audio| Sound {
            bit_rate: number(&audio["bit_rate"]),
            sample_rate: number(&audio["sample_rate"]),
            channels: number(&audio["channels"]),
        }),
        keys,
    })
}

/// A number ffprobe wrote, as a string or a number.
fn number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
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

/// A file a node reads that is no node's, as its key (`<node>.<field>`) and sha256 (`inputs`: the
/// files project.yaml names, as taken in; one gone and never taken in reads as none).
fn input_file(inputs: &BTreeMap<String, folder::Input>, id: &str, field: &str, path: &str) -> Result<(String, String), Error> {
    let sha256 = inputs.get(path).map(|input| input.asset.sha256.clone()).unwrap_or_default();
    Ok((format!("{id}.{field}"), sha256))
}

/// A shot's own files: its references', its plate's, its first and last frames.
fn shot_inputs(input_file: &impl Fn(&str, &str, &str) -> Result<(String, String), Error>, shot: &Shot) -> Result<Vec<(String, String)>, Error> {
    let mut found = vec![];
    for entry in &shot.references {
        let name = entry.get("name").and_then(Value::as_str).unwrap_or_default();
        for field in FILE_FIELDS {
            if let Some(path) = entry.get(field).and_then(Value::as_str) {
                found.push(input_file(&shot.name, &format!("{name}.{field}"), path)?);
            }
        }
    }
    if let Some(path) = shot.plate.as_ref().and_then(|plate| plate.get("video")).and_then(Value::as_str) {
        found.push(input_file(&shot.name, "plate", path)?);
    }
    for (field, path) in [("first_frame", &shot.first_frame), ("last_frame", &shot.last_frame)] {
        if let Some(path) = path {
            found.push(input_file(&shot.name, field, path)?);
        }
    }
    Ok(found)
}

/// A node's current file (path and digest) and its versions' files.
type Versions = (Option<(String, String)>, Vec<String>);

/// A node's current file and its versions' files (`Node::file`, `Node::versions`): the version
/// project.lock approves; else the file attached in project.yaml (its asset; none when it is gone and
/// was never taken in), or that picture's clean when the node `cleans`.
fn current(
    inputs: &BTreeMap<String, folder::Input>,
    lock: &Lock,
    id: &str,
    attached: &Option<String>,
    cleans: bool,
) -> Result<Versions, Error> {
    let node = lock.node(id);
    let versions: Vec<String> =
        (node.into_iter().flat_map(|node| &node.versions)).filter_map(|made| Some(made.primary()?.path.clone())).collect();
    let approved = node.and_then(|node| node.approved.as_ref()).and_then(|path| lock.made(id, path)?.primary());
    if let Some(approved) = approved {
        return Ok((Some((approved.path.clone(), approved.sha256.clone())), versions));
    }
    let Some(file) = attached else { return Ok((None, versions)) };
    let Some(input) = inputs.get(file) else { return Ok((None, versions)) };
    let asset = &input.asset;
    let clean = (node.into_iter().flat_map(|node| &node.versions))
        .find(|made| cleans && made.clean_of.as_deref() == Some(asset.sha256.as_str()));
    let clean = clean.and_then(Made::primary).unwrap_or(asset);
    Ok((Some((clean.path.clone(), clean.sha256.clone())), versions))
}

/// Every file project.yaml names: attached pictures and videos, voices, shots' references, plates
/// and frames.
fn named(project: &Project) -> Vec<&str> {
    let mut files: Vec<&str> = vec![];
    for entity in &project.entities {
        files.extend(entity.image.iter().chain(&entity.voice).map(String::as_str));
        files.extend(entity.setups.iter().filter_map(|setup| setup.image.as_deref()));
    }
    files.extend(project.world_states.iter().filter_map(|state| state.image.as_deref()));
    for shot in &project.shots {
        let own = [&shot.video, &shot.upscale.video, &shot.first_frame, &shot.last_frame];
        files.extend(own.into_iter().filter_map(Option::as_deref));
        let plate = shot.plate.as_ref().and_then(|plate| plate.get("video"));
        let references = shot.references.iter().flat_map(|entry| FILE_FIELDS.iter().filter_map(|field| entry.get(*field)));
        files.extend(plate.into_iter().chain(references).filter_map(Value::as_str));
    }
    files
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
