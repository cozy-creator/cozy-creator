//! The agent API (tracker design/agent-api.md): a project's nodes as a file, read whole with `cozy get`, written back with `cozy apply`,
//! the same merge shown by `cozy diff`, gone by `cozy delete`, and what a file can hold by `cozy explain`. One file of nodes, edited with
//! the agent's own editor; Calcifer merges each node against the rev it was read at, so a person's edits on the page meanwhile stand.
use std::process::ExitCode;

use api::{
    calcifer::{
        node::{self, Nodes},
        project::{Asset, State},
        tool::{Tool, Tools},
    },
    error::{Error, Kind},
};
use serde_json::{Value, json};

use crate::{Context, calcifer_client::{Client, path}, output};

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
