//! What the canvas does, as an agent types it: `add`, `set`, `delete`, `restore`, and one asset of `status`. Each is the route the page
//! calls, with its fields, so what either does shows on the other at once.
use std::{
    collections::BTreeMap,
    io::Read as _,
    path::{Path, PathBuf},
    process::ExitCode,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use protocol::{
    error::{Error, Kind},
    project::{AssetBack, AssetDeleted, AssetMade, AssetPatch, AssetPatched, AssetVersions, CanvasPatch, CanvasPatched, Card, NewAsset},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use super::open;
use crate::{Context, calcifer_client::path as route, output, run::mint_key};

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
    protocol::yaml::to_json(&text).map_err(|why| input_error("input_unreadable", format!("{}: {why}", file.display())))
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

pub(super) async fn add(ctx: &Context, at: Option<&PathBuf>, args: Add) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let (names, terms): (Vec<String>, Vec<String>) = args.terms.into_iter().partition(|term| !term.contains('='));
    if names.len() > 1 {
        return Err(input_error("field_term", format!("one name, not {}: a field is field=value", names.join(" and "))));
    }
    let exact = !names.is_empty();
    let mut fields = fields_of(None, &terms)?;
    // `text`, no tool: a text asset.
    let tool = if args.tool == "text" { String::new() } else { args.tool };
    let ask = NewAsset { tool, exact, at: place(&mut fields)?, name: names.into_iter().next(), fields };
    let made: AssetMade = ctx.calcifer().await?.post(&route(&["v1", "projects", &opened.id, "assets"]), &ask, Some(&mint_key())).await?;
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
  cozy project set kyogre tool=qwen-image-2/generate   what makes it
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

pub(super) async fn set(ctx: &Context, at: Option<&PathBuf>, args: Set) -> Result<ExitCode, Error> {
    let mut fields = fields_of(args.input.as_ref(), &args.terms)?;
    if fields.is_empty() {
        let said = "nothing to set: name a field=value, or --input a file";
        return Err(input_error("field_term", said.to_owned()).next("cozy run <tool> --describe   its fields"));
    }
    let names = fields.keys().cloned().collect::<Vec<_>>().join(", ");
    let card = place(&mut fields)?;
    let tool = match fields.remove(TOOL) {
        Some(Value::String(tool)) => Some(tool),
        Some(other) => return Err(input_error("field_term", format!("tool is a function, not {other}")).next("cozy package ls")),
        None => None,
    };
    let opened = open(ctx, at, false).await?;
    let calcifer = ctx.calcifer().await?;
    let path = route(&["v1", "projects", &opened.id, "assets", &args.name]);
    if let Some(card) = card {
        // A row only under the name the project has for it: one of nothing would wait for it for ever.
        let held: AssetVersions = calcifer.get(&path).await?;
        let ask = CanvasPatch { cards: BTreeMap::from([(held.asset.name, Some(card))]) };
        let _: CanvasPatched = calcifer.patch(&route(&["v1", "projects", &opened.id, "canvas"]), &ask).await?;
    }
    let patched: Option<AssetPatched> = match fields.is_empty() && tool.is_none() {
        true => None,
        false => Some(calcifer.patch(&path, &AssetPatch { fields, tool }).await?),
    };
    patched.iter().flat_map(|patched| &patched.warnings).for_each(|warning| output::warn(warning));
    match (ctx.json, &patched) {
        (true, Some(patched)) => output::json(patched),
        (true, None) => output::json(&serde_json::json!({ "name": args.name })),
        (false, _) => output::say(ctx, &format!("{}: set {names}", args.name)),
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

/// The words of `-p` when it was given, as what's printed has to work from here.
fn from(at: Option<&PathBuf>) -> String {
    at.map_or_else(String::new, |at| match at.to_string_lossy().contains(' ') {
        true => format!(" -p '{}'", at.display()),
        false => format!(" -p {}", at.display()),
    })
}

/// `names` deleted, each printing the command that restores it.
pub(super) async fn delete(ctx: &Context, at: Option<&PathBuf>, names: Vec<String>) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let calcifer = ctx.calcifer().await?;
    for name in names {
        let deleted: AssetDeleted = calcifer.delete(&route(&["v1", "projects", &opened.id, "assets", &name])).await?;
        let kept = serde_json::to_vec(&Kept { text: deleted.text.clone(), at: deleted.at }).expect("it serializes");
        let back = format!("cozy project{} restore {} {}", from(at), deleted.name, URL_SAFE_NO_PAD.encode(kept));
        match ctx.json {
            true => output::json(&Deleted { deleted: &deleted, next: [back] }),
            false => {
                output::say(ctx, &format!("deleted {}: what it made stays until `cozy project reclaim`, and what's going was canceled", deleted.name));
                anstream::println!("{back}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `delete`'s line: the asset back as it was, where it was, with what it made.
pub(super) async fn restore(ctx: &Context, at: Option<&PathBuf>, name: String, kept: String) -> Result<ExitCode, Error> {
    let unreadable = |why: String| input_error("restore_unreadable", format!("that isn't what deleting {name} printed: {why}")).next("cozy project status");
    let bytes = URL_SAFE_NO_PAD.decode(kept.trim()).map_err(|why| unreadable(why.to_string()))?;
    let kept: Kept = serde_json::from_slice(&bytes).map_err(|why| unreadable(why.to_string()))?;
    let opened = open(ctx, at, false).await?;
    let made: AssetMade = ctx.calcifer().await?.put(&route(&["v1", "projects", &opened.id, "assets", &name]), &AssetBack { text: kept.text, at: kept.at }).await?;
    match ctx.json {
        true => output::json(&made),
        false => output::say(ctx, &format!("{} is back", made.name)),
    }
    Ok(ExitCode::SUCCESS)
}

/// `status <name>`: its fields and each take or version.
pub(super) async fn one(ctx: &Context, at: Option<&PathBuf>, name: String) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let held: AssetVersions = ctx.calcifer().await?.get(&route(&["v1", "projects", &opened.id, "assets", &name])).await?;
    if ctx.json {
        output::json(&held);
        return Ok(ExitCode::SUCCESS);
    }
    let asset = &held.asset;
    let mut record = output::Record::new().field("name", &asset.name).field("kind", &asset.kind).field("status", &asset.status);
    if let Some(tool) = &asset.tool {
        record = record.field("tool", tool);
    }
    for (field, value) in &asset.fields {
        record = record.field(field, value.as_str().map_or_else(|| value.to_string(), str::to_owned));
    }
    record.print(ctx);
    let table = output::Table::new(ctx, [asset.noun.to_uppercase().as_str(), "APPROVED", "MADE BY", "SEED", "FILE"]);
    let rows = held.versions.iter().map(|version| {
        let file = version.files.values().next().map_or("-".to_owned(), |file| file.path.clone());
        [version.number.to_string(), if version.approved { "yes" } else { "-" }.to_owned(), version.made_by.clone().unwrap_or_else(|| "-".into()), version.seed.map_or("-".to_owned(), |seed| seed.to_string()), file]
    });
    rows.fold(table, output::Table::row).print();
    Ok(ExitCode::SUCCESS)
}
