//! The `video` project type: entities (characters, props and environments; an environment's camera
//! setups are nodes of their own), world states that shots write and read, shots of any length, an
//! upscale per shot, and the edit, cut here with ffmpeg. Each node renders through its
//! render block: a function, cozy run's model choices and the function's options.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::{Path, PathBuf},
};

use protocol::error::{Error, Kind};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    folder::{self, Asset, Call, Folder, Lock, Made, Shape, Upstream},
    graph::{Edge, Graph, Node, mentions, name_key},
    host::{Accepts, Renderer},
};

/// The name `cozy project render all` means: every node, brought up to date.
pub const ALL: &str = "all";

/// Each node kind's function unless `defaults:` or the node's `render:` names another.
const RENDERERS: [(&str, &str); 6] = [
    ("entity", "qwen-image-2/generate"),
    ("setup", "qwen-image-2/generate"),
    ("world_state", "qwen-image-2/generate"),
    ("shot", "minimax-h3/generate"),
    ("upscale", "minimax-h3/upscale"),
    // Characters' and props' pictures its renderer can't draw transparent, before any use: the subject
    // on white, so a shot renderer never paints their backgrounds into the set.
    ("clean", "birefnet/remove_background"),
];

/// A new project's project.yaml: a short film that renders as it stands. Cozy never writes it again:
/// each node's versions, and which is approved, are in project.lock.
pub fn skeleton() -> String {
    r#"# A short film, ready to render: cozy project render all --on <machine>
# Change anything. More kinds and options (props, world states, references, plates, LoRAs, upscales,
# the edit's frames) are in the README.
version: 1
style: High-quality Unreal Engine graphics in a bright, hand-painted, cel-shaded anime style.
aspect_ratio: "16:9"
fps: 24

entities:
  - name: kyogre
    kind: character
    description: Kyogre, the colossal blue whale-like legendary Pokémon with huge fins, no legs, and glowing red lines.

  - name: groudon
    kind: character
    description: Groudon, the colossal red legendary Pokémon with grey stone plates and huge claws.

  - name: coast
    kind: environment
    description: A volcanic coast under a storm sky, with a dark sea on the left, lava cliffs on the right, and a small white crater city far away at the center.
    setups:
      - name: sea
        camera: The camera stands at the water's edge and looks left, out to sea, with the crater city small on the far right.
      - name: cliffs
        camera: The camera stands at the foot of the cliffs and looks right, up at them, with the crater city small on the far left.
      - name: wide
        camera: The camera stands high and far back, with the sea on the left, the cliffs on the right, and the crater city small at the center.

shots:
  - name: awaken
    setup: coast.sea
    duration_s: 6
    summary: |
      <kyogre> bursts out of the stormy sea of <coast>.
    soundscape: Howling wind, crashing waves, and rolling thunder.
    beats:
      - at: 0
        description: |
          A wide shot from the water's edge looks out over the stormy sea of <coast>, with lightning flickering behind the clouds and the crater city small on the far right. The water bulges at the center of the frame, and <kyogre> bursts out of it, facing right, with sheets of white water pouring off its fins and the red lines along its sides starting to glow. The camera tilts up at slow speed to follow its head as heavy rain starts to fall.

  - name: erupt
    setup: coast.cliffs
    duration_s: 6
    summary: |
      <groudon> claws its way out of the lava cliffs of <coast> and roars.
    soundscape: Cracking rock, hissing lava, heavy footfalls, and a deep roar.
    beats:
      - at: 0
        description: |
          A wide shot from the rocks looks up at the lava cliffs of <coast>, with a stream of lava crossing near the bottom of the frame and rain turning to steam above it. The rock face cracks at the center of the frame, and <groudon> claws its way out, lava sliding off its stone plates in glowing strings. It plants one foot, the ground jumps, and it rears up and roars toward the left. The camera pushes in at slow speed and shakes with each step.

  - name: clash
    setup: coast.wide
    duration_s: 38
    summary: |
      <kyogre> and <groudon> fight on <coast> in one continuous shot with no cuts, with storm clouds and heavy rain on the left and hard sunlight on the right.
    soundscape: Crashing surf, roaring fire, bursts of steam, and deep impacts.
    beats:
      - at: 0
        description: |
          A wide shot shows <kyogre> in the surf on the left, facing right, and <groudon> on the rocks on the right, facing left. The camera moves slowly closer throughout. <kyogre> surges forward, pushing a wave ahead of it, and fires a thick column of water across the frame. It bursts into a cloud of white steam against the stone plates of <groudon>, which slides back one step, its claws scraping sparks from the rock.
      - at: 14
        description: |
          <groudon> leans out of the steam and breathes a wide torrent of orange fire across the surf, and the water boils under it. <kyogre> dives beneath the flames, and a long wake races toward the rocks.
      - at: 28
        description: |
          <kyogre> erupts from the water at the shoreline, and the two crash together chest to chest in the breaking waves. A wall of water and steam explodes outward and rushes toward the camera, which shakes as spray hits the lens.
"#
    .into()
}

#[derive(Deserialize)]
struct Project {
    /// The project format; a file with none reads as 1.
    version: Option<u64>,
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
    #[serde(default, deserialize_with = "or_default")]
    entities: Vec<Entity>,
    #[serde(default, deserialize_with = "or_default")]
    references: Vec<Reference>,
    #[serde(default, deserialize_with = "or_default")]
    world_states: Vec<WorldState>,
    #[serde(default, deserialize_with = "or_default")]
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

/// A file that is no entity, which each shot mentioning its `name` is given: one `video` (a motion
/// guide), `audio`, or `image` (a `prop` to its renderer). Any other field is its function's
/// (`use_audio`, `description`, `follow`).
#[derive(Deserialize)]
struct Reference {
    name: String,
    video: Option<String>,
    audio: Option<String>,
    image: Option<String>,
    #[serde(flatten)]
    fields: Map<String, Value>,
}

impl Reference {
    /// The files it names, by field; `load` refuses any but one.
    fn files(&self) -> Vec<(&'static str, &str)> {
        let fields = [("video", &self.video), ("audio", &self.audio), ("image", &self.image)];
        fields.into_iter().filter_map(|(field, path)| Some((field, path.as_deref()?))).collect()
    }

    /// Its file, by field.
    fn file(&self) -> (&'static str, &str) {
        self.files()[0]
    }
}

/// A camera setup on its environment; its versions are setup plates.
#[derive(Deserialize)]
struct Setup {
    name: String,
    /// Where the camera is, in screen terms: what is near or far, left or right.
    #[serde(default)]
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
    /// The scene, the same in every segment: opaque here but for its `<mentions>`.
    #[serde(default, deserialize_with = "or_default")]
    summary: String,
    /// Its moments, on its own timeline. A cut is a new shot.
    #[serde(default, deserialize_with = "or_default")]
    beats: Vec<Beat>,
    /// Its sound bed (else the project's), and its music.
    soundscape: Option<String>,
    music: Option<String>,
    duration_s: f64,
    /// World states its frame at `at` seconds (default: its end) becomes the next revision of.
    #[serde(default)]
    writes: Vec<Write>,
    /// The video it edits: `video`, and `use_audio` to keep its sound.
    plate: Option<Map<String, Value>>,
    first_frame: Option<String>,
    last_frame: Option<String>,
    video: Option<String>,
    render: Option<Render>,
    #[serde(default)]
    upscale: Upscale,
}

#[derive(Clone, Deserialize)]
struct Beat {
    /// Seconds from the shot's start.
    #[serde(default)]
    at: f64,
    #[serde(default, deserialize_with = "or_default")]
    description: String,
}

#[derive(Clone, Deserialize)]
struct Write {
    state: String,
    at: Option<f64>,
}

impl Shot {
    /// Everything it says, where its `<mentions>` are.
    fn texts(&self) -> Vec<String> {
        std::iter::once(self.summary.clone()).chain(self.beats.iter().map(|beat| beat.description.clone())).collect()
    }

    /// Its beats inside its length, as a record has them.
    fn beat_records(&self) -> Vec<Value> {
        self.beats_to(self.duration_s).iter().map(|beat| json!({"at": beat.at, "description": beat.description})).collect()
    }

    /// Its beats before `end`, in order.
    fn beats_to(&self, end: f64) -> Vec<Beat> {
        let mut beats: Vec<Beat> = self.beats.iter().filter(|beat| beat.at < end).cloned().collect();
        beats.sort_by(|a, b| a.at.total_cmp(&b.at));
        beats
    }

    /// Its text up to `end` as one text, for a function with no adapter: the summary, then each beat's description in order.
    fn flat(&self, end: f64) -> String {
        let told: Vec<String> = self.beats_to(end).iter().map(|beat| beat.description.trim().to_owned()).collect();
        [self.summary.trim().to_owned(), told.join("\n")].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n\n")
    }

    /// The state each of its writes is of, and where, inside its length.
    fn written(&self) -> impl Iterator<Item = (&str, f64)> {
        (self.writes.iter()).map(|write| (write.state.as_str(), write.at.unwrap_or(self.duration_s).min(self.duration_s).max(0.0)))
    }
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
    Reference(usize),
    Setup(usize, usize),
    State(usize),
    Shot(usize),
    Upscale(usize),
}

/// A piece of the edit: a shot's node, its file (here, its digest, its asset's path in the project,
/// `assets/<sha256>.<ext>`, and its shape as project.lock records it), and its frames from `start`
/// to `end` (exclusive) at the project's rate.
pub struct Piece {
    pub node: String,
    pub path: PathBuf,
    pub digest: String,
    pub asset: String,
    pub shape: Option<Shape>,
    pub start: u64,
    pub end: u64,
}

impl Piece {
    /// Its frames at `fps` within its take, `end` at most the take's last; None before its take is
    /// probed.
    fn frames(&self, fps: f64) -> Option<(u64, u64)> {
        let frames = available(&self.shape?, fps);
        Some((self.start.min(frames), self.end.min(frames)))
    }
}

/// Whether a take of `shape` is retimed to `fps`: it runs at another rate.
fn retimed(shape: &Shape, fps: f64) -> bool {
    (shape.fps - fps).abs() > 0.01
}

/// How many frames a take of `shape` has at `fps`: its own, else as many as retiming makes.
fn available(shape: &Shape, fps: f64) -> u64 {
    match retimed(shape, fps) {
        false => shape.frames,
        true => (shape.frames as f64 * fps / shape.fps).floor() as u64,
    }
}

/// A cut: its length in frames, and the takes it retimes to the project's rate (`<node> (<fps> fps)`).
pub struct Cutted {
    pub frames: u64,
    pub retimed: Vec<String>,
}

/// Takes from `pieces` what starts past its take's end, a take being what its renderer made however
/// long it was asked for, and says each.
pub fn past_end(pieces: &mut Vec<Piece>, fps: f64) -> Vec<String> {
    let mut said = vec![];
    pieces.retain(|piece| {
        let past = piece.frames(fps).is_some_and(|(start, end)| start >= end);
        if past {
            said.push(format!("the edit cuts {} from frame {}, past its take's end: that cut is left out", piece.node, piece.start));
        }
        !past
    });
    said
}

/// Takes from `pieces` what an edit cannot read here, and says each: the rest still make the edit.
pub fn unreadable(pieces: &mut Vec<Piece>) -> Vec<String> {
    let mut said = vec![];
    pieces.retain(|piece| {
        let why = match (piece.path.is_file(), piece.shape.is_some()) {
            (false, _) => "is not on this computer: cozy project pull",
            (true, false) => "holds no video ffprobe reads",
            (true, true) => return true,
        };
        said.push(format!("{}: {} {why}: that cut is left out", piece.node, piece.path.display()));
        false
    });
    said
}

/// The cut `pieces` make at `fps`, as project.lock records their takes; None before each is probed.
pub fn measure(pieces: &[Piece], fps: f64) -> Option<Cutted> {
    let frames = pieces.iter().map(|piece| piece.frames(fps).map(|(start, end)| end - start)).sum::<Option<u64>>()?;
    let mut takes: Vec<String> = vec![];
    for piece in pieces {
        let Some(shape) = piece.shape.filter(|shape| retimed(shape, fps)) else { continue };
        let take = format!("{} ({} fps)", piece.node, shape.fps);
        if !takes.contains(&take) {
            takes.push(take);
        }
    }
    Some(Cutted { frames, retimed: takes })
}

pub struct Video {
    project: Project,
    /// Each file project.yaml names, by the path written there, as taken in.
    inputs: BTreeMap<String, Asset>,
    /// The files it names that are gone: what reads one cannot render.
    missing: BTreeSet<String>,
    items: Vec<Item>,
    pub graph: Graph,
    pub warnings: Vec<String>,
    /// The project's canvas.
    pub canvas: Canvas,
    /// Each node's current file's shape, when it is a video project.lock records one of.
    shapes: Vec<Option<Shape>>,
}

/// How far a take's aspect may be from the canvas's and still be it: models round their sizes to
/// multiples, which moves it this much at most (832x480 is 2.5% from 16:9).
const ROUNDED: f64 = 0.03;

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

    /// The edit's size: `resolution:`'s, else the first take's own (`first`) when its aspect is this one
    /// but for how models round their sizes and its sides are even (H3's 1344x768 at 16:9), so takes of
    /// that size join with no encode; else this aspect at that take's short edge.
    pub fn edit_size(&self, first: (u64, u64)) -> (u64, u64) {
        if let Some(short) = self.short {
            return self.size(short);
        }
        let rounded = (first.0 as f64 / first.1 as f64 / self.ratio - 1.0).abs() <= ROUNDED;
        match rounded && first.0 % 2 == 0 && first.1 % 2 == 0 {
            true => first,
            false => self.size(first.0.min(first.1)),
        }
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
    /// The flat request, for a function with no adapter. Each file in it as its digest; `files` say where each one is here.
    pub payload: Value,
    /// The node as the project says it, for its package's adapter (`adapter`, the function to run on it).
    pub node: Value,
    pub adapter: Option<String>,
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

/// A file a request may read: its path here and its digest. With `at`, that video's frame at `at`
/// seconds, which the walk takes out before it sends, and `field` is where its digest goes in the
/// request and the node.
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
        Self::read(folder, &folder.text()?)
    }

    /// The project `text` says. Only a file that reads as no project at all is refused: what one
    /// line gets wrong is said, and that line does the best it can (`spell`, `settle`).
    pub fn read(folder: &Folder, text: &str) -> Result<Self, Error> {
        let mut value = protocol::yaml::to_json(text)
            .map_err(|why| {
                invalid("project_yaml", format!("{}: {why}", folder::PROJECT))
                    .remedy("text that starts with `[` goes on the lines under `summary: |` or `description: |`")
            })?;
        let mut spelled = vec![];
        spell(&mut value, &mut spelled);
        let mut ignored: Vec<String> = vec![];
        let mut note = |path: serde_ignored::Path<'_>| ignored.push(path.to_string().replace("?.", ""));
        let read = serde_ignored::Deserializer::new(&value, &mut note);
        let mut project: Project = serde_path_to_error::deserialize(read).map_err(|e| {
            let at = e.path().to_string();
            invalid("project_yaml", format!("{}: {at}: {}", folder::PROJECT, e.into_inner()))
                .remedy("each entity has a name and a kind: see `cozy project --help`")
        })?;
        // A reference's kind follows from its file.
        ignored.extend(
            (project.references.iter().enumerate())
                .filter(|(_, reference)| reference.fields.contains_key("kind"))
                .map(|(n, _)| format!("references.{n}.kind")),
        );
        let mut warnings: Vec<String> = ignored
            .iter()
            .map(|path| format!("{}: cozy reads no field {path}; it is ignored", folder::PROJECT))
            .collect();
        if project.version.is_none() {
            warnings.push(format!("{} has no `version:`, so it reads as version 1: add `version: 1`", folder::PROJECT));
        }
        warnings.extend(spelled);
        settle(&mut project, &mut warnings);
        warnings.extend(
            (project.defaults.keys())
                .filter(|kind| !RENDERERS.iter().any(|(k, _)| k == kind))
                .map(|kind| format!("{}: defaults.{kind} is no node kind; it is ignored", folder::PROJECT)),
        );
        let mut lock = folder.lock()?;
        // Every file it names, linked as an asset on first use (and again when it changed), and
        // recorded in project.lock with the path written. One not at its path whose asset is here is
        // that asset; one gone keeps its record: what reads it cannot render until it is back.
        let (was, mut taken) = (folder.taken(), BTreeMap::new());
        let (mut inputs, mut missing) = (BTreeMap::new(), BTreeSet::new());
        for file in named(&project) {
            let (known, before) = (lock.inputs.get(file), was.get(file));
            match folder.take_in(file, known, before)? {
                Some((asset, now)) => {
                    // A link is the original: it changing changes the project. A clone or copy is taken in again.
                    let changed = known.is_some_and(|known| known.sha256 != asset.sha256);
                    if changed && before.is_some_and(|before| before.method.linked()) {
                        warnings.push(format!("reference changed: {file}; what reads it is out of date"));
                    }
                    let copied = now.as_ref().is_some_and(|now| now.method == folder::Method::Copy);
                    if copied && before.is_none_or(|before| before.method != folder::Method::Copy) {
                        warnings.push(format!("{file}: copied into {}/, as this computer allows it no link", folder::ASSETS));
                    }
                    inputs.insert(file.to_owned(), asset);
                    taken.extend(now.or_else(|| before.cloned()).map(|now| (file.to_owned(), now)));
                }
                None => {
                    let said = format!("reference missing: {file}: relink (put it back, fix its path in {}, or cozy project pull)", folder::PROJECT);
                    warnings.push(said);
                    missing.insert(file.to_owned());
                    inputs.extend(known.map(|known| (file.to_owned(), known.clone())));
                    taken.extend(before.map(|before| (file.to_owned(), before.clone())));
                }
            }
        }
        if inputs != lock.inputs {
            folder.update_lock(|lock| lock.inputs = inputs.clone())?;
            lock.inputs = inputs;
        }
        if taken != was {
            folder.keep_taken(&taken)?;
        }
        folder.shape_videos(&mut lock)?;
        let inputs = lock.inputs.clone();
        // A shot with no length: its video's when it has one attached, else one segment's, or past its last beat.
        for shot in project.shots.iter_mut().filter(|shot| shot.duration_s <= 0.0) {
            let attached = shot.video.as_deref().and_then(|file| inputs.get(file)).and_then(|asset| asset.video);
            let last = shot.beats.iter().map(|beat| beat.at).fold(0.0, f64::max);
            shot.duration_s = match attached {
                Some(shape) => shape.frames as f64 / shape.fps,
                None => (last + 5.0).max(15.0),
            };
            if attached.is_none() {
                warnings.push(format!("{} has no duration_s: it renders {} s", shot.name, shot.duration_s));
            }
        }
        let input_file = |id: &str, field: &str, path: &str| input_file(&inputs, id, field, path);
        // Its current file and its versions' files: the approved version, else the file attached by
        // hand (there, or the node is named with the fix), or that picture's clean.
        let versions = |id: &str, attached: &Option<String>, cleans: bool| current(&inputs, &lock, id, attached, cleans);
        let mut nodes = vec![];
        let mut items = vec![];
        for (a, entity) in project.entities.iter().enumerate() {
            let kind = entity.kind.name();
            let cleans = entity.kind != EntityKind::Environment;
            let (file, made) = versions(&entity.name, &entity.image, cleans)?;
            let mut reads = vec![];
            if let Some(voice) = &entity.voice {
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
                implied: vec![],
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
                    implied: vec![],
                    writes: vec![],
                    inputs: vec![],
                    file,
                    versions: made,
                });
                items.push(Item::Setup(a, s));
            }
        }
        for (r, reference) in project.references.iter().enumerate() {
            let (file, made) = versions(&reference.name, &Some(reference.file().1.to_owned()), false)?;
            nodes.push(Node {
                id: reference.name.clone(),
                kind: "reference",
                noun: "version",
                mentionable: true,
                state: false,
                order: None,
                texts: vec![],
                needs: vec![],
                implied: vec![],
                writes: vec![],
                inputs: vec![],
                file,
                versions: made,
            });
            items.push(Item::Reference(r));
        }
        for entity in &project.entities {
            for (setup, frame) in entity.setups.iter().filter_map(|s| s.frame.as_ref().map(|f| (s, f))) {
                let id = format!("{}.{}", entity.name, setup.name);
                let shot = project.shots.iter().find(|shot| name_key(&shot.name) == name_key(&frame.shot));
                let Some(shot) = shot else { continue };
                if !(0.0..shot.duration_s).contains(&frame.at) {
                    warnings.push(format!(
                        "{id}'s frame is at {} s, outside {}'s 0–{} s: the nearest frame stands in",
                        frame.at, shot.name, shot.duration_s
                    ));
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
                implied: vec![],
                writes: vec![],
                inputs: vec![],
                file,
                versions: made,
            });
            items.push(Item::State(w));
        }
        for (k, shot) in project.shots.iter().enumerate() {
            let texts = shot.texts();
            let (mut needs, mut implied) = (vec![], vec![]);
            needs.extend(shot.setup.clone());
            // An environment named with no setup of it is shot from its first setup.
            let framed = shot.setup.as_deref().and_then(|s| s.rsplit_once('.')).map(|(env, _)| name_key(env));
            for mention in texts.iter().flat_map(|text| mentions(text)) {
                let env = (project.entities.iter())
                    .find(|e| e.kind == EntityKind::Environment && name_key(&e.name) == name_key(mention));
                if let Some(env) = env.filter(|env| framed.as_deref() != Some(&name_key(&env.name)))
                    && let Some(first) = env.setups.first()
                {
                    implied.push(format!("{}.{}", env.name, first.name));
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
                implied,
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
                implied: vec![],
                writes: vec![],
                inputs: vec![],
                file,
                versions: made,
            });
            items.push(Item::Upscale(k));
        }
        if let Some(all) = nodes.iter().find(|node| name_key(&node.id) == name_key(ALL)) {
            warnings.push(format!("{} is named `{ALL}`: `cozy project render {ALL}` means everything, and makes it with the rest", all.id));
        }
        let mut graph = Graph::new(nodes)?;
        warnings.extend(graph.unknown.iter().map(|(i, mention)| {
            format!(
                "{} mentions {mention}, which is nothing in the project: it reaches the renderer as written",
                graph.nodes[*i].id
            )
        }));
        warnings.extend(graph.closing.iter().map(|&(reader, read)| {
            let (reader, read) = (&graph.nodes[reader].id, &graph.nodes[read].id);
            format!("{reader} mentions {read}, which is made from it: that mention stays plain text")
        }));
        for shot in &project.shots {
            if shot.video.is_none() && shot.texts().iter().all(|text| text.trim().is_empty()) {
                warnings.push(format!("{} has no summary and no beats: its renderer is given no text", shot.name));
            }
            for write in &shot.writes {
                let at = write.at.unwrap_or(shot.duration_s);
                if !(at > 0.0 && at <= shot.duration_s) {
                    warnings.push(format!(
                        "{} writes {} at {at} s, outside its 0–{} s: the nearest frame stands in",
                        shot.name, write.state, shot.duration_s
                    ));
                }
            }
        }
        // A still is made from pictures: a clip or a sound mentioned there stays plain text.
        for (node, reads) in graph.nodes.iter().zip(&mut graph.upstream).filter(|(node, _)| node.kind != "shot") {
            reads.retain(|edge| {
                let Item::Reference(r) = items[edge.node] else { return true };
                let (field, _) = project.references[r].file();
                if field != "image" {
                    let name = &graph.nodes[edge.node].id;
                    warnings.push(format!("{} mentions {name}, a {field} reference, which only a shot reads: it stays plain text", node.id));
                }
                field == "image"
            });
        }
        let canvas = Canvas::read(project.resolution.as_deref(), project.aspect_ratio.as_deref(), project.fps)?;
        // An `edit:` entry that cuts nothing is left out: every other piece still makes the edit.
        if let Some(edit) = &mut project.edit {
            let shots = &project.shots;
            edit.retain(|cut| {
                let shot = shots.iter().find(|shot| name_key(&shot.name) == name_key(&cut.shot));
                let why = match shot {
                    None => Some("which is no shot".to_owned()),
                    Some(shot) => {
                        let (start, end) = (cut.start.unwrap_or(0), cut.end.unwrap_or((shot.duration_s * canvas.fps).round() as u64));
                        (start >= end).then(|| format!("from frame {start} to {end}: nothing"))
                    }
                };
                if let Some(why) = &why {
                    warnings.push(format!("the edit cuts {}, {why}: that cut is left out", cut.shot));
                }
                why.is_none()
            });
        }
        let shapes = (graph.nodes.iter())
            .map(|node| {
                let (path, _) = node.file.as_ref()?;
                let made = lock.made(&node.id, path).and_then(Made::primary);
                let attached = || inputs.values().find(|asset| asset.path == *path);
                made.or_else(attached)?.video
            })
            .collect();
        Ok(Self { project, inputs, missing, items, graph, warnings, canvas, shapes })
    }

    /// Node `i`'s render block (`resolve`).
    pub fn render(&self, i: usize) -> Render {
        let (kind, own) = match self.items[i] {
            Item::Entity(a) => ("entity", self.project.entities[a].render.as_ref()),
            Item::Reference(_) => ("reference", None),
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

    /// The project format it is written in.
    pub fn version(&self) -> u64 {
        self.project.version.unwrap_or(1)
    }

    /// The field that holds the node's file.
    pub fn field(&self, i: usize) -> &'static str {
        match self.items[i] {
            Item::Shot(_) | Item::Upscale(_) => "video",
            Item::Reference(r) => self.project.references[r].file().0,
            _ => "image",
        }
    }

    /// Whether node `i` is a person's file (`references:`), of which cozy makes nothing.
    pub fn is_reference(&self, i: usize) -> bool {
        matches!(self.items[i], Item::Reference(_))
    }

    /// Whether `render all` makes the node: every node but an upscale the edit does not need. A
    /// shot is upscaled when `resolution:` is set and its current version's short edge is below it
    /// (or not probed yet).
    pub fn wanted(&self, i: usize) -> bool {
        let Item::Upscale(k) = self.items[i] else { return true };
        let Some(short) = self.canvas.short else { return false };
        let shot = self.graph.find(&self.project.shots[k].name).expect("a shot is a node");
        self.shapes[shot].is_none_or(|shape| shape.width.min(shape.height) < short)
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
            Item::Reference(r) => files.push(self.project.references[r].file().1),
            Item::Setup(a, s) => files.extend(self.project.entities[a].setups[s].image.as_deref()),
            Item::State(w) => files.extend(self.project.world_states[w].image.as_deref()),
            Item::Shot(k) => {
                let shot = &self.project.shots[k];
                files.extend([&shot.video, &shot.first_frame, &shot.last_frame].into_iter().filter_map(Option::as_deref));
                files.extend(shot.plate.as_ref().and_then(|plate| plate.get("video")).and_then(Value::as_str));
            }
            Item::Upscale(k) => files.extend(self.project.shots[k].upscale.video.as_deref()),
        }
        files.into_iter().filter(|file| self.missing.contains(*file)).collect()
    }

    /// The request that makes a version of node `i`, from what it reads now, as its function
    /// (`renderer`) takes it; a shot's is its first call.
    pub fn request(&self, folder: &Folder, lock: &Lock, i: usize, renderer: &Renderer) -> Result<Request, Error> {
        let mut said = vec![];
        let call = match self.items[i] {
            Item::Shot(k) => Some(Call { ends: self.ends(k, &renderer.accepts, &mut said), ..Default::default() }),
            _ => None,
        };
        let mut request = self.compile(folder, lock, i, renderer, call, &BTreeMap::new())?;
        request.made.warnings.extend(said.iter().cloned());
        request.warnings.extend(said);
        Ok(request)
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
            style => format!("Style: {}.\n\n{text}", style.trim_end_matches('.')),
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
        let (version, function, style) = (self.version(), renderer.function.as_str(), self.project.style.trim());
        let kind = match self.items[i] {
            Item::Entity(_) => "entity",
            Item::Setup(..) => "setup",
            Item::State(_) => "world_state",
            Item::Shot(_) => "shot",
            Item::Upscale(_) => "upscale",
            Item::Reference(_) => "reference",
        };
        let (mut payload, record, call) = match self.items[i] {
            Item::Entity(a) => {
                let entity = &self.project.entities[a];
                let shown = pictures(&mut files);
                let node = json!({"version": version, "name": entity.name, "function": function, "kind": entity.kind.name(),
                    "description": entity.description, "style": style, "aspect_ratio": self.canvas.aspect, "pictures": shown});
                (json!({"prompt": styled(entity.description.trim().to_owned()), "pictures": shown, "aspect_ratio": self.canvas.aspect}), node, None)
            }
            Item::Reference(_) => return Err(reference_made(&node.id)),
            Item::Setup(a, s) => {
                // The environment's picture (or a shot's frame) first.
                let (env, setup) = (&self.project.entities[a], &self.project.entities[a].setups[s]);
                let camera = setup.camera.trim();
                let shown: Vec<Value> = (reads.iter().enumerate())
                    .map(|(n, read)| {
                        let up = &graph.nodes[read.edge.node];
                        let (name, at) = match n {
                            0 => (env.name.as_str(), setup.frame.as_ref().map(|frame| frame.at.max(0.0))),
                            _ => (up.id.as_str(), None),
                        };
                        json!({"name": name, "image": read.input(&mut files, format!("pictures.{n}.image"), at)})
                    })
                    .collect();
                let node = json!({"version": version, "name": node.id, "function": function, "environment": env.name,
                    "description": env.description, "camera": camera, "from_frame": setup.frame.is_some(), "style": style,
                    "aspect_ratio": self.canvas.aspect, "pictures": shown});
                (json!({"prompt": styled(format!("<{}>. {camera}", env.name)), "pictures": shown, "aspect_ratio": self.canvas.aspect}), node, None)
            }
            Item::State(w) => {
                let state = &self.project.world_states[w];
                let shown = pictures(&mut files);
                let node = json!({"version": version, "name": state.name, "function": function, "init": state.init,
                    "blocking": state.blocking, "style": style, "aspect_ratio": self.canvas.aspect, "pictures": shown});
                let prompt = styled([state.init.trim(), state.blocking.trim()].join("\n\n").trim().to_owned());
                (json!({"prompt": prompt, "pictures": shown, "aspect_ratio": self.canvas.aspect}), node, None)
            }
            Item::Shot(k) => {
                let call = call.expect("a shot is made in calls");
                let (payload, node, call) = self.shot(folder, k, &reads, &mut files, call, own, renderer, &mut warnings)?;
                (payload, node, Some(call))
            }
            Item::Upscale(k) => {
                // The text the shot was made from, for an upscaler that re-renders it (Paul, 10-31: the
                // upscaler gets the details of the original): its description, the descriptions of
                // what it reads, and the style.
                let shot = &self.project.shots[k];
                let mut text = shot.flat(shot.duration_s);
                let mut read_by = vec![];
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
                        read_by.push(json!({"name": name, "kind": "environment", "description": description.trim()}));
                    }
                }
                let video = reads[0].input(&mut files, "video".into(), None);
                let mut payload = json!({"video": video, "prompt": styled(text.replace(['<', '>'], ""))});
                let mut node = json!({"version": version, "name": node.id, "function": function, "video": video, "summary": shot.summary,
                    "beats": shot.beat_records(), "style": style, "references": read_by});
                if let Some(resolution) = &self.project.resolution {
                    payload["resolution"] = Value::String(resolution.clone());
                    node["resolution"] = Value::String(resolution.clone());
                }
                // The shot's latents, for an upscaler of its package (one that reads its format).
                let made = lock.made(&shot.name, &reads[0].file);
                let same = made.and_then(|made| made.made_by.as_ref())
                    .is_some_and(|by| by.with.starts_with(&format!("{}/", renderer.package)));
                if let Some(latents) = made.and_then(|made| made.assets.get("latents")).filter(|_| same) {
                    let digest = latents.sha256.clone();
                    files.push(Input { field: String::new(), path: folder.path(&latents.path), digest: digest.clone(), at: None });
                    payload["latents"] = Value::String(digest.clone());
                    node["latents"] = Value::String(digest);
                }
                (payload, node, None)
            }
        };
        // What the function does not take is left out, and said.
        let accepts = &renderer.accepts;
        let fields = payload.as_object_mut().expect("a request is an object");
        let left: Vec<String> = fields.keys().filter(|key| !accepts.fields.contains(*key)).cloned().collect();
        for key in &left {
            fields.remove(key);
            warnings.push(format!("{} takes no {key}: {}'s is left out", self.render(i).target(), node.id));
        }
        let adapter = renderer.adapter(kind, version);
        if adapter.is_none() && call.as_ref().is_none_or(|call| call.index == 0) {
            let times = if matches!(self.items[i], Item::Shot(k) if self.project.shots[k].beats.iter().any(|beat| beat.at > 0.0)) { ", and its beat times are not honoured" } else { "" };
            warnings.push(format!("{} has no project adapter for `{kind}`: it gets one flat prompt{times}", self.render(i).target()));
        }
        let warnings = if adapter.is_some() { vec![] } else { warnings };
        let made = Made { warnings: warnings.clone(), ..made };
        Ok(Request { payload, node: record, adapter, made, field: self.field(i), call, files, render: self.render(i), warnings })
    }

    /// Where shot `k`'s consecutive calls end: at each time it writes a state its text mentions (a
    /// read of its own write, so the call after it reads the frame), and at its end. A renderer that cannot continue a video, or whose
    /// shortest piece the split would go under, renders on through: the state reads as it stood
    /// before, and it is said.
    fn ends(&self, k: usize, accepts: &Accepts, said: &mut Vec<String>) -> Vec<f64> {
        let shot = &self.project.shots[k];
        let texts = shot.texts();
        let read: Vec<&str> = texts.iter().flat_map(|text| mentions(text)).collect();
        let mut splits: Vec<(&str, f64)> = (shot.written())
            .filter(|&(state, at)| at > 0.0 && at < shot.duration_s && read.iter().any(|m| name_key(m) == name_key(state)))
            .collect();
        splits.sort_by(|a, b| a.1.total_cmp(&b.1));
        let (mut ends, mut from) = (vec![], 0.0);
        for (state, at) in splits {
            if ends.last() == Some(&at) {
                continue;
            }
            let why = match accepts.min_piece_s.filter(|min| at - from < *min) {
                _ if !accepts.fields.contains("continue_from") => Some("its renderer cannot continue a video (continue_from)".to_owned()),
                Some(min) => Some(format!("a call from {from} s to {at} s is under its renderer's {min} s")),
                None => None,
            };
            match why {
                Some(why) => said.push(format!("{} mentions {state} and writes it at {at} s, but {why}: it reads {state} as it stood before", shot.name)),
                None => {
                    ends.push(at);
                    from = at;
                }
            }
        }
        ends.push(shot.duration_s);
        ends
    }

    /// Shot `k`'s call `call`, on the shot's one timeline, both ways:
    /// - as the project says it, for its function's adapter: its summary and beats, its style, sound and music, each
    ///   reference it reads by name, kind, description and digest, its plate, its first frame (first call), its last
    ///   frame (last call) and the call before it. A world state's picture is the version current at the call's start:
    ///   the shot's own frame once it wrote it, `own`.
    /// - as one flat request for a function with no adapter: the summary and each beat's description as `prompt`, with
    ///   what its schema takes (`Accepts`). What it does not take is left out and said (`said`).
    #[allow(clippy::too_many_arguments)]
    fn shot(
        &self,
        folder: &Folder,
        k: usize,
        reads: &[Read],
        files: &mut Vec<Input>,
        mut call: Call,
        own: &BTreeMap<String, Asset>,
        renderer: &Renderer,
        said: &mut Vec<String>,
    ) -> Result<(Value, Value, Call), Error> {
        let accepts = &renderer.accepts;
        let shot = &self.project.shots[k];
        let (index, last) = (call.index, call.index + 1 == call.ends.len());
        let (start, end) = (if index == 0 { 0.0 } else { call.ends[index - 1] }, call.ends[index]);
        let takes = |field: &str| accepts.fields.contains(field);
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
        let keep = |path: PathBuf, digest: &str, files: &mut Vec<Input>| {
            files.push(Input { field: String::new(), path, digest: digest.to_owned(), at: None });
            Value::String(digest.to_owned())
        };
        let file = |path: &str| -> Result<(PathBuf, String), Error> {
            let input = self.inputs.get(path).ok_or_else(|| invalid("file_missing", format!("{}: {path} is not there", shot.name)))?;
            Ok((folder.path(&input.path), input.sha256.clone()))
        };
        // Each reference as the project knows it.
        let mut references: Vec<Value> = vec![];
        for read in reads {
            let picture = |files: &mut Vec<Input>| keep(read.path.clone(), &read.digest, files);
            match (self.items[read.edge.node], &read.edge.state) {
                (Item::Entity(a), _) if !framed.contains(&a) => {
                    let entity = &self.project.entities[a];
                    let mut entry = json!({"name": entity.name, "kind": entity.kind.name(), "description": entity.description, "image": picture(files)});
                    if let Some(voice) = &entity.voice {
                        let (path, digest) = file(voice)?;
                        entry["voice"] = keep(path, &digest, files);
                    }
                    references.push(entry);
                }
                (Item::Setup(a, _), _) => {
                    let env = &self.project.entities[a];
                    references.push(json!({"name": env.name, "kind": "environment", "description": env.description, "image": picture(files)}));
                }
                (Item::State(_), _) | (Item::Shot(_), Some(_)) => {
                    let name = read.edge.state.clone().unwrap_or_else(|| self.graph.nodes[read.edge.node].id.clone());
                    // Once this shot wrote it, its own frame.
                    let image = match own.iter().find(|(state, _)| name_key(state) == name_key(&name)) {
                        Some((_, frame)) => keep(folder.path(&frame.path), &frame.sha256, files),
                        None => picture(files),
                    };
                    references.push(json!({"name": name, "kind": "world_state", "description": blocking(&name), "image": image}));
                }
                _ => {}
            }
        }
        for read in reads {
            let Item::Reference(r) = self.items[read.edge.node] else { continue };
            let reference = &self.project.references[r];
            let field = reference.file().0;
            let mut fields = reference.fields.clone();
            let mut entry = json!({"name": reference.name, "kind": if field == "image" { "prop" } else { field }});
            entry[field] = keep(read.path.clone(), &read.digest, files);
            if let Some(description) = fields.remove("description") {
                entry["description"] = description;
            }
            entry["fields"] = Value::Object(fields);
            references.push(entry);
        }
        let sound = shot.soundscape.as_deref().unwrap_or(&self.project.soundscape).trim();
        let music = shot.music.as_deref().unwrap_or_default().trim();
        let style = self.project.style.trim();
        let mut node = json!({"version": self.version(), "name": shot.name, "function": renderer.function, "summary": shot.summary,
            "beats": shot.beat_records(), "soundscape": sound, "music": music, "style": style, "aspect_ratio": self.canvas.aspect,
            "duration_s": shot.duration_s, "start_s": start, "end_s": end, "references": references});
        if let Some(plate) = &shot.plate {
            let mut plate = plate.clone();
            if let Some(path) = plate.get("video").and_then(Value::as_str) {
                let (path, digest) = file(path)?;
                plate.insert("video".into(), keep(path, &digest, files));
            }
            node["plate"] = Value::Object(plate);
        }
        for (field, path, here) in [("first_frame", &shot.first_frame, index == 0), ("last_frame", &shot.last_frame, last)] {
            let Some(path) = path.as_ref().filter(|_| here) else { continue };
            let (path, digest) = file(path)?;
            node[field] = keep(path, &digest, files);
        }
        if let Some((video, latents)) = call.from.as_ref().filter(|_| index > 0) {
            let mut from = Map::new();
            for (field, file) in std::iter::once(("video", Some(video))).chain([("latents", latents.as_ref())]) {
                let Some(file) = file else { continue };
                from.insert(field.into(), keep(folder.path(&file.path), &file.sha256, files));
            }
            node["continues"] = Value::Object(from);
        }
        // The flat request, with what its schema takes.
        let mut payload = Map::new();
        let mut text = shot.flat(end);
        match (takes("style"), style.is_empty()) {
            (true, false) => drop(payload.insert("style".into(), json!(style))),
            (false, false) => text = format!("{text}\n\nStyle: {style}."),
            _ => {}
        }
        for (field, value) in [("soundscape", sound), ("music", music)] {
            match (takes(field), value.is_empty()) {
                (true, false) => drop(payload.insert(field.into(), json!(value))),
                (false, false) => said.push(format!("{}'s renderer takes no {field}: it is left out", shot.name)),
                _ => {}
            }
        }
        if takes("references") {
            let mut flat = vec![];
            for record in node["references"].as_array().expect("a list") {
                let mut entry = record.as_object().expect("an object").clone();
                if let Some(Value::Object(fields)) = entry.remove("fields") {
                    entry.extend(fields);
                }
                if entry["kind"] == "world_state" {
                    entry.insert("kind".into(), json!("environment"));
                    entry.insert("follow".into(), json!("loose"));
                }
                let kind = entry["kind"].as_str().unwrap_or_default().to_owned();
                let name = entry["name"].as_str().unwrap_or_default().to_owned();
                let Some(fields) = accepts.kinds.get(&kind) else {
                    said.push(format!("{}'s renderer takes no {kind} reference: {name} is left out", shot.name));
                    continue;
                };
                let dropped: Vec<String> = entry.keys().filter(|key| *key != "kind" && !fields.contains(*key)).cloned().collect();
                for key in dropped {
                    entry.remove(&key);
                    said.push(format!("{}'s renderer takes no {key} on a {kind} reference: {name}'s is left out", shot.name));
                }
                flat.push(Value::Object(entry));
            }
            payload.insert("references".into(), Value::Array(flat));
        } else {
            // With no references, a mention is the name itself.
            text = text.replace(['<', '>'], "");
        }
        payload.insert("prompt".into(), json!(text));
        payload.insert("duration_s".into(), json!(end));
        if takes("aspect_ratio") {
            payload.insert("aspect_ratio".into(), json!(self.canvas.aspect));
        }
        if let Some(plate) = node.get("plate") {
            match takes("plate") {
                true => drop(payload.insert("plate".into(), plate.clone())),
                false => said.push(format!("{}'s renderer takes no plate: it renders without it", shot.name)),
            }
        }
        for field in ["first_frame", "last_frame"] {
            let Some(value) = node.get(field) else { continue };
            match takes(field) {
                true => drop(payload.insert(field.into(), value.clone())),
                false => said.push(format!("{}'s renderer takes no {field}: it is left out", shot.name)),
            }
        }
        if let Some(from) = node.get("continues") {
            payload.insert("continue_from".into(), from.clone());
        }
        call.writes = (shot.written())
            .filter(|(_, at)| *at >= start && (*at < end || last))
            .map(|(state, at)| (state.to_owned(), at))
            .collect();
        Ok((Value::Object(payload), node, call))
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
            let Some(k) = (self.project.shots.iter()).position(|s| name_key(&s.name) == name_key(name)) else {
                continue;
            };
            let shot = &self.project.shots[k];
            let upscale = self.graph.find(&format!("{}.upscale", shot.name)).expect("a shot's upscale is a node");
            let i = match !preview && self.wanted(upscale) {
                true => upscale,
                false => self.graph.find(&shot.name).expect("a shot is a node"),
            };
            let node = &self.graph.nodes[i];
            let (start, end) = (start.unwrap_or(0), end.unwrap_or((shot.duration_s * self.canvas.fps).round() as u64));
            match &node.file {
                Some((file, digest)) => {
                    let (node, path, asset, shape) = (node.id.clone(), folder.path(file), file.clone(), self.shapes[i]);
                    pieces.push(Piece { node, path, digest: digest.clone(), asset, shape, start, end })
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
    let probes = pieces.iter().map(|piece| probe(ffprobe, &piece.path, true)).collect::<Result<Vec<_>, _>>()?;
    let (fps, rate) = (canvas.fps, canvas.fps.to_string());
    let (width, height) = canvas.edit_size(probes[0].size);
    // Each piece's frames at the project's rate, within its take, as project.lock records it.
    let Some(made) = measure(pieces, fps) else {
        let unprobed: Vec<&str> = pieces.iter().filter(|piece| piece.shape.is_none()).map(|piece| piece.node.as_str()).collect();
        return Err(failed(format!("no frame rate is recorded for {}'s file", unprobed.join(", "))));
    };
    let ranges: Vec<(u64, u64)> = pieces.iter().filter_map(|piece| piece.frames(fps)).collect();
    let native = |piece: &Piece| piece.shape.is_some_and(|shape| !retimed(&shape, fps));
    let io = |e: std::io::Error| failed(format!("{}: {e}", cache.display()));
    std::fs::create_dir_all(cache).map_err(io)?;
    let work = tempfile::tempdir_in(cache).map_err(io)?;
    let video = work.path().join("video.mp4");
    let highest = probes.iter().filter_map(|probe| probe.video_bit_rate).max();
    let os = |args: &[&str]| args.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
    let fast = (pieces.iter().zip(&probes))
        .all(|(piece, p)| p.video == probes[0].video && p.video.starts_with("h264 ") && p.size == (width, height) && native(piece));
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
        for (k, (piece, (start, end))) in pieces.iter().zip(&ranges).enumerate() {
            inputs.extend(["-i".into(), piece.path.clone().into_os_string()]);
            // A take at another rate is retimed first (each frame its source's nearest in time), then
            // cut by frame like any other.
            let retime = if native(piece) { String::new() } else { format!("fps={rate},") };
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
    Ok(made)
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
            let shape = piece.shape.expect("a piece cut is probed");
            let metadata = match retimed(&shape, fps) {
                true => json!({"cozy": {"retimed_from_fps": shape.fps}}),
                false => json!({}),
            };
            let mut clip = item("Clip.2", &piece.node, metadata, range(start, end - start));
            clip["media_references"] = json!({"DEFAULT_MEDIA": {
                "OTIO_SCHEMA": "ExternalReference.1",
                "metadata": {},
                "name": piece.node,
                "available_range": range(0, available(&shape, fps)),
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

/// A video's shape, when ffprobe reads it.
pub fn shape(ffprobe: &Path, path: &Path) -> Option<Shape> {
    let probe = probe(ffprobe, path, false).ok()?;
    Some(Shape { width: probe.size.0, height: probe.size.1, fps: probe.rate, frames: probe.frames })
}

/// What the edit needs of the video at `path`; its keyframes only with `keys`.
fn probe(ffprobe: &Path, path: &Path, keys: bool) -> Result<Probe, Error> {
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
        "-count_packets",
        "-show_entries",
        "stream=codec_type,codec_name,width,height,pix_fmt,r_frame_rate,nb_frames,nb_read_packets,bit_rate,sample_rate,channels:format=duration,bit_rate,start_time",
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
    let keys = match keys {
        true => read(&["-select_streams", "v:0", "-skip_frame", "nokey", "-show_entries", "frame=pts_time", "-of", "csv=p=0"])?,
        false => vec![],
    };
    let keys = (String::from_utf8_lossy(&keys).lines())
        .filter_map(|line| line.trim().trim_end_matches(',').parse::<f64>().ok())
        .map(|at| ((at - start) * rate).round().max(0.0) as u64)
        .collect();
    Ok(Probe {
        video: format!("{codec} {width}x{height} {pix_fmt} {rate_text}"),
        size: (width, height),
        pix_fmt,
        rate,
        rate_text,
        frames: (number(&video["nb_frames"]).or_else(|| number(&video["nb_read_packets"])))
            .unwrap_or_else(|| (seconds * rate).floor() as u64),
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
fn input_file(inputs: &BTreeMap<String, Asset>, id: &str, field: &str, path: &str) -> Result<(String, String), Error> {
    let sha256 = inputs.get(path).map(|input| input.sha256.clone()).unwrap_or_default();
    Ok((format!("{id}.{field}"), sha256))
}

/// A shot's own files: its plate's, its first and last frames.
fn shot_inputs(input_file: &impl Fn(&str, &str, &str) -> Result<(String, String), Error>, shot: &Shot) -> Result<Vec<(String, String)>, Error> {
    let mut found = vec![];
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
    inputs: &BTreeMap<String, Asset>,
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
    let Some(asset) = inputs.get(file) else { return Ok((None, versions)) };
    let clean = (node.into_iter().flat_map(|node| &node.versions))
        .find(|made| cleans && made.clean_of.as_deref() == Some(asset.sha256.as_str()));
    let clean = clean.and_then(Made::primary).unwrap_or(asset);
    Ok((Some((clean.path.clone(), clean.sha256.clone())), versions))
}

/// Every file project.yaml names: attached pictures and videos, voices, references, plates and
/// frames.
fn named(project: &Project) -> Vec<&str> {
    let mut files: Vec<&str> = vec![];
    for entity in &project.entities {
        files.extend(entity.image.iter().chain(&entity.voice).map(String::as_str));
        files.extend(entity.setups.iter().filter_map(|setup| setup.image.as_deref()));
    }
    files.extend(project.references.iter().map(|reference| reference.file().1));
    files.extend(project.world_states.iter().filter_map(|state| state.image.as_deref()));
    for shot in &project.shots {
        let own = [&shot.video, &shot.upscale.video, &shot.first_frame, &shot.last_frame];
        files.extend(own.into_iter().filter_map(Option::as_deref));
        files.extend(shot.plate.as_ref().and_then(|plate| plate.get("video")).and_then(Value::as_str));
    }
    files
}

/// What a person wrote, as the reader takes it: an empty `key:` is no key, a bare number is text where
/// text goes and a word a number where a number goes, a kind in any case, a canvas in its common
/// spellings. What can't be read is said and left out, so one line never refuses the project.
fn spell(value: &mut Value, said: &mut Vec<String>) {
    let Value::Object(top) = value else { return };
    tidy(top);
    canvas_spelled(top, said);
    for (list, name) in [("references", "name"), ("edit", "shot")] {
        items(top, list, name, said);
    }
    for entity in items(top, "entities", "name", said) {
        let setups = entity.contains_key("setups");
        let written = entity.get("kind").and_then(Value::as_str).map(|kind| kind.trim().to_ascii_lowercase());
        let kind = match written.as_deref() {
            Some("character" | "prop" | "environment") => written.clone().expect("matched"),
            _ => {
                let kind = if setups { "environment" } else if entity.contains_key("voice") { "character" } else { "prop" };
                let name = entity.get("name").and_then(Value::as_str).unwrap_or_default();
                said.push(format!("{name}'s kind is {}: read as {kind}", written.as_deref().unwrap_or("not given")));
                kind.to_owned()
            }
        };
        entity.insert("kind".into(), Value::String(kind));
        items(entity, "setups", "name", said);
    }
    items(top, "world_states", "name", said);
    for shot in items(top, "shots", "name", said) {
        as_number(shot, "duration_s");
        items(shot, "writes", "state", said);
        for beat in items(shot, "beats", "at", said) {
            as_number(beat, "at");
        }
    }
}

/// The objects in `map`'s list `key`, each tidied and `name` read as text; an item with no `name` is said and left out.
fn items<'a>(map: &'a mut Map<String, Value>, key: &str, name: &str, said: &mut Vec<String>) -> Vec<&'a mut Map<String, Value>> {
    if map.get(key).is_some_and(|value| !value.is_array()) {
        said.push(format!("{}: {key} is not a list: it is ignored", folder::PROJECT));
        map.remove(key);
    }
    let Some(Value::Array(list)) = map.get_mut(key) else {
        return vec![];
    };
    let mut at = 0;
    list.retain_mut(|item| {
        at += 1;
        let Value::Object(item) = item else {
            said.push(format!("{}: {key}[{}] is ignored: it names no {name}", folder::PROJECT, at - 1));
            return false;
        };
        tidy(item);
        if let Some(text) = item.get(name).filter(|value| name != "at" && (value.is_number() || value.is_boolean())).map(Value::to_string) {
            item.insert(name.into(), Value::String(text));
        }
        let named = item.contains_key(name);
        if !named {
            said.push(format!("{}: {key}[{}] is ignored: it names no {name}", folder::PROJECT, at - 1));
        }
        named
    });
    list.iter_mut().filter_map(Value::as_object_mut).collect()
}

/// `map` with no empty keys: `key:` alone is as if it were not written.
fn tidy(map: &mut Map<String, Value>) {
    map.retain(|_, value| !value.is_null());
}

/// `map`'s `key` as a number when it was written as text (`"6"`, `6s`); left out when it is neither.
fn as_number(map: &mut Map<String, Value>, key: &str) {
    let Some(Value::String(text)) = map.get(key) else { return };
    match text.trim().trim_end_matches('s').trim().parse::<f64>().ok().and_then(serde_json::Number::from_f64) {
        Some(read) => drop(map.insert(key.into(), Value::Number(read))),
        None => drop(map.remove(key)),
    }
}

/// `resolution`, `aspect_ratio` and `fps` as `Canvas::read` takes them, or left out with a word.
fn canvas_spelled(top: &mut Map<String, Value>, said: &mut Vec<String>) {
    let scalar = |value: &Value| value.as_str().map_or_else(|| value.to_string(), |text| text.trim().to_ascii_lowercase());
    if let Some(written) = top.get("resolution").map(scalar) {
        let short = match written.as_str() {
            "4k" => Some(2160),
            "2k" => Some(1440),
            "8k" => Some(4320),
            text => text.strip_suffix('p').unwrap_or(text).parse::<u64>().ok().filter(|short| *short >= 16),
        };
        match short {
            Some(short) => drop(top.insert("resolution".into(), Value::String(format!("{short}p")))),
            None => {
                said.push(format!("resolution {written} is not a short edge such as 1080p: the takes' own size stands"));
                top.remove("resolution");
            }
        }
    }
    if let Some(written) = top.get("aspect_ratio").map(scalar) {
        let ratio = written.replace(['/', 'x', '×'], ":");
        let ratio = if ratio.contains(':') { ratio } else { format!("{ratio}:1") };
        let side = |text: &str| text.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0);
        match ratio.split_once(':').and_then(|(w, h)| Some((side(w)?, side(h)?))) {
            Some(_) => drop(top.insert("aspect_ratio".into(), Value::String(ratio))),
            None => {
                said.push(format!("aspect_ratio {written} is not W:H: 16:9 stands"));
                top.remove("aspect_ratio");
            }
        }
    }
    let Some(written) = top.get("fps").cloned() else { return };
    as_number(top, "fps");
    match top.get("fps").and_then(Value::as_f64).filter(|fps| fps.is_finite() && *fps > 0.0) {
        Some(fps) if fps <= 240.0 => {}
        Some(_) => {
            said.push(format!("fps {written}: 240 stands"));
            top.insert("fps".into(), json!(240));
        }
        None => {
            said.push(format!("fps {written} is not a frame rate: 24 stands"));
            top.remove("fps");
        }
    }
}

/// What the project's lines say of each other, made to agree: what can't apply is said and left out.
fn settle(project: &mut Project, said: &mut Vec<String>) {
    project.references.retain_mut(|reference| {
        let files = reference.files().len();
        if files > 1 {
            said.push(format!("reference {} names {files} files: its first is read", reference.name));
            let first = reference.files()[0].0;
            for (field, file) in [("video", &mut reference.video), ("audio", &mut reference.audio), ("image", &mut reference.image)] {
                if field != first {
                    *file = None;
                }
            }
        }
        if files == 0 {
            said.push(format!("reference {} names no file (video:, audio: or image:): it is left out", reference.name));
        }
        files > 0
    });
    let shots: Vec<String> = project.shots.iter().map(|shot| name_key(&shot.name)).collect();
    for entity in &mut project.entities {
        let kind = entity.kind.name();
        if !entity.setups.is_empty() && entity.kind != EntityKind::Environment {
            said.push(format!("{} is a {kind}: only an environment has setups: they are ignored", entity.name));
            entity.setups.clear();
        }
        if entity.voice.is_some() && entity.kind != EntityKind::Character {
            said.push(format!("{} is a {kind}: only a character has a voice: it is ignored", entity.name));
            entity.voice = None;
        }
        for setup in &mut entity.setups {
            if setup.camera.trim().is_empty() {
                said.push(format!("{}.{} has no camera: it is drawn from the environment alone", entity.name, setup.name));
            }
            if setup.frame.as_ref().is_some_and(|frame| !shots.contains(&name_key(&frame.shot))) {
                said.push(format!("{}.{}'s frame names no shot: it is drawn from the environment", entity.name, setup.name));
                setup.frame = None;
            }
        }
    }
    let states: Vec<String> = project.world_states.iter().map(|state| name_key(&state.name)).collect();
    let setups: Vec<(String, String, String)> = (project.entities.iter())
        .flat_map(|entity| entity.setups.iter().map(|setup| (entity.name.clone(), setup.name.clone(), format!("{}.{}", entity.name, setup.name))))
        .collect();
    for shot in &mut project.shots {
        if let Some(written) = shot.setup.take() {
            let key = name_key(&written);
            // `coast.sea`; `coast`, its first setup, as a mention of it reads; `sea`, when one environment has it.
            let exact = setups.iter().find(|(.., id)| name_key(id) == key);
            let of_place = setups.iter().find(|(place, ..)| name_key(place) == key);
            let mut named = setups.iter().filter(|(_, setup, _)| name_key(setup) == key);
            let bare = named.next().filter(|_| named.next().is_none());
            match exact.or(of_place).or(bare) {
                Some((.., id)) => shot.setup = Some(id.clone()),
                None => said.push(format!("{}'s setup {written} is no setup (write Environment.Setup): it renders without one", shot.name)),
            }
        }
        let mut written = BTreeSet::new();
        let name = shot.name.clone();
        shot.writes.retain(|write| {
            let key = name_key(&write.state);
            if !states.contains(&key) {
                said.push(format!("{name} writes {}, which is no world state: it is ignored", write.state));
                return false;
            }
            if !written.insert(key) {
                said.push(format!("{name} writes {} twice: the first stands", write.state));
                return false;
            }
            true
        });
    }
}

/// A field a person may leave empty as `key:` alone (a null).
fn or_default<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(read: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(read)?.unwrap_or_default())
}

fn invalid(code: &str, message: String) -> Error {
    Error::new(Kind::Validation, code, message)
}

/// The refusal to render a reference.
pub fn reference_made(name: &str) -> Error {
    let said = format!("{name} is a file project.yaml names: cozy makes no versions of it");
    Error::new(Kind::Usage, "reference_file", said).remedy("render a shot that mentions it")
}

fn failed(message: String) -> Error {
    Error::new(Kind::Unavailable, "edit_failed", message)
}

/// Where each of `files` is in `value`, a request by its digests: its field, path and digest.
pub fn files_in(value: &Value, files: &[Input]) -> Vec<(String, PathBuf, String)> {
    fn walk(value: &Value, at: String, files: &[Input], found: &mut Vec<(String, PathBuf, String)>) {
        let under = |key: String| if at.is_empty() { key } else { format!("{at}.{key}") };
        match value {
            Value::Object(fields) => fields.iter().for_each(|(key, value)| walk(value, under(key.clone()), files, found)),
            Value::Array(items) => items.iter().enumerate().for_each(|(n, value)| walk(value, under(n.to_string()), files, found)),
            Value::String(text) => {
                if let Some(input) = files.iter().find(|input| input.digest == *text) {
                    found.push((at, input.path.clone(), input.digest.clone()));
                }
            }
            _ => {}
        }
    }
    let mut found = vec![];
    walk(value, String::new(), files, &mut found);
    found
}

/// The share of `picture`'s pixels that are fully transparent; none if it can't be read.
pub fn transparent_share(ffmpeg: &Path, picture: &Path) -> f64 {
    let read = std::process::Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(picture)
        .args(["-frames:v", "1", "-vf", "format=rgba,alphaextract", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
        .output();
    match read {
        Ok(out) if out.status.success() && !out.stdout.is_empty() => {
            out.stdout.iter().filter(|alpha| **alpha == 0).count() as f64 / out.stdout.len() as f64
        }
        _ => 0.0,
    }
}
