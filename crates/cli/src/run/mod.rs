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
    config::Origin,
    error::{Error, Kind},
    interface::Interface,
    machine::WarmLevel,
};

use crate::{Context, output};
pub(crate) use follow::{exit_code, follow, follow_until, mint_key, settled, submit};
pub(crate) use hub_client::cards::Card;

mod follow;
pub(crate) mod list;
pub(crate) mod live;
pub(crate) mod local;
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

A video path may name a segment, path#t=<start>,<end> (whole seconds, m:ss or h:mm:ss; either may be left
out): it is cut once into ~/.cozy/cache/clips and sent instead of the whole file.

A run is followed to its end: Ctrl-C stops following, never the run; `cozy run show N` follows it again.";

#[derive(clap::Args, Debug)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true, arg_required_else_help = true, after_help = EXAMPLES)]
pub struct Args {
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
    /// Where the outputs land (default ~/.cozy/outputs).
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    out: Option<PathBuf>,
    /// Keep the function ready instead of running it.
    #[arg(long, value_enum, value_name = "LEVEL")]
    warm: Option<Warm>,
    /// Print the function's request contract instead of running it.
    #[arg(long)]
    describe: bool,
}

/// Where a run goes; shared by every verb that submits one.
#[derive(clap::Args, Debug, Clone)]
pub struct Placement {
    /// `local`, a machine's name, `any`, or `new[:sku]` to rent one.
    #[arg(long = "on", value_name = "MACHINE", default_value = "local")]
    on: String,
}

impl Placement {
    /// Where the run goes; `local` refused where this OS runs no models itself.
    pub fn on(&self) -> Result<On, Error> {
        protocol::calcifer::local_usable(&self.on)?;
        Ok(On::from(self.on.clone()))
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
    Ls {
        /// Only runs in this state.
        #[arg(long, value_parser = ["queued", "in_progress", "completed", "failed", "canceled"])]
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

pub async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    match args.verb {
        Some(Verb::Ls {
            state,
            package,
            on,
            limit,
        }) => {
            let query = list::Query {
                status: state,
                package,
                machine: on,
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
        None => run(ctx, *args.run).await,
    }
}


async fn run(ctx: &Context, args: RunArgs) -> Result<ExitCode, Error> {
    let cwd = std::env::current_dir()
        .map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let target = args.target.clone().unwrap_or_default();
    let inputs = !args.inputs.is_empty() || args.input_file.is_some();
    if args.describe && !inputs {
        return describe(ctx, &target, &cwd).await;
    }
    let ask = request(ctx, &args, target.clone(), &cwd)?;
    match send(ctx, ask).await {
        // A package's functions, when the run names none and gives it nothing.
        Err(e) if e.code == "function_unnamed" && !inputs => functions(ctx, &target, &cwd).await,
        Err(e) => Err(e),
        Ok(run) => Ok(exit_code(&run)),
    }
}

/// The request `args` make of `target`.
fn request(ctx: &Context, args: &RunArgs, target: String, cwd: &Path) -> Result<RunRequest, Error> {
    Ok(RunRequest {
        target,
        on: Some(args.placement.on()?),
        hub: ctx.hub().to_string(),
        input: None,
        input_file: args.input_file.clone(),
        terms: args.inputs.clone(),
        files: args.assets.iter().map(|raw| FileTerm::parse(raw)).collect(),
        fidelities: args.fidelities.clone(),
        loras: args.loras.clone(),
        cwd: Some(cwd.to_owned()),
        gpus: args.gpus,
        lane: args.lane.clone(),
        attention_kernel: args.attention_kernel.clone(),
        warm: args.warm.map(Warm::level),
        out: args.out.clone(),
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
async fn describe(ctx: &Context, raw: &str, cwd: &Path) -> Result<ExitCode, Error> {
    let target = target(ctx, raw, cwd).await?;
    let interface = parsed(&target)?;
    let Some(function) = &target.function else {
        print_functions(ctx, &target, &interface);
        return Ok(ExitCode::SUCCESS);
    };
    let Some(callable) = interface.callables.iter().find(|c| &c.name == function) else {
        let said = format!("{} has no function {function:?} this cozy reads", target.package);
        return Err(Error::new(Kind::Unavailable, "callable_unreadable", said).remedy("update cozy"));
    };
    if ctx.json {
        let mut request = hub_client::cards::request(&target.interface, function);
        if target.release_from != ReleaseFrom::Local {
            request["release"] = target.release.clone().into();
            request["release_from"] = target.release_from.to_string().into();
        }
        output::json(&request);
        return Ok(ExitCode::SUCCESS);
    }
    let named = format!("{}/{function}", target.package);
    let text = protocol::contract::contract(&named, &interface, callable);
    let text = match target.release_from {
        ReleaseFrom::Local => text,
        from => text.replacen('\n', &format!("\n  release: {} ({from})\n", target.release), 1),
    };
    anstream::print!("{}", protocol::names::shown(&text));
    Ok(ExitCode::SUCCESS)
}

/// `cozy run [org/]pkg`: the package's public functions.
async fn functions(ctx: &Context, raw: &str, cwd: &Path) -> Result<ExitCode, Error> {
    let target = target(ctx, raw, cwd).await?;
    print_functions(ctx, &target, &parsed(&target)?);
    Ok(ExitCode::SUCCESS)
}

async fn target(ctx: &Context, raw: &str, cwd: &Path) -> Result<Target, Error> {
    let cwd = cwd.to_string_lossy();
    let query = [("target", raw), ("hub", ctx.hub().as_str()), ("cwd", &cwd)];
    ctx.calcifer().await?.get_query("/v1/targets", &query).await
}

fn parsed(target: &Target) -> Result<Interface, Error> {
    Interface::parse(&target.interface).map_err(|why| {
        Error::new(Kind::Unavailable, "package_interface_invalid", format!("the package interface is unreadable: {why}"))
    })
}

fn print_functions(ctx: &Context, target: &Target, interface: &Interface) {
    let from = (target.release_from != ReleaseFrom::Local).then(|| target.release_from.to_string());
    functions_of(ctx, &target.package, (&target.release, &target.summary), from.as_deref(), interface);
}

/// A package's functions, and what each does. Its recipes are not functions: `cozy model
/// quantize` runs them.
pub(crate) fn functions_of(
    ctx: &Context,
    package: &str,
    (release, summary): (&str, &str),
    from: Option<&str>,
    interface: &Interface,
) {
    let public = interface.public();
    let description = |name: &str| {
        (interface.callables.iter().find(|c| c.name == name)).map_or("", |c| c.description.as_str())
    };
    if ctx.json {
        let rows: Vec<_> = (public.iter())
            .map(|name| serde_json::json!({"function": name, "description": description(name)}))
            .collect();
        output::json(&serde_json::json!({
            "package": package, "release": release, "release_from": from, "summary": summary,
            "functions": rows,
        }));
        return;
    }
    let mut table = output::Table::new(ctx, ["FUNCTION", "DESCRIPTION"]);
    for name in &public {
        table = table.row([*name, description(name)]);
    }
    if public.is_empty() {
        table = table.note(format!("{package} has no functions: its recipes run through cozy model quantize"));
    }
    let from = from.map(|from| format!(" ({from})")).unwrap_or_default();
    let summary = match summary.is_empty() {
        true => String::new(),
        false => format!(": {summary}"),
    };
    table.note(format!("{package} {release}{from}{summary}")).print();
    let next: Vec<String> = public
        .iter()
        .take(2)
        .map(|name| format!("cozy run {package}/{name} --describe"))
        .collect();
    output::next(ctx, &next.iter().map(String::as_str).collect::<Vec<_>>());
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

/// What another verb's call runs: a package's release, or local code.
pub(crate) enum Callee<'a> {
    Release { package: &'a str, card: &'a Card },
    Local(&'a local::Prepared),
}

impl Callee<'_> {
    /// The package as its runs name it: `org/name`, or `local/<name>`.
    pub(crate) fn package(&self) -> &str {
        match self {
            Callee::Release { package, .. } => package,
            Callee::Local(prepared) => prepared.package(),
        }
    }

    pub(crate) fn interface(&self) -> &Interface {
        match self {
            Callee::Release { card, .. } => &card.interface,
            Callee::Local(prepared) => prepared.interface(),
        }
    }

    /// The code exactly: its release, or a local source's manifest digest.
    pub(crate) fn release(&self) -> &str {
        match self {
            Callee::Release { card, .. } => &card.release,
            Callee::Local(prepared) => prepared.source(),
        }
    }

    /// `function` of it as a run's target.
    fn target(&self, function: &str) -> String {
        match self {
            Callee::Release { package, card } => format!("{package}/{function}@{}", card.release),
            Callee::Local(prepared) => format!("{}/{function}", prepared.path().display()),
        }
    }
}

impl std::fmt::Display for Callee<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Callee::Release { package, card } => write!(f, "{package}@{}", card.release),
            Callee::Local(prepared) => f.write_str(prepared.package()),
        }
    }
}

/// `cozy run <package>/<function> <inputs…> --on …` for another verb: `model quantize` runs a
/// package's recipes this way. The run as it settled, or as it was when Ctrl-C stopped following
/// it; its record is printed either way.
pub(crate) async fn call(
    ctx: &Context,
    callee: &Callee<'_>,
    function: &str,
    inputs: Vec<String>,
    placement: &Placement,
) -> Result<Run, Error> {
    let cwd = std::env::current_dir()
        .map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    send(ctx, RunRequest {
        target: callee.target(function),
        on: Some(placement.on()?),
        hub: ctx.hub().to_string(),
        terms: inputs,
        cwd: Some(cwd),
        recipes: true,
        ..Default::default()
    })
    .await
}

/// A release's card, kept on this computer (the Hub's read once).
pub(crate) async fn card(ctx: &Context, hub: &Origin, package: &str, release: &str) -> Result<Card, Error> {
    hub_client::cards::card(|| ctx.hub_client(hub), ctx.home.root(), hub, package, release).await
}

/// The Hub's newest release of `package` now, and kept.
pub(crate) async fn fresh(ctx: &Context, hub: &Origin, package: &str) -> Result<String, Error> {
    hub_client::cards::fresh(&ctx.hub_client(hub)?, ctx.home.root(), hub, package).await
}

/// `release` of `package`'s card, else its newest this computer keeps.
pub(crate) async fn resolve(ctx: &Context, hub: &Origin, package: &str, release: Option<&str>) -> Result<Card, Error> {
    let root = ctx.home.root();
    let release = match release {
        Some(release) => release.to_owned(),
        None => hub_client::cards::latest(|| ctx.hub_client(hub), root, hub, package).await?,
    };
    card(ctx, hub, package, &release).await
}
