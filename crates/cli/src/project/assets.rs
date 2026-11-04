//! What the canvas does, as verbs an agent can type: `add`, `set`, `delete`, `restore`, `canvas`, `caption`, `tools`, `show`. Each is the
//! route the page calls, with the same fields, so what either does shows on the other at once.
use std::{
    collections::BTreeMap,
    io::Read as _,
    path::{Path, PathBuf},
    process::ExitCode,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use protocol::{
    error::{Error, Kind},
    project::{AssetBack, AssetDeleted, AssetMade, AssetPatch, AssetPatched, AssetVersions, Canvas, CanvasPatch, CanvasPatched, CaptionAsked, Captioned, Card, NewAsset},
    tool::Tools,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use super::open;
use crate::{Context, calcifer_client::path as route, output, run::mint_key};

/// What `tool=` on `set` is: what makes it, not one of its inputs.
const TOOL: &str = "tool";

/// `X,Y` or `X,Y,W`.
pub(super) fn card(text: &str) -> Result<Card, String> {
    let numbers: Vec<f64> = text.split(',').map(|part| part.trim().parse().map_err(|_| format!("{text} is not X,Y or X,Y,W"))).collect::<Result<_, _>>()?;
    match numbers[..] {
        [x, y] => Ok(Card { x, y, w: None }),
        [x, y, w] => Ok(Card { x, y, w: Some(w) }),
        _ => Err(format!("{text} is not X,Y or X,Y,W")),
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
            return Err(input_error("field_term", format!("{term} isn't field=value")).next("cozy project tools --json   each tool's fields"));
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
  cozy project tools
  cozy project add qwen-image-2/generate_turbo kyogre instructions='A colossal blue whale.' aspect_ratio=1:1
  cozy project add minimax-h3/generate_turbo fight --input fight.yaml --at 400,120,360
  cozy project add cozy/edit film --input timeline.yaml

A name something has takes _2. Without --at the page puts it somewhere free, right of what it reads.")]
pub(super) struct Add {
    /// A tool's id, from `cozy project tools`. Local code is its path, from the project's folder, and its function.
    tool: String,
    /// `field=value`, `field:=json`, and one word with no `=` for what to call it. Default: the tool's name.
    #[arg(value_name = "NAME|FIELD=VALUE")]
    terms: Vec<String>,
    /// Its fields as YAML or JSON, `-` for stdin. A term on the line goes over the file.
    #[arg(long, value_name = "FILE")]
    input: Option<PathBuf>,
    /// Where it sits on the canvas, and how wide.
    #[arg(long, value_name = "X,Y[,W]", value_parser = card, allow_hyphen_values = true)]
    at: Option<Card>,
}

pub(super) async fn add(ctx: &Context, at: Option<&PathBuf>, args: Add) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let (names, terms): (Vec<String>, Vec<String>) = args.terms.into_iter().partition(|term| !term.contains('='));
    if names.len() > 1 {
        return Err(input_error("field_term", format!("one name, not {}: a field is field=value", names.join(" and "))));
    }
    let ask = NewAsset { tool: args.tool, at: args.at, name: names.into_iter().next(), fields: fields_of(args.input.as_ref(), &terms)? };
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
  cozy project set kyogre description='The storm whale.' lora:='[{\"name\": \"a/b\", \"strength\": 0.8}]'
  cozy project set kyogre style=                         clears it
  cozy project set kyogre tool=qwen-image-2/generate     what makes it, among those of its kind
  cozy project set film --input timeline.yaml            an Edit's tracks and clips
  cozy project set fight --if-match \"$etag\" summary='…'  only if its lines are what `show --json` read

A value is a number, true or false, or its text; := is JSON. Only what's named moves, and a comment stays.")]
pub(super) struct Set {
    /// The asset.
    name: String,
    /// `field=value` or `field:=json`; `field=` clears it. Each tool's fields: `cozy project tools --json`.
    #[arg(value_name = "FIELD=VALUE")]
    terms: Vec<String>,
    /// Its fields as YAML or JSON, `-` for stdin. A term on the line goes over the file.
    #[arg(long, value_name = "FILE")]
    input: Option<PathBuf>,
    /// Refused if its lines aren't what this `etag` read.
    #[arg(long, value_name = "ETAG")]
    if_match: Option<String>,
}

pub(super) async fn set(ctx: &Context, at: Option<&PathBuf>, args: Set) -> Result<ExitCode, Error> {
    let mut fields = fields_of(args.input.as_ref(), &args.terms)?;
    if fields.is_empty() {
        let said = "nothing to set: name a field=value, or --input a file";
        return Err(input_error("field_term", said.to_owned()).next("cozy project tools --json   each tool's fields"));
    }
    let tool = match fields.remove(TOOL) {
        Some(Value::String(tool)) => Some(tool),
        Some(other) => return Err(input_error("field_term", format!("tool is a tool's id, not {other}")).next("cozy project tools")),
        None => None,
    };
    let opened = open(ctx, at, false).await?;
    let names: Vec<&str> = fields.keys().map(String::as_str).chain(tool.as_ref().map(|_| TOOL)).collect();
    let said = names.join(", ");
    let path = route(&["v1", "projects", &opened.id, "assets", &args.name]);
    let patched: AssetPatched = ctx.calcifer().await?.patch_if(&path, &AssetPatch { fields, tool }, args.if_match.as_deref()).await?;
    patched.warnings.iter().for_each(|warning| output::warn(warning));
    match ctx.json {
        true => output::json(&patched),
        false => output::say(ctx, &format!("{}: set {said}", patched.name)),
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

/// Where boxes sit on the canvas: print it, or move one, as dragging does.
#[derive(clap::Args, Debug)]
#[command(after_help = "Examples:
  cozy project canvas                     where each sits
  cozy project canvas fight               where it sits
  cozy project canvas fight 400 120       moves it
  cozy project canvas fight 400 120 360   and makes it 360 wide
  cozy project canvas --input cards.yaml  several in one go: `fight: {x: 400, y: 120, w: 360}`, null for one to be placed again

Something with no row yet is placed by the page when it opens the project. Last move wins, and nothing here is an input.")]
pub(super) struct CanvasArgs {
    /// The asset.
    name: Option<String>,
    /// Where it goes across.
    #[arg(allow_negative_numbers = true, requires = "y")]
    x: Option<f64>,
    /// Where it goes down.
    #[arg(allow_negative_numbers = true, requires = "x")]
    y: Option<f64>,
    /// How wide.
    #[arg(requires = "y")]
    w: Option<f64>,
    /// Rows as YAML or JSON, name to `{x, y, w}`, `-` for stdin.
    #[arg(long, value_name = "FILE", conflicts_with = "name")]
    input: Option<PathBuf>,
}

pub(super) async fn canvas(ctx: &Context, at: Option<&PathBuf>, args: CanvasArgs) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let path = route(&["v1", "projects", &opened.id, "canvas"]);
    let calcifer = ctx.calcifer().await?;
    let cards: Option<BTreeMap<String, Option<Card>>> = match (&args.input, &args.name, args.x, args.y) {
        (Some(file), ..) => Some(serde_json::from_value(mapping(file)?).map_err(|why| input_error("input_unreadable", format!("{}: {why}", file.display())))?),
        (None, Some(name), Some(x), Some(y)) => Some(BTreeMap::from([(name.clone(), Some(Card { x, y, w: args.w }))])),
        _ => None,
    };
    if let Some(cards) = cards {
        let patched: CanvasPatched = calcifer.patch(&path, &CanvasPatch { cards }).await?;
        match ctx.json {
            true => output::json(&patched),
            false => output::say(ctx, "moved"),
        }
        return Ok(ExitCode::SUCCESS);
    }
    let mut held: Canvas = calcifer.get(&path).await?;
    if let Some(name) = &args.name {
        held.cards.retain(|there, _| there == name);
        if held.cards.is_empty() {
            return Err(Error::new(Kind::NotFound, "row_unknown", format!("{name} has no place yet: the page gives it one when it opens the project")).next(format!("cozy project{} open", from(at))));
        }
    }
    if ctx.json {
        output::json(&held);
        return Ok(ExitCode::SUCCESS);
    }
    let table = output::Table::new(ctx, ["NAME", "X", "Y", "W"]);
    let rows = held.cards.iter().map(|(name, card)| [name.clone(), card.x.to_string(), card.y.to_string(), card.w.map_or("-".to_owned(), |w| w.to_string())]);
    rows.fold(table, output::Table::row).print();
    Ok(ExitCode::SUCCESS)
}

/// Say what an asset shows, by a run on a machine that's up, and print it. Nothing is written.
#[derive(clap::Args, Debug)]
#[command(after_help = "Examples:
  cozy project caption kyogre --on kiyui
  cozy project set kyogre description=\"$(cozy project caption kyogre --on kiyui)\"")]
pub(super) struct Caption {
    /// The asset.
    name: String,
    /// The machine it runs on (default: as a run with none named goes).
    #[arg(long = "on", value_name = "MACHINE")]
    on: Option<String>,
}

pub(super) async fn caption(ctx: &Context, at: Option<&PathBuf>, args: Caption) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await?;
    let path = route(&["v1", "projects", &opened.id, "assets", &args.name, "caption"]);
    let captioned: Captioned = ctx.calcifer().await?.post(&path, &CaptionAsked { on: args.on }, Some(&mint_key())).await?;
    captioned.warnings.iter().for_each(|warning| output::warn(warning));
    match ctx.json {
        true => output::json(&captioned),
        false => anstream::println!("{}", captioned.text),
    }
    Ok(ExitCode::SUCCESS)
}

/// List what can make an asset: every function that makes media, of what's installed and on the Hub.
#[derive(clap::Args, Debug)]
#[command(after_help = "Examples:
  cozy project tools
  cozy project tools upscale
  cozy project tools --on kiyui --json     each one's fields, their types and limits")]
pub(super) struct ToolsArgs {
    /// Words to look for.
    query: Option<String>,
    /// The machine whose installed packages come first (default: this computer).
    #[arg(long = "on", value_name = "MACHINE")]
    on: Option<String>,
}

/// In a project, the local code it names is listed too, and anywhere else this still answers.
pub(super) async fn tools(ctx: &Context, at: Option<&PathBuf>, args: ToolsArgs) -> Result<ExitCode, Error> {
    let opened = open(ctx, at, false).await.ok();
    let hub = ctx.hub().to_string();
    let mut asked = vec![("hub", hub.as_str())];
    asked.extend(args.query.as_deref().map(|q| ("q", q)));
    asked.extend(args.on.as_deref().map(|on| ("on", on)));
    asked.extend(opened.as_ref().map(|opened| ("project", opened.id.as_str())));
    let tools: Tools = ctx.calcifer().await?.get_query("/v1/tools", &asked).await?;
    tools.note.iter().for_each(|note| output::warn(note));
    if ctx.json {
        output::json(&tools);
        return Ok(ExitCode::SUCCESS);
    }
    let table = output::Table::new(ctx, ["TOOL", "MAKES", "FIELDS", "WHAT IT DOES"]);
    let rows = tools.tools.iter().map(|tool| {
        let fields = tool.controls.iter().map(|control| control.field.as_str()).collect::<Vec<_>>().join(", ");
        [tool.id.clone(), tool.kind.clone(), fields, tool.description.clone()]
    });
    rows.fold(table, output::Table::row).print();
    Ok(ExitCode::SUCCESS)
}

/// One asset: its fields, its `etag` and each take or version.
pub(super) async fn show(ctx: &Context, at: Option<&PathBuf>, name: String) -> Result<ExitCode, Error> {
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
    if let Some(etag) = &asset.etag {
        record = record.field("etag", etag);
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
