//! `cozy run`: run a function on a machine, and the verbs over runs. Calcifer resolves, binds and
//! submits a run (`POST /v1/runs`); this module turns arguments into its request and prints.
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::ValueHint;
use clap_complete::ArgValueCompleter;
use api::{
    calcifer::{FileTerm, On, ReleaseFrom, Run, RunRequest, Target},
    code::LocalCode,
    error::{Error, Kind},
};
use serde_json::{Map, Value};

use crate::{
    Context, output,
    send::{Paths, Sender},
};
pub(crate) use follow::{done, exit_code, follow, follow_until, handle, in_folder, job, mint_key, note_folder, settled};

pub(crate) mod fetch;
mod follow;
pub(crate) mod list;
pub(crate) mod live;
mod previews;
mod watch;

/// `cozy run --help`'s examples: a YAML request file first, inline inputs as overrides.
const EXAMPLES: &str = "Examples
  cozy run sdxl/generate --input request.yaml
  cozy run sdxl/generate --input request.yaml --model model=stabilityai/sdxl@1.0.0
  cozy run sdxl/generate prompt='a fox in fresh snow' --on local
  cozy run birefnet/remove_background --file image=./character.png

Repeat --model for multiple slots. For the same slot, the last flag overrides earlier flags,
model.<slot>= inputs and the request file.";

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
    /// Run on `local`, a machine, several (`a,b`: whichever finishes first), `any`, or `new[:sku]`. Default: the project's
    /// machines (`cozy machine use`), else any.
    #[arg(long = "on", value_name = "MACHINE[,MACHINE]")]
    on: Option<String>,
    /// Limit GPU use; with --on new, rent this many GPUs. Automatic when omitted.
    #[arg(long, value_name = "N", value_parser = api::calcifer::gpu_count)]
    gpus: Option<u32>,
    /// Main model precision lane (bf16, fp8 or int8). Auto-selected when omitted.
    #[arg(long, value_parser = api::model_ref::lane)]
    lane: Option<String>,
    /// Select an attention kernel for all models or one `[slot/]component`.
    #[arg(long, value_name = "[[SLOT/]COMPONENT=]KERNEL", value_parser = api::calcifer::attention_pin)]
    attention_kernel: Option<String>,
    // Dev-only (xs-171). The Runtime writes what attention kernels receive on the machine.
    /// Capture attention Q/K/V and every call's shape: a JSON spec or `@FILE`.
    #[arg(long, value_name = "SPEC")]
    attention_capture: Option<String>,
    /// Read inputs from a JSON or YAML file. Inline inputs override it.
    #[arg(long = "input", value_name = "FILE")]
    input_file: Option<PathBuf>,
    /// Attach files with `[NAME=]FILE` or `field.path=FILE`.
    #[arg(long = "file", value_name = "[NAME=]FILE", value_hint = ValueHint::FilePath)]
    assets: Vec<String>,
    /// Set reference-image detail: `label-or-index=auto|low|medium|high`.
    #[arg(long = "file-fidelity", value_name = "FILE=FIDELITY", value_parser = api::calcifer::fidelity)]
    fidelities: Vec<String>,
    /// Override a model slot. Use --describe to list slots.
    #[arg(long = "model", value_name = "SLOT=REF", value_parser = model_override)]
    models: Vec<String>,
    /// Apply a LoRA using `[SLOT=]REF[,STRENGTH]`. Without a strength, its recommended one, else 1.
    #[arg(long = "lora", value_name = "[SLOT=]REF[,STRENGTH]")]
    loras: Vec<String>,
    /// Show the function's inputs and outputs.
    #[arg(long)]
    describe: bool,
    /// Export output copies to this directory. Project locks do not protect exported copies.
    #[arg(short = 'o', long = "output", value_name = "DIR", value_hint = ValueHint::DirPath)]
    output: Option<PathBuf>,
    /// Save previews of what it's making to this directory while it runs.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    previews: Option<PathBuf>,
}

impl RunArgs {
    fn terms(&self) -> Vec<String> {
        self.inputs.iter().chain(&self.models).cloned().collect()
    }
}

fn model_override(raw: &str) -> Result<String, String> {
    let (slot, reference) = raw.split_once('=').ok_or("use SLOT=REF")?;
    if slot.trim().is_empty() || reference.trim().is_empty() {
        return Err("SLOT and REF must both be nonempty; use SLOT=REF".to_owned());
    }
    Ok(format!("model.{}={}", slot.trim(), reference.trim()))
}

/// Where one of Calcifer's jobs goes (an install, a download, a quantization): one place, shared by those verbs.
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
        /// Export output copies to this directory. Project locks do not protect exported copies.
        #[arg(short = 'o', long = "output", value_name = "DIR", value_hint = ValueHint::DirPath)]
        output: Option<PathBuf>,
        /// Save previews of what it's making to this directory while it runs.
        #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
        previews: Option<PathBuf>,
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
                Some(crate::project::identity(ctx).await?.id)
            };
            let query = list::Query { status: state, package, machine: on, project, before: None, limit };
            list::exec(ctx, query).await
        }
        Some(Verb::Watch { run, once, output, previews }) => watch::exec(ctx, &run, once, output.as_deref(), previews.as_deref()).await,
        Some(Verb::Cancel { run }) => {
            let calcifer = ctx.calcifer().await?;
            let address = crate::calcifer_client::RunAddress::current(ctx, &run).await?;
            let mut run = calcifer.run(&address).await?;
            if !follow::settled(&run) {
                run = calcifer.cancel(&crate::calcifer_client::RunAddress::of(&run)).await?;
            }
            follow::record(ctx, &run);
            Ok(ExitCode::SUCCESS)
        }
        None => run(ctx, *args.run).await,
    }
}


async fn run(ctx: &Context, args: RunArgs) -> Result<ExitCode, Error> {
    let cwd = std::env::current_dir()
        .map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let written = args.target.clone().unwrap_or_default();
    let inputs = !args.inputs.is_empty() || !args.models.is_empty() || args.input_file.is_some();
    if args.describe && !inputs {
        return match local(ctx, &written, &cwd)? {
            None => describe(ctx, &written, None, args.on.as_deref(), None).await,
            Some((package, function)) => {
                let project = match crate::project::identity(ctx).await {
                    Ok(project) => Some(project.id),
                    Err(error) if error.code == "not_in_a_project" => None,
                    Err(error) => return Err(error),
                };
                let code = match &project {
                    Some(id) => Sender::for_project(ctx, id).await?.code(&package).await?,
                    None => Sender::independent_code(ctx, &package).await?,
                };
                describe(ctx, &function.unwrap_or_default(), Some(&code), args.on.as_deref(), project.as_deref()).await
            }
        };
    }
    let within = crate::project::identity(ctx).await?.id;
    let sender = Sender::for_project(ctx, &within).await?;
    // Local code goes by content: the target is then the function it names.
    let (target, code, package) = match local(ctx, &written, &cwd)? {
        Some((package, function)) => (function.unwrap_or_default(), Some(sender.code(&package).await?), Some(package)),
        None => (written, None, None),
    };
    // A named function with default inputs still runs; an unnamed package lists its functions.
    if !inputs && args.assets.is_empty() {
        let described = self::target(ctx, &target, code.as_ref(), args.on.as_deref(), Some(&within)).await?;
        if described.contract.is_none() {
            print_functions(ctx, &described);
            return Ok(ExitCode::SUCCESS);
        }
    }
    let ask = request(ctx, &args, target.clone(), code.clone(), &cwd, within.clone()).await?;
    match send(ctx, ask, package.as_deref(), args.previews.as_deref()).await {
        // A package's functions, when the run names none and gives it nothing.
        Err(e) if e.code == "function_unnamed" && !inputs => functions(ctx, &target, code.as_ref(), args.on.as_deref(), Some(&within)).await,
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
        Error::new(Kind::NotFound, "target_not_found", format!("Package directory or script not found: {raw}"))
    })?;
    Ok(Some(found))
}

/// The request `args` make of `target` (of `code`, when it's local), with every file and folder it names here sent first.
async fn request(ctx: &Context, args: &RunArgs, target: String, code: Option<LocalCode>, cwd: &Path, project: String) -> Result<RunRequest, Error> {
    let sender = Sender::for_project(ctx, &project).await?;
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
        // None: Calcifer's to say, the project's machines, else by what this OS runs and the machines there are.
        on: args.on.as_deref().map(api::calcifer::on_set).transpose()?.unwrap_or_default(),
        hub: ctx.hub().to_string(),
        input,
        input_text: None,
        terms: args.terms(),
        files,
        fidelities: args.fidelities.clone(),
        loras: args.loras.clone(),
        paths: paths.sent(),
        gpus: args.gpus,
        lane: args.lane.clone(),
        attention_kernel: args.attention_kernel.clone(),
        attention_capture: args.attention_capture.as_deref().map(|spec| capture(ctx, spec, cwd)).transpose()?,
        warm: None,
        project: Some(project),
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
        Error::new(Kind::NotFound, "input_unreadable", format!("Cannot read {what}: {e}"))
    })?;
    let yaml = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"));
    let loaded = match serde_json::from_str(&text) {
        Ok(value) if !yaml => value,
        _ => api::yaml::to_json(&text).map_err(|e| Error::new(Kind::Validation, "input_invalid", format!("{what}: {e}")))?,
    };
    let Value::Object(loaded) = loaded else {
        return Err(Error::new(Kind::Validation, "input_invalid", format!("{what} must contain a JSON or YAML object"))
            .next("cozy run --help"));
    };
    Ok((loaded, path.parent().map_or_else(|| cwd.to_owned(), Path::to_owned)))
}

/// `--attention-capture`'s spec (dev-only): JSON, inline or `@FILE`, sent compact; the Runtime judges its fields.
fn capture(ctx: &Context, spec: &str, cwd: &Path) -> Result<String, Error> {
    let text = match spec.strip_prefix('@') {
        Some(file) => std::fs::read_to_string(api::target::spelled(file, cwd, &ctx.user_home)).map_err(|e| {
            Error::new(Kind::NotFound, "attention_capture_unreadable", format!("Cannot read --attention-capture {spec}: {e}"))
        })?,
        None => spec.to_owned(),
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(value @ Value::Object(_)) => Ok(value.to_string()),
        _ => Err(Error::new(Kind::Validation, "attention_capture_invalid", "--attention-capture takes a JSON object or @FILE")),
    }
}

/// Submits `ask` under a key minted for it (its seed derives from it) and follows the run. Local
/// code not yet ready on its machine is installed first: that run is followed, then the same
/// request goes again under the same key. The uv.lock Calcifer made for local code goes into its
/// `package` folder, as `cargo run` writes Cargo.lock (cl-871).
async fn send(ctx: &Context, ask: RunRequest, package: Option<&Path>, previews: Option<&Path>) -> Result<Run, Error> {
    let key = follow::mint_key();
    loop {
        let submitted = follow::submit_request(ctx, &ask, &key).await?;
        if let (Some(lock), Some(dir)) = (&submitted.lock, package.filter(|package| package.is_dir()))
            && std::fs::read_to_string(dir.join("uv.lock")).ok().as_deref() != Some(lock.as_str())
        {
            let path = crate::package::author::written(dir, lock)?;
            output::say(ctx, &format!("wrote {}", path.display()));
        }
        if !submitted.preparing {
            return previewed(ctx, submitted.run, previews).await;
        }
        let install = follow::settle(ctx, submitted.run).await?;
        if !settled(&install) {
            // Following stopped: the install goes on, and nothing is called.
            return Ok(install);
        }
        // cozy updated itself for it (cl-978): the run goes on as the new cozy.
        crate::update::after_update(ctx, &install)?;
    }
}

/// Follows `run`, saving its previews into `previews` meanwhile when asked (`--previews`).
pub(crate) async fn previewed(ctx: &Context, run: Run, previews: Option<&Path>) -> Result<Run, Error> {
    let saving = match previews {
        Some(dir) => Some(previews::Saving::start(ctx.calcifer().await?, &run, dir)?),
        None => None,
    };
    let ended = follow(ctx, run).await;
    if let Some(saving) = saving {
        saving.end(ctx);
    }
    ended
}

/// `GET /v1/targets`: the target's function's contract, or its package's functions.
async fn describe(ctx: &Context, raw: &str, code: Option<&LocalCode>, on: Option<&str>, project: Option<&str>) -> Result<ExitCode, Error> {
    let target = target(ctx, raw, code, on, project).await?;
    match (&target.contract, &target.request) {
        (Some(_), Some(request)) if ctx.json => output::json(request),
        (Some(contract), _) => anstream::print!("{contract}"),
        _ => print_functions(ctx, &target),
    }
    Ok(ExitCode::SUCCESS)
}

/// `cozy run [org/]pkg`: the package's public functions.
async fn functions(ctx: &Context, raw: &str, code: Option<&LocalCode>, on: Option<&str>, project: Option<&str>) -> Result<ExitCode, Error> {
    print_functions(ctx, &target(ctx, raw, code, on, project).await?);
    Ok(ExitCode::SUCCESS)
}

async fn target(ctx: &Context, raw: &str, code: Option<&LocalCode>, on: Option<&str>, project: Option<&str>) -> Result<Target, Error> {
    let tree = code.map(|code| code.tree.to_string());
    let mut query = vec![("target", raw), ("hub", ctx.hub().as_str())];
    query.extend(on.map(|on| ("on", on)));
    query.extend(tree.as_deref().map(|tree| ("tree", tree)));
    query.extend(code.map(|code| ("at", code.at.as_str())).filter(|(_, at)| !at.is_empty()));
    query.extend(project.map(|project| ("project", project)));
    let path = if project.is_some() { "/v1/project-targets" } else { "/v1/targets" };
    ctx.calcifer().await?.get_query(path, &query).await
}

/// A package's functions, and what each does. Its recipes are not functions: `cozy model
/// quantize` runs them.
fn print_functions(ctx: &Context, target: &Target) {
    let (package, release) = (&target.package, &target.release);
    let from = (target.release_from != ReleaseFrom::Local).then(|| target.release_from.to_string());
    if ctx.json {
        let mut document = serde_json::json!({
            "package": package, "release": release, "release_from": from, "summary": target.summary,
            "functions": target.functions,
        });
        // With --full, its interface document: each function's model slots and the release each names by default.
        if ctx.full {
            document["interface"] = target.interface.clone();
        }
        output::json(&document);
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct Command {
        #[command(flatten)]
        args: Args,
    }

    #[test]
    fn model_flags_follow_legacy_terms_and_keep_order() {
        let parsed = Command::try_parse_from([
            "run", "sdxl/generate", "--input", "request.yaml", "model.model=legacy",
            "--model", "model=first", "--model", "decoder=other", "--model", "model=last",
        ]).unwrap();
        assert_eq!(parsed.args.run.input_file.as_deref(), Some(Path::new("request.yaml")));
        assert_eq!(parsed.args.run.terms(), [
            "model.model=legacy", "model.model=first", "model.decoder=other", "model.model=last",
        ]);
    }

    #[test]
    fn model_flags_require_a_slot_and_reference() {
        for invalid in ["model", "=ref", "model=", " =ref", "model= "] {
            let error = Command::try_parse_from(["run", "sdxl/generate", "--model", invalid]).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        }
        assert_eq!(model_override(" called.model = owner/model@1.0 ").unwrap(), "model.called.model=owner/model@1.0");
    }

    #[test]
    fn retired_run_flags_are_rejected() {
        for flag in ["--estimate", "--warm"] {
            let error = Command::try_parse_from(["run", "sdxl/generate", flag]).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
        }
    }
}
