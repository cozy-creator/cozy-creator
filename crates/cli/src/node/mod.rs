//! The agent API (tracker design/agent-api.md): a project's nodes as a file, read whole with `cozy get`, written back with `cozy apply`,
//! the same merge shown by `cozy diff`, gone by `cozy delete`, and what a file can hold by `cozy explain`. One file of nodes, edited with
//! the agent's own editor; Calcifer merges each node against the rev it was read at, so a person's edits on the page meanwhile stand.
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use api::{
    calcifer::{
        node::{self, Applied, Apply as ApplyAsk, Nodes},
        project::{Asset, AssetDeleted, State},
        tool::{Tool, Tools},
    },
    error::{Error, Kind},
};
use serde_json::{Map, Value, json};

use crate::{Context, calcifer_client::{Client, path}, output, run::mint_key, send::Sender};

mod yaml;

const GET: &str = "Examples
  cozy get                     one line per node
  cozy get shot_1 shot_2       each node's every field, as a file apply takes
  cozy get shot_1 --takes      every take, not only the approved, the newest and what's going
  cozy get shot_1.12           take 12 of it
  cozy get --all > film.yaml   every node";

const EXPLAIN: &str = "Examples
  cozy explain                          what a file holds, and the tools a node can be
  cozy explain video                    a video node, and the tools that make one
  cozy explain minimax-h3/generate      a tool's block: each field, model slot and loras
  cozy explain minimax-h3/generate.beats
  cozy explain --schema > cozy.json     JSON Schema 2020-12 for any validator or editor";

/// List nodes, or print named nodes whole as a file.
#[derive(clap::Args, Debug)]
#[command(after_help = GET)]
pub(crate) struct Get {
    /// Nodes to print whole; `NAME.N` for take N of one. None: one line per node.
    #[arg(value_name = "NAME")]
    names: Vec<String>,
    /// Every node, whole.
    #[arg(long, conflicts_with = "names")]
    all: bool,
    /// Every take of each node.
    #[arg(long)]
    takes: bool,
}

/// What a file of nodes can hold: a kind, a tool's block, a field of it.
#[derive(clap::Args, Debug)]
#[command(after_help = EXPLAIN)]
pub(crate) struct Explain {
    /// `image`, `video`, `audio`, `text`, `edit`; a tool's id; or a field of one, `TOOL.FIELD[.MEMBER]`.
    #[arg(value_name = "KIND|TOOL[.FIELD]")]
    path: Option<String>,
    /// JSON Schema 2020-12 instead of words.
    #[arg(long)]
    schema: bool,
}

const APPLY: &str = "Examples
  cozy get shot_2 > shot_2.yaml       read it, edit it with any editor, then
  cozy apply -f shot_2.yaml           merge it in; the file is rewritten as it stands, with new revs
  cozy apply -f film.yaml --force-conflicts
  cozy get --all | cozy apply -f -    the merged nodes print on stdout

Each node merges against its rev: what the file changed is written, what the project changed since stands, and a value both
changed is a conflict (words both edited merge unless the edits overlap). A new name makes a node; a take with only
`file: <path>` imports that file as its next take. Exit 0 applied, 1 refused (nothing written).";

const DIFF: &str = "Examples
  cozy diff -f shot_2.yaml

What `cozy apply -f` would change, its own merge with nothing written. Exit 0 no changes, 1 changes, 2 it would refuse.";

/// Merge a file of nodes into the project: one edit, which the page's Undo takes back.
#[derive(clap::Args, Debug)]
#[command(after_help = APPLY)]
pub(crate) struct Apply {
    /// The file: YAML or JSON, node name to node; `-` reads stdin.
    #[arg(short = 'f', long = "filename", value_name = "FILE", required = true)]
    file: PathBuf,
    /// Where both sides changed a value, keep the file's.
    #[arg(long)]
    force_conflicts: bool,
}

/// What `cozy apply -f` would change, written nowhere.
#[derive(clap::Args, Debug)]
#[command(after_help = DIFF)]
pub(crate) struct Diff {
    /// The file: YAML or JSON, node name to node; `-` reads stdin.
    #[arg(short = 'f', long = "filename", value_name = "FILE", required = true)]
    file: PathBuf,
}

/// Remove nodes: one edit, which the page's Undo brings back.
#[derive(clap::Args, Debug)]
pub(crate) struct Delete {
    #[arg(value_name = "NAME", required = true)]
    names: Vec<String>,
}

/// The project the current directory is, as its routes name it.
struct Project {
    id: String,
    calcifer: Client,
}

impl Project {
    async fn current(ctx: &Context) -> Result<Self, Error> {
        let id = crate::project::identity(ctx).await?.id;
        Ok(Self { id, calcifer: ctx.calcifer().await? })
    }

    fn at(&self, rest: &[&str]) -> String {
        path(&[["v1", "projects", self.id.as_str()].as_slice(), rest].concat())
    }
}

pub(crate) async fn get(ctx: &Context, args: Get) -> Result<ExitCode, Error> {
    let project = Project::current(ctx).await?;
    if args.names.is_empty() && !args.all {
        if args.takes {
            return Err(Error::new(Kind::Usage, "takes_of_nothing", "--takes prints the takes of the nodes it names").next("cozy get NAME --takes"));
        }
        return list(ctx, &project).await;
    }
    let mut query = vec![];
    if !args.names.is_empty() {
        query.push(("names", args.names.join(",")));
    }
    if args.takes {
        query.push(("takes", "true".to_owned()));
    }
    let nodes: Nodes = project.calcifer.get_query(&project.at(&["nodes"]), &query).await?;
    if ctx.json {
        output::json(&nodes);
        return Ok(ExitCode::SUCCESS);
    }
    for warning in &nodes.warnings {
        output::warn(warning);
    }
    if nodes.nodes.is_empty() {
        output::nothing(ctx, "0 nodes");
        output::next(ctx, &["cozy explain says what a file holds; cozy apply -f FILE makes its nodes"]);
        return Ok(ExitCode::SUCCESS);
    }
    anstream::print!("{}", yaml::file(&nodes.nodes, &nodes.older));
    Ok(ExitCode::SUCCESS)
}

/// One line per node: its name, kind, tool, approved take and its newest take's state.
async fn list(ctx: &Context, project: &Project) -> Result<ExitCode, Error> {
    let state: State = project.calcifer.get(&project.at(&[])).await?;
    let rows: Vec<Value> = state.assets.iter().map(|asset| json!({
        "name": asset.name,
        "kind": node::kind_of(&asset.kind, asset.tool.as_deref()),
        "tool": asset.tool,
        "approved": asset.approved,
        "latest": latest(asset),
    })).collect();
    if ctx.json {
        output::json(&json!({"nodes": rows}));
        return Ok(ExitCode::SUCCESS);
    }
    if rows.is_empty() {
        output::nothing(ctx, "0 nodes");
        output::next(ctx, &["cozy explain says what a file holds; cozy apply -f FILE makes its nodes"]);
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = output::Table::new(ctx, ["NAME", "KIND", "TOOL", "APPROVED", "LATEST"]);
    for row in &rows {
        let text = |key: &str| match &row[key] {
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            _ => String::new(),
        };
        table = table.row([text("name"), text("kind"), text("tool"), text("approved"), text("latest")]);
    }
    table.print();
    output::next(ctx, &["cozy get NAME prints every field"]);
    Ok(ExitCode::SUCCESS)
}

/// Its newest take's state: what's going, else what failed, else its newest version, done.
fn latest(asset: &Asset) -> Option<String> {
    if asset.kind == "text" && asset.tool.is_none() {
        return None;
    }
    let going = asset.takes.iter().rev().find(|take| take.failure.is_none());
    if let Some(take) = going {
        return Some(if take.queued { "queued".to_owned() } else { "running".to_owned() });
    }
    if asset.takes.iter().any(|take| take.failure.is_some()) {
        return Some("failed".to_owned());
    }
    asset.prefixes.iter().map(|prefix| prefix.number).max().map(|number| format!("{number} done"))
}

pub(crate) async fn explain(ctx: &Context, args: Explain) -> Result<ExitCode, Error> {
    let calcifer = ctx.calcifer().await?;
    let project = crate::project::identity(ctx).await.ok().map(|address| address.id);
    let tools = listed(&calcifer, project.as_deref(), None).await?;
    let Some(asked) = args.path.as_deref().map(str::trim).filter(|asked| !asked.is_empty()) else {
        match args.schema {
            true => output::json(&node::schema(&tools)),
            false => print(&node::explain_file(&tools)),
        }
        return Ok(ExitCode::SUCCESS);
    };
    if node::KINDS.contains(&asked) {
        match args.schema {
            true => output::json(&node::schema(&tools.into_iter().filter(|tool| node::kind_of(&tool.makes, Some(&tool.id)) == asked).collect::<Vec<_>>())),
            false => print(&node::explain_kind(asked, &tools)),
        }
        return Ok(ExitCode::SUCCESS);
    }
    let (tool, fields) = match find(&tools, asked) {
        Some(found) => found,
        None => {
            // Not on your list: the Hub's, by its package.
            let package = node::plain(asked).split('/').rev().nth(1).unwrap_or(asked).to_owned();
            let searched = listed(&calcifer, project.as_deref(), Some(&package)).await?;
            match find(&searched, asked) {
                Some((tool, fields)) => (tool, fields),
                None => {
                    let known = tools.iter().map(|tool| tool.id.as_str()).chain(node::KINDS);
                    let said = match node::near(asked.split('.').next().unwrap_or(asked), known) {
                        Some(near) => format!("no tool or kind {asked}: did you mean {near}?"),
                        None => format!("no tool or kind {asked}"),
                    };
                    return Err(Error::new(Kind::NotFound, "explain_unknown", said).next("cozy explain"));
                }
            }
        }
    };
    if args.schema {
        let schema = match fields.as_slice() {
            [] => node::block_schema(&tool),
            [field, rest @ ..] => {
                let mut at = tool.fields.iter().find(|held| held.field == *field);
                for step in rest {
                    at = at.and_then(|field| field.members.iter().find(|member| member.field == *step));
                }
                match at {
                    Some(field) => node::field_schema(field),
                    None => return Err(Error::new(Kind::NotFound, "explain_unknown", node::explain_tool(&tool, &fields).err().unwrap_or_default()).next(format!("cozy explain {}", tool.id))),
                }
            }
        };
        output::json(&schema);
        return Ok(ExitCode::SUCCESS);
    }
    match node::explain_tool(&tool, &fields) {
        Ok(lines) => print(&lines),
        Err(said) => return Err(Error::new(Kind::NotFound, "explain_unknown", said).next(format!("cozy explain {}", tool.id))),
    }
    Ok(ExitCode::SUCCESS)
}

/// The tools a box can be, as the page lists them: of this project's local code too, and with `q`, the Hub's search.
async fn listed(calcifer: &Client, project: Option<&str>, q: Option<&str>) -> Result<Vec<Tool>, Error> {
    let mut query = vec![];
    if let Some(project) = project {
        query.push(("project", project.to_owned()));
    }
    if let Some(q) = q {
        query.push(("q", q.to_owned()));
    }
    let tools: Tools = calcifer.get_query(&path(&["v1", "tools"]), &query).await?;
    Ok(tools.tools)
}

/// The tool `asked` names, with or without its release, and the field path after it.
fn find<'a>(tools: &[Tool], asked: &'a str) -> Option<(Tool, Vec<&'a str>)> {
    let mut best: Option<(&Tool, &str)> = None;
    for tool in tools {
        for id in [tool.id.as_str(), node::plain(&tool.id)] {
            let rest = match asked.strip_prefix(id) {
                Some("") => "",
                Some(rest) if rest.starts_with('.') => &rest[1..],
                _ => continue,
            };
            if best.is_none_or(|(held, _)| held.id.len() < tool.id.len()) {
                best = Some((tool, rest));
            }
        }
    }
    best.map(|(tool, rest)| (tool.clone(), rest.split('.').filter(|step| !step.is_empty()).collect()))
}

fn print(lines: &[String]) {
    for line in lines {
        anstream::println!("{line}");
    }
}

/// A file of nodes, read: its nodes as JSON, each take with only `file:` held for the project and named by its hash.
async fn read_file(ctx: &Context, project: &Project, file: &Path) -> Result<Map<String, Value>, Error> {
    let stdin = file == Path::new("-");
    let text = match stdin {
        true => std::io::read_to_string(std::io::stdin()).map_err(|why| Error::new(Kind::Usage, "file_unreadable", format!("stdin: {why}")))?,
        false => std::fs::read_to_string(file).map_err(|why| Error::new(Kind::NotFound, "file_unreadable", format!("{}: {why}", file.display())))?,
    };
    let shown = if stdin { "stdin".to_owned() } else { file.display().to_string() };
    let nodes = match serde_json::from_str::<Value>(&text).or_else(|_| api::yaml::to_json(&text)) {
        Ok(Value::Object(nodes)) => nodes,
        Ok(Value::Null) => Map::new(),
        Ok(_) => return Err(Error::new(Kind::Validation, "file_not_nodes", format!("{shown} is not a map of node name to node")).next("cozy explain")),
        Err(why) => return Err(Error::new(Kind::Validation, "file_not_yaml", format!("{shown}: {why}"))),
    };
    let base = match stdin {
        true => std::env::current_dir().map_err(|why| Error::new(Kind::Unavailable, "cwd", why.to_string()))?,
        false => std::path::absolute(file).ok().and_then(|at| at.parent().map(Path::to_path_buf)).unwrap_or_default(),
    };
    let mut nodes = nodes;
    let mut sender = None;
    for node in nodes.values_mut() {
        let Some(takes) = node.get_mut("takes").and_then(Value::as_array_mut) else { continue };
        for take in takes.iter_mut() {
            let Some(entry) = take.as_object_mut().filter(|entry| entry.len() == 1) else { continue };
            let Some(written) = entry.get("file").and_then(Value::as_str).filter(|file| !file.starts_with("sha256:")) else { continue };
            let at = api::target::spelled(written, &base, &ctx.user_home);
            if sender.is_none() {
                sender = Some(Sender::for_project(ctx, &project.id).await?.quiet());
            }
            let digest = sender.as_ref().expect("a sender").file(&at).await?;
            entry.insert("file".to_owned(), json!(digest.to_string()));
        }
    }
    if let Some(sent) = sender.as_ref().and_then(Sender::said) {
        output::say(ctx, &sent);
    }
    Ok(nodes)
}

pub(crate) async fn apply(ctx: &Context, args: Apply) -> Result<ExitCode, Error> {
    let project = Project::current(ctx).await?;
    let nodes = read_file(ctx, &project, &args.file).await?;
    let ask = ApplyAsk { nodes: nodes.clone(), dry_run: false, force_conflicts: args.force_conflicts };
    let applied: Applied = project.calcifer.post(&project.at(&["apply"]), &ask, Some(&mint_key())).await?;
    let to_stdout = args.file == Path::new("-");
    if let Some(merged) = applied.merged.as_ref().filter(|_| applied.applied) {
        // Each node of the file as it stands now, with its new rev, in the file's order: applying it again changes nothing.
        let mut file = Map::new();
        for name in nodes.keys() {
            if let Some(node) = merged.nodes.get(name) {
                file.insert(name.clone(), node.clone());
            }
        }
        let text = yaml::file(&file, &merged.older);
        match to_stdout {
            true if !ctx.json => anstream::print!("{text}"),
            true => {}
            false => std::fs::write(&args.file, text).map_err(|why| Error::new(Kind::Unavailable, "file_unwritten", format!("{}: {why}", args.file.display())))?,
        }
    }
    if ctx.json {
        output::json(&applied);
        return Ok(if applied.applied { ExitCode::SUCCESS } else { ExitCode::from(1) });
    }
    if !applied.applied {
        refused(&applied);
        return Ok(ExitCode::from(1));
    }
    said(&applied);
    // What stood is in the file once it's rewritten; on stdout, there.
    kept(&applied, Some(if to_stdout { Path::new("stdout") } else { args.file.as_path() }));
    let mut lines = vec![];
    for did in ["created", "configured", "unchanged"] {
        let named: Vec<String> = applied.nodes.iter().filter(|outcome| outcome.did == did).map(|outcome| {
            // A node made says only its takes; one changed, what changed.
            let changed: Vec<&str> = outcome.changed.iter().map(String::as_str).filter(|said| did == "configured" || said.starts_with("take ")).collect();
            match changed.is_empty() || did == "unchanged" {
                true => outcome.name.clone(),
                false => format!("{} ({})", outcome.name, changed.join(", ")),
            }
        }).collect();
        if !named.is_empty() {
            lines.push(format!("{did}: {}", named.join(", ")));
        }
    }
    let line = if lines.is_empty() { "0 nodes".to_owned() } else { lines.join("\n") };
    match to_stdout {
        true => output::say(ctx, &line),
        false => {
            anstream::println!("{line}");
            anstream::println!("{} rewritten: each node as it stands, with its rev", args.file.display());
        }
    }
    let next: Vec<&str> = applied.next.iter().map(String::as_str).collect();
    output::next(ctx, &next);
    Ok(ExitCode::SUCCESS)
}

pub(crate) async fn diff(ctx: &Context, args: Diff) -> Result<ExitCode, Error> {
    let project = Project::current(ctx).await?;
    let nodes = read_file(ctx, &project, &args.file).await?;
    let ask = ApplyAsk { nodes, dry_run: true, force_conflicts: false };
    // Nothing is written, so nothing is kept under a key.
    let applied: Applied = project.calcifer.private_post(&project.at(&["apply"]), &ask).await?;
    let refuses = !applied.errors.is_empty() || !applied.conflicts.is_empty();
    let code = if refuses { 2 } else if applied.changes.is_empty() { 0 } else { 1 };
    if ctx.json {
        output::json(&applied);
        return Ok(ExitCode::from(code));
    }
    said(&applied);
    kept(&applied, None);
    // A node made is its name and kind, then each field it sets, as kubectl diff shows an object made.
    let made: Vec<&str> = applied.nodes.iter().filter(|outcome| outcome.did == "created").map(|outcome| outcome.name.as_str()).collect();
    for change in &applied.changes {
        if made.contains(&change.path.as_str()) {
            anstream::println!("+ {} ({})", change.path, change.after["kind"].as_str().unwrap_or_default());
            continue;
        }
        if let Some((_, field)) = change.path.split_once('.').filter(|(name, _)| made.contains(name)) {
            anstream::println!("  + {field}: {}", inline(&change.after));
            continue;
        }
        anstream::println!("{}", change.path);
        for (mark, value) in [("-", &change.before), ("+", &change.after)] {
            for line in shown(value).lines() {
                anstream::println!("  {mark} {line}");
            }
        }
    }
    if refuses {
        for error in &applied.errors {
            anstream::println!("error: {}: {}", error.path, error.message);
        }
        conflicts(&applied);
        return Ok(ExitCode::from(code));
    }
    match applied.changes.is_empty() {
        true => output::nothing(ctx, "no changes"),
        false => output::next(ctx, &[&format!("cozy apply -f {}", args.file.display())]),
    }
    Ok(ExitCode::from(code))
}

/// A refused apply: every error with its fix and every conflict, and what to do.
fn refused(applied: &Applied) {
    said(applied);
    let mut counted = vec![];
    if !applied.errors.is_empty() {
        counted.push(format!("{} error{}", applied.errors.len(), if applied.errors.len() == 1 { "" } else { "s" }));
    }
    if !applied.conflicts.is_empty() {
        counted.push(format!("{} conflict{}", applied.conflicts.len(), if applied.conflicts.len() == 1 { "" } else { "s" }));
    }
    anstream::eprintln!("nothing applied: {}", counted.join(", "));
    for error in &applied.errors {
        anstream::eprintln!("  {}: {}", error.path, error.message);
    }
    for conflict in &applied.conflicts {
        anstream::eprintln!("  {}", conflict_line(conflict));
    }
    if let Some(next) = conflict_next(applied) {
        anstream::eprintln!("next: {next}");
    }
}

fn conflicts(applied: &Applied) {
    for conflict in &applied.conflicts {
        anstream::println!("{}", conflict_line(conflict));
    }
    if let Some(next) = conflict_next(applied) {
        anstream::println!("next: {next}");
    }
}

/// `conflict: shot_2.beats.0.duration: yours 6, the page's 8 since its rev 3e1f0a9c`.
fn conflict_line(conflict: &node::Conflict) -> String {
    format!("conflict: {}: yours {}, {}'s {} since its rev {}", conflict.path, inline(&conflict.yours), conflict.by, inline(&conflict.theirs), conflict.rev)
}

fn conflict_next(applied: &Applied) -> Option<String> {
    match applied.conflicts.as_slice() {
        [] => None,
        [one] if !one.theirs.is_object() && !one.theirs.is_array() => {
            Some(format!("set {} and apply, or apply --force-conflicts to keep {}", inline(&one.theirs), inline(&one.yours)))
        }
        _ => Some("settle each in the file and apply, or apply --force-conflicts to keep yours".to_owned()),
    }
}

/// What the project changed since the file's rev that the file left as it was: it stands, said once, as nothing changes silently.
fn kept(applied: &Applied, file: Option<&Path>) {
    for kept in &applied.kept {
        match file {
            Some(file) => anstream::eprintln!("{}: {}; {} now shows it", kept.path, kept.message, file.display()),
            None => anstream::eprintln!("{}: {}", kept.path, kept.message),
        }
    }
}

/// What it fixed and what it left, on stderr.
fn said(applied: &Applied) {
    if applied.applied {
        for conflict in &applied.conflicts {
            anstream::eprintln!("kept yours: {}: {}, over {}'s {}", conflict.path, inline(&conflict.yours), conflict.by, inline(&conflict.theirs));
        }
    }
    for warning in &applied.warnings {
        output::warn(&at(&warning.path, &warning.message));
    }
    for note in &applied.notes {
        anstream::eprintln!("note: {}", at(&note.path, &note.message));
    }
}

fn at(path: &str, message: &str) -> String {
    if path.is_empty() { message.to_owned() } else { format!("{path}: {message}") }
}

/// A value on a line of its own: words as they are, anything else as JSON.
fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "null".to_owned(),
        other => other.to_string(),
    }
}

fn inline(value: &Value) -> String {
    match value {
        Value::String(text) if !text.contains('\n') => text.clone(),
        other => other.to_string(),
    }
}

pub(crate) async fn delete(ctx: &Context, args: Delete) -> Result<ExitCode, Error> {
    let project = Project::current(ctx).await?;
    let state: State = project.calcifer.get(&format!("{}?assets=true", project.at(&[]))).await?;
    let held: Vec<&str> = state.assets.iter().map(|asset| asset.name.as_str()).collect();
    let mut missing = vec![];
    for name in &args.names {
        let key = |name: &str| name.replace(['_', '-'], " ").to_lowercase();
        if !held.iter().any(|held| key(held) == key(name)) {
            missing.push(match node::near(name, held.iter().copied()) {
                Some(near) => format!("{name} (did you mean {near}?)"),
                None => name.clone(),
            });
        }
    }
    if !missing.is_empty() {
        return Err(Error::new(Kind::NotFound, "node_unknown", format!("the project has no node {}: nothing deleted", missing.join(", "))).next("cozy get"));
    }
    // One edit, one entry of the history: the page's Undo brings them all back.
    let edit = mint_key();
    let mut gone = vec![];
    for name in &args.names {
        let deleted: AssetDeleted = project.calcifer.delete_edit(&project.at(&["assets", name]), &edit).await?;
        gone.push(deleted.name);
    }
    if ctx.json {
        output::json(&json!({"deleted": gone}));
        return Ok(ExitCode::SUCCESS);
    }
    anstream::println!("deleted: {}", gone.join(", "));
    anstream::println!("the page's Undo brings {} back", if gone.len() == 1 { "it" } else { "them" });
    Ok(ExitCode::SUCCESS)
}
