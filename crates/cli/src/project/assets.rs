//! What the canvas does, as an agent types it: `add`, `set`, `delete`, `restore`, and one asset of `status`. Each is the route the page
//! calls, with its fields, so what either does shows on the other at once.
use std::{
    io::Read as _,
    path::{Path, PathBuf},
    process::ExitCode,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use api::{
    error::{Error, Kind},
    calcifer::project::{AssetBack, AssetDeleted, AssetMade, AssetPatch, AssetPatched, AssetVersions, Card, NewAsset, VersionBack, VersionMoved},
};
use api::calcifer::tool::{FieldType, Tools};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use super::open;
use crate::{Context, calcifer_client::{Client, path as route}, output, run::mint_key};

/// What `tool=` is: what makes it, not one of its inputs.
const TOOL: &str = "tool";
/// What `at=` is: where it sits on the canvas, `x,y` or `x,y,w`, not one of its inputs.
const AT: &str = "at";

/// `at=` out of `fields`.
fn place(fields: &mut Map<String, Value>) -> Result<Option<Card>, Error> {
    let Some(at) = fields.remove(AT) else { return Ok(None) };
    let refused = || input_error("field_term", format!("at is x,y or x,y,w, not {at}"));
    let numbers: Option<Vec<f64>> = at.as_str().ok_or_else(refused)?.split(',').map(|part| part.trim().parse().ok()).collect();
    match numbers.as_deref() {
        Some(&[x, y]) => Ok(Some(Card { x, y, w: None })),
        Some(&[x, y, w]) => Ok(Some(Card { x, y, w: Some(w) })),
        _ => Err(refused()),
    }
}

/// A term's `value`: nothing clears the field, a plain number or `true` or `false` is that, anything else its text. `:=` says anything else.
fn scalar(text: &str) -> Value {
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let plain_number = unsigned.split_once('.').map_or_else(|| digits(unsigned), |(whole, part)| digits(whole) && digits(part));
    match text {
        "" => Value::Null,
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ if plain_number => text.parse::<Number>().map_or_else(|_| Value::String(text.to_owned()), Value::Number),
        _ => Value::String(text.to_owned()),
    }
}

fn input_error(code: &'static str, said: String) -> Error {
    Error::new(Kind::Usage, code, said)
}

/// A file's mapping, YAML or JSON; `-` is stdin.
fn mapping(file: &Path) -> Result<Value, Error> {
    let mut text = String::new();
    let unreadable = |e: std::io::Error| input_error("input_unreadable", format!("{}: {e}", file.display()));
    match file.as_os_str() == "-" {
        true => drop(std::io::stdin().read_to_string(&mut text).map_err(unreadable)?),
        false => text = std::fs::read_to_string(file).map_err(unreadable)?,
    }
    api::yaml::to_json(&text).map_err(|why| input_error("input_unreadable", format!("{}: {why}", file.display())))
}

/// What a file holds, with `field=value` and `field:=json` over it, as `cozy run` reads them.
fn fields_of(file: Option<&PathBuf>, terms: &[String]) -> Result<Map<String, Value>, Error> {
    let mut fields = match file {
        Some(file) => match mapping(file)? {
            Value::Object(fields) => fields,
            _ => return Err(input_error("input_unreadable", format!("{} isn't a mapping of field to value", file.display()))),
        },
        None => Map::new(),
    };
    for term in terms {
        let Some((field, value)) = term.split_once('=') else {
            return Err(input_error("field_term", format!("{term} isn't field=value")).next("cozy run <tool> --describe   its fields"));
        };
        let (field, value) = match field.strip_suffix(':') {
            Some(field) => (field, serde_json::from_str(value).map_err(|why| input_error("field_term", format!("{term}: {why}")))?),
            None => (field, scalar(value)),
        };
        fields.insert(field.to_owned(), value);
    }
    Ok(fields)
}

/// A list or a record typed as JSON with `=`, where `tool` declares one: `beats='[…]'` is what it looks like, and what's text in a text field stays text.
async fn as_declared(calcifer: &Client, project: &str, tool: &str, fields: &mut Map<String, Value>) {
    let looks = |value: &Value| value.as_str().is_some_and(|text| text.starts_with(['[', '{']));
    if !fields.values().any(looks) {
        return;
    }
    let Ok(listed) = calcifer.get_query::<_, Tools>(&route(&["v1", "tools"]), &[("project", project)]).await else { return };
    let plain = tool.trim_start_matches("./");
    let Some(held) = listed.tools.iter().find(|held| held.id == tool || held.id.ends_with(plain)) else { return };
    for declared in held.fields.iter().filter(|declared| matches!(declared.ty, FieldType::Group) || declared.multiple) {
        let parsed = fields.get(&declared.field).filter(|value| looks(value)).and_then(Value::as_str).and_then(|text| serde_json::from_str::<Value>(text).ok());
        if let Some(parsed) = parsed {
            fields.insert(declared.field.clone(), parsed);
        }
    }
}

/// Make an asset of a tool, as dropping it on the canvas does, and print the name it got.
#[derive(clap::Args, Debug)]
#[command(after_help = "Examples:
  cozy project add qwen-image-2/generate_turbo kyogre instructions='A colossal blue whale.' aspect_ratio=1:1
  cozy project add cozy/edit film at=400,120,720
  cozy project add text style text='Soft morning light.'   words that <style> stands for anywhere

A tool is a function: `cozy package ls`, `cozy run <package>`, and `cozy run <tool> --describe` for its fields. `cozy/edit` is the Edit.
A name you give that the project has is refused; one cozy picks takes _2. With no at=, the page puts it somewhere free, right of what it reads.")]
pub(super) struct Add {
    /// A function, local code by its path from the project's folder and its function, or `text` for a text asset.
    tool: String,
    /// One word for what to call it, default the tool's, then fields as `set` takes them.
    #[arg(value_name = "NAME|FIELD=VALUE")]
    terms: Vec<String>,
}

pub(super) async fn add(ctx: &Context, at: Option<&String>, args: Add) -> Result<ExitCode, Error> {
    let opened = open(ctx, at).await?;
    let (names, terms): (Vec<String>, Vec<String>) = args.terms.into_iter().partition(|term| !term.contains('='));
    if names.len() > 1 {
        return Err(input_error("field_term", format!("one name, not {}: a field is field=value", names.join(" and "))));
    }
    let exact = !names.is_empty();
    let mut fields = fields_of(None, &terms)?;
    // `text`, no tool: a text asset.
    let tool = if args.tool == "text" { String::new() } else { args.tool };
    let calcifer = ctx.calcifer().await?;
    as_declared(&calcifer, &opened.id, &tool, &mut fields).await;
    let ask = NewAsset { tool, exact, at: place(&mut fields)?, name: names.into_iter().next(), fields, kind: None, moves: Default::default(), placed: Default::default() };
    let made: AssetMade = calcifer.post(&route(&["v1", "projects", &opened.id, "assets"]), &ask, Some(&mint_key())).await?;
    if ctx.json {
        output::json(&made);
    } else {
        anstream::println!("{}", made.name);
    }
    Ok(ExitCode::SUCCESS)
}

/// Set an asset's fields, as its box's controls do.
#[derive(clap::Args, Debug)]
#[command(after_help = "Examples:
  cozy project set kyogre instructions='A colossal blue whale.' megapixels=1
  cozy project set kyogre lora:='[{\"name\": \"a/b\", \"strength\": 0.8}]'
  cozy project set kyogre style=                       clears it
  cozy project set kyogre tool=qwen-image-2/generate   what makes it; on a picture, the picture is its first version
  cozy project set kyogre tool=                        takes it off: its pictures and words stay
  cozy project set kyogre at=400,120,360               where it sits and how wide, as dragging it
  cozy project set film --input timeline.yaml          an Edit's tracks and clips, or any fields

A value is a number, true or false, or its text; := is JSON. Only what's named moves, and a comment stays.")]
pub(super) struct Set {
    /// The asset.
    name: String,
    /// `field=value` or `field:=json`; `field=` clears it.
    #[arg(value_name = "FIELD=VALUE")]
    terms: Vec<String>,
    /// Its fields as YAML or JSON, `-` for stdin. A term on the line goes over the file.
    #[arg(long, value_name = "FILE")]
    input: Option<PathBuf>,
}

pub(super) async fn set(ctx: &Context, at: Option<&String>, args: Set) -> Result<ExitCode, Error> {
    let mut fields = fields_of(args.input.as_ref(), &args.terms)?;
    if fields.is_empty() {
        let said = "nothing to set: name a field=value, or --input a file";
        return Err(input_error("field_term", said.to_owned()).next("cozy run <tool> --describe   its fields"));
    }
    let names = fields.keys().cloned().collect::<Vec<_>>().join(", ");
    let card = place(&mut fields)?;
    let tool = match fields.remove(TOOL) {
        Some(Value::String(tool)) => Some(tool),
        Some(Value::Null) => Some(String::new()),
        Some(other) => return Err(input_error("field_term", format!("tool is a function, not {other}")).next("cozy package ls")),
        None => None,
    };
    let opened = open(ctx, at).await?;
    let calcifer = ctx.calcifer().await?;
    let path = route(&["v1", "projects", &opened.id, "assets", &args.name]);
    if fields.values().any(|value| value.as_str().is_some_and(|text| text.starts_with(['[', '{']))) {
        let held: AssetVersions = calcifer.get(&path).await?;
        if let Some(of) = tool.as_deref().or(held.asset.tool.as_deref()) {
            as_declared(&calcifer, &opened.id, of, &mut fields).await;
        }
    }
    // Its fields and its row in one step: one transaction, one entry.
    let patched: AssetPatched = calcifer.patch(&path, &AssetPatch { fields, tool, at: card }).await?;
    match ctx.json {
        true => output::json(&patched),
        false => {
            println!("{}: set {names}", args.name);
            patched.warnings.iter().for_each(|warning| println!("warning: {warning}"));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// What `restore` puts back, on one line that any shell passes whole.
#[derive(Serialize, Deserialize)]
struct Kept {
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<Card>,
}

/// `delete --json`: what the route answers, and the command that undoes it.
#[derive(Serialize)]
struct Deleted<'a> {
    #[serde(flatten)]
    deleted: &'a AssetDeleted,
    next: [String; 1],
}

/// `delete --version --json`: what the route answers, and the command that undoes it.
#[derive(Serialize)]
struct VersionDeleted<'a> {
    #[serde(flatten)]
    moved: &'a VersionMoved,
    next: [String; 1],
}

/// The words of `-p` when it was given, as what's printed has to work from here.
fn from(at: Option<&String>) -> String {
    at.map_or_else(String::new, |at| match at.contains(' ') {
        true => format!(" -p '{at}'"),
        false => format!(" -p {at}"),
    })
}

/// `names` deleted, each printing the command that restores it.
pub(super) async fn delete(ctx: &Context, at: Option<&String>, names: Vec<String>, versions: Vec<u32>) -> Result<ExitCode, Error> {
    if !versions.is_empty() {
        return discard(ctx, at, names, versions).await;
    }
    let opened = open(ctx, at).await?;
    let calcifer = ctx.calcifer().await?;
    for name in names {
        let deleted: AssetDeleted = calcifer.delete(&route(&["v1", "projects", &opened.id, "assets", &name])).await?;
        let kept = serde_json::to_vec(&Kept { text: deleted.text.clone(), at: deleted.at }).expect("it serializes");
        let back = format!("cozy project{} restore {} {}", from(at), deleted.name, URL_SAFE_NO_PAD.encode(kept));
        match ctx.json {
            true => output::json(&Deleted { deleted: &deleted, next: [back] }),
            false => {
                println!("deleted {}: what it made stays until `cozy project reclaim`, and what's going was canceled", deleted.name);
                anstream::println!("{back}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `versions` of one asset discarded, each printing the command that brings it back.
async fn discard(ctx: &Context, at: Option<&String>, names: Vec<String>, versions: Vec<u32>) -> Result<ExitCode, Error> {
    let [name] = names.as_slice() else {
        return Err(input_error("project_usage", "--version is of one asset".to_owned()).next(format!("cozy project delete {} --version {}", names[0], versions[0])));
    };
    let opened = open(ctx, at).await?;
    let calcifer = ctx.calcifer().await?;
    for number in versions {
        let moved: VersionMoved = calcifer.delete(&route(&["v1", "projects", &opened.id, "assets", name, "versions", &number.to_string()])).await?;
        let back = format!("cozy project{} restore {} --version {number}{}", from(at), moved.name, if moved.approved { " --approved" } else { "" });
        if ctx.json {
            output::json(&VersionDeleted { moved: &moved, next: [back] });
            continue;
        }
        let (name, noun) = (&moved.name, &moved.noun);
        let approval = if moved.approved { ", and its approval with it" } else { "" };
        let stands = moved.stands.map_or_else(|| format!("it has no {noun} now"), |stands| format!("it's on {noun} {stands} now"));
        println!("discarded {name}'s {noun} {number}{approval}: {stands}, and the files stay until `cozy project reclaim`");
        if !moved.read_by.is_empty() {
            println!("warning: {} read {noun} {number} by name: a render of them fails until it's back", moved.read_by.join(", "));
        }
        say_stale(&moved);
        anstream::println!("{back}");
    }
    Ok(ExitCode::SUCCESS)
}

fn say_stale(moved: &VersionMoved) {
    if !moved.stale.is_empty() {
        println!("out of date now: {}", moved.stale.join(", "));
    }
}

/// `delete`'s line: the asset back as it was, where it was, with what it made, or a take it discarded.
pub(super) async fn restore(ctx: &Context, at: Option<&String>, name: String, kept: Option<String>, versions: Vec<u32>, approved: bool) -> Result<ExitCode, Error> {
    let Some(kept) = kept else { return undiscard(ctx, at, name, versions, approved).await };
    let unreadable = |why: String| input_error("restore_unreadable", format!("that isn't what deleting {name} printed: {why}")).next("cozy project status");
    let bytes = URL_SAFE_NO_PAD.decode(kept.trim()).map_err(|why| unreadable(why.to_string()))?;
    let kept: Kept = serde_json::from_slice(&bytes).map_err(|why| unreadable(why.to_string()))?;
    let opened = open(ctx, at).await?;
    let made: AssetMade = ctx.calcifer().await?.put(&route(&["v1", "projects", &opened.id, "assets", &name]), &AssetBack { text: kept.text, at: kept.at }).await?;
    match ctx.json {
        true => output::json(&made),
        false => println!("{} is back", made.name),
    }
    Ok(ExitCode::SUCCESS)
}

async fn undiscard(ctx: &Context, at: Option<&String>, name: String, versions: Vec<u32>, approved: bool) -> Result<ExitCode, Error> {
    if approved && versions.len() > 1 {
        return Err(input_error("project_usage", "--approved is of one --version: the one `delete` printed it for".to_owned()).next(format!("cozy project restore {name} --version {} --approved", versions[0])));
    }
    let opened = open(ctx, at).await?;
    let calcifer = ctx.calcifer().await?;
    for number in versions {
        let moved: VersionMoved = calcifer.put(&route(&["v1", "projects", &opened.id, "assets", &name, "versions", &number.to_string()]), &VersionBack { approved }).await?;
        if ctx.json {
            output::json(&moved);
            continue;
        }
        let approval = if moved.approved { ", approved" } else { "" };
        println!("{}'s {} {number} is back{approval}", moved.name, moved.noun);
        say_stale(&moved);
    }
    Ok(ExitCode::SUCCESS)
}

/// `status <name>`: its fields and each take or version.
pub(super) async fn one(ctx: &Context, at: Option<&String>, name: String) -> Result<ExitCode, Error> {
    let opened = open(ctx, at).await?;
    let path = route(&["v1", "projects", &opened.id, "assets", &name]);
    let mut held: AssetVersions = ctx.calcifer().await?.get_query(&path, &[("call", "true")]).await?;
    // The newest `SHOWN`, unless --full; `asset.versions` counts them all.
    let all = held.versions.len();
    if !ctx.full && all > SHOWN {
        held.versions.drain(..all - SHOWN);
    }
    if ctx.json {
        output::view(ctx, &held, FIELDS);
        return Ok(ExitCode::SUCCESS);
    }
    let asset = &held.asset;
    let mut record = output::Record::new().field("name", &asset.name).field("kind", &asset.kind).field("status", &asset.status);
    if let Some(tool) = &asset.tool {
        record = record.field("tool", tool);
    }
    for (field, value) in &asset.fields {
        record = record.field(field, output::cut(ctx, &value.as_str().map_or_else(|| value.to_string(), str::to_owned), false));
    }
    record.print(ctx);
    let table = output::Table::new(ctx, [asset.noun.to_uppercase().as_str(), "APPROVED", "MADE BY", "SEED", "FILE"]);
    let rows = held.versions.iter().map(|version| {
        let file = version.files.values().next().map_or("-".to_owned(), |file| file.path.clone());
        [version.number.to_string(), if version.approved { "yes" } else { "-" }.to_owned(), version.made_by.clone().unwrap_or_else(|| "-".into()), version.seed.map_or("-".to_owned(), |seed| seed.to_string()), file]
    });
    rows.fold(table, output::Table::row).print();
    if held.versions.len() < all {
        println!("{} newest of {all} {}s: --full for all", held.versions.len(), asset.noun);
    }
    if !held.discarded.is_empty() {
        let numbers: Vec<String> = held.discarded.iter().map(u32::to_string).collect();
        println!("discarded, until `cozy project reclaim`: {}. To bring one back: cozy project restore {} --version <n>", numbers.join(", "), asset.name);
    }
    // What a render would send now: the JSON `cozy run --input` takes.
    if let Some(call) = &held.call {
        println!("call: {}", serde_json::to_string_pretty(call).unwrap_or_default());
        held.call_warnings.iter().for_each(|said| output::warn(said));
    }
    Ok(ExitCode::SUCCESS)
}

/// The versions `status <name>` shows, newest last, unless --full.
const SHOWN: usize = 20;
/// What `status <name> --json` keeps (`--full`: all of it): the asset as the view says it, and each version but the words it was made from.
const FIELDS: &[&str] = &[
    "discarded", "asset.name", "asset.kind", "asset.tool", "asset.status", "asset.standing", "asset.approved", "asset.version",
    "asset.versions", "asset.fields", "asset.reads", "asset.failure", "versions.number", "versions.approved", "versions.made_by",
    "versions.seed", "versions.runs", "versions.files", "versions.execution_ms", "call", "call_warnings",
];
