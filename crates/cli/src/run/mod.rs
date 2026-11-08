//! `cozy run`: run a function on a machine, and the verbs over runs. Calcifer resolves, binds and
//! submits a run (`POST /v1/runs`); this module turns arguments into its request and prints.
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::ValueHint;
use clap_complete::ArgValueCompleter;
use protocol::{
    calcifer::{FileTerm, On, ReleaseFrom, Run, RunRequest, Target},
    code::LocalCode,
    error::{Error, Kind},
    machine::WarmLevel,
};
use serde_json::{Map, Value};

use crate::{
    Context, output,
    send::{Paths, Sender},
};
pub(crate) use follow::{exit_code, follow, follow_until, mint_key, settle, settled, stalled_words, watched};

pub(crate) mod fetch;
mod follow;
pub(crate) mod list;
pub(crate) mod live;
mod play;
mod show;

/// `cozy run --help`'s examples: a YAML request file first, inline inputs as overrides.
const EXAMPLES: &str = "Examples:
  cozy run sdxl/generate --input request.yaml
      request.yaml:  prompt: a lighthouse at dusk, oil painting
                     aspect_ratio: \"16:9\"
                     seed: 7
  cozy run sdxl/generate --input request.yaml seed=8   (an inline input overrides the file)
  cozy run sdxl/generate --input request.yaml --lora org/sdxl-lora,0.8
  cozy run birefnet/remove_background --file image=./character.png
  cozy run sdxl/generate prompt='a fox in fresh snow'  (inline, for quick runs)
  cozy run sdxl/generate --describe                    (the fields it takes)
  cozy run ./my_package/generate prompt=x -o out       (local code, sent by content; its outputs downloaded to out/)
  cozy run ./my_package/edit 'video=<clip:48-144>'    (frames 48 to 144 of the project's box clip)

A run belongs to a project: -p, else COZY_PROJECT. Its outputs land there; -o downloads them here.
A run is followed to its end: Ctrl-C stops following, never the run; `cozy run show N` follows it again.";

#[derive(clap::Args, Debug)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true, arg_required_else_help = true, after_help = EXAMPLES)]
pub struct Args {
    /// The project a run belongs to: its name or id, or its folder on Calcifer's computer by its absolute path. Default:
    /// COZY_PROJECT. `ls` lists that project's.
    #[arg(short = 'p', value_name = "PROJECT", global = true)]
    project: Option<String>,
    #[command(subcommand)]
    verb: Option<Verb>,
    #[command(flatten)]
    run: Box<RunArgs>,
}

#[derive(clap::Args, Debug)]
struct RunArgs {
    /// `[org/]pkg[/fn][@release]` (no org: first-party), `./dir[/fn]` or `./script.py[/fn]`;
    /// without a function, its functions (local code's one function runs).
    #[arg(required = true, value_name = "TARGET", add = ArgValueCompleter::new(crate::completion::local))]
    target: Option<String>,
    /// `key=value`, `key:=json`, `model.<slot>=<ref>`, one positional value, input files.
    #[arg(value_name = "INPUT", value_hint = ValueHint::FilePath)]
    inputs: Vec<String>,
    #[command(flatten)]
    placement: Placement,
    /// Use at most N GPUs for the run and its calls.
    #[arg(long, value_name = "N", value_parser = protocol::calcifer::gpu_count)]
    gpus: Option<u32>,
    /// The lane of the function's main model (a label: bf16, fp8, int8, …); its LoRAs and other
    /// models take their own. Absent, the Hub picks what this machine's hardware runs best.
    #[arg(long, value_parser = protocol::model_ref::lane)]
    lane: Option<String>,
    /// Pin an attention kernel for this run: everywhere, or at one `[slot/]component`.
    #[arg(long, value_name = "[[SLOT/]COMPONENT=]KERNEL", value_parser = protocol::calcifer::attention_pin, conflicts_with = "warm")]
    attention_kernel: Option<String>,
    /// The whole payload from a JSON or YAML file; inline inputs override it.
    #[arg(long = "input", value_name = "FILE")]
    input_file: Option<PathBuf>,
    /// Attach a file: `field.path=FILE`, `label=FILE`, or `FILE` into the declared files.
    #[arg(long = "file", value_name = "[NAME=]FILE", value_hint = ValueHint::FilePath)]
    assets: Vec<String>,
    /// A hint for one attached file: `label-or-index=auto|low|medium|high`.
    #[arg(long = "file-fidelity", value_name = "FILE=FIDELITY", value_parser = protocol::calcifer::fidelity)]
    fidelities: Vec<String>,
    /// Apply a LoRA, in order: `org/lora[,strength]` adapts the function's model; `slot=org/lora`
    /// names one of several (strength 1 by default).
    #[arg(long = "lora", value_name = "[SLOT=]REF[,STRENGTH]")]
    loras: Vec<String>,
    /// Keep the function ready instead of running it.
    #[arg(long, value_enum, ignore_case = true, value_name = "LEVEL")]
    warm: Option<Warm>,
    /// Print the function's request contract instead of running it.
    #[arg(long)]
    describe: bool,
    /// Download its outputs into this folder once they land, each named `<run>-<output>.<ext>`.
    #[arg(short = 'o', long = "output", value_name = "DIR", value_hint = ValueHint::DirPath)]
    output: Option<PathBuf>,
}

/// Where a run goes; shared by every verb that submits one.
#[derive(clap::Args, Debug, Clone)]
pub struct Placement {
    /// `local` (the default), a machine's name, `any`, or `new[:sku]` to rent one.
    #[arg(long = "on", value_name = "MACHINE")]
    on: Option<String>,
}

impl Placement {
    /// Where the run goes; `local` refused where this OS runs no models itself.
    pub fn on(&self) -> Result<On, Error> {
        let on = self.named();
        protocol::calcifer::local_usable(&on.to_string())?;
        Ok(on)
    }

    /// Where it would go, for Calcifer to judge.
    pub fn named(&self) -> On {
        On::from(self.on.clone().unwrap_or_else(|| "local".to_owned()))
    }
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Warm {
    Installed,
    Downloaded,
    Imported,
    Host,
    Gpu,
    /// Take it out of the machine's warm set.
    Off,
}

#[derive(clap::Subcommand, Debug)]
enum Verb {
    /// Every run, newest first; live on a terminal.
    #[command(alias = "list")]
    Ls {
        /// Only runs in this state: queued, in_progress (or running), completed, failed or canceled.
        #[arg(long, value_name = "STATE", value_parser = run_state)]
        state: Option<String>,
        /// Only this package's runs.
        #[arg(long)]
        package: Option<String>,
        /// Only the runs on this machine: its name or alias, or `local`.
        #[arg(long = "on", value_name = "MACHINE")]
        on: Option<String>,
        /// At most N runs (default 50; 0 shows all).
        #[arg(long, value_name = "N")]
        limit: Option<u64>,
        /// Every project's runs, not this one's.
        #[arg(long)]
        all: bool,
    },
    /// One run: its result, outputs, calls, logs and how it ran. A run not yet settled is
    /// followed to its end (Ctrl-C stops following, never the run); its exit code is the run's.
    Show {
        /// The run's number or id.
        run: String,
        /// A snapshot of a run still going, instead of following it.
        #[arg(long)]
        once: bool,
        /// Download its outputs into this folder, each named `<run>-<output>.<ext>`.
        #[arg(short = 'o', long = "output", value_name = "DIR", value_hint = ValueHint::DirPath)]
        output: Option<PathBuf>,
    },
    /// Cancel a queued or running run.
    Cancel {
        /// The run's number or id.
        run: String,
    },
    /// Print a link that plays a run's output in a browser, straight from its machine.
    Play {
        /// The run's number or id.
        run: String,
        /// The output to play: its name, or name/index for one item of a list.
        #[arg(long, default_value = "video")]
        output: String,
        /// How long the link works: anyone holding it may watch until then.
        #[arg(long, default_value = "24h")]
        expires: String,
    },
}

/// A state `run ls` filters on, in any case, with its plain synonyms.
fn run_state(text: &str) -> Result<String, String> {
    match text.to_ascii_lowercase().as_str() {
        state @ ("queued" | "in_progress" | "completed" | "failed" | "canceled") => Ok(state.to_owned()),
        "running" => Ok("in_progress".to_owned()),
        "cancelled" => Ok("canceled".to_owned()),
        _ => Err("use queued, in_progress, completed, failed or canceled".to_owned()),
    }
}

pub async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    let within = crate::project::chosen(args.project.as_ref());
    match args.verb {
        Some(Verb::Ls {
            state,
            package,
            on,
            limit,
            all,
        }) => {
            // A project named: only its runs.
            let project = match within.filter(|_| !all) {
                Some(named) => Some(crate::project::open(ctx, Some(&named)).await?.id),
                None => None,
            };
            let query = list::Query { status: state, package, machine: on, project, before: None, limit };
            list::exec(ctx, query).await
        }
        Some(Verb::Show { run, once, output }) => show::exec(ctx, &run, once, output.as_deref()).await,
        Some(Verb::Cancel { run }) => {
            let calcifer = ctx.calcifer().await?;
            let mut run = calcifer.run(&run).await?;
            if !follow::settled(&run) {
                run = calcifer.cancel(run.id.as_str()).await?;
            }
            follow::record(ctx, &run);
            Ok(ExitCode::SUCCESS)
        }
        Some(Verb::Play {
            run,
            output,
            expires,
        }) => play::exec(ctx, &run, &output, &expires).await,
        None => run(ctx, *args.run, within).await,
    }
}


async fn run(ctx: &Context, args: RunArgs, within: Option<String>) -> Result<ExitCode, Error> {
    let cwd = std::env::current_dir()
        .map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let written = args.target.clone().unwrap_or_default();
    // Local code goes by content: the target is then the function it names.
    let (target, code) = match local(ctx, &written, &cwd)? {
        Some((package, function)) => (function.unwrap_or_default(), Some(Sender::new(ctx).await?.code(&package).await?)),
        None => (written, None),
    };
    let inputs = !args.inputs.is_empty() || args.input_file.is_some();
    if args.describe && !inputs {
        return describe(ctx, &target, code.as_ref(), args.placement.on.as_deref()).await;
    }
    let ask = request(ctx, &args, target.clone(), code.clone(), &cwd, within).await?;
    match send(ctx, ask).await {
        // A package's functions, when the run names none and gives it nothing.
        Err(e) if e.code == "function_unnamed" && !inputs => functions(ctx, &target, code.as_ref(), args.placement.on.as_deref()).await,
        Err(e) => Err(e),
        Ok(run) => {
            if let Some(dir) = &args.output {
                downloaded(ctx, &run, dir).await?;
            }
            paste(ctx, &run);
            Ok(exit_code(&run))
        }
    }
}

/// `--output`: its outputs downloaded into `dir`, each said where it went.
pub(crate) async fn downloaded(ctx: &Context, run: &Run, dir: &Path) -> Result<(), Error> {
    for (name, path) in fetch::outputs(ctx, run, dir).await? {
        if !ctx.json {
            output::say(ctx, &format!("{name}: {}", output::link(ctx, &std::path::absolute(&path).unwrap_or(path.clone()))));
        }
    }
    Ok(())
}

/// One line per file a run left in its project, to paste into a box: `image: 65b436d4f9a3`.
pub(crate) fn paste(ctx: &Context, run: &Run) {
    if ctx.json || run.project.is_none() {
        return;
    }
    for output in follow::for_people(run) {
        let Some(sha256) = output.url.as_ref().and(output.sha256.as_ref()) else { continue };
        let field = match output.media_type.split('/').next() {
            Some(kind @ ("image" | "video" | "audio")) => kind,
            _ => output.name.as_str(),
        };
        let hex = sha256.to_string();
        let hex = hex.trim_start_matches("sha256:");
        println!("{field}: {}", &hex[..hex.len().min(12)]);
    }
}

/// The package (a folder or a script) and function `raw` names when it is local code here (`./dir[/fn]`, `x.py[/fn]`): it goes
/// by content, as Calcifer reads no client's folder. A path that names neither is refused.
pub(crate) fn local(ctx: &Context, raw: &str, cwd: &Path) -> Result<Option<(PathBuf, Option<String>)>, Error> {
    let Some(path) = protocol::target::local(raw, cwd, &ctx.user_home) else { return Ok(None) };
    let found = protocol::code::split(&path).ok_or_else(|| {
        Error::new(Kind::NotFound, "target_not_found", format!("{raw} is not a package directory or a script here"))
    })?;
    Ok(Some(found))
}

/// The request `args` make of `target` (of `code`, when it's local), with every file and folder it names here sent first.
async fn request(ctx: &Context, args: &RunArgs, target: String, code: Option<LocalCode>, cwd: &Path, within: Option<String>) -> Result<RunRequest, Error> {
    let sender = Sender::new(ctx).await?;
    let mut paths = Paths::new(&sender);
    for term in &args.inputs {
        term_paths(&mut paths, term, cwd).await?;
    }
    let files: Vec<FileTerm> = args.assets.iter().map(|raw| FileTerm::parse(raw)).collect();
    for file in &files {
        paths.add(&file.file, cwd, false, "--file").await?;
    }
    let input = match &args.input_file {
        Some(file) => {
            let (input, dir) = read_input(ctx, file, cwd)?;
            paths.add_all(&Value::Object(input.clone()), &dir, &format!("--input {}", file.display())).await?;
            Some(input)
        }
        None => None,
    };
    Ok(RunRequest {
        target,
        code,
        // None: Calcifer's to say, by what this OS runs and the machines there are (`default_on`).
        on: args.placement.on.clone().map(On::from),
        hub: ctx.hub().to_string(),
        input,
        input_text: None,
        terms: args.inputs.clone(),
        files,
        fidelities: args.fidelities.clone(),
        loras: args.loras.clone(),
        paths: paths.sent(),
        gpus: args.gpus,
        lane: args.lane.clone(),
        attention_kernel: args.attention_kernel.clone(),
        warm: args.warm.map(Warm::level),
        project: within,
        recipes: false,
        agreed: None,
    })
}

/// What a term may name here: itself (a positional file), its value (`key=value`, a folder too), or the strings of its JSON (`key:=<json>`).
async fn term_paths(paths: &mut Paths<'_, '_>, term: &str, cwd: &Path) -> Result<(), Error> {
    paths.add(term, cwd, false, "the command line").await?;
    match (term.find(":="), term.find('=')) {
        (Some(colon), Some(equals)) if equals == colon + 1 => {
            if let Ok(value) = serde_json::from_str::<Value>(&term[colon + 2..]) {
                paths.add_all(&value, cwd, "the command line").await?;
            }
        }
        (_, Some(equals)) if !term[..equals].starts_with("model.") => paths.add(&term[equals + 1..], cwd, true, "the command line").await?,
        _ => {}
    }
    Ok(())
}

/// `--input FILE`: JSON, or YAML by its name or when it is not JSON, holding one object; and its folder, where the files it names start.
fn read_input(ctx: &Context, file: &Path, cwd: &Path) -> Result<(Map<String, Value>, PathBuf), Error> {
    let path = protocol::target::spelled(&file.to_string_lossy(), cwd, &ctx.user_home);
    let what = format!("--input {}", file.display());
    let text = std::fs::read_to_string(&path).map_err(|e| {
        Error::new(Kind::NotFound, "input_unreadable", format!("{what}: {e}")).remedy("--input takes one JSON or YAML file holding the whole payload")
    })?;
    let yaml = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"));
    let loaded = match serde_json::from_str(&text) {
        Ok(value) if !yaml => value,
        _ => protocol::yaml::to_json(&text).map_err(|e| Error::new(Kind::Validation, "input_invalid", format!("{what}: {e}")))?,
    };
    let Value::Object(loaded) = loaded else {
        return Err(Error::new(Kind::Validation, "input_invalid", format!("{what} does not hold one object")));
    };
    Ok((loaded, path.parent().map_or_else(|| cwd.to_owned(), Path::to_owned)))
}

/// Submits `ask` under a key minted for it (its seed derives from it) and follows the run. Local
/// code not yet ready on its machine is installed first: that run is followed, then the same
/// request goes again under the same key.
async fn send(ctx: &Context, ask: RunRequest) -> Result<Run, Error> {
    let key = follow::mint_key();
    loop {
        let submitted = follow::submit_request(ctx, &ask, &key).await?;
        if !submitted.preparing {
            return follow(ctx, submitted.run).await;
        }
        let install = follow::settle(ctx, submitted.run).await?;
        if !settled(&install) {
            // Following stopped: the install goes on, and nothing is called.
            return Ok(install);
        }
    }
}

/// `GET /v1/targets`: the target's function's contract, or its package's functions.
async fn describe(ctx: &Context, raw: &str, code: Option<&LocalCode>, on: Option<&str>) -> Result<ExitCode, Error> {
    let target = target(ctx, raw, code, on).await?;
    match (&target.contract, &target.request) {
        (Some(_), Some(request)) if ctx.json => output::json(request),
        (Some(contract), _) => anstream::print!("{contract}"),
        _ => print_functions(ctx, &target),
    }
    Ok(ExitCode::SUCCESS)
}

/// `cozy run [org/]pkg`: the package's public functions.
async fn functions(ctx: &Context, raw: &str, code: Option<&LocalCode>, on: Option<&str>) -> Result<ExitCode, Error> {
    print_functions(ctx, &target(ctx, raw, code, on).await?);
    Ok(ExitCode::SUCCESS)
}

async fn target(ctx: &Context, raw: &str, code: Option<&LocalCode>, on: Option<&str>) -> Result<Target, Error> {
    let tree = code.map(|code| code.tree.to_string());
    let mut query = vec![("target", raw), ("hub", ctx.hub().as_str())];
    query.extend(on.map(|on| ("on", on)));
    query.extend(tree.as_deref().map(|tree| ("tree", tree)));
    query.extend(code.map(|code| ("at", code.at.as_str())).filter(|(_, at)| !at.is_empty()));
    ctx.calcifer().await?.get_query("/v1/targets", &query).await
}

/// A package's functions, and what each does. Its recipes are not functions: `cozy model
/// quantize` runs them.
fn print_functions(ctx: &Context, target: &Target) {
    let (package, release) = (&target.package, &target.release);
    let from = (target.release_from != ReleaseFrom::Local).then(|| target.release_from.to_string());
    if ctx.json {
        output::json(&serde_json::json!({
            "package": package, "release": release, "release_from": from, "summary": target.summary,
            "functions": target.functions,
        }));
        return;
    }
    let mut table = output::Table::new(ctx, ["FUNCTION", "DESCRIPTION"]);
    for row in &target.functions {
        table = table.row([row.function.as_str(), row.description.as_str()]);
    }
    if target.functions.is_empty() {
        table = table.note(format!("{package} has no functions: its recipes run through cozy model quantize"));
    }
    let from = from.map(|from| format!(" ({from})")).unwrap_or_default();
    let summary = match target.summary.is_empty() {
        true => String::new(),
        false => format!(": {}", target.summary),
    };
    table.note(format!("{package} {release}{from}{summary}")).print();
    output::next(ctx, &target.next.iter().map(String::as_str).collect::<Vec<_>>());
}

impl Warm {
    fn level(self) -> WarmLevel {
        match self {
            Warm::Installed => WarmLevel::Installed,
            Warm::Downloaded => WarmLevel::Downloaded,
            Warm::Imported => WarmLevel::Imported,
            Warm::Host => WarmLevel::Host,
            Warm::Gpu => WarmLevel::Gpu,
            Warm::Off => WarmLevel::Off,
        }
    }
}
