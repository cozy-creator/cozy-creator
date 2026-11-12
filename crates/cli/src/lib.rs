//! The `cozy` command line. One module per verb group; each owns its clap arguments and handler.
#![forbid(unsafe_code)]
#![deny(unreachable_pub)]
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches as _, Parser, Subcommand};
use api::{
    config,
    error::{Error, Kind},
};

mod account;
mod calcifer_client;
mod completion;
mod context;
mod daemon;
mod dev;
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
    disable_help_subcommand = true,
    about = "Run models on your machines and rented GPUs",
    after_help = NEXT
)]
struct Cli {
    /// Print JSON (NDJSON when following).
    #[arg(long, global = true)]
    json: bool,
    /// Include complete records and untruncated text.
    #[arg(long, global = true)]
    full: bool,
    /// Confirm actions without prompting.
    #[arg(long, short = 'y', global = true)]
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
    /// Encrypt, lock or recover the current project.
    Vault(project::vault::Args),
    /// Local, rented and added machines.
    Machine(machine::Args),
    /// Browse and install models.
    Model(model::Args),
    /// Browse and install LoRAs.
    Lora(model::LoraArgs),
    /// Browse and install packages.
    Package(package::Args),
    /// Replace this binary with the newest release.
    Update(update::Args),
    /// Diagnose this computer's Cozy installation and machines.
    Doctor(doctor::Args),
    /// Restart Calcifer on the installed cozy.
    Restart(daemon::Restart),
    /// Sign in to your TensorHub account.
    Login(account::Login),
    /// Sign out of your TensorHub account.
    Logout(account::Logout),
    /// Show your account and available credit.
    Whoami(account::Whoami),
    /// Set up Tab completion for your shell.
    Completion(completion::Args),
}

/// Parses the command line, loads the config once, runs the verb and renders its refusal. Exit
/// codes follow the operation's outcome.
pub fn main() -> ExitCode {
    completion::answer(|| dev::hide(Cli::command()));
    let matches = match dev::hide(Cli::command()).try_get_matches() {
        Ok(matches) => matches,
        Err(error) => return usage(error),
    };
    let cli = match Cli::from_arg_matches(&matches) {
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
    if let Err(error) = dev::admit(&matches, loaded.config.dev) {
        return output::refuse(cli.json, &error);
    }
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
    output::refuse(json, &usage_error(&error, &arguments))
}

fn usage_error(error: &clap::Error, arguments: &[std::ffi::OsString]) -> Error {
    let rendered = error.render().to_string();
    let message = rendered.lines().take_while(|line| !line.trim().is_empty()).collect::<Vec<_>>().join("\n");
    let message = message.trim_start_matches("error: ");
    let mut command = dev::hide(Cli::command());
    command.build();
    let mut invocation = String::from("cozy");
    for argument in arguments {
        if matches!(argument.to_str(), Some("--json" | "--full" | "--yes" | "-y")) {
            continue;
        }
        let Some(child) = argument.to_str().and_then(|word| command.find_subcommand(word)) else { break };
        invocation.push(' ');
        invocation.push_str(&argument.to_string_lossy());
        command = child.clone().bin_name(invocation.clone());
    }
    Error::new(Kind::Usage, "usage", message).remedy(format!("{invocation} --help"))
}

async fn dispatch(ctx: &Context, command: Command) -> Result<ExitCode, Error> {
    match command {
        Command::Run(args) => run::exec(ctx, args).await,
        Command::Machine(args) => machine::exec(ctx, args).await,
        Command::Package(args) => package::exec(ctx, args).await,
        Command::Model(args) => model::exec(ctx, args).await,
        Command::Lora(args) => model::lora_exec(ctx, args).await,
        Command::New(args) => project::create(ctx, args).await,
        Command::Status(args) => daemon::status(ctx, args).await,
        Command::Vault(args) => project::vault::exec(ctx, args).await,
        Command::Open(args) => open::exec(ctx, args).await,
        Command::Login(args) => account::login(ctx, args).await,
        Command::Logout(args) => account::logout(ctx, args).await,
        Command::Whoami(args) => account::whoami(ctx, args).await,
        Command::Update(args) => update::exec(ctx, args).await,
        Command::Completion(args) => completion::exec(ctx, args).await,
        Command::Doctor(args) => doctor::exec(ctx, args).await,
        Command::Restart(args) => daemon::restart_calcifer(ctx, args).await,
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn usage_errors_name_the_nearest_help_command() {
        for (words, expected) in [
            (vec!["ls"], "cozy --help"),
            (vec!["machine", "unknown-command"], "cozy machine --help"),
            (vec!["--unknown-option"], "cozy --help"),
            (vec!["machine", "ls", "--unknown-option"], "cozy machine ls --help"),
            (vec!["run", "watch", "1", "--unknown-option"], "cozy run watch --help"),
            (vec!["--json", "--full", "machine", "rent", "--unknown-option"], "cozy machine rent --help"),
        ] {
            let arguments: Vec<std::ffi::OsString> = words.iter().map(|word| std::ffi::OsString::from(*word)).collect();
            let error = dev::hide(Cli::command()).try_get_matches_from(std::iter::once("cozy").chain(words)).unwrap_err();
            let body = usage_error(&error, &arguments);
            assert_eq!(body.remedy.as_deref(), Some(expected));
            assert_eq!(body.kind, Kind::Usage);
            assert_eq!(body.exit_code(), 2);
            assert!(!body.message.contains("Usage:"));
        }
    }

    #[test]
    fn usage_errors_retain_missing_argument_names() {
        let arguments = ["machine".into(), "show".into()];
        let error = dev::hide(Cli::command()).try_get_matches_from(["cozy", "machine", "show"]).unwrap_err();
        let body = usage_error(&error, &arguments);
        assert!(body.message.contains("<NAME>"));
        assert_eq!(body.remedy.as_deref(), Some("cozy machine show --help"));
    }
}
