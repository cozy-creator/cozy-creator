//! `cozy box`: the current project's boxes, made and rendered as the page makes and renders them (cl-956). Each verb is the route
//! the page calls for the same gesture, so what it does shows on an open canvas at once, and the page's renders show here: a drop
//! of a file or a tool (`add`), a box's controls (`set`), Render with Focus's Redo and Continue from (`render`), and what each holds
//! (`ls`). A `<name>` in a field mentions that box, as typed on the page: a picture is a reference, `<name.N>` its take N.
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    process::ExitCode,
};

use api::{
    calcifer::{
        NoteLevel, ProjectShowReply, Run, Shown, StreamEvent,
        project::{
            AssetMade, AssetPatch, AssetPatched, AssetVersions, Asked, Board, Import, Imported, Job, JobState, NewAsset, Pickup,
            Runs, State, VersionPrefix,
        },
        tool::Tools,
    },
    error::{Error, Kind},
    interface::Snap,
};
use serde_json::{Map, Number, Value, json};

use crate::{
    Context,
    calcifer_client::{Client, Watch, path},
    output,
    run::mint_key,
    send::Sender,
};

/// A take's frames per second when its file says none, as the page reads it.
const FPS: f64 = 24.0;
/// Seconds `--continue` waits for a take that just landed to be measured.
const MEASURED_WAIT: u32 = 30;

const EXAMPLES: &str = "Examples
  cozy box add mara ./mara.png
  cozy box add shot --tool minimax-h3/generate --input shot.yaml summary='<mara> crosses the bridge.'
  cozy box render shot
  cozy box render shot --redo 2             segment 2 of its newest take again, segment 1 kept
  cozy box add after --tool minimax-h3/generate summary='She turns back.'
  cozy box render after --continue shot.1   goes on from the end of shot's take 1
  cozy box add big --tool minimax-h3/upscale video='<shot.1>' resolution=1080p
  cozy box ls shot

A `<name>` in a field mentions that box: a picture is a reference, `<name.N>` its take N. Fields are `field=value`
(a number, true or false, else text; `field=` clears it) or `field:=<json>`. Each verb does what the page does, and an
open canvas shows it at once.";

#[derive(clap::Args, Debug)]
#[command(after_help = EXAMPLES)]
pub(crate) struct Args {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(clap::Subcommand, Debug)]
enum Verb {
    /// Make a box of a file or a tool, as a drop on the canvas does.
    Add(Add),
    /// Set a box's fields, as its controls do.
    Set(Set),
    /// Render boxes, as Render does, and follow them until their takes land.
    Render(Render),
    /// List the project's boxes, or one box's takes.
    #[command(alias = "list")]
    Ls {
        /// One box: its takes, each with its run, file, pickup and seams.
        name: Option<String>,
    },
}

#[derive(clap::Args, Debug)]
struct Add {
    /// What to call it: refused if the project has it.
    name: String,
    /// A file to make it of, and its fields as `field=value` or `field:=json`.
    #[arg(value_name = "FILE|FIELD=VALUE")]
    terms: Vec<String>,
    /// The tool that makes it: `[org/]pkg/fn`, or local code by its path. None: text, or the file's.
    #[arg(long)]
    tool: Option<String>,
    /// Read its fields from a JSON or YAML file. Fields given inline override it.
    #[arg(long = "input", value_name = "FILE")]
    input: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct Set {
    name: String,
    /// Fields as `field=value` or `field:=json`; `field=` clears one.
    #[arg(value_name = "FIELD=VALUE")]
    terms: Vec<String>,
    /// Read fields from a JSON or YAML file. Fields given inline override it.
    #[arg(long = "input", value_name = "FILE")]
    input: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct Render {
    /// The boxes, or `all`. Each gets a new take.
    #[arg(required = true, value_name = "NAME")]
    names: Vec<String>,
    /// Render on `local`, a machine, or several (`a,b`: each box on whichever finishes it first). Default: the project's machines
    /// (`cozy machine use`), else any.
    #[arg(long = "on", value_name = "MACHINE[,MACHINE]")]
    on: Option<String>,
    /// Takes per box.
    #[arg(short = 'n', value_name = "N", default_value_t = 1)]
    n: u32,
    /// Render segment SEGMENT (2 or later) and the rest again, keeping the segments before it: Focus's Redo.
    #[arg(long, value_name = "SEGMENT", conflicts_with = "from")]
    redo: Option<u32>,
    /// With --redo: of this take. Default: the newest.
    #[arg(long, value_name = "N", requires = "redo")]
    take: Option<u32>,
    /// Go on from the end of another box's take, `<box.N>` (`<box>`: its newest): Continue from.
    #[arg(long = "continue", value_name = "BOX.N")]
    from: Option<String>,
}

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    let project = Project::current(ctx).await?;
    match args.verb {
        Verb::Add(add) => project.add(ctx, add).await,
        Verb::Set(set) => project.set(ctx, set).await,
        Verb::Render(render) => project.render(ctx, render).await,
        Verb::Ls { name: None } => project.list(ctx).await,
        Verb::Ls { name: Some(name) } => project.takes(ctx, &name).await,
    }
}

/// The project the current directory is in, as the routes name it.
struct Project {
    id: String,
    path: PathBuf,
    calcifer: Client,
}

impl Project {
    async fn current(ctx: &Context) -> Result<Self, Error> {
        let id = crate::project::identity(ctx).await?.id;
        let calcifer = ctx.calcifer().await?;
        let head: State = calcifer.get(&format!("{}?assets=false", path(&["v1", "projects", &id]))).await?;
        Ok(Self { id: head.id, path: head.path, calcifer })
    }

    fn at(&self, rest: &[&str]) -> String {
        path(&[["v1", "projects", self.id.as_str()].as_slice(), rest].concat())
    }

    async fn versions(&self, name: &str) -> Result<AssetVersions, Error> {
        self.calcifer.get(&self.at(&["assets", name])).await
    }

    async fn add(&self, ctx: &Context, args: Add) -> Result<ExitCode, Error> {
        let (files, terms): (Vec<String>, Vec<String>) = args.terms.into_iter().partition(|term| !term.contains('='));
        let fields = fields_of(ctx, args.input.as_deref(), &terms)?;
        let tool = args.tool.map(|tool| self.tool(ctx, &tool)).transpose()?;
        let made = match files.as_slice() {
            [] => {
                let fields = fields.into_iter().filter(|(_, value)| !value.is_null()).collect();
                let ask = NewAsset { tool: tool.unwrap_or_default(), name: Some(args.name), exact: true, fields, at: None, kind: None, moves: BTreeMap::new(), placed: BTreeMap::new() };
                let made: AssetMade = self.calcifer.post(&self.at(&["assets"]), &ask, Some(&mint_key())).await?;
                json!(made)
            }
            [file] => {
                let digest = Sender::for_project(ctx, &self.id).await?.file(&api::target::spelled(file, &cwd()?, &ctx.user_home)).await?;
                let ask = Import { source: digest.to_string(), name: Some(args.name), exact: true, at: None, description: None, moves: BTreeMap::new(), placed: BTreeMap::new() };
                let imported: Imported = self.calcifer.post(&self.at(&["imports"]), &ask, Some(&mint_key())).await?;
                // A tool or fields on it, as the page puts a tool on a picture: it starts from that picture.
                if tool.is_some() || !fields.is_empty() {
                    let patched: AssetPatched = self.calcifer.patch(&self.at(&["assets", &imported.name]), &AssetPatch { fields, tool, typed: None }).await?;
                    said(&patched.warnings);
                }
                json!(imported)
            }
            more => return Err(usage("box_terms", format!("one file makes a box, not {}: a field is field=value", more.join(" and ")))),
        };
        match ctx.json {
            true => output::json(&made),
            false => anstream::println!("{}", made["name"].as_str().unwrap_or_default()),
        }
        Ok(ExitCode::SUCCESS)
    }

    async fn set(&self, ctx: &Context, args: Set) -> Result<ExitCode, Error> {
        if let Some(file) = args.terms.iter().find(|term| !term.contains('=')) {
            return Err(usage("box_terms", format!("{file} isn't field=value")).next(format!("cozy box set {} --help", args.name)));
        }
        let fields = fields_of(ctx, args.input.as_deref(), &args.terms)?;
        let patched: AssetPatched = self.calcifer.patch(&self.at(&["assets", &args.name]), &AssetPatch { fields, tool: None, typed: None }).await?;
        match ctx.json {
            true => output::json(&patched),
            false => {
                said(&patched.warnings);
                anstream::println!("{}", patched.name);
            }
        }
        Ok(ExitCode::SUCCESS)
    }

    /// A tool as a box names it: local code by its path from the project's folder when it's in it, else wherever it is.
    fn tool(&self, ctx: &Context, tool: &str) -> Result<String, Error> {
        if !api::target::written_as_path(tool) {
            return Ok(tool.to_owned());
        }
        let at = std::path::absolute(api::target::spelled(tool, &cwd()?, &ctx.user_home)).map_err(|why| usage("tool_path", format!("{tool}: {why}")))?;
        Ok(match at.strip_prefix(&self.path) {
            Ok(within) => format!("./{}", within.to_string_lossy()),
            Err(_) => at.to_string_lossy().into_owned(),
        })
    }

    async fn render(&self, ctx: &Context, args: Render) -> Result<ExitCode, Error> {
        let one = match args.names.as_slice() {
            [one] if one != "all" => Some(one.as_str()),
            _ => None,
        };
        let pickup = match (args.redo, &args.from, one) {
            (None, None, _) => None,
            (_, _, None) => return Err(usage("pickup_one_box", "--redo and --continue are for one box")),
            (Some(segment), _, Some(name)) => Some(self.redo(name, segment, args.take).await?),
            (None, Some(from), Some(name)) => Some(self.continues(name, from).await?),
        };
        let on = args.on.as_deref().map(api::calcifer::on_set).transpose()?.unwrap_or_default();
        // The page's stream, showing this project, from before the ask: every change of its render from here on.
        let mut watch = self.watch().await?;
        let ask = Runs { names: args.names, n: args.n.max(1), on, hub: ctx.hub().to_string(), approve: false, missing: false, outdated: false, pickup, takes: vec![] };
        let asked: Asked = self.calcifer.post(&self.at(&["runs"]), &ask, Some(&mint_key())).await?;
        let mut follow = Following::new(ctx, asked.board.clone());
        follow.asked(&asked);
        if asked.added == 0 {
            return Ok(ExitCode::SUCCESS);
        }
        while !follow.ended(asked.ask.as_deref()) {
            match watch.change().await {
                Some(StreamEvent::BoardUpdated { project, board }) if project == self.id => follow.board(*board),
                Some(StreamEvent::JobUpdated { project, job, seq }) if project == self.id => follow.job(*job, seq),
                Some(_) => {}
                // Behind or closed: a new stream first, then the board as it stands, so nothing falls between them.
                None => {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    watch = self.watch().await?;
                    let head: State = self.calcifer.get(&format!("{}?assets=false", self.at(&[]))).await?;
                    if let Some(board) = head.board {
                        follow.board(board);
                    }
                }
            }
        }
        let ended = follow.now.ended.clone().unwrap_or_default();
        let takes = self.landed(&follow.now.jobs).await?;
        match ctx.json {
            true => output::ndjson(&json!({"type": "ended", "ended": ended, "takes": takes})),
            false => {
                for take in &takes {
                    anstream::println!("{}", take_line(take));
                }
                if let Some(name) = takes.first().and_then(|take| take["name"].as_str()) {
                    output::next(ctx, &[&format!("cozy box ls {name}")]);
                }
            }
        }
        match ended.error {
            Some(error) => Err(error),
            None => Ok(ExitCode::SUCCESS),
        }
    }

    /// `GET /v1/events`, showing this project as the page does (`PUT /v1/events/{stream}`): its render's changes come on it.
    async fn watch(&self) -> Result<Watch<StreamEvent>, Error> {
        let mut watch = self.calcifer.changes().await?;
        let Some(StreamEvent::Hello { stream, .. }) = watch.change().await else {
            return Err(Error::new(Kind::Unavailable, "events_unanswered", "Calcifer's event stream ended before it began").next("cozy doctor"));
        };
        let shown: ProjectShowReply = self.calcifer.put(&path(&["v1", "events", &stream]), &Shown { projects: vec![self.id.clone()] }).await?;
        match shown.refused.into_values().next() {
            Some(refused) => Err(refused.error),
            None => Ok(watch),
        }
    }

    /// Focus's Redo segment N: the take kept up to the seam segment N starts at, as its run said its seams (se-130), and the box's beats
    /// from there.
    async fn redo(&self, name: &str, segment: u32, take: Option<u32>) -> Result<Pickup, Error> {
        let held = self.versions(name).await?;
        let number = match take {
            Some(number) => number,
            None => held.asset.prefixes.iter().rev().find(|prefix| is_video(prefix)).map(|prefix| prefix.number)
                .ok_or_else(|| usage("pickup_no_take", format!("{name} has no take to redo")).next(format!("cozy box render {name}")))?,
        };
        let prefix = held.asset.prefixes.iter().find(|prefix| prefix.number == number && is_video(prefix))
            .ok_or_else(|| usage("pickup_no_take", format!("{name} has no take {number} with a video")).next(format!("cozy box ls {name}")))?;
        if segment < 2 {
            return Err(usage("redo_first_segment", format!("redoing segment 1 of <{name}.{number}> is a new take")).next(format!("cozy box render {name}")));
        }
        let seams = match prefix.run {
            Some(run) => seams(&self.calcifer.run(&run.to_string()).await?).unwrap_or_default(),
            None => vec![],
        };
        let Some(&seam) = seams.get(segment as usize - 2) else {
            let said = format!("<{name}.{number}> has {} segment{}", seams.len() + 1, if seams.is_empty() { "" } else { "s" });
            return Err(usage("redo_no_segment", said).next(format!("cozy box ls {name}")));
        };
        let fps = prefix.main.as_ref().and_then(|main| main.fps).unwrap_or(FPS);
        Ok(Pickup { take: format!("<{name}.{number}>"), out_frame: seam, beats_from: Some(f64::from(seam) / fps) })
    }

    /// Continue from: `name`'s new take goes on from the end of `from`'s, on the frames its tool picks up at, its beats from their start.
    async fn continues(&self, name: &str, from: &str) -> Result<Pickup, Error> {
        let mention = from.trim().trim_start_matches('<').trim_end_matches('>');
        let (of, number) = match mention.rsplit_once('.') {
            Some((of, number)) => (of, Some(number.parse::<u32>().map_err(|_| usage("continue_malformed", format!("--continue {from}: it's <box.N> or <box>")))?)),
            None => (mention, None),
        };
        // A take that just landed is measured a moment later: its length is what this goes on from.
        let mut tries = 0;
        let (prefix, ms) = loop {
            let held = self.versions(of).await?;
            let prefix = match number {
                Some(number) => held.asset.prefixes.into_iter().find(|prefix| prefix.number == number && is_video(prefix)),
                None => held.asset.prefixes.into_iter().rev().find(is_video),
            };
            let Some(prefix) = prefix else {
                return Err(usage("pickup_no_take", format!("{of} has no take {} with a video", number.map_or_else(|| "yet".to_owned(), |n| n.to_string()))).next(format!("cozy box ls {of}")));
            };
            match prefix.main.as_ref().and_then(|main| main.duration_ms).filter(|ms| *ms > 0) {
                Some(ms) => break (prefix, ms),
                None if tries < MEASURED_WAIT => tries += 1,
                None => return Err(Error::new(Kind::Unavailable, "take_unmeasured", format!("<{of}.{}>'s length isn't measured", prefix.number)).next(format!("cozy box ls {of}"))),
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        };
        let fps = prefix.main.as_ref().and_then(|main| main.fps).unwrap_or(FPS);
        // Its length as the page counts it (`framesIn`), so both send the same frame.
        let last = ((ms as f64 / 1000.0) * fps).round().max(1.0) as u32;
        let snap = self.snap(name).await?;
        let Some(end) = pickup_at(last, last, snap) else {
            return Err(usage("continue_too_short", format!("<{of}.{}> is shorter than {name}'s tool picks up from", prefix.number)));
        };
        Ok(Pickup { take: format!("<{of}.{}>", prefix.number), out_frame: end, beats_from: Some(0.0) })
    }

    /// The frames `name`'s tool picks up at, as the page lists its tools (`GET /v1/tools`); its own when it lists none.
    async fn snap(&self, name: &str) -> Result<Snap, Error> {
        let Some(tool) = self.versions(name).await?.asset.tool else { return Ok(Snap::default()) };
        let tools: Tools = self.calcifer.get_query(&path(&["v1", "tools"]), &[("project", self.id.as_str())]).await?;
        Ok(tools.tools.into_iter().find(|listed| listed.id == tool).and_then(|listed| listed.pickup).unwrap_or_default())
    }

    /// The takes `jobs` made, each with its file, run, pickup and seams.
    async fn landed(&self, jobs: &[Job]) -> Result<Vec<Value>, Error> {
        let mut held = HashMap::<String, AssetVersions>::new();
        let mut takes = vec![];
        for job in jobs.iter().filter(|job| job.state == JobState::Done) {
            let Some(number) = job.number else { continue };
            if !held.contains_key(&job.asset) {
                held.insert(job.asset.clone(), self.versions(&job.asset).await?);
            }
            takes.push(self.take(&held[&job.asset], number).await?);
        }
        Ok(takes)
    }

    /// One take or version: its file, the run that made it, what it picked up from, its seams and its warnings.
    async fn take(&self, held: &AssetVersions, number: u32) -> Result<Value, Error> {
        let version = held.versions.iter().find(|version| version.number == number);
        let prefix = held.asset.prefixes.iter().find(|prefix| prefix.number == number);
        let run = prefix.and_then(|prefix| prefix.run).or_else(|| version.and_then(|version| version.runs.last().copied()));
        let seams = match (run, prefix.is_some_and(is_video)) {
            (Some(run), true) => seams(&self.calcifer.run(&run.to_string()).await?),
            _ => None,
        };
        Ok(json!({
            "name": held.asset.name,
            "number": number,
            "noun": held.asset.noun,
            "run": run,
            "path": prefix.and_then(|prefix| prefix.main.as_ref()).map(|main| &main.path),
            "approved": held.asset.approved == Some(number),
            "pickup": version.and_then(|version| version.pickup.as_ref()),
            "seams": seams,
            "warnings": version.map(|version| &version.warnings),
        }))
    }

    async fn list(&self, ctx: &Context) -> Result<ExitCode, Error> {
        let state: State = self.calcifer.get(&self.at(&[])).await?;
        if ctx.json {
            let boxes: Vec<Value> = match ctx.full {
                true => state.assets.iter().map(|asset| json!(asset)).collect(),
                false => state.assets.iter().map(|asset| json!({
                    "name": asset.name, "kind": asset.kind, "tool": asset.tool, "versions": asset.versions, "version": asset.version,
                    "approved": asset.approved, "status": asset.status,
                })).collect(),
            };
            output::json(&json!({"boxes": boxes}));
            return Ok(ExitCode::SUCCESS);
        }
        let mut table = output::Table::new(ctx, ["NAME", "KIND", "TOOL", "TAKES", "STATUS"]);
        for asset in &state.assets {
            let takes = asset.versions.to_string();
            table = table.row([asset.name.as_str(), asset.kind.as_str(), asset.tool.as_deref().unwrap_or("-"), takes.as_str(), asset.status.as_str()]);
        }
        if state.assets.is_empty() {
            table = table.note("no boxes yet: cozy box add <name> --tool <pkg/fn>, or a file");
        }
        table.print();
        Ok(ExitCode::SUCCESS)
    }

    async fn takes(&self, ctx: &Context, name: &str) -> Result<ExitCode, Error> {
        let held = self.versions(name).await?;
        let mut takes = vec![];
        for version in &held.versions {
            takes.push(self.take(&held, version.number).await?);
        }
        if ctx.json {
            let asset = &held.asset;
            output::json(&json!({
                "name": asset.name, "kind": asset.kind, "tool": asset.tool, "status": asset.status, "fields": asset.fields, "takes": takes,
            }));
            return Ok(ExitCode::SUCCESS);
        }
        let mut table = output::Table::new(ctx, [held.asset.noun.to_uppercase().as_str(), "RUN", "FILE", "FROM", "SEAMS"]);
        for take in &takes {
            let mark = if take["approved"] == true { "*" } else { "" };
            table = table.row([
                format!("{}{mark}", take["number"]),
                take["run"].as_u64().map_or_else(|| "-".to_owned(), |run| run.to_string()),
                take["path"].as_str().unwrap_or("-").to_owned(),
                take["pickup"]["take"].as_str().map_or_else(|| "-".to_owned(), |from| format!("{from} at {}", take["pickup"]["out_frame"])),
                frames(&take["seams"]),
            ]);
        }
        let tool = held.asset.tool.as_deref().map_or_else(String::new, |tool| format!(" of {tool}"));
        table.note(format!("{name}{tool}: {}", held.asset.status)).print();
        Ok(ExitCode::SUCCESS)
    }
}

/// A render as the board says it, each change of a job said once, for a person on stderr or as an NDJSON line.
struct Following<'a> {
    ctx: &'a Context,
    now: Board,
    told: HashMap<String, (JobState, Option<u32>)>,
    said: usize,
}

impl<'a> Following<'a> {
    fn new(ctx: &'a Context, board: Board) -> Self {
        Self { ctx, now: board, told: HashMap::new(), said: 0 }
    }

    fn asked(&mut self, asked: &Asked) {
        if self.ctx.json {
            output::ndjson(&json!({"type": "asked", "ask": asked.ask, "joined": asked.joined, "queued": asked.queued, "skipped": asked.skipped, "said": asked.said}));
        } else {
            for said in &asked.said {
                say(said.level, &said.message);
            }
            for name in &asked.skipped {
                output::say(self.ctx, &format!("{name}: approved, left as it is"));
            }
        }
        self.tell();
    }

    /// Its walk ended: `ask`'s, or with none (it joined one going and added nothing), that one.
    fn ended(&self, ask: Option<&str>) -> bool {
        self.now.ended.as_ref().is_some_and(|ended| ask.is_none_or(|ask| ended.asks.iter().any(|held| held == ask)))
    }

    fn board(&mut self, board: Board) {
        if board.seq >= self.now.seq {
            self.now = board;
            self.tell();
        }
    }

    fn job(&mut self, job: Job, seq: u64) {
        if seq < self.now.seq {
            return;
        }
        self.now.seq = seq;
        match self.now.jobs.iter_mut().find(|held| held.id == job.id) {
            Some(held) => *held = job,
            None => self.now.jobs.push(job),
        }
        self.tell();
    }

    fn tell(&mut self) {
        for job in &self.now.jobs {
            let now = (job.state, job.number);
            if self.told.get(&job.id) == Some(&now) {
                continue;
            }
            self.told.insert(job.id.clone(), now);
            if self.ctx.json {
                output::ndjson(&json!({"type": "job", "job": job}));
                continue;
            }
            let number = job.number.map_or_else(String::new, |number| format!(" {number}"));
            let state = match job.state {
                JobState::Waiting if !job.needs.is_empty() => format!("waits for {}", job.needs.join(", ")),
                JobState::NotSent => "not sent".to_owned(),
                state => json!(state).as_str().unwrap_or_default().to_owned(),
            };
            output::say(self.ctx, &format!("{}: {}{number} {state}", job.asset, job.noun));
        }
        for said in self.now.said.iter().skip(self.said) {
            match self.ctx.json {
                true => output::ndjson(&json!({"type": "said", "said": said})),
                false => say(said.level, &said.message),
            }
        }
        self.said = self.said.max(self.now.said.len());
    }
}

fn say(level: NoteLevel, message: &str) {
    match level {
        NoteLevel::Warning => output::warn(message),
        NoteLevel::Info => output::warned(message),
    }
}

fn said(warnings: &[String]) {
    warnings.iter().for_each(|warning| output::warn(warning));
}

/// `shot: take 2 files/….mp4 (run 41, seams 6)`.
fn take_line(take: &Value) -> String {
    let mut about = vec![];
    if let Some(run) = take["run"].as_u64() {
        about.push(format!("run {run}"));
    }
    if take["seams"].is_array() {
        about.push(format!("seams {}", frames(&take["seams"])));
    }
    if let Some(from) = take["pickup"]["take"].as_str() {
        about.push(format!("from {from} at {}", take["pickup"]["out_frame"]));
    }
    let mut line = format!("{}: {} {} {}", take["name"].as_str().unwrap_or_default(), take["noun"].as_str().unwrap_or_default(), take["number"], take["path"].as_str().unwrap_or("-"));
    if !about.is_empty() {
        line.push_str(&format!(" ({})", about.join(", ")));
    }
    for warning in take["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        line.push_str(&format!("\n  warning: {warning}"));
    }
    line
}

fn frames(seams: &Value) -> String {
    match seams.as_array() {
        Some(seams) if !seams.is_empty() => seams.iter().map(Value::to_string).collect::<Vec<_>>().join(","),
        _ => "-".to_owned(),
    }
}

fn is_video(prefix: &VersionPrefix) -> bool {
    prefix.main.as_ref().is_some_and(|main| main.field == "video")
}

/// Where each segment after the first starts, as the run that made a take said it (`result.seams`, se-130).
fn seams(run: &Run) -> Option<Vec<u32>> {
    let seams = run.result.as_ref()?.get("seams")?.as_array()?;
    seams.iter().map(|seam| seam.as_u64().and_then(|seam| u32::try_from(seam).ok())).collect()
}

/// The page's `pickupAt`: `frame` (no later than `last`) on the frames `snap` picks up at, at or before it; none before its first.
fn pickup_at(frame: u32, last: u32, snap: Snap) -> Option<u32> {
    let (at, every, offset, first) = (i64::from(frame.min(last)), i64::from(snap.every.max(1)), i64::from(snap.offset), i64::from(snap.first));
    (at >= first).then(|| first.max(offset + (at - offset).div_euclid(every) * every) as u32)
}

/// A file's mapping of fields, with `field=value` and `field:=json` over it.
fn fields_of(ctx: &Context, input: Option<&Path>, terms: &[String]) -> Result<Map<String, Value>, Error> {
    let mut fields = match input {
        Some(file) => {
            let at = api::target::spelled(&file.to_string_lossy(), &cwd()?, &ctx.user_home);
            let what = format!("--input {}", file.display());
            let text = std::fs::read_to_string(&at).map_err(|why| Error::new(Kind::NotFound, "input_unreadable", format!("Cannot read {what}: {why}")))?;
            match serde_json::from_str(&text).or_else(|_| api::yaml::to_json(&text)) {
                Ok(Value::Object(fields)) => fields,
                _ => return Err(Error::new(Kind::Validation, "input_invalid", format!("{what} must hold a JSON or YAML mapping of field to value"))),
            }
        }
        None => Map::new(),
    };
    for term in terms {
        let (field, value) = term.split_once('=').expect("terms hold =");
        let (field, value) = match field.strip_suffix(':') {
            Some(field) => (field, serde_json::from_str(value).map_err(|why| usage("json_malformed", format!("{term}: {why}")).remedy("`field:=<json>` is JSON; `field=value` is a number, true, false or text"))?),
            None => (field, scalar(value)),
        };
        fields.insert(field.to_owned(), value);
    }
    Ok(fields)
}

/// `value` as typed: nothing clears the field, a plain number or `true` or `false` is that, anything else its text.
fn scalar(text: &str) -> Value {
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let number = unsigned.split_once('.').map_or_else(|| digits(unsigned), |(whole, part)| digits(whole) && digits(part));
    match text {
        "" => Value::Null,
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ if number => text.parse::<Number>().map_or_else(|_| Value::String(text.to_owned()), Value::Number),
        _ => Value::String(text.to_owned()),
    }
}

fn cwd() -> Result<PathBuf, Error> {
    std::env::current_dir().map_err(|why| Error::new(Kind::Unavailable, "cwd", format!("the current directory: {why}")))
}

fn usage(code: &str, message: impl Into<String>) -> Error {
    Error::new(Kind::Usage, code, message)
}
