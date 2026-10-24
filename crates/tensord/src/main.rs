//! The `tensord` binary: the machine role for TensorD's and Calcifer's tests. `cozy machine
//! serve` is the product's entry.
#![forbid(unsafe_code)]
use clap::{Parser, Subcommand};
use std::process::ExitCode;
use tensord::{config::Flags, serve};

#[derive(Parser)]
#[command(name = "tensord", version)]
struct Tensord {
    #[command(subcommand)]
    mode: Option<Mode>,
    #[command(flatten)]
    flags: Flags,
    /// Windows: the readiness pipe a stable parent handed the service it started.
    #[arg(long, hide = true, global = true)]
    service_ready: Option<u64>,
}

#[derive(Subcommand)]
enum Mode {
    /// The machine (the default): this computer's, or with --rental a pod's.
    Run(Flags),
    /// What this binary is, as JSON.
    Version {
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    let tensord = Tensord::parse();
    let done = match tensord.mode {
        None => return serve::machine(tensord.flags.lifetime(), tensord.service_ready),
        Some(Mode::Run(flags)) => return serve::machine(flags.lifetime(), tensord.service_ready),
        Some(Mode::Version { .. }) => serve::version(),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error}");
            ExitCode::FAILURE
        }
    }
}
