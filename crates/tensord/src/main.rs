//! The `tensord` binary: the machine role for TensorD's and Calcifer's tests. `cozy machine
//! serve` is the product's entry.
#![forbid(unsafe_code)]
#![deny(unreachable_pub)]
use clap::{Parser, Subcommand};
use std::process::ExitCode;
use tensord::{config::Flags, serve};

#[derive(Parser)]
#[command(name = "tensord", version = ::api::VERSION)]
struct Tensord {
    #[command(subcommand)]
    mode: Option<Mode>,
    #[command(flatten)]
    flags: Flags,
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
    /// The machine's own process opening its GPUs.
    #[command(hide = true)]
    ProbeGpus {
        ids: String,
        #[arg(long)]
        contexts: bool,
    },
}

fn main() -> ExitCode {
    let tensord = Tensord::parse();
    let done = match tensord.mode {
        None => return serve::machine(tensord.flags.lifetime()),
        Some(Mode::Run(flags)) => return serve::machine(flags.lifetime()),
        Some(Mode::Version { .. }) => serve::version(),
        Some(Mode::ProbeGpus { ids, contexts }) => {
            let contexts = contexts.then(|| "--contexts".into());
            return serve::probe_gpus(std::iter::once(ids.into()).chain(contexts));
        }
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error}");
            ExitCode::FAILURE
        }
    }
}
