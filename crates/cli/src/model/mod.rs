//! `cozy model`: the Hub's model catalog, and models on machines.
use std::process::ExitCode;

use clap::Subcommand;
use protocol::error::Error;

use crate::{Context, output};

mod catalog;
mod machine;
mod quantize;
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
    #[command(alias = "list")]
    Ls(Ls),
    /// A model's releases and their lanes; to its org's members, every commit too.
    Info(Info),
    /// What its owner says of a model on the Hub: its family, tags, trigger words and recommended strength. A flag left
    /// out keeps its field.
    Edit(Edit),
    /// Make a model present in a machine's store. Follows it to its end; Ctrl-C stops following, never the work.
    Download(Download),
    /// Make a model on a machine, or take a run's weights output (run:N), and upload it as a new
    /// private commit in a lane: its id. `--base` makes it another model with the source's
    /// components in place of its own. Follows it to its end; Ctrl-C stops following, never the work.
    Upload(Upload),
    /// Write a new commit in a lane from a commit or release, by the recipe a named package
    /// declares for that lane. Follows it to its end; Ctrl-C stops following, never the work.
    Quantize(Quantize),
    /// Release commits as a version, public until you delete it: one commit per lane, the version
    /// higher than every earlier one, deleted or not.
    Publish(Publish),
    /// Delete a release (its version is never reused), a commit no release names, or the whole model.
    Delete(Delete),
}

#[derive(clap::Args, Debug)]
pub(crate) struct Ls {
    /// Search text, or an exact org/name; none lists every model you can see.
    query: Vec<String>,
    /// Only models of this discovery family.
    #[arg(long)]
    family: Option<String>,
    /// Only models with this tag; repeat it for several.
    #[arg(long = "tag", value_name = "TAG")]
    tags: Vec<String>,
    /// Only models without this tag (`--no-tag nsfw` leaves out models that make sexual or nude images); repeat it for
    /// several.
    #[arg(long = "no-tag", value_name = "TAG")]
    no_tags: Vec<String>,
    /// At most this many models (the Hub lists up to 100 at once).
    #[arg(long, default_value_t = 20)]
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
pub(crate) struct Edit {
    /// org/name.
    model: String,
    /// What it runs as, which picks the tool that runs it (a LoRA needs one); `none` clears your label, and the
    /// family its weights verify as shows.
    #[arg(long)]
    family: Option<String>,
    /// Add a tag (`nsfw` for one that makes sexual or nude images, asked for or not); repeat it for several.
    #[arg(long = "tag", value_name = "TAG")]
    tag: Vec<String>,
    /// Remove a tag; repeat it for several.
    #[arg(long = "untag", value_name = "TAG")]
    untag: Vec<String>,
    /// Every tag, comma-separated, in place of its tags; empty clears them.
    #[arg(long, value_name = "A,B,…")]
    tags: Option<String>,
    /// The words its prompts use; empty clears them.
    #[arg(long)]
    trigger: Option<String>,
    /// Its recommended strength, or `none` to clear it. Never applied: a run's default stays 1.0.
    #[arg(long, value_parser = strength)]
    strength: Option<Strength>,
}

/// `--strength`: a number, or `none`.
#[derive(Clone, Debug)]
pub(crate) struct Strength(Option<f64>);

fn strength(text: &str) -> Result<Strength, String> {
    match text.trim() {
        none if none.eq_ignore_ascii_case("none") => Ok(Strength(None)),
        number => number.parse().map(|n| Strength(Some(n))).map_err(|_| format!("{text:?} is no number; `none` clears it")),
    }
}

#[derive(clap::Args, Debug)]
pub(crate) struct Download {
    /// The lane (a label: bf16, fp8, …); absent, the Hub picks what the machine runs best.
    #[arg(long, value_parser = protocol::model_ref::lane)]
    lane: Option<String>,
    /// org/name[@release], org/name@sha256:<commit>, hf://…, civitai://… or https://….
    model: String,
    /// Keep it on the machine under this name.
    #[arg(value_name = "local/ALIAS")]
    alias: Option<String>,
    /// The source file's convention: a reviewed TensorFS format, e.g. h3.lora; usually detected.
    #[arg(long = "source-format", value_name = "FORMAT")]
    format: Option<String>,
    #[command(flatten)]
    placement: crate::run::Placement,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Upload {
    /// A provider source, local/<alias>, a Hub model or commit, ./file.safetensors, a
    /// CozyTensors folder (one .cozytensors file and its repo files; its chunks may already be at
    /// the Hub), or run:N[#output], a run's weights output, uploaded from the machine that holds it.
    /// A Hub commit (org/name@sha256:…) is recorded at the Hub directly: nothing moves.
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::local))]
    source: String,
    /// The Hub model it becomes a commit of: org/name.
    destination: String,
    /// The commit's lane, a label (bf16, fp8, int8-convrot, …); from a Hub model, the lane it
    /// copies. A lane named bf16, fp16, int8, fp8, nvfp4, … must be the weights' width.
    #[arg(long, value_parser = protocol::model_ref::lane)]
    lane: String,
    /// The source file's convention: a reviewed TensorFS format, e.g. h3.lora; usually detected.
    #[arg(long = "source-format", value_name = "FORMAT")]
    format: Option<String>,
    /// It mirrors its author's model (admins): the destination org is parked for the author to
    /// claim. Bare, the hf:// or civitai:// source is the upstream page; else name it.
    #[arg(long, value_name = "URL", num_args = 0..=1, default_missing_value = "")]
    mirror: Option<String>,
    /// The base component a LoRA was trained on, when its file does not declare it (H3:
    /// transformer for FL2VA, transformer_ref for Ref2VA).
    #[arg(long = "trained-on", value_name = "COMPONENT")]
    trained_on: Option<String>,
    /// Words the LoRA's prompt uses, shown by `cozy model info`; a Civitai source's own when
    /// omitted.
    #[arg(long)]
    trigger: Option<String>,
    /// The LoRA's recommended strength, shown by `cozy model info`; --lora still defaults to 1.0.
    #[arg(long, allow_hyphen_values = true)]
    strength: Option<f64>,
    /// Start from this model (org/name@release in the commit's --lane, or org/name@sha256:<commit>):
    /// its header with the source's components in place of its own, the rest its objects as the
    /// Hub holds them. `:<component>[,…]` replaces only those: the source's of each name, or its
    /// one component under every name (a hybrid DiT under transformer,transformer_ref), stored and
    /// uploaded once. A Hub source is referenced too: nothing is downloaded.
    #[arg(long, value_name = "MODEL[:COMPONENT,…]")]
    base: Option<String>,
    #[command(flatten)]
    placement: crate::run::Placement,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Quantize {
    /// What it reads: org/name@sha256:<commit> or org/name@<release> (none: the newest).
    model: String,
    /// The lane to write: a recipe the package declares for this model; the new commit's lane.
    #[arg(long, value_parser = protocol::model_ref::lane)]
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
    /// org/name for the whole model, org/name@<version> for one release, or org/name@sha256:<commit>
    /// for a commit no release names.
    model: String,
}

pub async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    match args.verb {
        Verb::Ls(args) => match args.on {
            Some(on) => {
                if args.family.is_some() || !args.tags.is_empty() || !args.no_tags.is_empty() {
                    output::warn("--family, --tag and --no-tag search the Hub; with --on they are ignored");
                }
                machine::list(ctx, (on != "all").then_some(on), &args.query).await
            }
            None => catalog::search(ctx, args).await,
        },
        Verb::Info(args) => catalog::info(ctx, args).await,
        Verb::Edit(args) => catalog::edit(ctx, args).await,
        Verb::Publish(args) => releases::publish(ctx, args).await,
        Verb::Delete(args) => releases::delete(ctx, args).await,
        Verb::Download(args) => machine::download(ctx, args).await,
        Verb::Upload(args) => machine::upload(ctx, args).await,
        Verb::Quantize(args) => quantize::quantize(ctx, args).await,
    }
}
