//! The `cozy` command line. One module per verb group; each owns its clap arguments and handler.
#![forbid(unsafe_code)]
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use protocol::{
    config,
    error::{Error, Kind},
};

mod account;
mod ask;
pub mod calcifer_client;
mod completion;
pub mod context;
mod daemon;
mod doctor;
mod machine;
mod model;
mod open;
mod org;
pub mod output;
mod package;
mod project;
mod run;
pub mod tools;
pub mod tui;
mod update;

pub use context::Context;

/// `cozy --help`'s last words: examples, then what each exit means, the same for every verb.
const EXITS: &str = "Examples:
  cozy                                      what's going here: the project, runs, machines, spend, and what's next
  cozy run birefnet/remove_background --file image=./dog.png --on corteo
  cozy machine rent rtx-4090 --yes          billing until `cozy machine end`
  cozy --json run ls                        the same facts as JSON, for a program or an agent

Exit codes, the same for every verb:
  0  done, or nothing to do
  1  failed: a run, render, install or check ran and failed, or was canceled
  2  usage: a flag or argument is wrong, or a price needs --yes; nothing happened
  3  partial: a push or pull left files out, an upgrade put something out of date, or you stopped following work that goes on
  4  stalled: a run stopped moving, as Calcifer says after 10 minutes with no progress; it goes on until you cancel it
  5  refused: cozy, Calcifer, a machine or the Hub said no, or did not answer
  6  not found: the run, machine, project, asset, package or model named does not exist

A refusal is one line with its code, then `next:`, the command to run. With --json it is {\"error\": {kind, code, message, next}}
on stdout, and work that is followed prints NDJSON. Progress and warnings go to stderr; nothing ever prompts off a terminal.
`cozy` alone says what's going here. --json prints what a view shows, --full all of it.";

#[derive(Parser, Debug)]
#[command(
    name = "cozy",
    version,
    about = "Run models on your machines and rented GPUs",
    after_help = EXITS
)]
struct Cli {
    /// Print JSON (NDJSON when following) instead of tables: what the view shows, by the record's own names.
    #[arg(long, global = true)]
    json: bool,
    /// The whole record where a view shows part of it: every field with --json, every table and line without.
    #[arg(long, global = true)]
    full: bool,
    /// Agree without being asked: to a rental's quote, a deletion.
    #[arg(long, short = 'y', global = true)]
    yes: bool,
    /// None: `cozy` alone, which is `cozy status`.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run a function: `pkg/fn` (first-party), `org/pkg/fn`, `./dir[/fn]` or `./script.py[/fn]`.
    Run(run::Args),
    /// Local, rented and added machines.
    #[command(after_help = "Examples:
  cozy machine ls
  cozy machine rent                         what is for rent
  cozy machine rent rtx-4090 --yes
  cozy machine end corteo")]
    Machine(machine::Args),
    /// Find, install and publish packages.
    #[command(after_help = "Examples:
  cozy package ls upscale
  cozy package info birefnet
  cozy package install birefnet --on corteo")]
    Package(package::Args),
    /// Find, download, upload and publish models.
    #[command(after_help = "Examples:
  cozy model ls flux
  cozy model info zhengpeng7/birefnet
  cozy model download zhengpeng7/birefnet --on corteo")]
    Model(model::Args),
    /// Projects: assets, and the operations that make them.
    Project(project::Args),
    /// Open the canvas in your browser: the project you're in, else the home.
    Open(open::Args),
    /// Claim an organization parked under your name.
    #[command(after_help = "Example:
  cozy org claim black-forest-labs")]
    Org(org::Args),
    /// Sign in to the Hub with this computer's device key.
    #[command(after_help = "Examples:
  cozy login you@example.com                in a terminal: type the code the Hub emails
  cozy login you@example.com --export ci.key   a key for another computer, such as CI
  cozy login --key ci.key                   on that computer: no terminal needed")]
    Login(account::Login),
    /// Sign out of the Hub.
    #[command(after_help = "Examples:
  cozy logout
  cozy logout --everywhere                  every computer of this account (in a terminal: it asks for an emailed code)")]
    Logout(account::Logout),
    /// Show the signed-in account, its credit and where to buy more.
    #[command(after_help = "Examples:
  cozy whoami
  cozy --json whoami")]
    Whoami(account::Whoami),
    /// This project, the runs going, machines and spend at a glance; `cozy` alone says the same.
    #[command(after_help = "Examples:
  cozy status
  cozy --json status                        the same, as JSON; --full the whole record")]
    Status(daemon::Status),
    /// Replace this binary with the newest release.
    #[command(after_help = "Examples:
  cozy update
  cozy update 0.20.0")]
    Update(update::Args),
    /// Print a shell completion script.
    #[command(after_help = "Examples:
  cozy completion bash > ~/.local/share/bash-completion/completions/cozy
  cozy completion zsh > \"${fpath[1]}/_cozy\"
  cozy completion fish > ~/.config/fish/completions/cozy.fish")]
    Completion(completion::Args),
    /// Diagnose this computer's cozy (Calcifer, its service, the machine) and fix a stuck Calcifer;
    /// then a machine's driver, GPUs, toolchain and kernels.
    #[command(after_help = "Examples:
  cozy doctor
  cozy doctor --on corteo
  cozy --json doctor")]
    Doctor(doctor::Args),
}

/// Parses the command line, loads the config once, runs the verb and renders its refusal. Exit
/// codes: `EXITS`.
pub fn main() -> ExitCode {
    completion::answer(Cli::command);
    let args: Vec<String> = std::env::args_os().map(|arg| arg.to_string_lossy().into_owned()).collect();
    output::VERB.get_or_init(|| verb(&Cli::command(), &args));
    let cli = match Cli::command().try_get_matches_from(std::env::args_os()).and_then(|matches| Cli::from_arg_matches(&matches)) {
        Ok(cli) => cli,
        Err(error) => return usage(error),
    };
    let loaded = match config::load() {
        Ok(loaded) => loaded,
        Err(error) => return output::refuse(cli.json, &error),
    };
    let warnings = loaded.warnings.clone();
    let ctx = Context::new(cli.json, cli.full, cli.yes, loaded, output::Term::detect());
    for warning in &warnings {
        output::warn(warning);
    }
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            return output::refusal(
                &ctx,
                &Error::new(Kind::Internal, "runtime", error.to_string()),
            );
        }
    };
    match runtime.block_on(dispatch(&ctx, cli.command)) {
        Ok(code) => code,
        Err(error) => output::refusal(&ctx, &error),
    }
}

/// The verbs `args` names, as far as they go (`machine end`), read off the grammar: an option's value is never a verb.
fn verb(root: &clap::Command, args: &[String]) -> String {
    let mut path: Vec<&clap::Command> = vec![root];
    let takes = |path: &[&clap::Command], hit: &dyn Fn(&clap::Arg) -> bool| {
        path.iter().flat_map(|command| command.get_arguments()).any(|arg| hit(arg) && arg.get_action().takes_values())
    };
    let mut words = args.iter().skip(1);
    while let Some(word) = words.next() {
        let skip = match word.strip_prefix("--") {
            Some("") => break,
            Some(long) => !long.contains('=') && takes(&path, &|arg| arg.get_long() == Some(long)),
            None => match word.strip_prefix('-').map(|short| short.chars().collect::<Vec<_>>()) {
                Some(short) if !short.is_empty() => short.len() == 1 && takes(&path, &|arg| arg.get_short() == Some(short[0])),
                _ => match path.last().and_then(|command| command.find_subcommand(word)) {
                    Some(sub) => {
                        path.push(sub);
                        continue;
                    }
                    None => break,
                },
            },
        };
        if skip {
            words.next();
        }
    }
    path[1..].iter().map(|command| command.get_name()).collect::<Vec<_>>().join(" ")
}

/// A grammar error as every refusal is said: one line, its code, and the verb's `--help` next (the envelope with `--json`), exit 2.
/// Help and the version are clap's own, as is a verb asked without its subcommand off `--json`: its help, on stderr.
fn usage(error: clap::Error) -> ExitCode {
    use clap::error::ErrorKind;
    let json = std::env::args_os().any(|arg| arg == "--json");
    let code = match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => error.exit(),
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand | ErrorKind::MissingSubcommand if !json => error.exit(),
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand | ErrorKind::MissingSubcommand => "verb_missing",
        ErrorKind::InvalidSubcommand => "verb_unknown",
        ErrorKind::UnknownArgument => "argument_unknown",
        ErrorKind::MissingRequiredArgument => "argument_missing",
        ErrorKind::ArgumentConflict => "arguments_conflict",
        ErrorKind::InvalidValue | ErrorKind::ValueValidation | ErrorKind::NoEquals => "value_invalid",
        ErrorKind::TooManyValues | ErrorKind::TooFewValues | ErrorKind::WrongNumberOfValues => "values_count",
        _ => "usage",
    };
    // clap's words: the error and what it names (`<NAME>`) up to a blank line, then tips and the usage.
    let rendered = error.render().to_string();
    let (head, rest) = rendered.split_once("\n\n").unwrap_or((&rendered, ""));
    let message = head.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    let message = message.strip_prefix("error: ").unwrap_or(&message);
    let said: Vec<&str> = rest.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with("For more information")).collect();
    let tips: Vec<&str> = said.iter().filter_map(|line| line.strip_prefix("tip: ")).collect();
    let remedy = match tips.is_empty() {
        true => said.iter().find(|line| line.starts_with("Usage: ")).map(|line| line.to_string()),
        false => Some(tips.join("; ")),
    };
    let mut refusal = Error::new(Kind::Usage, code, message);
    refusal.remedy = remedy;
    output::refuse(json, &refusal)
}

async fn dispatch(ctx: &Context, command: Option<Command>) -> Result<ExitCode, Error> {
    let Some(command) = command else {
        return daemon::status(ctx, daemon::Status {}).await;
    };
    match command {
        Command::Run(args) => run::exec(ctx, args).await,
        Command::Machine(args) => machine::exec(ctx, args).await,
        Command::Package(args) => package::exec(ctx, args).await,
        Command::Model(args) => model::exec(ctx, args).await,
        Command::Project(args) => project::exec(ctx, args).await,
        Command::Open(args) => open::exec(ctx, args).await,
        Command::Org(args) => org::exec(ctx, args).await,
        Command::Login(args) => account::login(ctx, args).await,
        Command::Logout(args) => account::logout(ctx, args).await,
        Command::Whoami(args) => account::whoami(ctx, args).await,
        Command::Status(args) => daemon::status(ctx, args).await,
        Command::Update(args) => update::exec(ctx, args).await,
        Command::Completion(args) => completion::exec(ctx, args).await,
        Command::Doctor(args) => doctor::exec(ctx, args).await,
    }
}
