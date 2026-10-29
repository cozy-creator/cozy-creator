//! `cozy model`: the Hub's model catalog, and models on machines.
use std::process::ExitCode;

use clap::{ArgGroup, Subcommand};
use protocol::error::{Error, Kind};

use crate::Context;

mod catalog;
mod from_run;
mod machine;
mod quantize;
pub(crate) mod refs;
mod releases;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Subcommand, Debug)]
enum Verb {
    /// The Hub's models you can see, one row each with its newest release: public ones, and your
    /// own and your orgs' while private (no release yet). Words search; `--on` lists what machines
    /// hold instead.
    Ls(Ls),
    /// A model's releases and their lanes; to its org's members, every commit too.
    Info(Info),
    /// Set or clear a model's discovery family.
    Family(Family),
    /// Make a model present in a machine's store. Follows it to its end; Ctrl-C stops following, never the work.
    Download(Download),
    /// Make a model on a machine, or take a run's weights output (run:N), and upload it as a new
    /// private commit in a lane: its id. Follows it to its end; Ctrl-C stops following, never the work.
    Upload(Upload),
    /// Write a new commit in a lane from a commit or release, by the recipe a named package
    /// declares for that lane. Follows it to its end; Ctrl-C stops following, never the work.
    Quantize(Quantize),
    /// Release commits as a version, public for good: one commit per lane, the version higher
    /// than every earlier one.
    Publish(Publish),
    /// Delete a commit no release names, or the whole model.
    Delete(Delete),
}

#[derive(clap::Args, Debug)]
pub(crate) struct Ls {
    /// Search text, or an exact org/name; none lists every model you can see.
    query: Vec<String>,
    /// Only models of this discovery family.
    #[arg(long)]
    family: Option<String>,
    /// At most this many models (1–100).
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
    limit: u32,
    /// What machines hold instead of the Hub's models: `local`, a machine's name, or `all`.
    #[arg(long, value_name = "MACHINE")]
    on: Option<String>,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Info {
    /// org/name, or org/name@release for one release.
    model: String,
}

#[derive(clap::Args, Debug)]
#[command(group(ArgGroup::new("change").required(true).args(["family", "clear"])))]
pub(crate) struct Family {
    /// org/name.
    model: String,
    /// The family to set.
    family: Option<String>,
    /// Clear the family.
    #[arg(long)]
    clear: bool,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Download {
    /// The lane (a label: bf16, fp8, …); absent, the Hub picks what the machine runs best.
    #[arg(long, value_parser = refs::lane)]
    lane: Option<String>,
    /// org/name[@release], org/name@sha256:<commit>, hf://…, civitai://… or https://….
    model: String,
    /// Keep it on the machine under this name.
    #[arg(value_name = "local/ALIAS")]
    alias: Option<String>,
    /// The source file's convention: a reviewed TensorFS format, e.g. h3.lora; usually detected.
    #[arg(long = "source-format", value_name = "FORMAT")]
    profiles: Vec<String>,
    #[command(flatten)]
    placement: crate::run::Placement,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Upload {
    /// A provider source, local/<alias>, a Hub model or commit, ./file.safetensors, a
    /// CozyTensors folder (one .cozytensors file and its repo files; its chunks may already be at
    /// the Hub), or run:N[#output], a run's weights output, uploaded from the machine that holds it.
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::local))]
    source: String,
    /// The Hub model it becomes a commit of: org/name.
    destination: String,
    /// The commit's lane, a label (bf16, fp8, int8-convrot, …).
    #[arg(long, value_parser = refs::lane)]
    lane: String,
    /// The source file's convention: a reviewed TensorFS format, e.g. h3.lora; usually detected.
    #[arg(long = "source-format", value_name = "FORMAT")]
    profiles: Vec<String>,
    #[command(flatten)]
    placement: crate::run::Placement,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Quantize {
    /// What it reads: org/name@sha256:<commit> or org/name@<release> (none: the newest).
    model: String,
    /// The lane to write: a recipe the package declares for this model; the new commit's lane.
    #[arg(long, value_parser = refs::lane)]
    lane: String,
    /// The package whose recipe writes the lane: org/name[@release], or ./dir for one not yet
    /// published. Required: packages that serve the same model can each declare one.
    #[arg(long)]
    package: String,
    #[command(flatten)]
    placement: crate::run::Placement,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Publish {
    /// org/name.
    model: String,
    /// The release's version: PEP 440, higher than every earlier release.
    version: String,
    /// The commits it names (sha256:<hex>), one per lane.
    #[arg(required = true, value_name = "COMMIT")]
    commits: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Delete {
    /// org/name for the whole model, or org/name@sha256:<commit> for a commit no release names.
    model: String,
}

pub async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    match args.verb {
        Verb::Ls(args) => match args.on {
            Some(on) if args.query.is_empty() && args.family.is_none() => {
                machine::list(ctx, (on != "all").then_some(on)).await
            }
            Some(_) => Err(Error::new(
                Kind::Usage,
                "model_ls_on_query",
                "--on lists what machines hold; words and --family search the Hub",
            )),
            None => catalog::search(ctx, args).await,
        },
        Verb::Info(args) => catalog::info(ctx, args).await,
        Verb::Family(args) => catalog::family(ctx, args).await,
        Verb::Publish(args) => releases::publish(ctx, args).await,
        Verb::Delete(args) => releases::delete(ctx, args).await,
        Verb::Download(args) => machine::download(ctx, args).await,
        Verb::Upload(args) => machine::upload(ctx, args).await,
        Verb::Quantize(args) => quantize::quantize(ctx, args).await,
    }
}
