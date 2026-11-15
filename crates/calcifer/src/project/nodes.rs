//! A project's boxes as the agent API's file holds them (tracker design/agent-api.md): each box a node, its tool's fields grouped under
//! the tool's id with every default filled from its package's declarations, `approved` a take number, its takes, and the rev it was
//! read at. Calcifer stores the box as the page writes it; this only regroups it. `GET /v1/projects/{id}/nodes` reads nodes.
use std::{
    collections::{BTreeSet, HashMap},
    path::Path as FsPath,
    sync::Arc,
};

use api::{
    calcifer::{
        node::{self, Nodes},
        project::TakeNow,
        tool::{FieldType, Tool, ToolField},
    },
    error::{Error, Kind},
};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{
    folder::{self, Folder, Lock},
    format::{self, NODE_KIND, SELECTED_VERSION, STARRED, TEXT},
    graph::name_key,
    written::{Boxed, Written},
};
use crate::api::{Api, Refusal};

/// What a box holds that is no field of its tool: the node's own keys say these.
const OWN: [&str; 7] = ["tool", "approved", "description", "text", NODE_KIND, SELECTED_VERSION, STARRED];

/// A box as its node says it.
#[derive(Clone, Debug)]
pub(super) struct View {
    pub name: String,
    /// The box's own id: its words are text at a path of it.
    pub id: String,
    /// The node's kind: an Edit's `edit`, else its file's.
    pub kind: String,
    /// Its tool as its box writes it.
    pub tool: Option<String>,
    pub description: String,
    pub text: Option<String>,
    /// Its tool's fields, `models` and `lora` as the box holds them, an Edit's `tracks` as the wire's `timeline`.
    pub fields: Map<String, Value>,
    pub approved: Option<u32>,
}

/// Box `boxed` as its node says it, of the files and versions `lock` holds.
pub(super) fn view(boxed: &Boxed, lock: &Lock, folder: &Folder) -> View {
    let held = boxed.map();
    let text = |field: &str| held.get(field).and_then(Value::as_str).map(str::to_owned);
    let tool = text("tool").map(|tool| tool.trim().to_owned()).filter(|tool| !tool.is_empty());
    let plain = tool.as_deref().map(|tool| format::plain_tool(tool, folder));
    let pin = format::approved_file(held.get("approved"), lock);
    let versions = lock.asset(&boxed.name).map(|asset| asset.versions.as_slice()).unwrap_or_default();
    let selected = format::selected_version(held.get(SELECTED_VERSION)).ok().flatten();
    let approved = pin.as_ref().and_then(|pin| {
        let chosen = selected.and_then(|number| versions.iter().find(|version| version.number == number && version.main() == Some(pin)));
        chosen.or_else(|| versions.iter().rev().find(|version| version.main() == Some(pin))).map(|version| version.number)
    });
    // As the reader says it (`Doc::declare`): what its tool makes here, its kind, its approved file, its newest take, what its tool made.
    let newest = versions.last().and_then(|version| format::KINDS.iter().copied().find(|kind| version.files.contains_key(*kind)));
    let of_file = pin.as_ref().and_then(|sha256| lock.files.get(sha256)).and_then(folder::File::media);
    let makes = plain.as_ref().and_then(|plain| lock.makes.get(plain)).and_then(|kind| format::KINDS.iter().copied().find(|known| known == kind));
    let kinded = format::node_kind(held.get(NODE_KIND)).ok().flatten().map(format::node_media);
    let words = tool.is_none() && pin.is_none() && versions.is_empty() && kinded.is_none_or(|kind| kind == TEXT);
    let media = plain.as_deref().and_then(format::kind_made_here).or(kinded).or(of_file).or(newest).or(makes)
        .or(words.then_some(TEXT)).unwrap_or("image");
    let mut fields: Map<String, Value> = held.iter().filter(|(field, _)| !OWN.contains(&field.as_str())).map(|(field, value)| (field.clone(), value.clone())).collect();
    if let Some(tracks) = fields.remove("tracks") {
        let timeline = api::calcifer::project::Timeline { tracks: vec![api::calcifer::project::Track { clips: super::tools::clips_of(&tracks) }] };
        fields.insert("timeline".to_owned(), serde_json::to_value(timeline).expect("clips serialize"));
    }
    View {
        name: boxed.name.clone(),
        id: boxed.id.clone(),
        kind: node::kind_of(media, plain.as_deref()).to_owned(),
        text: (media == TEXT && tool.is_none()).then(|| text(TEXT).unwrap_or_default()),
        tool,
        description: text("description").map(|words| words.trim().to_owned()).unwrap_or_default(),
        fields,
        approved,
    }
}

/// One take as a node lists it: a version that landed, or one asked and not landed.
#[derive(Clone, Debug)]
pub(super) struct Take {
    pub number: Option<u32>,
    pub state: &'static str,
    pub file: Option<String>,
    pub run: Option<u64>,
}

impl Take {
    fn value(&self) -> Value {
        let mut take = Map::new();
        if let Some(number) = self.number { take.insert("number".to_owned(), json!(number)); }
        take.insert("state".to_owned(), json!(self.state));
        if let Some(file) = &self.file { take.insert("file".to_owned(), json!(file)); }
        if let Some(run) = self.run.filter(|_| self.state != "done") { take.insert("run".to_owned(), json!(run)); }
        Value::Object(take)
    }
}

/// Asset `name`'s takes: each version that landed, its main file whole, then what failed, what runs and what waits.
pub(super) fn takes(name: &str, lock: &Lock, dir: &FsPath, going: &[TakeNow]) -> Vec<Take> {
    let held = lock.asset(name);
    let file = |sha256: &String| lock.files.get(sha256).map(|file| {
        let path = FsPath::new(&file.path);
        if path.is_absolute() { file.path.clone() } else { dir.join(path).to_string_lossy().into_owned() }
    });
    let mut takes: Vec<Take> = held.into_iter().flat_map(|held| &held.versions)
        .map(|version| Take { number: Some(version.number), state: "done", file: version.main().and_then(file), run: version.run })
        .collect();
    takes.extend(held.into_iter().flat_map(|held| &held.ended).map(|ended| Take { number: None, state: "failed", file: None, run: ended.run }));
    let (running, queued): (Vec<&TakeNow>, Vec<&TakeNow>) = going.iter().filter(|take| take.failure.is_none()).partition(|take| !take.queued);
    takes.extend(running.into_iter().map(|take| Take { number: None, state: "running", file: None, run: take.run }));
    takes.extend(queued.into_iter().map(|_| Take { number: None, state: "queued", file: None, run: None }));
    takes
}

/// Which of a node's takes it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shown {
    /// The approved, the newest, and any queued or running.
    Some,
    All,
    One(u32),
}

/// What `takes` the file shows, and how many of the rest it counts in its comment.
fn shown(takes: &[Take], approved: Option<u32>, which: Shown) -> (Vec<&Take>, usize) {
    let newest = takes.iter().filter_map(|take| take.number).max();
    let kept = |take: &&Take| match which {
        Shown::All => true,
        Shown::One(number) => take.number == Some(number),
        Shown::Some => matches!(take.state, "running" | "queued") || take.number.is_some_and(|number| Some(number) == approved || Some(number) == newest),
    };
    let shown: Vec<&Take> = takes.iter().filter(kept).collect();
    let older = match which {
        Shown::Some => takes.iter().filter(|take| take.number.is_some()).count() - shown.iter().filter(|take| take.number.is_some()).count(),
        _ => 0,
    };
    (shown, older)
}

/// Its node: `kind`, `description` (and a text node's `text`), its tool block, `approved`, its takes, and `rev`.
pub(super) fn node(view: &View, tool: Option<&Tool>, takes: &[Take], which: Shown, rev: &str) -> (Value, usize) {
    let mut node = Map::new();
    node.insert("kind".to_owned(), json!(view.kind));
    node.insert("description".to_owned(), json!(view.description));
    if let Some(text) = &view.text {
        node.insert(TEXT.to_owned(), json!(text));
    }
    if let Some(id) = &view.tool {
        node.insert(id.clone(), Value::Object(block(&view.fields, tool)));
    }
    let mut older = 0;
    if view.kind != TEXT {
        node.insert("approved".to_owned(), json!(view.approved));
        let (listed, counted) = shown(takes, view.approved, which);
        older = counted;
        node.insert("takes".to_owned(), Value::Array(listed.into_iter().map(Take::value).collect()));
    }
    node.insert("rev".to_owned(), json!(rev));
    (Value::Object(node), older)
}

/// A tool block: each field of `tool` as the box holds it, else its default; each model slot; `loras`; then whatever else the box holds.
pub(super) fn block(fields: &Map<String, Value>, tool: Option<&Tool>) -> Map<String, Value> {
    let mut block = Map::new();
    let models = fields.get("models").and_then(Value::as_object);
    let declared: Vec<&ToolField> = tool.map(|tool| tool.fields.iter().collect()).unwrap_or_default();
    for field in &declared {
        block.insert(field.field.clone(), filled(field, fields.get(&field.field)));
    }
    let slot = |entry: &Value| match entry.get("lora").filter(|lora| lora.as_array().is_some_and(|lora| !lora.is_empty())) {
        Some(lora) => json!({"ref": entry.get("ref").cloned().unwrap_or(Value::Null), node::LORAS: lora}),
        None => entry.get("ref").cloned().unwrap_or(Value::Null),
    };
    // A slot by its parameter, as the box names it, else by a child call's path to it.
    let named = |key: &str| models.and_then(|models| models.get(key).or_else(|| models.iter().find(|(path, _)| path.ends_with(&format!(".models.{key}"))).map(|(_, entry)| entry)));
    let slots = tool.map(node::model_slots).unwrap_or_default();
    for (key, default) in &slots {
        let held = named(key).map(slot).filter(|held| !held.is_null());
        block.insert(key.clone(), held.unwrap_or_else(|| default.clone().map_or(Value::Null, Value::String)));
    }
    let covered = |path: &str| slots.iter().any(|(key, _)| path == key || path.ends_with(&format!(".models.{key}")));
    let undeclared: Vec<(String, Value)> = models.into_iter().flatten().filter(|(name, _)| !covered(name) && !block.contains_key(*name)).map(|(name, entry)| (name.clone(), slot(entry))).collect();
    block.extend(undeclared);
    block.insert(node::LORAS.to_owned(), fields.get("lora").filter(|lora| !lora.is_null()).cloned().unwrap_or_else(|| json!([])));
    let held: Vec<(String, Value)> = fields.iter().filter(|(field, _)| !matches!(field.as_str(), "models" | "lora") && !block.contains_key(*field)).map(|(field, value)| (field.clone(), value.clone())).collect();
    block.extend(held);
    block
}

/// `field` as a box holding `held` has it, its defaults filled in, as deep as its groups go.
pub(super) fn filled(field: &ToolField, held: Option<&Value>) -> Value {
    let held = held.filter(|held| !held.is_null());
    match (field.ty, held) {
        (FieldType::Group, held) => {
            let value = held.cloned().unwrap_or_else(|| node::unset(field));
            let row = |row: &Value| match row {
                Value::Object(members) => {
                    let mut out: Map<String, Value> = field.members.iter().map(|member| (member.field.clone(), filled(member, members.get(&member.field)))).collect();
                    let held: Vec<(String, Value)> = members.iter().filter(|(key, _)| !out.contains_key(*key)).map(|(key, value)| (key.clone(), value.clone())).collect();
                    out.extend(held);
                    Value::Object(out)
                }
                other => other.clone(),
            };
            match (field.list, &value) {
                (true, Value::Array(rows)) => Value::Array(rows.iter().map(row).collect()),
                (false, value) => row(value),
                (true, other) => other.clone(),
            }
        }
        (_, Some(held)) => held.clone(),
        (_, None) => node::unset(field),
    }
}

/// Each of `ids` read as `GET /v1/tools` reads it, statically: what can't be read is said by why.
pub(super) async fn tools(api: &Arc<Api>, ids: impl IntoIterator<Item = String>, dir: &FsPath) -> HashMap<String, Result<Tool, Error>> {
    let ids: BTreeSet<String> = ids.into_iter().collect();
    let reads = ids.into_iter().map(|id| async move {
        let read = crate::toolbox::tool_of(api, &id, dir).await;
        (id, read)
    });
    futures_util::future::join_all(reads).await.into_iter().collect()
}

/// Which nodes a read names: `shot_2`, or `shot_2.12` for one take of it.
fn named(written: &Written, asked: &str) -> Result<(String, Option<u32>), Error> {
    let found = |name: &str| written.get(name).map(|held| held.name.clone());
    if let Some(name) = found(asked) {
        return Ok((name, None));
    }
    if let Some((name, number)) = asked.rsplit_once('.').and_then(|(name, number)| Some((found(name)?, number.parse::<u32>().ok()?))) {
        return Ok((name, Some(number)));
    }
    let base = asked.rsplit_once('.').filter(|(_, number)| number.parse::<u32>().is_ok()).map_or(asked, |(name, _)| name);
    let near = node::near(base, written.boxes.iter().map(|held| held.name.as_str()));
    let said = match near {
        Some(near) => format!("the project has no node {base}: did you mean {near}?"),
        None => format!("the project has no node {base}"),
    };
    Err(Error::new(Kind::NotFound, "node_unknown", said).next("cozy get"))
}

#[derive(Deserialize)]
pub(super) struct Asked {
    /// Comma-separated: each a node, or `name.N` for one take of it. None: every node.
    #[serde(default)]
    names: Option<String>,
    /// Every take of each, not only the approved, the newest and what's going.
    #[serde(default)]
    takes: bool,
}

/// `GET /v1/projects/{id}/nodes?names=&takes=`: the nodes named, or every node, each as the file holds it with every field, defaults
/// too, and the rev it was read at. Its tools are read as a run of them here would read them, from their packages' declarations.
pub(super) async fn read(State(api): State<Arc<Api>>, Path(id): Path<String>, Query(asked): Query<Asked>) -> Result<Json<Nodes>, Refusal> {
    let project = api.projects.get(&api, &id)?;
    let names: Vec<String> = asked.names.as_deref().unwrap_or_default().split(',').map(str::trim).filter(|name| !name.is_empty()).map(str::to_owned).collect();
    let which = if asked.takes { Shown::All } else { Shown::Some };
    Ok(Json(nodes_of(&api, &project, &names, which).await.map_err(Refusal)?))
}

/// `names` (each a node, or `name.N` for one take of it; none, every node) as the file holds them, each showing `which` takes.
pub(super) async fn nodes_of(api: &Arc<Api>, project: &Arc<super::Project>, names: &[String], which: Shown) -> Result<Nodes, Error> {
    let folder = &project.folder;
    let (written, heads) = folder.reading_document(|written, heads| (written.clone(), heads))?;
    let lock = folder.lock()?;
    let wanted: Vec<(String, Option<u32>)> = match names {
        [] => written.boxes.iter().map(|held| (held.name.clone(), None)).collect(),
        names => names.iter().map(|name| named(&written, name)).collect::<Result<_, _>>()?,
    };
    let views: Vec<(View, Option<u32>)> = wanted.iter().filter_map(|(name, one)| Some((view(written.get(name)?, &lock, folder), *one))).collect();
    let tools = tools(api, views.iter().filter_map(|(view, _)| view.tool.clone()), &folder.dir).await;
    let going = super::takes_now(project, &folder.walk_now()?);
    let rev = node::rev_of(&heads);
    let mut answer = Nodes { rev: rev.clone(), ..Nodes::default() };
    for (id, read) in &tools {
        if let Err(why) = read {
            answer.warnings.push(format!("{id} can't be read, so its blocks show only what they set: {}", why.message));
        }
    }
    for (view, one) in &views {
        let tool = view.tool.as_ref().and_then(|id| tools.get(id)).and_then(|read| read.as_ref().ok());
        let which = one.map_or(which, Shown::One);
        let going = going.get(&name_key(&view.name)).map(Vec::as_slice).unwrap_or_default();
        let (value, older) = node(view, tool, &takes(&view.name, &lock, &folder.dir, going), which, &rev);
        if older > 0 {
            answer.older.insert(view.name.clone(), older);
        }
        answer.nodes.insert(view.name.clone(), value);
    }
    Ok(answer)
}
