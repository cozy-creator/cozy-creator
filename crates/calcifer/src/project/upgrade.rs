//! One way, once: a project folder of format 2 to format 3 (design/project-format-ui.md), in this process, with no Calcifer, for a Calcifer of
//! this format can't read it yet. Each asset's current version is its state in format 2, worked out as the cozy that reads it did (`format2`),
//! or what its `cozy project status --json` said; the new key is stamped on those versions, so nothing goes out of date and nothing runs. Each
//! entity folds into its sheet, which takes its name, `description` and `asset_type`; each form becomes an asset of its dotted name; a written
//! still a file. Comments come along. The old files stay beside the new. It writes the file whole, the one place anything does, as nothing of
//! the old lines can stand.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
    sync::Mutex,
};

use api::{
    error::{Error, Kind},
    target::written_as_path,
    calcifer::tool::EDIT,
    yaml::{keys, to_json, to_json_keeping},
};
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::{
    folder::{self, Folder, Lock},
    legacy::{self, MEASURED},
    written::mentions_renamed,
    format::{Doc, FORMAT, State, TEXT, canonical, invalid, made_here, names_a_file, plain_tool, yaml_key},
    format2,
    graph::{mentions, mentions_in, name_key},
    holding,
};

const KINDS: [&str; 3] = ["image", "video", "audio"];
const ASSET_TYPES: [&str; 3] = ["character", "prop", "environment"];
const WAS_YAML: &str = "project.yaml.v2";
const FRAME: &str = "cozy/frame";
const WAS_LOCK: &str = "project.lock.v2";
const WAS_THREE: &str = "project.yaml.v3";
const WAS_THREE_LOCK: &str = "project.lock.v3";
/// An asset's keys in the order they're written, then any other by name, then what's said of it.
const FIRST: [&str; 14] = ["tool", "models", "lora", "instructions", "summary", "soundscape", "music", "style", "aspect_ratio", "resolution", "video", "beats", "fps", "tracks"];
const LAST: [&str; 4] = ["approved", "description", "asset_type", "voice"];
/// A beat's, a clip's, a LoRA's.
const INSIDE: [&str; 10] = ["duration", "description", "video", "image", "in", "out", "frames", "clips", "ref", "strength"];

/// What became of a project.
#[derive(Serialize)]
pub struct Upgraded {
    pub assets: Vec<Row>,
    /// What it couldn't carry over by itself, and what the file says now.
    pub said: Vec<String>,
    /// It was of this format already: nothing moved.
    pub current: bool,
}

/// An asset before and after: `current`, `out_of_date`, `missing`; `new` for what had no row.
#[derive(Serialize)]
pub struct Row {
    pub name: String,
    pub was: String,
    pub before: String,
    pub after: String,
    pub version: Option<u32>,
}

fn refused(code: &str, said: String) -> Error {
    Error::new(Kind::Usage, code, said)
}

fn io(at: &Path, why: impl std::fmt::Display) -> Error {
    Error::new(Kind::Unavailable, "project_io", format!("{}: {why}", at.display()))
}

/// What an entity link holds.
#[derive(Clone)]
enum Link {
    Asset(String),
    Hash(String),
}

fn link_of(value: &Value) -> Option<Link> {
    let text = match value {
        Value::String(text) => text.trim(),
        _ => return None,
    };
    match mentions(text).as_slice() {
        [one] if text == format!("<{one}>") => Some(Link::Asset((*one).to_owned())),
        _ if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_hexdigit()) => Some(Link::Hash(text.to_owned())),
        _ => None,
    }
}

fn text_of(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or_default().trim().to_owned()
}

/// What an entity or form says of the asset it folds into.
#[derive(Clone, Default)]
struct Said {
    description: String,
    asset_type: String,
    voice: Option<Link>,
}

/// What the file's old parts turn into.
#[derive(Default)]
struct Folding {
    /// Old name key to new name.
    renames: BTreeMap<String, String>,
    /// What each entity says of its sheet, by the sheet's old name key.
    said_of: BTreeMap<String, Said>,
    /// Assets that are nothing but what an entity held.
    made: Vec<(String, Map<String, Value>)>,
    notes: Vec<String>,
}

fn said_body(said: &Said) -> Map<String, Value> {
    let mut body = Map::new();
    if !said.description.is_empty() {
        body.insert("description".into(), json!(said.description));
    }
    if !said.asset_type.is_empty() {
        body.insert("asset_type".into(), json!(said.asset_type));
    }
    body
}

impl Folding {
    /// `name`, an entity or form, onto `sheet`: the name its mentions read from now on is `new`.
    fn fold(&mut self, name: &str, new: &str, sheet: Option<(&str, Link)>, said: Said, declared: &BTreeSet<String>) {
        self.renames.insert(name_key(name), new.to_owned());
        match sheet {
            Some((_, Link::Asset(target))) if declared.contains(&name_key(&target)) && !self.said_of.contains_key(&name_key(&target)) => {
                // An asset goes by that name already, and stands: the sheet keeps its own.
                let new = if declared.contains(&name_key(new)) && name_key(new) != name_key(&target) { target.as_str() } else { new };
                self.renames.insert(name_key(name), new.to_owned());
                self.renames.insert(name_key(&target), new.to_owned());
                self.said_of.insert(name_key(&target), said);
            }
            Some((_, Link::Asset(target))) => {
                self.notes.push(format!("{name}: its picture <{target}> is another's, or nothing in the project, so it now reads that"));
                let there = self.renames.get(&name_key(&target)).cloned().unwrap_or(target);
                self.renames.insert(name_key(name), there);
            }
            Some((_, Link::Hash(hash))) => {
                let mut body = said_body(&said);
                body.insert("approved".into(), json!(hash));
                self.made.push((new.to_owned(), body));
                self.said_of.insert(format!("\0{}", name_key(new)), said);
            }
            None => {
                self.made.push((new.to_owned(), said_body(&said)));
                self.said_of.insert(format!("\0{}", name_key(new)), said);
            }
        }
    }

    /// An entity or form's links: the sheet, its voice, and any other link's name.
    fn links(&mut self, name: &str, new: &str, body: &Map<String, Value>, parent: &Said, declared: &BTreeSet<String>) {
        let word = |field: &str| text_of(body.get(field));
        let said = Said {
            description: Some(word("description")).filter(|text| !text.is_empty()).unwrap_or_else(|| parent.description.clone()),
            asset_type: Some(word("kind")).filter(|kind| ASSET_TYPES.contains(&kind.as_str())).unwrap_or_else(|| parent.asset_type.clone()),
            voice: body.get("voice").and_then(link_of).or_else(|| parent.voice.clone()),
        };
        let role = KINDS.iter().find_map(|role| Some((*role, link_of(body.get(*role)?)?)));
        if role.is_none() && !parent.description.is_empty() {
            self.notes.push(format!("{name} had its parent's picture, and is now only words: give it `approved`"));
        }
        self.fold(name, new, role, said.clone(), declared);
        for (form, inner) in body.iter().filter_map(|(field, value)| Some((field, value.as_object()?))) {
            self.links(&format!("{name}.{form}"), &format!("{new}.{form}"), inner, &said, declared);
        }
        for (field, value) in body.iter().filter(|(field, _)| !["kind", "description", "render", "voice"].contains(&field.as_str())) {
            let full = name_key(&format!("{name}.{field}"));
            match link_of(value) {
                Some(Link::Asset(target)) => {
                    let there = self.renames.get(&name_key(&target)).cloned().unwrap_or(target);
                    self.renames.insert(full, there);
                }
                Some(Link::Hash(_)) if KINDS.contains(&field.as_str()) && self.made.iter().any(|(there, _)| there == new) => {
                    self.renames.insert(full, new.to_owned());
                }
                Some(Link::Hash(hash)) => {
                    let named = format!("{new}.{field}");
                    self.made.push((named.clone(), Map::from_iter([("approved".to_owned(), json!(hash))])));
                    self.renames.insert(full, named);
                }
                None => {}
            }
        }
    }
}

/// The function a tool naming `with` means, for what the file says of the asset. A package alone was the adapter's to pick, and now it's said.
#[allow(clippy::too_many_arguments)]
fn tool_of(with: &str, kind: &str, inputs: &Map<String, Value>, turbo: Option<bool>, made_by: Option<&str>, folder: &Folder, notes: &mut Vec<String>, name: &str) -> String {
    let with = with.trim();
    if made_here(with) {
        return if kind == "video" { EDIT.to_owned() } else { FRAME.to_owned() };
    }
    let is_path = written_as_path(with) || with.ends_with(".py");
    let named = match is_path {
        true => {
            let home = platform::paths::login_home().unwrap_or_default();
            matches!(crate::local_code::target(with, Some(&folder.dir), &home), Some(Ok((_, Some(_)))))
        }
        false => with.split('@').next().unwrap_or(with).trim_start_matches("_/").contains('/'),
    };
    if named {
        // H3's one function takes Turbo.
        return if names_package(with, "minimax-h3") { with.replace("/generate_turbo", "/generate") } else { with.to_owned() };
    }
    let (path, release) = match is_path {
        true => (with.trim_end_matches('/'), None),
        false => with.split_once('@').map_or((with, None), |(path, release)| (path, Some(release))),
    };
    let package = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let said = |field: &str| inputs.get(field).is_some_and(|value| !value.is_null());
    let function = if kind == "video" && !said("summary") && !said("beats") && !said("instructions") && (said("video") || said("resolution")) {
        "upscale"
    } else {
        match (turbo, package, made_by) {
            // Its `generate` takes `turbo`, on unless the file said off: what it was called with.
            (_, "minimax-h3", _) => "generate",
            (Some(true), ..) => "generate_turbo",
            (Some(false), ..) => "generate",
            (None, _, Some(by)) => by.rsplit('/').next().unwrap_or(by).split('@').next().unwrap_or(by),
            (None, _, None) => {
                notes.push(format!("{name}: {with} names a package, and it's taken as {path}/generate: if that's wrong, write its `tool`"));
                "generate"
            }
        }
    };
    match release {
        Some(release) => format!("{path}/{function}@{release}"),
        None => format!("{path}/{function}"),
    }
}

/// `<name>` a line of the file left in a comment when a pin went over it: `video: <hash>  # was <clash>`.
fn pinned_over(text: &str, field: &str, hash: &str) -> Option<String> {
    let start = format!("{field}: ");
    let line = text.lines().find(|line| line.trim_start().starts_with(&start) && line.contains(hash) && line.contains("# was "))?;
    let was = line.split_once("# was ")?.1.trim();
    (!mentions(was).is_empty()).then(|| was.to_owned())
}

/// `pieces` as `tracks`.
fn tracks_of(pieces: &Value) -> Value {
    let clip = |piece: &Value| match piece.as_object() {
        None => piece.clone(),
        Some(fields) => {
            let renamed = fields.iter().map(|(field, value)| {
                let field = match field.as_str() {
                    "start" => "in",
                    "end" => "out",
                    field => field,
                };
                (field.to_owned(), value.clone())
            });
            Value::Object(renamed.collect())
        }
    };
    json!([{"clips": pieces.as_array().into_iter().flatten().map(clip).collect::<Vec<_>>()}])
}

/// What the sections of `defaults` shared that goes on as text assets.
#[derive(Default)]
struct Shared {
    /// By section, the text asset its style is now, and that style.
    style: BTreeMap<String, (String, String)>,
    /// By section, the text asset its soundscape is now.
    soundscape: BTreeMap<String, (String, String)>,
    assets: Vec<(String, Map<String, Value>)>,
}

impl Shared {
    fn of(defaults: &Map<String, Value>, mut taken: Vec<String>) -> Self {
        let mut shared = Self::default();
        shared.style = shared.text_assets("style", defaults, &mut taken);
        shared.soundscape = shared.text_assets("soundscape", defaults, &mut taken);
        shared
    }

    /// `field` of each section as a text asset: the first called that, and another that extends it as `<field>` and the rest of its words.
    fn text_assets(&mut self, field: &str, defaults: &Map<String, Value>, taken: &mut Vec<String>) -> BTreeMap<String, (String, String)> {
        let mut named: BTreeMap<String, (String, String)> = BTreeMap::new();
        let mut first: Option<(String, String)> = None;
        for kind in KINDS {
            let Some(text) = defaults.get(kind).and_then(|section| section.get(field)?.as_str()).map(str::trim).filter(|text| !text.is_empty()) else { continue };
            if let Some((name, _)) = named.values().find(|(_, held)| held == text) {
                named.insert(kind.to_owned(), (name.clone(), text.to_owned()));
                continue;
            }
            let wanted = if first.is_none() { field.to_owned() } else { format!("{kind}_{field}") };
            let name = (1..).map(|n| if n == 1 { wanted.clone() } else { format!("{wanted}_{n}") }).find(|name| !taken.iter().any(|held| name_key(held) == name_key(name))).expect("a free name");
            taken.push(name.clone());
            let written = match &first {
                Some((of, words)) if text.strip_prefix(words.as_str()).is_some_and(|rest| rest.starts_with(char::is_whitespace)) => format!("<{of}>{}", &text[words.len()..]),
                _ => text.to_owned(),
            };
            self.assets.push((name.clone(), Map::from_iter([(TEXT.to_owned(), json!(written))])));
            first.get_or_insert((name.clone(), text.to_owned()));
            named.insert(kind.to_owned(), (name, text.to_owned()));
        }
        named
    }
}

/// Whether `with` names `package`, as a package or as a path to one, with a function or none.
fn names_package(with: &str, package: &str) -> bool {
    with.split('@').next().unwrap_or(with).trim_end_matches('/').rsplit(['/', '\\']).take(2).any(|part| part == package)
}

/// What `section` of `defaults` gave an asset, in the asset: its own values stand, shared words are mentions, and a style goes into the text
/// that took it, as each package put it there. Nothing is prepended at render: it's only where it's mentioned.
fn filled_in(inputs: &mut Map<String, Value>, section: Option<&Map<String, Value>>, kind: &str, with: &str, tool: &str, shared: &Shared, notes: &mut Vec<String>) {
    let (qwen, h3) = (names_package(with, "qwen-image-2"), names_package(with, "minimax-h3"));
    for (field, value) in section.into_iter().flatten() {
        match field.as_str() {
            "with" | "turbo" | "models" | "lora" | "style" => {}
            "transparent" if qwen => {}
            "options" => {
                for (field, value) in value.as_object().into_iter().flatten() {
                    inputs.entry(field.clone()).or_insert_with(|| value.clone());
                }
            }
            field if inputs.contains_key(field) => {}
            "soundscape" if shared.soundscape.contains_key(kind) => drop(inputs.insert(field.clone(), json!(format!("<{}>", shared.soundscape[kind].0)))),
            // An empty field says the same.
            "music" if value.as_str().is_some_and(|music| music.trim().eq_ignore_ascii_case("n/a")) => {}
            _ => drop(inputs.insert(field.clone(), value.clone())),
        }
    }
    let style = section.and_then(|_| shared.style.get(kind)).filter(|_| !inputs.contains_key("style"));
    let clear = if qwen { inputs.remove("transparent").or_else(|| section?.get("transparent").cloned()) } else { None };
    let upscale = tool.split('@').next().is_some_and(|tool| tool.ends_with("/upscale"));
    let said = |inputs: &Map<String, Value>, field: &str| inputs.get(field).and_then(Value::as_str).filter(|text| !text.trim().is_empty()).map(str::to_owned);
    if qwen || (h3 && upscale) {
        if let Some(words) = said(inputs, "instructions") {
            inputs.insert("instructions".to_owned(), json!(worded(&words, style, clear.and_then(|clear| clear.as_bool()) == Some(true))));
        }
    } else if let (true, Some((name, _))) = (h3, style) {
        let field = if said(inputs, "summary").is_none() && said(inputs, "instructions").is_some() { "instructions" } else { "summary" };
        let front = match said(inputs, field) {
            Some(words) => in_front(&words, name),
            None => format!("<{name}>"),
        };
        inputs.insert(field.to_owned(), json!(front));
        let line = "H3 shots now carry the style in their summary: it was a field of its own, and the same words made these versions, so none goes out of date".to_owned();
        if !notes.contains(&line) {
            notes.push(line);
        }
    } else if let Some((name, _)) = style {
        inputs.insert("style".to_owned(), json!(format!("<{name}>")));
    }
}

/// `<name>` at the front of `words`, after a `[…]` they open with: H3 reads its task there, and writes its own if it finds none.
fn in_front(words: &str, name: &str) -> String {
    let words = words.trim_start();
    let tag = words.strip_prefix('[').and_then(|rest| rest.find([']', '\n']).filter(|&end| rest.as_bytes()[end] == b']')).map(|end| end + 2);
    match tag {
        Some(end) => format!("{} <{name}>\n{}", &words[..end], words[end..].trim_start()),
        None => format!("<{name}>\n{words}"),
    }
}

/// `words` as Qwen's adapter said them, with `style` and a clear background: the words it has to be asked for in.
fn worded(words: &str, style: Option<&(String, String)>, clear: bool) -> String {
    let words = words.trim();
    let styled = match style {
        Some((name, text)) if text.ends_with('.') => format!("Style: <{name}>\n\n{words}"),
        Some((name, _)) => format!("Style: <{name}>.\n\n{words}"),
        None => words.to_owned(),
    };
    match clear {
        true => format!("This is an RGBA image with transparency. {styled}\n\nThe image has alpha channel and the background is transparent."),
        false => styled,
    }
}

/// One asset of format 2 as format 3 writes it, under its old name, and what it was.
#[allow(clippy::too_many_arguments)]
fn asset_of(name: &str, body: &Map<String, Value>, defaults: &Map<String, Value>, text: &str, old: &Value, folder: &Folder, shared: &Shared, notes: &mut Vec<String>) -> (Map<String, Value>, String) {
    let mut render = match body.get("render") {
        Some(Value::Object(render)) => render.clone(),
        Some(Value::String(prompt)) => Map::from_iter([("prompt".to_owned(), json!(prompt))]),
        _ => Map::new(),
    };
    let hash = |field: &&str| body.get(*field).and_then(Value::as_str).is_some_and(|said| !said.is_empty() && mentions(said).is_empty());
    let held = KINDS.iter().copied().find(hash);
    let written = text_of(body.get("kind"));
    let kind = KINDS.iter().copied().find(|kind| *kind == written).or(held).unwrap_or("image");
    let versions = old["assets"][name_key(name)]["versions"].as_array();
    // With versions of its own it was made, by `defaults`, and its hash is its approval.
    let made = versions.is_some_and(|versions| !versions.is_empty());
    let is_constant = !made && held.is_some() && body.keys().all(|key| ["kind", "description", "image", "video", "audio"].contains(&key.as_str()));
    let section = defaults.get(kind).and_then(Value::as_object).filter(|_| !is_constant);
    let own_with = render.remove("with");
    let with = text_of(own_with.as_ref().or_else(|| section?.get("with")));
    let turbo = render.remove("turbo").as_ref().and_then(Value::as_bool).or_else(|| section.filter(|_| own_with.is_none())?.get("turbo")?.as_bool());
    let mut out = Map::new();
    for field in ["models", "lora"] {
        if let Some(value) = render.remove(field).or_else(|| section.filter(|_| own_with.is_none())?.get(field).cloned()) {
            out.insert(field.to_owned(), value);
        }
    }
    let mut inputs = Map::new();
    if let Some(prompt) = render.remove("prompt") {
        inputs.insert("instructions".to_owned(), prompt);
    }
    inputs.extend(render);
    for (field, value) in body {
        match field.as_str() {
            "kind" | "render" | "description" | "writes" | "with" | "options" | "models" | "lora" | "instructions" => {}
            "image" | "video" | "audio" if value.as_str().is_some_and(|said| mentions(said).is_empty()) || mentions_in(value).is_empty() => {}
            "pieces" => drop(inputs.insert("tracks".to_owned(), tracks_of(value))),
            "beats" => {
                let mut beats = value.clone();
                beats.as_array_mut().into_iter().flatten().filter_map(Value::as_object_mut).for_each(|beat| drop(beat.remove("writes")));
                inputs.insert("beats".to_owned(), beats);
            }
            field => drop(inputs.insert(field.to_owned(), value.clone())),
        }
    }
    if let Some(field) = held {
        let hash = text_of(body.get(field));
        // A pin over an input left that in a comment, and now it's the input.
        if let Some(was) = pinned_over(text, field, &hash) {
            inputs.insert(field.to_owned(), json!(was));
        }
        out.insert("approved".to_owned(), json!(hash));
    }
    let description = text_of(body.get("description"));
    if !with.is_empty() && !description.is_empty() && !["instructions", "summary", "beats", "video"].iter().any(|field| inputs.contains_key(*field)) {
        // Named alone, a package could take these words for what to draw.
        inputs.insert("instructions".to_owned(), json!(description));
    }
    if !description.is_empty() {
        let said = versions.and_then(|versions| versions.last()?["invocations"][0]["input"]["prompt"].as_str());
        if !inputs.values().any(|value| value == description.as_str()) && said.is_some_and(|prompt| prompt.contains(description.trim())) {
            notes.push(format!("{name}: its description was in its prompt, and now goes only to what mentions it: move those words to its `instructions` to keep them there"));
        }
        out.insert("description".to_owned(), json!(description));
    }
    if !with.is_empty() {
        let made_by = versions.and_then(|versions| versions.last()?["made_by"]["with"].as_str());
        let h3 = names_package(&with, "minimax-h3");
        if h3 && (turbo == Some(false) || (turbo.is_none() && inputs.get("steps").is_some_and(|steps| !steps.is_null()))) {
            inputs.insert("turbo".to_owned(), json!(false));
        }
        let tool = tool_of(&with, kind, &inputs, turbo, made_by, folder, notes, name);
        filled_in(&mut inputs, section.filter(|_| own_with.is_none()), kind, &with, &tool, shared, notes);
        out.insert("tool".to_owned(), json!(tool));
    }
    out.extend(inputs);
    (out, kind.to_owned())
}

/// Each comment of the old file by the asset it sits on or before, under its old name key, those before anything, and those after everything.
/// A `#` in a `|` text is its text.
fn comments_of(text: &str) -> (Vec<String>, HashMap<String, Vec<String>>, Vec<String>) {
    let (mut head, mut before, mut pending): (Vec<String>, HashMap<String, Vec<String>>, Vec<String>) = (vec![], HashMap::new(), vec![]);
    let (mut section, mut current, mut block): (String, Option<String>, Option<usize>) = (String::new(), None, None);
    for line in text.lines() {
        let at = line.len() - line.trim_start_matches(' ').len();
        if block.is_some_and(|indent| at > indent || line.trim().is_empty()) {
            continue;
        }
        block = None;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            match &current {
                Some(name) if at > 2 => before.entry(name.clone()).or_default().push(trimmed.to_owned()),
                _ => pending.push(trimmed.to_owned()),
            }
            continue;
        }
        let (written, comment) = split_comment(trimmed);
        if written.ends_with(['|', '>']) || written.ends_with("|-") || written.ends_with(">-") || written.ends_with("|+") || written.ends_with(">+") {
            block = Some(at);
        }
        let key = written.split(':').next().unwrap_or(written).trim_matches(['"', '\'']);
        if at == 0 {
            if section.is_empty() {
                head.append(&mut pending);
            }
            section = key.to_owned();
            current = None;
        } else if at == 2 && (section == "assets" || section == "entities") {
            let name = name_key(key);
            before.entry(name.clone()).or_default().append(&mut pending);
            current = Some(name);
        }
        if let (Some(comment), Some(name)) = (comment.filter(|comment| !comment.starts_with("# was ")), &current) {
            before.entry(name.clone()).or_default().push(comment.to_owned());
        }
    }
    (head, before, pending)
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

/// The file whole, its comments back. Err: it wouldn't read back as what it says.
fn written(assets: &[(String, Map<String, Value>)], head: &[String], before: &HashMap<String, Vec<String>>, tail: &[String], format: u64) -> Result<String, Error> {
    let mut out = String::new();
    head.iter().for_each(|line| out.push_str(&format!("{line}\n")));
    out.push_str(&format!("version: {format}\n"));
    // No box is `{}`: a bare `assets:` reads back as null.
    out.push_str(if assets.is_empty() { "\nassets: {}\n" } else { "\nassets:\n" });
    for (at, (name, body)) in assets.iter().enumerate() {
        if at > 0 {
            out.push('\n');
        }
        before.get(&name_key(name)).into_iter().flatten().for_each(|line| out.push_str(&format!("  {line}\n")));
        match body.is_empty() {
            true => out.push_str(&format!("  {}: {{}}\n", yaml_key(name))),
            false => {
                out.push_str(&format!("  {}:\n", yaml_key(name)));
                ordered(body, &FIRST, &LAST).into_iter().for_each(|(field, value)| write_field(&mut out, field, value, 4));
            }
        }
    }
    tail.iter().for_each(|line| out.push_str(&format!("{line}\n")));
    let mut wanted = Map::from_iter([("version".to_owned(), json!(format))]);
    wanted.insert("assets".to_owned(), Value::Object(assets.iter().map(|(name, body)| (name.clone(), Value::Object(body.clone()))).collect()));
    let read = to_json_keeping(&out, names_a_file).map_err(|why| invalid("project_yaml", format!("what it would write: {why}")))?;
    (canonical(&read) == canonical(&Value::Object(wanted))).then_some(out).ok_or_else(|| invalid("project_edit", format!("{} wouldn't read back as what it says, so it is left as it is", folder::PROJECT)))
}

/// `fields` in the order they're written: `first`, then any other by name, then `last`.
fn ordered<'a>(fields: &'a Map<String, Value>, first: &[&str], last: &[&str]) -> Vec<(&'a String, &'a Value)> {
    let rank = |field: &str| first.iter().position(|named| *named == field).map(|at| (0, at)).or_else(|| last.iter().position(|named| *named == field).map(|at| (2, at))).unwrap_or((1, 0));
    let mut sorted: Vec<_> = fields.iter().collect();
    sorted.sort_by(|(a, _), (b, _)| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    sorted
}

/// `key: value` at `at`, on as many lines as it takes.
fn write_field(out: &mut String, key: &str, value: &Value, at: usize) {
    let head = format!("{}{}:", " ".repeat(at), yaml_key(key));
    match value {
        Value::Array(items) if !items.is_empty() => {
            out.push_str(&format!("{head}\n"));
            items.iter().for_each(|item| write_item(out, item, at + 2));
        }
        Value::Object(fields) if !fields.is_empty() => {
            out.push_str(&format!("{head}\n"));
            ordered(fields, &INSIDE, &[]).into_iter().for_each(|(field, value)| write_field(out, field, value, at + 2));
        }
        Value::String(text) if as_block(text) || key == TEXT => {
            out.push_str(&format!("{head} |\n"));
            write_text(out, text, at + 2);
        }
        value => out.push_str(&format!("{head} {}\n", inline(value))),
    }
}

/// A list's item at `at`: a mapping's first key on the dash's line.
fn write_item(out: &mut String, item: &Value, at: usize) {
    let pad = " ".repeat(at);
    match item {
        Value::Object(fields) if !fields.is_empty() => {
            let mut inner = String::new();
            ordered(fields, &INSIDE, &[]).into_iter().for_each(|(field, value)| write_field(&mut inner, field, value, at + 2));
            out.push_str(&format!("{pad}- {}", &inner[at + 2..]));
        }
        Value::String(text) if as_block(text) => {
            out.push_str(&format!("{pad}- |\n"));
            write_text(out, text, at + 2);
        }
        item => out.push_str(&format!("{pad}- {}\n", inline(item))),
    }
}

fn write_text(out: &mut String, text: &str, at: usize) {
    for line in text.trim().split('\n') {
        match line.is_empty() {
            true => out.push('\n'),
            false => out.push_str(&format!("{}{line}\n", " ".repeat(at))),
        }
    }
}

/// Whether `text` reads as itself with no quotes, on one line. A `:` is quoted, as `3:2` is a number to other YAML readers.
fn is_plain(text: &str) -> bool {
    !text.contains([':', '\n']) && to_json(&format!("k: {text}\n")).is_ok_and(|read| read["k"].as_str() == Some(text))
}

/// Whether `text` goes as `|`: several lines, or words that would need quotes. What YAML couldn't hold in that goes quoted.
fn as_block(text: &str) -> bool {
    let text = text.trim();
    (text.contains('\n') || (text.contains(' ') && !is_plain(text))) && !text.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
}

/// A value on one line: JSON is YAML.
fn inline(value: &Value) -> String {
    match value {
        Value::String(text) if is_plain(text.trim()) => text.trim().to_owned(),
        Value::String(text) => json!(text.trim()).to_string(),
        value => value.to_string(),
    }
}


/// Asset `name` and the lines of `text` where it says `<mention>`.
fn where_in(text: &str, name: &str, mention: &str) -> String {
    let (said, mut inside, mut lines) = (format!("<{mention}>"), false, vec![]);
    for (n, line) in text.lines().enumerate().filter(|(_, line)| !line.trim().is_empty()) {
        let at = line.len() - line.trim_start_matches(' ').len();
        if at <= 2 && !line.trim_start().starts_with('#') {
            inside = at == 2 && name_key(line.trim().split(':').next().unwrap_or_default().trim_matches(['"', '\''])) == name_key(name);
        } else if inside && line.contains(&said) {
            lines.push((n + 1).to_string());
        }
    }
    match lines.as_slice() {
        [] => name.to_owned(),
        [one] => format!("{name}, line {one}"),
        many => format!("{name}, lines {}", many.join(", ")),
    }
}

/// The old lock's rows as versions now are: what they keep, and an Edit's clips each with the file it cut.
fn shrunk(lock: &mut Value) {
    for asset in lock.get_mut("assets").and_then(Value::as_object_mut).into_iter().flat_map(|assets| assets.values_mut()) {
        for version in asset.get_mut("versions").and_then(Value::as_array_mut).into_iter().flatten() {
            let cut: HashMap<String, Value> = version["reads"].as_array().into_iter().flatten().filter_map(|read| {
                let files = read["files"].as_object()?;
                let main = ["video", "image", "audio"].iter().find_map(|role| files.get(*role)).or_else(|| files.values().next())?;
                Some((name_key(read["name"].as_str()?), main.clone()))
            }).collect();
            for clip in version.get_mut("clips").and_then(Value::as_array_mut).into_iter().flatten() {
                let file = clip["clip"].as_str().and_then(|clip| mentions(clip).first().map(|name| name_key(name))).and_then(|name| cut.get(&name).cloned());
                if let (Some(file), Some(clip)) = (file, clip.as_object_mut()) {
                    clip.insert("sha256".to_owned(), file);
                }
            }
            let Some(fields) = version.as_object_mut() else { continue };
            let run = fields.get("runs").and_then(|runs| runs.as_array()?.last().cloned());
            let with = fields.get("made_by").and_then(|by| by["with"].as_str()).filter(|with| !with.starts_with("cozy/")).map(str::to_owned);
            fields.extend(run.map(|run| ("run".to_owned(), run)));
            fields.extend(with.map(|with| ("tool".to_owned(), json!(with))));
            if let Some(Value::String(inputs)) = fields.get_mut("inputs") {
                inputs.truncate(16);
            }
        }
    }
}

/// The version `status` says is asset `name`'s state, if it stands.
fn current_of(row: Option<&Value>, name: &str, lock: &Lock, pin: Option<&str>, old: &HashMap<String, (u32, String)>) -> Option<u32> {
    let versions = lock.asset(name).map(|held| held.versions.as_slice()).unwrap_or_default();
    let Some(row) = row else {
        // With nothing said: what it approved, else its newest.
        return match pin {
            Some(pin) => versions.iter().rev().find(|version| version.main().is_some_and(|main| folder::hex_of(main).starts_with(pin))).map(|version| version.number),
            None => versions.last().map(|version| version.number),
        };
    };
    if row["standing"] != "current" {
        return None;
    }
    if let Some(number) = row["approved"].as_u64().or(row["version"].as_u64()) {
        return Some(number as u32);
    }
    let inputs = row["inputs"].as_str()?;
    old.get(inputs).map(|(number, _)| *number).filter(|number| versions.iter().any(|version| version.number == *number))
}

fn word_of(state: &State) -> &'static str {
    match state {
        State::Pinned { changed, .. } if !changed.is_empty() => "out_of_date",
        State::Stale(_) => "out_of_date",
        State::Missing | State::Waits => "missing",
        _ => "current",
    }
}

/// Held while a project moves, so that two touches of it make one move.
static MOVING: Mutex<()> = Mutex::new(());

/// The format of `text`, a project.yaml: 2 with no `version:` and `entities:`.
fn format_of(text: &str) -> Option<u64> {
    match text.lines().find_map(|line| line.strip_prefix("version:")) {
        Some(version) => version.split('#').next().and_then(|version| version.trim().parse().ok()),
        None => text.lines().any(|line| line.starts_with("entities:")).then_some(2),
    }
}

/// The first thing to touch `dir`'s project moves it, if it's format 2, as `cozy project upgrade` would. What to say of that, once:
/// nothing if there was nothing to move. What can't convert refuses, says why and is left as it is.
pub(super) fn on_touch(dir: &Path) -> Result<Vec<String>, Error> {
    let _one = MOVING.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(text) = std::fs::read_to_string(dir.join(folder::PROJECT)) else { return Ok(vec![]) };
    let (said, done) = match format_of(&text) {
        Some(2) => (format!("{} was format 2 and is format {FORMAT} now: the old files stay beside it as {WAS_YAML} and {WAS_LOCK}", folder::PROJECT), upgrade(dir, None)?),
        Some(3) => (format!("{} was format 3 and is format {FORMAT} now: the old files stay beside it as {WAS_THREE} and {WAS_THREE_LOCK}", folder::PROJECT), tools(dir)?),
        _ => return Ok(vec![]),
    };
    Ok(std::iter::once(said).chain(done.said).collect())
}

/// `dir`'s project, format 2, as format 3. `status`: what `cozy project status --json` said.
pub(super) fn upgrade(dir: &Path, status: Option<&str>) -> Result<Upgraded, Error> {
    let dir = holding(dir)?;
    let folder = Folder::at(&dir);
    let text = std::fs::read_to_string(dir.join(folder::PROJECT)).map_err(|e| io(&dir, e))?;
    let names_a_file_then = |path: &[&str]| matches!(path, ["entities", ..]) || matches!(path, ["assets", .., "image" | "video" | "audio"]);
    let top = to_json_keeping(&text, names_a_file_then).map_err(|why| refused("project_yaml", format!("{}: {why}", folder::PROJECT)))?;
    let Some(top) = top.as_object() else { return Err(refused("project_yaml", format!("{} is no mapping of sections", folder::PROJECT))) };
    match top.get("version").and_then(Value::as_u64) {
        Some(FORMAT) => return Ok(Upgraded { assets: vec![], said: vec![format!("{} is format {FORMAT} already: nothing to upgrade", folder::PROJECT)], current: true }),
        Some(3) => return tools(&dir),
        Some(2) | None => {}
        Some(other) => return Err(refused("project_format", format!("{} is format {other}, and this moves format 2", folder::PROJECT)).next("cozy project new")),
    }
    // A move cut short left the new lock beside the old: it starts from the old. A lock still of format 2 is the truth, whatever is beside it.
    let here = dir.join(folder::LOCK);
    let is_new = std::fs::read_to_string(&here).ok().and_then(|text| to_json(&text).ok()).is_some_and(|lock| lock["format"] == json!(folder::FORMAT));
    let lock_at = [here.clone(), dir.join(WAS_LOCK)].into_iter().filter(|at| !(is_new && *at == here)).find(|at| at.is_file());
    let mut old: Value = match &lock_at {
        Some(at) => {
            let read = std::fs::read_to_string(at).map_err(|e| io(at, e))?;
            serde_json::from_str(&read).map_err(|e| Error::new(Kind::Validation, "lock_unreadable", format!("{}: {e}", at.display())))?
        }
        None => json!({"format": 2}),
    };
    if old["pending"].as_object().is_some_and(|pending| !pending.is_empty()) || old["walk"]["asks"].as_array().is_some_and(|asks| !asks.is_empty()) {
        return Err(Error::new(Kind::Conflict, "project_busy", "it has runs on their way: let them land first").next("cozy project status"));
    }
    let was_lock: format2::Lock = serde_json::from_value(old.clone()).map_err(|e| Error::new(Kind::Validation, "lock_unreadable", format!("{}: {e}", folder::LOCK)))?;
    let was = format2::Doc::read(&dir, &text, top, &was_lock);
    let status: Option<Value> = status.map(serde_json::from_str).transpose().map_err(|e| refused("status_unreadable", format!("that isn't what `cozy project status --json` says: {e}")))?;
    let rows = match &status {
        Some(status) => status["assets"].as_array().cloned().unwrap_or_default(),
        None => was.rows(&was_lock),
    };
    let rows: BTreeMap<String, Value> = rows.into_iter().filter_map(|row| Some((name_key(row["name"].as_str()?), row))).collect();
    let mut said = vec![];

    let empty = Map::new();
    let old_defaults = top.get("defaults").and_then(Value::as_object).unwrap_or(&empty);
    let declared = top.get("assets").and_then(Value::as_object).unwrap_or(&empty);
    let entities = top.get("entities").and_then(Value::as_object).unwrap_or(&empty);
    let written_names = keys(&text, &["assets"]);
    let with_body: BTreeSet<String> = written_names.iter().filter(|name| declared.get(*name).is_some_and(|body| body.as_object().is_some_and(|body| !body.is_empty()))).map(|name| name_key(name)).collect();

    // Entities and forms fold into their sheets, and take their names.
    let mut folding = Folding::default();
    for name in keys(&text, &["entities"]) {
        let Some(body) = entities.get(&name).and_then(Value::as_object) else { continue };
        folding.links(&name, &name, body, &Said::default(), &with_body);
    }
    // A frame a beat wrote is a box of `cozy/frame`, of that shot at that time, as it was made: format 3 has no beat writes.
    let mut stills: Vec<(String, String, Map<String, Value>)> = (was.frames().into_iter())
        .filter(|(name, ..)| !with_body.contains(&name_key(name)))
        .map(|(name, shot, at)| (name_key(&name), name, Map::from_iter([("tool".to_owned(), json!(FRAME)), ("video".to_owned(), json!(format!("<{shot}>"))), ("at".to_owned(), json!(at))])))
        .collect();
    // A still no beat writes now, and something reads, is a file.
    let said_anywhere: BTreeSet<String> = mentions_in(&json!(declared)).iter().chain(mentions_in(&json!(old_defaults)).iter()).map(|name| name_key(name)).collect();
    let framed: BTreeSet<String> = stills.iter().map(|(key, ..)| key.clone()).collect();
    for (key, held) in old["assets"].as_object().into_iter().flatten().filter(|(key, _)| !framed.contains(*key)) {
        let newest = held["versions"].as_array().and_then(|versions| versions.last());
        let is_still = newest.is_some_and(|newest| newest["made_by"]["with"] == FRAME);
        let Some(main) = newest.and_then(|newest| newest["files"]["image"].as_str()).filter(|_| is_still && !with_body.contains(key)) else { continue };
        if !written_names.iter().any(|name| name_key(name) == *key) && !said_anywhere.contains(key) {
            continue;
        }
        let shown = held["name"].as_str().filter(|name| !name.is_empty()).unwrap_or(key).to_owned();
        stills.push((key.clone(), shown, Map::from_iter([("approved".to_owned(), json!(folder::prefix_of(main)))])));
    }

    let renames = folding.renames.clone();
    let renamed = |name: &str| renames.get(&name_key(name)).cloned();
    let mut assets: Vec<(String, Map<String, Value>)> = std::mem::take(&mut folding.made);
    let mut notes = std::mem::take(&mut folding.notes);
    let taken: Vec<String> = written_names.iter().cloned().chain(renames.keys().cloned()).chain(renames.values().cloned()).chain(stills.iter().map(|(_, new, _)| new.clone())).collect();
    let shared = Shared::of(old_defaults, taken);
    assets.splice(0..0, shared.assets.iter().cloned());
    assets.extend(stills.iter().map(|(_, new, body)| (new.clone(), body.clone())));
    let mut kinds: Vec<(String, String)> = vec![];
    for name in &written_names {
        let key = name_key(name);
        if !with_body.contains(&key) {
            // Saying nothing, it had nothing to make, and a still's is written above.
            if !stills.iter().any(|(still, ..)| *still == key) {
                notes.push(format!("{name} said nothing: it is gone from the file"));
            }
            continue;
        }
        let Some(body) = declared.get(name).and_then(Value::as_object) else { continue };
        let (mut asset, kind) = asset_of(name, body, old_defaults, &text, &old, &folder, &shared, &mut notes);
        if let Some(tool) = asset.get("tool").and_then(Value::as_str) {
            kinds.push((plain_tool(tool, &folder), kind));
        }
        if let Some(held) = folding.said_of.get(&key) {
            if !held.description.is_empty() {
                asset.insert("description".to_owned(), json!(held.description));
            }
            if !held.asset_type.is_empty() {
                asset.insert("asset_type".to_owned(), json!(held.asset_type));
            }
        }
        assets.push((renamed(name).unwrap_or_else(|| name.clone()), asset));
    }
    // Voices, once every name is known. A voice by its hash is a file of its own.
    let mut voices = vec![];
    for (key, held) in &folding.said_of {
        let Some(voice) = &held.voice else { continue };
        let owner = key.strip_prefix('\0').map_or_else(|| renamed(key).unwrap_or_else(|| key.clone()), str::to_owned);
        let target = match voice {
            Link::Asset(asset) => renamed(asset).unwrap_or_else(|| asset.clone()),
            Link::Hash(hash) => {
                let made = format!("{owner}.voice");
                voices.push((made.clone(), Map::from_iter([("approved".to_owned(), json!(hash))])));
                made
            }
        };
        if let Some((_, body)) = assets.iter_mut().find(|(name, _)| name_key(name) == name_key(&owner)) {
            body.insert("voice".to_owned(), json!(format!("<{target}>")));
        }
    }
    assets.extend(voices);
    let mut seen: HashMap<String, &str> = HashMap::new();
    let twice: Vec<String> = assets.iter().filter_map(|(name, _)| Some(format!("{} and {name}", seen.insert(name_key(name), name)?))).collect();
    if !twice.is_empty() {
        let said = format!("{} are one name, as a name matches in any case, with `_` and `-` alike", twice.join(", "));
        return Err(refused("project_name_twice", said).remedy(format!("rename one in {}, then upgrade again", folder::PROJECT)));
    }
    for (_, body) in &mut assets {
        // A voice is under its new name already.
        let voice = body.remove("voice");
        body.values_mut().for_each(|value| mentions_renamed(value, &renamed));
        body.extend(voice.map(|voice| ("voice".to_owned(), voice)));
    }
    // Comments come along, each on the asset it sat on, whatever that is called now.
    let (head, mut before, tail) = comments_of(&text);
    for (from, to) in &renames {
        if let Some(lines) = before.remove(from) {
            before.entry(name_key(to)).or_default().extend(lines);
        }
    }
    let new_text = written(&assets, &head, &before, &tail, 3)?;

    // The lock under the new names, in what a version keeps. Only the versions that stand get the new key.
    let was_key: HashMap<String, (u32, String)> = old["assets"].as_object().into_iter().flatten().flat_map(|(key, held)| {
        let key = key.clone();
        held["versions"].as_array().cloned().unwrap_or_default().into_iter().filter_map(move |version| Some((version["inputs"].as_str()?.to_owned(), (version["number"].as_u64()? as u32, key.clone()))))
    }).collect();
    let measured: Map<String, Value> = old["files"].as_object().into_iter().flatten().filter_map(|(sha256, file)| {
        let keep = ["width", "height", "fps", "frames", "alpha", "duration_ms"].iter().filter_map(|field| Some(((*field).to_owned(), file.get(*field)?.clone()))).collect::<Map<_, _>>();
        (!keep.is_empty()).then(|| (sha256.clone(), Value::Object(keep)))
    }).collect();
    let paths: BTreeMap<String, String> = old["files"].as_object().into_iter().flatten().filter_map(|(sha256, file)| Some((sha256.clone(), file["path"].as_str()?.to_owned()))).collect();
    shrunk(&mut old);
    old["format"] = json!(folder::FORMAT);
    let mut lock: Lock = serde_json::from_value(old).map_err(|e| Error::new(Kind::Validation, "lock_unreadable", format!("{}: {e}", folder::LOCK)))?;
    for (key, mut held) in std::mem::take(&mut lock.assets) {
        let shown = renamed(&key).or_else(|| renamed(&held.name)).unwrap_or_else(|| if held.name.is_empty() { key.clone() } else { held.name.clone() });
        for version in &mut held.versions {
            for left in &mut version.without {
                if let Some(to) = renamed(&left.piece) {
                    left.piece = to;
                }
            }
            for clip in &mut version.clips {
                let mut value = json!(clip.clip);
                mentions_renamed(&mut value, &renamed);
                clip.clip = value.as_str().unwrap_or_default().to_owned();
            }
        }
        held.name = shown.clone();
        lock.assets.insert(name_key(&shown), held);
    }
    folder.fill(&mut lock, &BTreeMap::new());
    // What each is, while this reads with nothing here to say.
    for (sha256, path) in &paths {
        if let Some(file) = lock.files.get_mut(sha256) {
            file.path.clone_from(path);
        }
    }
    lock.makes.extend(kinds);
    let old_reserved = std::mem::take(&mut lock.reserved);
    let doc = Doc::of_three(&folder, &new_text, &lock)?;
    for asset in &doc.assets {
        let Some(tool) = &asset.tool else { continue };
        let package = tool.split('/').next().unwrap_or(tool);
        if let Some(tags) = old_reserved.get(package) {
            lock.reserved.insert(tool.clone(), tags.clone());
        }
    }
    let doc = Doc::of_three(&folder, &new_text, &lock)?;
    // What its status said, under whichever name it had.
    let was_of = |name: &str| renames.iter().filter(|(_, new)| name_key(new) == name_key(name)).map(|(old, _)| old).find(|old| rows.contains_key(*old)).cloned();
    let row_of = |name: &str| rows.get(&was_of(name).unwrap_or_else(|| name_key(name)));
    let current: BTreeMap<String, u32> = (doc.assets.iter())
        .filter_map(|asset| {
            let pin = asset.pin.as_deref().map(|pin| &folder::hex_of(pin)[..12]);
            Some((name_key(&asset.name), current_of(row_of(&asset.name), &asset.name, &lock, pin, &was_key)?))
        })
        .collect();
    doc.stamped(&mut lock, &current);

    // Old files aside, then the lock, the canvas and last the file, each whole. Until the file is replaced it's format 2, and this starts again
    // from the originals, which are kept at every step.
    let yaml_at = dir.join(folder::PROJECT);
    if !dir.join(WAS_YAML).is_file() {
        platform::durable::replace(&dir.join(WAS_YAML), text.as_bytes(), 0o644).map_err(|e| io(&dir, e))?;
    }
    if let Some(at) = lock_at.filter(|at| !at.ends_with(WAS_LOCK)) {
        let bytes = std::fs::read(&at).map_err(|e| io(&at, e))?;
        platform::durable::replace(&dir.join(WAS_LOCK), &bytes, 0o644).map_err(|e| io(&dir, e))?;
    }
    platform::durable::replace(&dir.join(MEASURED), json!(measured).to_string().as_bytes(), 0o644).map_err(|e| io(&dir, e))?;
    platform::durable::replace(&dir.join(folder::LOCK), lock.written().as_bytes(), 0o644).map_err(|e| io(&dir, e))?;
    legacy::canvas_renamed(&dir, &renamed)?;
    platform::durable::replace(&yaml_at, new_text.as_bytes(), 0o644).map_err(|e| io(&dir, e))?;

    said.extend(notes);
    // Format 2 had no `<name.2>`, and format 3 has no beat writes: a mention read as a version, or of what a beat wrote, means another thing now.
    let mut told: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for i in 0..doc.assets.len() {
        let name = &doc.assets[i].name;
        let gone = doc.missing_names(i, &doc.tags(i).cloned().unwrap_or_default(), &lock);
        let mut moved: Vec<(&str, String)> = vec![];
        for read in &doc.assets[i].reads {
            let Some(number) = read.number else { continue };
            let none = if gone.contains(&read.said) { ", which it hasn't: edit its text" } else { "" };
            moved.push((read.said.as_str(), format!("now reads as {}'s version {number}{none}", doc.assets[read.asset].name)));
        }
        let unwritten: Vec<&str> = gone.iter().map(String::as_str).filter(|mention| was.beat_of(mention).is_some() && !moved.iter().any(|(said, _)| said == mention)).collect();
        moved.extend(unwritten.into_iter().map(|mention| (mention, "now names nothing: edit its text".to_owned())));
        for (mention, now) in &moved {
            said.push(format!("{}: <{mention}> {}, and {now}", where_in(&new_text, name, mention), was.meant(mention)));
        }
        told.entry(name_key(name)).or_default().extend(moved.iter().map(|(mention, _)| (*mention).to_owned()));
    }

    // Then on to format 4, each state carried, and reported as a status reads it now, from what is on disk.
    let four = four(&dir, &told)?;
    said.extend(four.said);
    let assets = four.assets.into_iter().map(|row| {
        let then = was_of(&row.name).or_else(|| (!row.was.is_empty()).then(|| row.was.clone()));
        let before = row_of(&row.name).and_then(|held| held["standing"].as_str()).unwrap_or("new").to_owned();
        Row { was: then.unwrap_or_default(), before, ..row }
    });
    Ok(Upgraded { assets: assets.collect(), said, current: false })
}

/// Each of H3's edits: the LoRA that is it, and the word its prompt opened with (H3's `EDITS`).
const H3_EDITS: [(&str, &str, &str); 5] = [
    ("character_swap", "alissonerdx/minimax-h3-character-swap", "body_swap:"),
    ("head_swap", "alissonerdx/minimax-h3-head-swap", "head_swap:"),
    ("style_transfer", "alissonerdx/minimax-h3-style-transfer", "style_transfer:"),
    ("vfx_edit", "alissonerdx/minimax-h3-vfx-edit", "vfx_edit:"),
    ("motion_transfer", "alissonerdx/minimax-h3-motion-transfer", "motion_transfer:"),
];

/// `dir`'s project, format 3, as format 4 (design/tools.md): each box's state as format 3 read it carried over, so nothing goes out of
/// date and nothing runs, and what format 4 reads another way said once. The old files stay beside it.
pub fn tools(dir: &Path) -> Result<Upgraded, Error> {
    four(dir, &BTreeMap::new())
}

/// `tools`, less the mentions `told` has said already, by box.
fn four(dir: &Path, told: &BTreeMap<String, BTreeSet<String>>) -> Result<Upgraded, Error> {
    let dir = holding(dir)?;
    let folder = Folder::at(&dir);
    let (text, mut lock) = legacy::files_of(&folder)?;
    if !lock.pending.is_empty() || !lock.walk.asks.is_empty() {
        return Err(Error::new(Kind::Conflict, "project_busy", "it has runs on their way: let them land first").next("cozy project status"));
    }
    // What stood, as format 3 read it.
    let was = Doc::of_three(&folder, &text, &lock)?;
    let stood = was.stand(&lock);
    let then: HashMap<String, (State, Option<u32>)> = was.assets.iter().zip(&stood).map(|(asset, standing)| (name_key(&asset.name), (standing.state.clone(), standing.version()))).collect();
    let top = to_json_keeping(&text, names_a_file).map_err(|why| refused("project_yaml", format!("{}: {why}", folder::PROJECT)))?;
    let empty = Map::new();
    let declared = top.get("assets").and_then(Value::as_object).unwrap_or(&empty);
    let names = keys(&text, &["assets"]);
    let project = dir.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());

    // A dotted name is `_` now, and so is every mention of it.
    let mut taken: BTreeSet<String> = names.iter().map(|name| name_key(name)).collect();
    let mut renames: BTreeMap<String, String> = BTreeMap::new();
    for name in names.iter().filter(|name| name.contains('.')) {
        let plain = name.replace('.', "_");
        let new = (1..).map(|n| if n == 1 { plain.clone() } else { format!("{plain}_{n}") }).find(|new| !taken.contains(&name_key(new))).expect("a free name");
        taken.insert(name_key(&new));
        renames.insert(name_key(name), new);
    }
    let renamed = |inner: &str| renames.get(&name_key(inner)).cloned();
    let (mut said, mut typed, mut assets) = (vec![], vec![], vec![]);
    for name in &names {
        let mut body = declared.get(name).and_then(Value::as_object).cloned().unwrap_or_default();
        let new = renamed(name).unwrap_or_else(|| name.clone());
        if body.remove("kind").is_some() | body.remove("asset_type").is_some() {
            typed.push(new.clone());
        }
        if body.remove("voice").is_some() {
            said.push(format!("{new}'s voice: a voice is a sound box of its own now, mentioned where it speaks"));
        }
        retooled(&new, &mut body, &mut said);
        body.values_mut().for_each(|value| mentions_renamed(value, &renamed));
        assets.push((new, body));
    }
    if !typed.is_empty() {
        let are = if typed.len() == 1 { "is" } else { "are" };
        said.push(format!("{project}: {} {are} now a picture reference like any other", typed.join(", ")));
    }
    for name in names.iter().filter(|name| name.contains('.')) {
        said.push(format!("{name} is {}: a name holds no `.` now", renamed(name).unwrap_or_default()));
    }
    let (head, mut before, tail) = comments_of(&text);
    for (from, to) in &renames {
        if let Some(lines) = before.remove(from) {
            before.entry(name_key(to)).or_default().extend(lines);
        }
    }
    let new_text = written(&assets, &head, &before, &tail, FORMAT)?;

    // The lock under the new names; each version that stood gets its new key.
    for (from, to) in &renames {
        let shown = lock.asset(from).map_or_else(|| from.clone(), |held| held.name.clone());
        super::renamed(&mut lock, &shown, to);
    }
    let doc = Doc::of_text(&folder, &new_text, &lock)?;
    let was_of = |name: &str| renames.iter().find(|(_, new)| name_key(new) == name_key(name)).map_or_else(|| name_key(name), |(old, _)| old.clone());
    let current: BTreeMap<String, u32> = (doc.assets.iter())
        .filter_map(|asset| match then.get(&was_of(&asset.name))? {
            (State::Current(number), _) => Some((name_key(&asset.name), *number)),
            (State::Pinned { version: Some(number), changed }, _) if changed.is_empty() => Some((name_key(&asset.name), *number)),
            _ => None,
        })
        .collect();
    doc.stamped(&mut lock, &current);

    // Each kept once: a move cut short between the two still keeps the other.
    if !dir.join(WAS_THREE_LOCK).is_file() {
        let held = std::fs::read(dir.join(folder::LOCK)).unwrap_or_default();
        platform::durable::replace(&dir.join(WAS_THREE_LOCK), &held, 0o644).map_err(|e| io(&dir, e))?;
    }
    if !dir.join(WAS_THREE).is_file() {
        platform::durable::replace(&dir.join(WAS_THREE), text.as_bytes(), 0o644).map_err(|e| io(&dir, e))?;
    }
    platform::durable::replace(&dir.join(folder::LOCK), lock.written().as_bytes(), 0o644).map_err(|e| io(&dir, e))?;
    legacy::canvas_renamed(&dir, &|name: &str| renamed(name))?;
    platform::durable::replace(&dir.join(folder::PROJECT), new_text.as_bytes(), 0o644).map_err(|e| io(&dir, e))?;

    let (text, lock) = legacy::files_of(&folder)?;
    let doc = Doc::of_text(&folder, &text, &lock)?;
    let now = doc.stand(&lock);
    for i in 0..doc.assets.len() {
        let told = told.get(&was_of(&doc.assets[i].name));
        let gone: Vec<String> = doc.missing_names(i, &doc.tags(i).cloned().unwrap_or_default(), &lock).into_iter().filter(|name| told.is_none_or(|told| !told.contains(name))).collect();
        if !gone.is_empty() {
            let named: Vec<String> = gone.iter().map(|name| format!("<{name}>")).collect();
            said.push(format!("{} mentions {}, which no version wrote and nothing makes: edit its text", doc.assets[i].name, named.join(", ")));
        }
    }
    said.extend(doc.warnings.iter().map(|warning| warning.message.clone()));
    let rows = (0..doc.assets.len()).map(|i| {
        let name = doc.assets[i].name.clone();
        let old = was_of(&name);
        let before = then.get(&old).map_or("new", |(state, _)| word_of(state)).to_owned();
        let was = renames.get(&old).map(|_| was.assets.iter().find(|asset| name_key(&asset.name) == old).map_or(old.clone(), |asset| asset.name.clone())).unwrap_or_default();
        Row { was, before, after: word_of(&now[i].state).to_owned(), version: now[i].version(), name }
    });
    Ok(Upgraded { assets: rows.collect(), said, current: false })
}

/// A format 3 box's tool and fields as format 4's: Qwen's two functions one tool with Turbo, a shot with a plate an Edit, what's no longer
/// a field said and dropped.
fn retooled(name: &str, body: &mut Map<String, Value>, said: &mut Vec<String>) {
    let Some(tool) = body.get("tool").and_then(Value::as_str).map(str::to_owned) else { return };
    let Some((head, function)) = tool.rsplit_once('/') else { return };
    let (function, release) = function.split_once('@').map_or((function, None), |(function, release)| (function, Some(release)));
    let package = api::calcifer::tool::package_word(head);
    let tool_of = |function: &str| format!("{head}/{function}{}", release.map_or_else(String::new, |release| format!("@{release}")));
    let dropped = |body: &mut Map<String, Value>, fields: &[&str], why: &str, said: &mut Vec<String>| {
        for field in fields {
            if body.remove(*field).is_some() {
                said.push(format!("{name}'s {field}: {why}: it's dropped"));
            }
        }
    };
    match (package, function) {
        ("qwen-image-2", "generate") => {
            body.insert("tool".into(), json!(tool_of("image")));
            body.insert("turbo".into(), json!(false));
        }
        ("qwen-image-2", "generate_turbo") => drop(body.insert("tool".into(), json!(tool_of("image")))),
        ("minimax-h3", "generate") if body.contains_key("plate") => {
            body.insert("tool".into(), json!(tool_of("edit")));
            dropped(body, &["summary", "soundscape", "music", "aspect_ratio", "first_frame", "last_frame", "context_frames"], "MiniMax H3 Edit follows its plate and has none", said);
            dropped(body, &["continue_from"], "a pickup isn't written; use Pick up from here", said);
            dropped(body, &["resolution", "upscale"], "upscaling is MiniMax H3 Upscale, a box of its own", said);
            dropped(body, &["references", "preset", "steps"], "MiniMax H3 Edit reads none", said);
            match lora_edit(body) {
                Some((edit, trigger)) => {
                    body.insert("edit".into(), json!(edit));
                    for beat in body.get_mut("beats").and_then(Value::as_array_mut).into_iter().flatten() {
                        if let Some(text) = beat["description"].as_str().and_then(|text| text.trim_start().strip_prefix(trigger)) {
                            beat["description"] = json!(text.trim_start());
                        }
                    }
                }
                None => said.push(format!("{name}: its lora is none of MiniMax H3's edits: choose its Edit")),
            }
        }
        ("minimax-h3", "generate") => {
            dropped(body, &["continue_from"], "a pickup isn't written; use Pick up from here", said);
            dropped(body, &["resolution", "upscale"], "upscaling is MiniMax H3 Upscale, a box of its own", said);
            dropped(body, &["references", "preset", "steps"], "MiniMax H3 reads none now", said);
            if let Some(frames) = body.remove("context_frames") {
                let held = [22, 39, 56].into_iter().min_by_key(|carried| (frames.as_i64().unwrap_or(22) - carried).abs()).unwrap_or(22);
                body.insert("carried_frames".into(), json!(held));
            }
        }
        ("minimax-h3", "upscale") => {
            let seedvr2 = body.get("upscale").and_then(|upscale| upscale["model"].as_str()) == Some("seedvr2");
            if seedvr2 {
                let parent = head.rsplit_once('/').map_or(String::new(), |(parent, _)| format!("{parent}/"));
                body.insert("tool".into(), json!(format!("{parent}seedvr2/upscale")));
                body.remove("upscale");
                dropped(body, &["prompt", "latents", "preset"], "SeedVR2 reads none", said);
                return;
            }
            if let Some(strength) = body.get("upscale").and_then(|upscale| upscale.get("strength")).filter(|strength| strength.is_number()).cloned() {
                body.insert("strength".into(), strength);
            }
            body.remove("upscale");
            if let Some(resolution) = body.remove("resolution") {
                match resolution.as_str().filter(|to| ["1080p", "1440p", "2160p"].contains(to)) {
                    Some(to) => drop(body.insert("to".into(), json!(to))),
                    None => said.push(format!("{name}'s resolution: {resolution} is no size MiniMax H3 Upscale goes to: it's 1440p")),
                }
            }
            dropped(body, &["prompt", "latents", "preset", "seed"], "MiniMax H3 Upscale reads its take's own", said);
        }
        _ => {}
    }
}

/// The edit a box's `lora` was, and its word, the LoRA taken off: its choice is the edit now.
fn lora_edit(body: &mut Map<String, Value>) -> Option<(&'static str, &'static str)> {
    let repo = |value: &Value| value.as_str().or_else(|| value["ref"].as_str()).map(|text| text.split('@').next().unwrap_or(text).to_ascii_lowercase());
    let found = |value: &Value| H3_EDITS.iter().find(|(_, model, _)| repo(value).as_deref() == Some(model));
    let edit = match body.get_mut("lora")? {
        Value::Array(loras) => {
            let at = loras.iter().position(|lora| found(lora).is_some())?;
            let edit = found(&loras[at]).copied();
            loras.remove(at);
            edit
        }
        one => found(one).copied(),
    }?;
    if body.get("lora").is_none_or(|lora| !lora.is_array() || lora.as_array().is_some_and(Vec::is_empty)) {
        body.remove("lora");
    }
    Some((edit.0, edit.2))
}
