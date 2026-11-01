//! The `cozy` command line. One module per verb group; each owns its clap arguments and handler.
#![forbid(unsafe_code)]
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
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
mod org;
pub mod output;
mod package;
mod project;
mod run;
pub mod tools;
pub mod tui;
mod update;

pub use context::Context;

#[derive(Parser, Debug)]
#[command(
    name = "cozy",
    version,
    about = "Run models on your machines and rented GPUs"
)]
struct Cli {
    /// Print JSON (NDJSON when following) instead of tables.
    #[arg(long, global = true)]
    json: bool,
    /// Agree without being asked: to a rental's quote, a deletion.
    #[arg(long, short = 'y', global = true)]
    yes: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run a function: `pkg/fn` (first-party), `org/pkg/fn`, `./dir[/fn]` or `./script.py[/fn]`.
    Run(run::Args),
    /// Local, rented and added machines.
    Machine(machine::Args),
    /// Find, install and publish packages.
    Package(package::Args),
    /// Find, download, upload and publish models.
    Model(model::Args),
    /// Video projects: make one, render and approve its takes and versions, see what is missing or out of date.
    Project(project::Args),
    /// Claim an organization parked under your name.
    Org(org::Args),
    /// Sign in to the Hub with this computer's device key.
    Login(account::Login),
    /// Sign out of the Hub.
    Logout(account::Logout),
    /// Show the signed-in account, its credit and where to buy more.
    Whoami(account::Whoami),
    /// Calcifer, machines and runs at a glance.
    Status(daemon::Status),
    /// Replace this binary with the newest release.
    Update(update::Args),
    /// Print a shell completion script.
    Completion(completion::Args),
    /// Diagnose this computer's cozy (Calcifer, its service, the machine) and fix a stuck Calcifer;
    /// then a machine's driver, GPUs, toolchain and kernels.
    Doctor(doctor::Args),
}

/// Parses the command line, loads the config once, runs the verb and renders its refusal. Exit
/// codes: 0 success, 1 a refusal or a run that did not succeed, 2 a usage error.
pub fn main() -> ExitCode {
    completion::answer(Cli::command);
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return usage(error),
    };
    let loaded = match config::load() {
        Ok(loaded) => loaded,
        Err(error) => return output::refuse(cli.json, &error),
    };
    let warnings = loaded.warnings.clone();
    let ctx = Context::new(cli.json, cli.yes, loaded, output::Term::detect());
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

/// clap's own rendering for a person; with `--json` the typed envelope, exit 2.
fn usage(error: clap::Error) -> ExitCode {
    use clap::error::ErrorKind;
    let json = std::env::args_os().any(|arg| arg == "--json");
    if !json
        || matches!(
            error.kind(),
            ErrorKind::DisplayHelp
                | ErrorKind::DisplayVersion
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        )
    {
        error.exit();
    }
    let message = error.render().to_string();
    let message = message.lines().next().unwrap_or_default();
    let message = message.strip_prefix("error: ").unwrap_or(message);
    output::refuse(true, &Error::new(Kind::Usage, "usage", message))
}

async fn dispatch(ctx: &Context, command: Command) -> Result<ExitCode, Error> {
    match command {
        Command::Run(args) => run::exec(ctx, args).await,
        Command::Machine(args) => machine::exec(ctx, args).await,
        Command::Package(args) => package::exec(ctx, args).await,
        Command::Model(args) => model::exec(ctx, args).await,
        Command::Project(args) => project::exec(ctx, args).await,
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
