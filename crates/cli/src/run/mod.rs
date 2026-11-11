//! `cozy run`: run a function on a machine, and the verbs over runs. Calcifer resolves, binds and
//! submits a run (`POST /v1/runs`); this module turns arguments into its request and prints.
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::ValueHint;
use clap_complete::ArgValueCompleter;
use api::{
    calcifer::{FileTerm, On, ReleaseFrom, Run, RunEstimate, RunRequest, Target},
    code::LocalCode,
    error::{Error, Kind},
    machine::{Basis, WarmLevel},
};
use serde_json::{Map, Value};

use crate::{
    Context, output,
    send::{Paths, Sender},
};
pub(crate) use follow::{done, exit_code, follow, follow_until, job, mint_key, settled};

pub(crate) mod fetch;
mod follow;
pub(crate) mod list;
pub(crate) mod live;
mod watch;

/// `cozy run --help`'s examples: a YAML request file first, inline inputs as overrides.
const EXAMPLES: &str = "Examples
  cozy run sdxl/generate --input request.yaml
  cozy run sdxl/generate prompt='a fox in fresh snow' --on local
  cozy run birefnet/remove_background --file image=./character.png";

#[derive(clap::Args, Debug)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true, arg_required_else_help = true, subcommand_value_name = "ls|watch|cancel", after_help = EXAMPLES)]
pub(crate) struct Args {
    #[command(subcommand)]
    verb: Option<Verb>,
    #[command(flatten)]
    run: Box<RunArgs>,
}

#[derive(clap::Args, Debug)]
struct RunArgs {
    /// `[org/]pkg[/fn][@release]`, `./dir[/fn]`, or `./script.py[/fn]`.
    #[arg(required = true, value_name = "TARGET", add = ArgValueCompleter::new(crate::completion::local))]
    target: Option<String>,
    /// Inputs as `key=value`, `key:=json`, `model.<slot>=<ref>`, or files.
    #[arg(value_name = "INPUT", value_hint = ValueHint::FilePath)]
    inputs: Vec<String>,
    #[command(flatten)]
    placement: Placement,
    /// Use at most N GPUs for the run and its calls.
    #[arg(long, value_name = "N", value_parser = api::calcifer::gpu_count)]
    gpus: Option<u32>,
    /// Main model precision lane (bf16, fp8 or int8). Auto-selected when omitted.
    #[arg(long, value_parser = api::model_ref::lane)]
    lane: Option<String>,
    /// Select an attention kernel for all models or one `[slot/]component`.
    #[arg(long, value_name = "[[SLOT/]COMPONENT=]KERNEL", value_parser = api::calcifer::attention_pin, conflicts_with = "warm")]
    attention_kernel: Option<String>,
    /// Read inputs from a JSON or YAML file. Inline inputs override it.
    #[arg(long = "input", value_name = "FILE")]
    input_file: Option<PathBuf>,
    /// Attach files with `[NAME=]FILE` or `field.path=FILE`.
    #[arg(long = "file", value_name = "[NAME=]FILE", value_hint = ValueHint::FilePath)]
    assets: Vec<String>,
    /// Set reference-image detail: `label-or-index=auto|low|medium|high`.
    #[arg(long = "file-fidelity", value_name = "FILE=FIDELITY", value_parser = api::calcifer::fidelity)]
    fidelities: Vec<String>,
    /// Apply a LoRA using `[SLOT=]REF[,STRENGTH]`. Default strength is 1.
    #[arg(long = "lora", value_name = "[SLOT=]REF[,STRENGTH]")]
    loras: Vec<String>,
    /// Keep the function ready at the selected level.
    #[arg(long, value_enum, ignore_case = true, value_name = "LEVEL")]
    warm: Option<Warm>,
    /// Show the function's inputs and outputs.
    #[arg(long)]
    describe: bool,
    /// Download outputs to this directory.
    #[arg(short = 'o', long = "output", value_name = "DIR", value_hint = ValueHint::DirPath)]
    output: Option<PathBuf>,
    /// Estimate machine finish times for N simultaneous runs.
    #[arg(long, value_name = "N", num_args = 0..=1, default_missing_value = "1", conflicts_with_all = ["warm", "describe"])]
    estimate: Option<u32>,
}

/// Where a run goes; shared by every verb that submits one.
#[derive(clap::Args, Debug, Clone)]
pub(crate) struct Placement {
    /// Run on `local`, a machine name, `any`, or `new[:sku]`.
    #[arg(long = "on", value_name = "MACHINE")]
    on: Option<String>,
}

impl Placement {
    /// Where the run goes when the command says; `local` refused where this OS runs no models itself.
    pub(crate) fn on(&self) -> Result<Option<On>, Error> {
        let Some(on) = &self.on else { return Ok(None) };
        api::calcifer::local_usable(on)?;
        Ok(Some(On::from(on.clone())))
    }
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Warm {
    Installed,
    Downloaded,
    Imported,
    Host,
    Gpu,
    /// Stop keeping the function ready.
    Off,
}

#[derive(clap::Subcommand, Debug)]
enum Verb {
    /// List this project's runs, newest first.
    #[command(alias = "list")]
    Ls {
        /// Filter by queued, in_progress (running), completed, failed or canceled.
        #[arg(long, value_name = "STATE", value_parser = run_state)]
        state: Option<String>,
        /// Only this package's runs.
        #[arg(long)]
        package: Option<String>,
        /// Filter by machine name, alias, or `local`.
        #[arg(long = "on", value_name = "MACHINE")]
        on: Option<String>,
        /// Show at most N runs (default 20, 0 for all).
        #[arg(long, value_name = "N")]
        limit: Option<u64>,
        /// Include runs from every project.
        #[arg(long)]
        all: bool,
    },
    /// Watch a run's progress, result, outputs and logs.
    Watch {
        /// The run's number or id.
        run: String,
        /// Show the current state and exit.
        #[arg(long)]
        once: bool,
        /// Download outputs to this directory.
        #[arg(short = 'o', long = "output", value_name = "DIR", value_hint = ValueHint::DirPath)]
        output: Option<PathBuf>,
    },
    /// Cancel a queued or running run.
    Cancel {
        /// The run's number or id.
        run: String,
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

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    match args.verb {
        Some(Verb::Ls {
            state,
            package,
            on,
            limit,
            all,
        }) => {
            let project = if all {
                None
            } else {
                Some(crate::project::current(ctx).await?.id)
            };
            let query = list::Query { status: state, package, machine: on, project, before: None, limit };
            list::exec(ctx, query).await
        }
        Some(Verb::Watch { run, once, output }) => watch::exec(ctx, &run, once, output.as_deref()).await,
        Some(Verb::Cancel { run }) => {
            let calcifer = ctx.calcifer().await?;
            let mut run = calcifer.run(&run).await?;
            if !follow::settled(&run) {
                run = calcifer.cancel(run.id.as_str()).await?;
            }
            follow::record(ctx, &run);
            Ok(ExitCode::SUCCESS)
        }
        None => run(ctx, *args.run).await,
    }
}


async fn run(ctx: &Context, args: RunArgs) -> Result<ExitCode, Error> {
    let project = crate::project::current(ctx).await?;
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
    // Listing a package's functions needs no project on disk. A named function with
    // default inputs still runs, and a local package can name its one function.
    if !inputs && args.assets.is_empty() && args.warm.is_none() {
        let described = self::target(ctx, &target, code.as_ref(), args.placement.on.as_deref()).await?;
        if described.contract.is_none() {
            print_functions(ctx, &described);
            return Ok(ExitCode::SUCCESS);
        }
    }
    let within = match args.warm {
        Some(_) => None,
        None => Some(project.id),
    };
    let ask = request(ctx, &args, target.clone(), code.clone(), &cwd, within).await?;
    if let Some(count) = args.estimate {
        return estimate(ctx, ask, count).await;
    }
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
    let Some(path) = api::target::local(raw, cwd, &ctx.user_home) else { return Ok(None) };
    let found = api::code::split(&path).ok_or_else(|| {
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
    let path = api::target::spelled(&file.to_string_lossy(), cwd, &ctx.user_home);
    let what = format!("--input {}", file.display());
    let text = std::fs::read_to_string(&path).map_err(|e| {
        Error::new(Kind::NotFound, "input_unreadable", format!("{what}: {e}")).remedy("--input takes one JSON or YAML file holding the whole payload")
    })?;
    let yaml = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"));
    let loaded = match serde_json::from_str(&text) {
        Ok(value) if !yaml => value,
        _ => api::yaml::to_json(&text).map_err(|e| Error::new(Kind::Validation, "input_invalid", format!("{what}: {e}")))?,
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

/// `POST /v1/run-estimates`: each machine's quote for the run, the soonest finish first; nothing is sent. Local code not yet ready
/// on its machine is installed first, then asked again under the same key.
async fn estimate(ctx: &Context, ask: RunRequest, count: u32) -> Result<ExitCode, Error> {
    let key = follow::mint_key();
    let calcifer = ctx.calcifer().await?;
    let estimate = loop {
        let estimate: RunEstimate = calcifer.post(&format!("/v1/run-estimates?count={count}"), &ask, Some(&key)).await?;
        let Some(install) = estimate.preparing else { break estimate };
        let install = follow::settle(ctx, *install).await?;
        if !settled(&install) {
            return Ok(exit_code(&install));
        }
    };
    for note in &estimate.notes {
        output::warn(&note.message);
    }
    if ctx.json {
        output::json(&estimate);
        return Ok(ExitCode::SUCCESS);
    }
    let copies = estimate.machines.iter().any(|machine| machine.job > 0);
    let header = ["MACHINE", "GPUS", "START", "FINISH", "PREPARE", "QUEUE", "BASIS"];
    let table = output::Table::new(ctx, copies.then_some("JOB").into_iter().chain(header));
    let rows = estimate.machines.iter().map(|machine| {
        let mut name = machine.machine.clone();
        if estimate.soonest.as_ref() == Some(&machine.machine) {
            name += " *";
        }
        let basis = match machine.basis {
            Basis::Measured => "measured",
            Basis::Prior => "prior",
            Basis::None => "none",
        };
        let basis = match &machine.refusal {
            Some(refusal) => refusal.message.clone(),
            None => basis.to_owned(),
        };
        let shown = |ms: Option<u64>| ms.map_or_else(|| "-".to_owned(), seconds);
        let width = if machine.fits { machine.width.to_string() } else { "-".to_owned() };
        let prepare = if machine.fits { seconds(machine.prepare_ms) } else { "-".to_owned() };
        let cells = [name, width, shown(machine.start_ms), shown(machine.finish_ms), prepare, machine.queue_position.to_string(), basis];
        copies.then(|| (machine.job + 1).to_string()).into_iter().chain(cells).collect::<Vec<_>>()
    });
    rows.fold(table, output::Table::row).print();
    if let Some(soonest) = &estimate.soonest {
        output::say(ctx, &format!("* where it would go: {soonest}"));
    }
    Ok(ExitCode::SUCCESS)
}

/// `41.2s`, `3m12s`.
fn seconds(ms: u64) -> String {
    match ms / 1000 {
        0..60 => format!("{:.1}s", ms as f64 / 1000.0),
        s => format!("{}m{:02}s", s / 60, s % 60),
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
