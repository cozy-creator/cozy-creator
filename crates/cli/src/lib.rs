//! The `cozy` command line. One module per verb group; each owns its clap arguments and handler.
#![forbid(unsafe_code)]
#![deny(unreachable_pub)]
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use api::{
    config,
    error::{Error, Kind},
};

mod account;
mod calcifer_client;
mod completion;
mod context;
mod daemon;
mod doctor;
mod machine;
mod model;
mod open;
mod output;
mod package;
mod progress;
mod project;
mod run;
mod send;
mod tools;
mod tui;
mod update;

use context::Context;

const NEXT: &str = "next: cozy new \"<directory>\"
next: cozy open
next: cozy run --help";

#[derive(Parser, Debug)]
#[command(
    name = "cozy",
    version,
    about = "Run models on your machines and rented GPUs",
    after_help = NEXT
)]
struct Cli {
    /// Print JSON (NDJSON when following).
    #[arg(long, global = true)]
    json: bool,
    /// The whole record where a view shows part of it: every field with --json, every table and line without.
    #[arg(long, global = true, hide = true)]
    full: bool,
    /// Agree without being asked: to a rental's quote, a deletion.
    #[arg(long, short = 'y', global = true, hide = true)]
    yes: bool,
    /// None: show the available commands.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create a project in a new or existing directory.
    New(project::New),
    /// Open the current project in your browser.
    Open(open::Args),
    /// Run a function in the current project.
    Run(run::Args),
    /// Show the current project's health and warnings.
    Status(daemon::Status),
    /// Upload the current project's files to your account.
    Push,
    /// Download the current project's files from your account.
    Pull,
    /// Local, rented and added machines.
    Machine(machine::Args),
    /// Find, download, upload and publish models.
    Model(model::Args),
    /// Find, install and publish packages.
    Package(package::Args),
    /// Replace this binary with the newest release.
    Update(update::Args),
    /// Diagnose this computer's Cozy installation and machines.
    Doctor(doctor::Args),
    #[command(hide = true)]
    Login(account::Login),
    #[command(hide = true)]
    Logout(account::Logout),
    #[command(hide = true)]
    Whoami(account::Whoami),
    /// Set up Tab completion for your shell.
    Completion(completion::Args),
}

/// Parses the command line, loads the config once, runs the verb and renders its refusal. Exit
/// codes follow the operation's outcome.
pub fn main() -> ExitCode {
    completion::answer(Cli::command);
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return usage(error),
    };
    let Some(command) = cli.command else {
        return match Cli::command().print_help() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => output::refuse(cli.json, &Error::new(Kind::Internal, "help", error.to_string())),
        };
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
    match runtime.block_on(dispatch(&ctx, command)) {
        Ok(code) => code,
        Err(error) => output::refusal(&ctx, &error),
    }
}

/// Help and version keep clap's rendering; usage errors carry a self-correcting reference.
fn usage(error: clap::Error) -> ExitCode {
    use clap::error::ErrorKind;
    let arguments: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let json = arguments.iter().any(|arg| arg == "--json");
    if matches!(error.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand) {
        error.exit();
    }
    let rendered = error.render().to_string();
    let message = rendered.lines().next().unwrap_or_default().trim_start_matches("error: ");
    let mut command = Cli::command();
    let mut invocation = String::from("cozy");
    for argument in &arguments {
        if matches!(argument.to_str(), Some("--json" | "--full" | "--yes" | "-y")) {
            continue;
        }
        let Some(child) = argument.to_str().and_then(|word| command.find_subcommand(word)) else { break };
        invocation.push(' ');
        invocation.push_str(&argument.to_string_lossy());
        command = child.clone().bin_name(invocation.clone());
    }
    let reference = command.render_help().to_string();
    output::refuse(json, &Error::new(Kind::Usage, "usage", message).remedy(reference))
}

async fn dispatch(ctx: &Context, command: Command) -> Result<ExitCode, Error> {
    match command {
        Command::Run(args) => run::exec(ctx, args).await,
        Command::Machine(args) => machine::exec(ctx, args).await,
        Command::Package(args) => package::exec(ctx, args).await,
        Command::Model(args) => model::exec(ctx, args).await,
        Command::New(args) => project::create(ctx, args).await,
        Command::Status(args) => daemon::status(ctx, args).await,
        Command::Push => project::transfer(ctx, "push", "sent").await,
        Command::Pull => project::transfer(ctx, "pull", "fetched").await,
        Command::Open(args) => open::exec(ctx, args).await,
        Command::Login(args) => account::login(ctx, args).await,
        Command::Logout(args) => account::logout(ctx, args).await,
        Command::Whoami(args) => account::whoami(ctx, args).await,
        Command::Update(args) => update::exec(ctx, args).await,
        Command::Completion(args) => completion::exec(ctx, args).await,
        Command::Doctor(args) => doctor::exec(ctx, args).await,
    }
}
