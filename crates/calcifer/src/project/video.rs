//! The `video` project type: characters, props and environments (an environment's camera setups
//! are entities of their own), world states that shots write and read, shots of any length with beats,
//! an upscale per shot, and the edit, cut here with ffmpeg. Stills render through the `image`
//! interface (xs-135); shots and upscales through `video/shot` and `video/upscale`.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::{Path, PathBuf},
};

use protocol::error::{Error, Kind};
use saphyr::{MappingOwned, ScalarOwned, YamlOwned};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    folder::{self, Folder, Lock, Made, Upstream},
    graph::{Edge, Entity, Graph, mentions, name_key},
};

pub const TYPE: &str = "video";
/// The name `cozy run all` means: every entity, brought up to date.
pub const ALL: &str = "all";

/// Each kind's renderer unless `renderers:` names another.
const RENDERERS: [(&str, &str); 8] = [
    ("character", "qwen-image-2/generate"),
    ("prop", "qwen-image-2/generate"),
    ("environment", "qwen-image-2/generate"),
    ("setup", "qwen-image-2/generate"),
    ("world_state", "qwen-image-2/generate"),
    ("shot", "minimax-h3/shot"),
    ("upscale", "minimax-h3/upscale"),
    // Characters' and props' pictures before any use: the subject on white, so a shot renderer
    // never paints their backgrounds into the set.
    ("clean", "birefnet/remove_background"),
];

/// A new project's project.yaml: each kind shown, commented out (cozy's writes keep comments).
pub fn skeleton() -> String {
    "# A video project (`cozy project --help`). Fill in the lists, then
#   cozy render all --project <this folder> --on <machine>
# makes everything missing or out of date, in order, and cuts edit.mp4 here.
type: video
style: \"\"   # the look, added to every picture and shot
assets:   # characters, props, and environments with camera setups (camera text in screen terms)
  # - {name: theo, kind: character, description: \"A tall man in a green raincoat.\"}
  # - {name: dock, kind: environment, description: \"a foggy harbour at dawn\",
  #    setups: [{name: a, camera: \"wide: the red hut near on the left, the crates far on the right\"}]}
world_states:   # how a set stands; a shot that writes one changes it for the shots after
  # - {name: stage, init: \"<theo> stands on <dock>.\", blocking: \"theo by the hut.\"}
shots:   # in order; past its renderer's longest piece (15 s for minimax-h3) a shot needs beats
  # - {name: walk, setup: dock.a, prompt: \"<theo> crosses <dock>.\", duration_s: 8, writes: [stage],
  #    beats: [{at: 0, prompt: \"He walks to the edge.\"}, {at: 4, prompt: \"He looks out to sea.\"}]}
# edit: [{shot: walk, end: 6}]   # which shots and spans the edit cuts; absent: every shot, whole
"
    .into()
}

#[derive(Deserialize)]
struct Project {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    renderers: BTreeMap<String, String>,
    #[serde(default)]
    style: String,
    #[serde(default)]
    edit_uses: Uses,
    #[serde(default, deserialize_with = "listed")]
    assets: Vec<Asset>,
    #[serde(default, deserialize_with = "listed")]
    world_states: Vec<WorldState>,
    #[serde(default, deserialize_with = "listed")]
    shots: Vec<Shot>,
    /// None: every shot, in order, whole.
    edit: Option<Vec<Cut>>,
}

#[derive(Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Uses {
    #[default]
    Native,
    Upscaled,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    kind: AssetKind,
    #[serde(default)]
    description: String,
    #[serde(default)]
    setups: Vec<Setup>,
    image: Option<String>,
    #[serde(default)]
    versions: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AssetKind {
    Character,
    Prop,
    Environment,
}

impl AssetKind {
    fn name(self) -> &'static str {
        match self {
            AssetKind::Character => "character",
            AssetKind::Prop => "prop",
            AssetKind::Environment => "environment",
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
    #[serde(default)]
    versions: Vec<String>,
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
    #[serde(default)]
    versions: Vec<String>,
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
    /// World states its last frame becomes the next revision of.
    #[serde(default)]
    writes: Vec<String>,
    video: Option<String>,
    #[serde(default)]
    versions: Vec<String>,
    #[serde(default)]
    upscale: Upscale,
    /// The renderer's own fields (minimax-h3's `turbo`, …), added to the request.
    #[serde(default)]
    settings: serde_json::Map<String, Value>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Beat {
    at: f64,
    prompt: String,
}

#[derive(Default, Deserialize)]
struct Upscale {
    video: Option<String>,
    #[serde(default)]
    versions: Vec<String>,
    /// The upscaler's own fields (minimax-h3's `resolution`, `upscale: {model: regenerate-2k}`, …).
    #[serde(default)]
    settings: serde_json::Map<String, Value>,
}

/// A piece of a shot in the edit; `start`/`end` in seconds (names pending Paul).
#[derive(Deserialize)]
struct Cut {
    shot: String,
    start: Option<f64>,
    end: Option<f64>,
}

/// Where an entity is in project.yaml.
#[derive(Clone, Copy)]
enum Item {
    Asset(usize),
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
}

/// A version's request, and what to record of it.
pub struct Request {
    /// Each file in it as its digest; `files` say where each one is here.
    pub payload: Value,
    pub made: Made,
    /// The result field holding the version.
    pub field: &'static str,
    pub writes: Vec<String>,
    pub files: Vec<Input>,
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
        // A chosen file is there, or the entity is named with the fix.
        let chosen = |id: &str, field: &str, file: &Option<String>| -> Result<Option<(String, String)>, Error> {
            let Some(file) = file else { return Ok(None) };
            if !folder.path(file).is_file() {
                return Err(invalid("file_missing", format!("{id}'s {field}: {file} is not there"))
                    .remedy(format!("fix the path, or remove `{field}:` and `cozy render {id}` makes one")));
            }
            Ok(Some((file.clone(), folder.digest(file)?)))
        };
        let mut entities = vec![];
        let mut items = vec![];
        for (a, asset) in project.assets.iter().enumerate() {
            let kind = asset.kind.name();
            if !asset.setups.is_empty() && asset.kind != AssetKind::Environment {
                return Err(invalid(
                    "setup_owner",
                    format!("{} is a {kind}: only an environment has setups", asset.name),
                ));
            }
            entities.push(Entity {
                id: asset.name.clone(),
                kind,
                mentionable: true,
                state: false,
                order: None,
                texts: vec![asset.description.clone()],
                needs: vec![],
                writes: vec![],
                file: chosen(&asset.name, "image", &asset.image)?,
                versions: asset.versions.clone(),
            });
            items.push(Item::Asset(a));
            for (s, setup) in asset.setups.iter().enumerate() {
                entities.push(Entity {
                    id: format!("{}.{}", asset.name, setup.name),
                    kind: "setup",
                    mentionable: false,
                    state: false,
                    order: None,
                    texts: vec![setup.camera.clone()],
                    needs: vec![setup.frame.as_ref().map_or_else(|| asset.name.clone(), |frame| frame.shot.clone())],
                    writes: vec![],
                    file: chosen(&format!("{}.{}", asset.name, setup.name), "image", &setup.image)?,
                    versions: setup.versions.clone(),
                });
                items.push(Item::Setup(a, s));
            }
        }
        for asset in &project.assets {
            for (setup, frame) in asset.setups.iter().filter_map(|s| s.frame.as_ref().map(|f| (s, f))) {
                let id = format!("{}.{}", asset.name, setup.name);
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
            entities.push(Entity {
                id: state.name.clone(),
                kind: "world_state",
                mentionable: true,
                state: true,
                order: None,
                texts: vec![state.init.clone(), state.blocking.clone()],
                needs: vec![],
                writes: vec![],
                file: chosen(&state.name, "image", &state.image)?,
                versions: state.versions.clone(),
            });
            items.push(Item::State(w));
        }
        for (k, shot) in project.shots.iter().enumerate() {
            let texts: Vec<String> =
                std::iter::once(shot.prompt.clone()).chain(shot.beats.iter().map(|b| b.prompt.clone())).collect();
            let mut needs = vec![];
            if let Some(setup) = &shot.setup {
                if !entities.iter().any(|n| n.kind == "setup" && name_key(&n.id) == name_key(setup)) {
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
                let env = (project.assets.iter())
                    .find(|a| a.kind == AssetKind::Environment && name_key(&a.name) == name_key(mention));
                if let Some(env) = env.filter(|env| framed.as_deref() != Some(&name_key(&env.name)))
                    && let Some(first) = env.setups.first()
                {
                    needs.push(format!("{}.{}", env.name, first.name));
                }
            }
            entities.push(Entity {
                id: shot.name.clone(),
                kind: "shot",
                mentionable: false,
                state: false,
                order: Some(k),
                texts,
                needs,
                writes: shot.writes.clone(),
                file: chosen(&shot.name, "video", &shot.video)?,
                versions: shot.versions.clone(),
            });
            items.push(Item::Shot(k));
            entities.push(Entity {
                id: format!("{}.upscale", shot.name),
                kind: "upscale",
                mentionable: false,
                state: false,
                order: None,
                texts: vec![],
                needs: vec![shot.name.clone()],
                writes: vec![],
                file: chosen(&format!("{}.upscale", shot.name), "video", &shot.upscale.video)?,
                versions: shot.upscale.versions.clone(),
            });
            items.push(Item::Upscale(k));
        }
        if let Some(all) = entities.iter().find(|entity| name_key(&entity.id) == name_key(ALL)) {
            let said = format!("{} is named `{ALL}`, which `cozy render {ALL}` means", all.id);
            return Err(invalid("name_reserved", said).remedy("rename it"));
        }
        let graph = Graph::new(entities)?;
        warnings.extend(graph.unknown.iter().map(|(i, mention)| {
            format!(
                "{} mentions {mention}, which is nothing in the project: it reaches the renderer as written",
                graph.entities[*i].id
            )
        }));
        Ok(Self { project, items, graph, warnings })
    }

    pub fn renderer(&self, i: usize) -> &str {
        renderer(&self.project, self.graph.entities[i].kind)
    }

    /// The renderer of shots.
    pub fn shot_renderer(&self) -> Option<&str> {
        (!self.project.shots.is_empty()).then(|| renderer(&self.project, "shot"))
    }

    /// The field that holds the entity's chosen file.
    pub fn field(&self, i: usize) -> &'static str {
        match self.items[i] {
            Item::Shot(_) | Item::Upscale(_) => "video",
            _ => "image",
        }
    }

    /// Whether `render` makes the entity: everything but upscales the edit does not use.
    pub fn wanted(&self, i: usize) -> bool {
        !matches!(self.items[i], Item::Upscale(_)) || self.project.edit_uses == Uses::Upscaled
    }

    /// Whether entity `i`'s pictures are cleaned before use: a character's or a prop's.
    pub fn cleans(&self, i: usize) -> bool {
        matches!(self.graph.entities[i].kind, "character" | "prop")
    }

    /// Entity `i`'s chosen picture and its digest when it still needs its clean: a character's or
    /// prop's that versions.lock does not mark as a clean.
    pub fn unclean(&self, lock: &Lock, i: usize) -> Option<(String, String)> {
        let file = self.graph.entities[i].file.as_ref().filter(|_| self.cleans(i))?;
        (!lock.versions.get(&file.1).is_some_and(|made| made.cleaned)).then(|| file.clone())
    }

    pub fn clean_renderer(&self) -> &str {
        renderer(&self.project, "clean")
    }

    /// Shots longer than `max_piece_s` with no beats.
    pub fn beatless(&self, max_piece_s: f64) -> Vec<(&str, f64)> {
        (self.project.shots.iter())
            .filter(|shot| shot.duration_s > max_piece_s && shot.beats.is_empty())
            .map(|shot| (shot.name.as_str(), shot.duration_s))
            .collect()
    }

    /// The digest of every file an entity lists or chose.
    pub fn listed(&self, folder: &Folder) -> Result<BTreeSet<String>, Error> {
        let mut listed = BTreeSet::new();
        for entity in &self.graph.entities {
            listed.extend(entity.file.iter().map(|(_, digest)| digest.clone()));
            for version in &entity.versions {
                listed.insert(folder.digest(version)?);
            }
        }
        Ok(listed)
    }

    /// The request that makes a version of entity `i`, from what it reads now.
    pub fn request(&self, folder: &Folder, lock: &Lock, i: usize) -> Result<Request, Error> {
        let graph = &self.graph;
        let entity = &graph.entities[i];
        let mut upstream = vec![];
        let mut reads = vec![];
        for edge in &graph.upstream[i] {
            let up = &graph.entities[edge.entity];
            let digest = graph.read(lock, edge).ok_or_else(|| {
                Error::new(
                    Kind::Usage,
                    "upstream_empty",
                    format!("{} reads {}, which has no version yet", entity.id, up.id),
                )
                .next(format!("cozy render {} --project …", up.id))
            })?;
            let path = match (&edge.state, &up.file) {
                (None, Some((file, _))) => folder.path(file),
                _ => folder.path(&folder.find(&digest)?),
            };
            upstream.push(Upstream { entity: up.id.clone(), state: edge.state.clone(), file: digest.clone() });
            reads.push(Read { edge, path, digest });
        }
        let made = Made { entity: entity.id.clone(), upstream: Some(upstream), ..Default::default() };
        let styled = |text: String| match self.project.style.trim() {
            "" => text,
            style => format!("{text}\n\nStyle: {style}."),
        };
        let mut files = vec![];
        let pictures = |files: &mut Vec<Input>| -> Vec<Value> {
            (reads.iter().enumerate())
                .map(|(n, read)| {
                    let up = &graph.entities[read.edge.entity];
                    let (name, kind) =
                        read.edge.state.as_ref().map_or((up.id.as_str(), up.kind), |s| (s.as_str(), "world_state"));
                    json!({"name": name, "kind": kind, "image": read.input(files, format!("assets.{n}.image"), None)})
                })
                .collect()
        };
        let (payload, writes) = match self.items[i] {
            Item::Asset(a) => {
                let asset = &self.project.assets[a];
                let (shown, ratio) = match asset.kind {
                    AssetKind::Character => {
                        ("Character reference: the whole figure, front view, on a plain white background.", "3:4")
                    }
                    AssetKind::Prop => ("Prop reference: the object alone, on a plain white background.", "1:1"),
                    AssetKind::Environment => ("An establishing view of the empty set, no people.", "16:9"),
                };
                let prompt = styled(format!("{}\n\n{shown}", asset.description.trim()));
                (json!({"prompt": prompt, "assets": pictures(&mut files), "aspect_ratio": ratio}), vec![])
            }
            Item::Setup(a, s) => {
                // The environment's picture (or a shot's frame) first, and the camera in screen terms
                // as written (environment-angles REPORT.md): bare camera words return a mirror.
                let (env, setup) = (&self.project.assets[a], &self.project.assets[a].setups[s]);
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
                let assets: Vec<Value> = (reads.iter().enumerate())
                    .map(|(n, read)| {
                        let up = &graph.entities[read.edge.entity];
                        let (name, kind, at) = match n {
                            0 => (env.name.as_str(), "environment", setup.frame.as_ref().map(|frame| frame.at)),
                            _ => (up.id.as_str(), up.kind, None),
                        };
                        json!({"name": name, "kind": kind, "image": read.input(&mut files, format!("assets.{n}.image"), at)})
                    })
                    .collect();
                (json!({"prompt": prompt, "assets": assets, "aspect_ratio": "16:9"}), vec![])
            }
            Item::State(w) => {
                let state = &self.project.world_states[w];
                let prompt = styled([state.init.trim(), state.blocking.trim()].join("\n\n").trim().to_owned());
                (json!({"prompt": prompt, "assets": pictures(&mut files), "aspect_ratio": "16:9"}), vec![])
            }
            Item::Shot(k) => self.shot(k, &reads, &mut files),
            Item::Upscale(k) => {
                // The text the shot was made from, for an upscaler that re-renders it.
                let shot = &self.project.shots[k];
                let beats = shot.beats.iter().map(|beat| format!("Then, {}", beat.prompt.trim()));
                let text = std::iter::once(shot.prompt.trim().to_owned()).chain(beats).collect::<Vec<_>>().join(" ");
                let mut payload = json!({"video": reads[0].input(&mut files, "video".into(), None), "prompt": text.replace(['<', '>'], "")});
                payload.as_object_mut().expect("an object").extend(shot.upscale.settings.clone());
                (payload, vec![])
            }
        };
        Ok(Request { payload, made, field: self.field(i), writes, files })
    }

    /// A shot's `video/shot` request: its breakdown (an environment's picture is its setup's
    /// plate), the world states it reads, and a last frame for each state it writes.
    fn shot(&self, k: usize, reads: &[Read], files: &mut Vec<Input>) -> (Value, Vec<String>) {
        let shot = &self.project.shots[k];
        let framed: BTreeSet<usize> = (reads.iter())
            .filter_map(|read| match self.items[read.edge.entity] {
                Item::Setup(a, _) => Some(a),
                _ => None,
            })
            .collect();
        let layout = |name: &str| {
            let state = self.project.world_states.iter().find(|s| name_key(&s.name) == name_key(name));
            state.map(|s| s.blocking.clone()).unwrap_or_default()
        };
        let (mut assets, mut states) = (vec![], vec![]);
        for read in reads {
            let (n, m) = (assets.len(), states.len());
            let state = |name: &str, files: &mut Vec<Input>| json!({"name": name, "image": read.input(files, format!("world_state.{m}.image"), None), "layout": layout(name)});
            match (self.items[read.edge.entity], &read.edge.state) {
                (Item::Asset(a), _) if !framed.contains(&a) => {
                    let asset = &self.project.assets[a];
                    let image = read.input(files, format!("assets.{n}.image"), None);
                    assets.push(json!({"name": asset.name, "kind": asset.kind.name(), "image": image, "description": asset.description}));
                }
                (Item::Setup(a, _), _) => {
                    let env = &self.project.assets[a];
                    let image = read.input(files, format!("assets.{n}.image"), None);
                    assets.push(json!({"name": env.name, "kind": "environment", "image": image, "description": env.description}));
                }
                (Item::State(w), _) => states.push(state(&self.project.world_states[w].name, files)),
                (Item::Shot(_), Some(name)) => states.push(state(name, files)),
                _ => {}
            }
        }
        let writes: Vec<String> = (shot.writes.iter())
            .filter_map(|name| self.graph.find(name).map(|s| self.graph.entities[s].id.clone()))
            .collect();
        let mut payload = json!({
            "prompt": shot.prompt, "duration_s": shot.duration_s, "beats": shot.beats, "style": self.project.style,
            "assets": assets, "world_state": states, "writes": vec!["last"; writes.len()],
        });
        payload.as_object_mut().expect("an object").extend(shot.settings.clone());
        (payload, writes)
    }

    /// The edit's pieces: each cut's shot file (its upscale's with `edit_uses: upscaled`) and range;
    /// and the shots it cuts that have no file yet, skipped.
    pub fn cuts(&self, folder: &Folder) -> Result<(Vec<Piece>, Vec<String>), Error> {
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
            let id = match self.project.edit_uses {
                Uses::Native => self.project.shots[k].name.clone(),
                Uses::Upscaled => format!("{}.upscale", self.project.shots[k].name),
            };
            let entity = self.graph.find(&id).map(|i| &self.graph.entities[i]);
            match entity.and_then(|entity| entity.file.as_ref()) {
                Some((file, digest)) => {
                    pieces.push(Piece { path: folder.path(file), digest: digest.clone(), start, end })
                }
                None if !skipped.contains(&id) => skipped.push(id),
                None => {}
            }
        }
        Ok((pieces, skipped))
    }

    /// Whether the edit cuts entity `i`'s file: a shot's, or with `edit_uses: upscaled` its upscale's.
    pub fn cut_reads(&self, i: usize) -> bool {
        matches!(
            (self.items[i], self.project.edit_uses),
            (Item::Shot(_), Uses::Native) | (Item::Upscale(_), Uses::Upscaled)
        )
    }
}

/// Entity `id`'s mapping in a project document: an asset, world state or shot by name, a setup as
/// `Environment.Setup`, or a shot's `upscale` (made when absent) as `shot.upscale`.
pub fn locate<'a>(doc: &'a mut YamlOwned, id: &str) -> Option<&'a mut MappingOwned> {
    for section in ["assets", "world_states", "shots"] {
        if let Some(i) = position(doc.as_mapping_get(section), id) {
            return doc.as_mapping_get_mut(section)?.as_vec_mut()?.get_mut(i)?.as_mapping_mut();
        }
    }
    let (owner, part) = id.rsplit_once('.')?;
    if name_key(part) == "upscale"
        && let Some(i) = position(doc.as_mapping_get("shots"), owner)
    {
        let shot = doc.as_mapping_get_mut("shots")?.as_vec_mut()?.get_mut(i)?.as_mapping_mut()?;
        let key = YamlOwned::Value(ScalarOwned::String("upscale".into()));
        if !matches!(shot.get(&key), Some(YamlOwned::Mapping(_))) {
            shot.insert(key.clone(), YamlOwned::Mapping(MappingOwned::new()));
        }
        return shot.get_mut(&key)?.as_mapping_mut();
    }
    let a = position(doc.as_mapping_get("assets"), owner)?;
    let asset = doc.as_mapping_get_mut("assets")?.as_vec_mut()?.get_mut(a)?;
    let s = position(asset.as_mapping_get("setups"), part)?;
    asset.as_mapping_get_mut("setups")?.as_vec_mut()?.get_mut(s)?.as_mapping_mut()
}

/// The entry of a list of named mappings that `name` names.
fn position(list: Option<&YamlOwned>, name: &str) -> Option<usize> {
    let named = |item: &YamlOwned| {
        item.as_mapping_get("name").and_then(YamlOwned::as_str).is_some_and(|n| name_key(n) == name_key(name))
    };
    list?.as_vec()?.iter().position(named)
}

/// `file` listed in an entity's `versions`, and chosen as its `field` when `choose` or it has none.
pub fn add_version(map: &mut MappingOwned, field: &str, file: &str, choose: bool) {
    let get = |map: &MappingOwned, key: &str| map.get(&YamlOwned::Value(ScalarOwned::String(key.to_owned()))).cloned();
    if choose || get(map, field).as_ref().and_then(YamlOwned::as_str).is_none() {
        folder::put(map, field, folder::text(file));
    }
    let mut versions = get(map, "versions").as_ref().and_then(YamlOwned::as_vec).cloned().unwrap_or_default();
    if !versions.iter().any(|v| v.as_str() == Some(file)) {
        versions.push(folder::text(file));
    }
    folder::put(map, "versions", YamlOwned::Sequence(versions));
}

/// Cuts `pieces` into `<project>/edit.mp4` with cozy's ffmpeg, here, by fast join: each piece's
/// span between keyframes is copied, only the GOPs at its ends are encoded (kept in
/// `<project>/.cuts` by file and span, so a re-cut encodes only what changed), and the sound is
/// encoded once. Pieces whose files differ in codec, size or frame rate are encoded whole.
pub fn cut(ffmpeg: &Path, ffprobe: &Path, folder: &Folder, pieces: &[Piece]) -> Result<PathBuf, Error> {
    let probes = pieces.iter().map(|piece| probe(ffprobe, &piece.path)).collect::<Result<Vec<_>, _>>()?;
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
    if probes.iter().all(|probe| probe.video == probes[0].video && probe.video.starts_with("h264 ")) {
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
        let (width, height) = probes[0].size;
        let (mut filter, mut joined, mut inputs) = (String::new(), String::new(), vec![]);
        for (k, (piece, (start, end))) in pieces.iter().zip(&ranges).enumerate() {
            inputs.extend(["-i".into(), piece.path.clone().into_os_string()]);
            let _ = write!(
                filter,
                "[{k}:v]trim=start={start}:end={end},setpts=PTS-STARTPTS,scale={width}:{height},setsar=1[v{k}];"
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
    let part = work.path().join("edit.mp4");
    mux.extend(["-map", "0:v", "-c", "copy", "-movflags", "+faststart", "-f", "mp4"].map(Into::into));
    mux.push(part.clone().into_os_string());
    run(ffmpeg, &mux.iter().map(|a| a.as_os_str()).collect::<Vec<_>>())?;
    let out = folder.dir.join("edit.mp4");
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

fn renderer<'a>(project: &'a Project, kind: &str) -> &'a str {
    match project.renderers.get(kind) {
        Some(target) => target,
        None => RENDERERS.iter().find(|(k, _)| *k == kind).map_or("", |(_, target)| *target),
    }
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
