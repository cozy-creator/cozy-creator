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
    error::{Error, Kind},
    machine::WarmLevel,
};

use crate::{Context, output};
pub(crate) use follow::{exit_code, follow, follow_until, mint_key, settle, settled, stalled_words, watched};

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

A video path may name a segment, path#t=<start>,<end> (seconds, m:ss or h:mm:ss; either may be left
out): it is cut once into ~/.cozy/cache/clips and sent instead of the whole file.

A run is followed to its end: Ctrl-C stops following, never the run; `cozy run show N` follows it again.";

#[derive(clap::Args, Debug)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true, arg_required_else_help = true, after_help = EXAMPLES)]
pub struct Args {
    /// The project's folder, if not the one you are in. A run belongs to a project; `ls` lists that project's.
    #[arg(short = 'p', value_name = "PATH", global = true)]
    path: Option<PathBuf>,
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
    /// models take their own. Absent, the machine picks what its GPUs run best.
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
    let cwd = std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    // The project is the folder, as a repository is to git.
    let within = match &args.path {
        Some(at) => protocol::target::spelled(&at.to_string_lossy(), &cwd, &ctx.user_home),
        None => cwd,
    };
    match args.verb {
        Some(Verb::Ls {
            state,
            package,
            on,
            limit,
            all,
        }) => {
            let query = list::Query {
                status: state,
                package,
                machine: on,
                project: Some(within).filter(|_| !all),
                before: None,
                limit,
            };
            list::exec(ctx, query).await
        }
        Some(Verb::Show { run, once }) => show::exec(ctx, &run, once).await,
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


async fn run(ctx: &Context, args: RunArgs, within: PathBuf) -> Result<ExitCode, Error> {
    let cwd = std::env::current_dir()
        .map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let target = args.target.clone().unwrap_or_default();
    let inputs = !args.inputs.is_empty() || args.input_file.is_some();
    if args.describe && !inputs {
        return describe(ctx, &target, &cwd, args.placement.on.as_deref()).await;
    }
    let ask = request(ctx, &args, target.clone(), &cwd, within)?;
    match send(ctx, ask).await {
        // A package's functions, when the run names none and gives it nothing.
        Err(e) if e.code == "function_unnamed" && !inputs => functions(ctx, &target, &cwd, args.placement.on.as_deref()).await,
        Err(e) => Err(e),
        Ok(run) => {
            paste(ctx, &run);
            Ok(exit_code(&run))
        }
    }
}

/// One line per file a run left in its project, to paste into project.yaml: `image: 65b436d4f9a3`.
pub(crate) fn paste(ctx: &Context, run: &Run) {
    if ctx.json || run.project.is_none() {
        return;
    }
    for output in follow::for_people(run) {
        let Some(sha256) = output.path.as_ref().and(output.sha256.as_ref()) else { continue };
        let field = match output.media_type.split('/').next() {
            Some(kind @ ("image" | "video" | "audio")) => kind,
            _ => output.name.as_str(),
        };
        let hex = sha256.to_string();
        let hex = hex.trim_start_matches("sha256:");
        println!("{field}: {}", &hex[..hex.len().min(12)]);
    }
}

/// The request `args` make of `target`.
fn request(ctx: &Context, args: &RunArgs, target: String, cwd: &Path, within: PathBuf) -> Result<RunRequest, Error> {
    Ok(RunRequest {
        target,
        // None: Calcifer's to say, by what this OS runs and the machines there are (`default_on`).
        on: args.placement.on.clone().map(On::from),
        hub: ctx.hub().to_string(),
        input: None,
        input_file: args.input_file.clone(),
        input_text: None,
        terms: args.inputs.clone(),
        files: args.assets.iter().map(|raw| FileTerm::parse(raw)).collect(),
        fidelities: args.fidelities.clone(),
        loras: args.loras.clone(),
        cwd: Some(cwd.to_owned()),
        gpus: args.gpus,
        lane: args.lane.clone(),
        attention_kernel: args.attention_kernel.clone(),
        warm: args.warm.map(Warm::level),
        project: Some(within),
        recipes: false,
        agreed: None,
    })
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
async fn describe(ctx: &Context, raw: &str, cwd: &Path, on: Option<&str>) -> Result<ExitCode, Error> {
    let target = target(ctx, raw, cwd, on).await?;
    match (&target.contract, &target.request) {
        (Some(_), Some(request)) if ctx.json => output::json(request),
        (Some(contract), _) => anstream::print!("{contract}"),
        _ => print_functions(ctx, &target),
    }
    Ok(ExitCode::SUCCESS)
}

/// `cozy run [org/]pkg`: the package's public functions.
async fn functions(ctx: &Context, raw: &str, cwd: &Path, on: Option<&str>) -> Result<ExitCode, Error> {
    print_functions(ctx, &target(ctx, raw, cwd, on).await?);
    Ok(ExitCode::SUCCESS)
}

async fn target(ctx: &Context, raw: &str, cwd: &Path, on: Option<&str>) -> Result<Target, Error> {
    let cwd = cwd.to_string_lossy();
    let mut query = vec![("target", raw), ("hub", ctx.hub().as_str()), ("cwd", &cwd)];
    query.extend(on.map(|on| ("on", on)));
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
