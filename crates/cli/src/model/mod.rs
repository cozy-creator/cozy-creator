//! `cozy model`: the Hub's model catalog, and models on machines.
use std::process::ExitCode;

use clap::Subcommand;
use api::error::{Error, Kind};

use crate::{Context, output};

mod catalog;
mod machine;
pub(crate) mod live;
mod quantize;
mod releases;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(clap::Args, Debug)]
pub(crate) struct LoraArgs {
    #[command(subcommand)]
    verb: LoraVerb,
}

#[derive(Subcommand, Debug)]
enum LoraVerb {
    /// Browse LoRAs from the model catalog.
    #[command(alias = "list")]
    Ls(Ls),
    /// Download a LoRA to a machine.
    Download(Download),
}

pub(crate) async fn lora_exec(ctx: &Context, args: LoraArgs) -> Result<ExitCode, Error> {
    match args.verb {
        LoraVerb::Ls(_args) => Err(Error::new(Kind::Unavailable, "catalog_role_unavailable", "LoRA-only browsing needs role metadata from the Hub")
            .remedy("The unfiltered catalog is still available").next("cozy model ls")),
        LoraVerb::Download(args) => machine::download(ctx, args).await,
    }
}

#[derive(Subcommand, Debug)]
enum Verb {
    /// Search models on the Hub, or list downloaded models with --on.
    #[command(alias = "list")]
    Ls(Ls),
    /// Update a model's family, tags, trigger words or recommended strength.
    Edit(Edit),
    /// Download a model to a machine.
    Download(Download),
    /// Upload weights as a private model commit.
    Upload(Upload),
    /// Create a model commit in a package-defined quantization lane.
    Quantize(Quantize),
    /// Publish commits as a model release.
    Publish(Publish),
    /// Delete a model, release or unpublished commit.
    Delete(Delete),
}

#[derive(clap::Args, Debug)]
pub(crate) struct Ls {
    /// Search words or an exact org/name.
    query: Vec<String>,
    /// Filter by discovery family.
    #[arg(long)]
    family: Option<String>,
    /// Require this tag. Repeat for multiple tags.
    #[arg(long = "tag", value_name = "TAG")]
    tags: Vec<String>,
    /// Exclude this tag. Repeat for multiple tags.
    #[arg(long = "no-tag", value_name = "TAG")]
    no_tags: Vec<String>,
    /// Models per page (1–100).
    #[arg(long, default_value_t = 20)]
    limit: u32,
    /// Skip this many matching models.
    #[arg(long, default_value_t = 0, conflicts_with = "on")]
    offset: u32,
    /// List downloaded models on local, a named machine, or all machines.
    #[arg(long, value_name = "MACHINE")]
    on: Option<String>,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Edit {
    /// org/name.
    model: String,
    /// Set the discovery family. Use none to clear.
    #[arg(long)]
    family: Option<String>,
    /// Add a tag. Repeat for multiple tags.
    #[arg(long = "tag", value_name = "TAG")]
    tag: Vec<String>,
    /// Remove a tag. Repeat for multiple tags.
    #[arg(long = "untag", value_name = "TAG")]
    untag: Vec<String>,
    /// Set trigger words. An empty value clears them.
    #[arg(long)]
    trigger: Option<String>,
    /// Set recommended strength, or none to clear.
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
    /// Storage lane, such as bf16 or fp8. Defaults to the machine's preferred lane.
    #[arg(long, value_parser = api::model_ref::lane)]
    lane: Option<String>,
    /// Model or commit, hf://…, civitai://…, or an HTTPS URL.
    model: String,
    /// Local alias for the downloaded model.
    #[arg(value_name = "local/ALIAS")]
    alias: Option<String>,
    /// Source format, such as h3.lora. Usually detected.
    #[arg(long = "source-format", value_name = "FORMAT")]
    format: Option<String>,
    #[command(flatten)]
    placement: crate::run::Placement,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Upload {
    /// Weights file, CozyTensors folder, local alias, model source, or run:N[#output].
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::local))]
    source: String,
    /// Destination model, org/name.
    destination: String,
    /// Commit lane, such as bf16 or fp8.
    #[arg(long, value_parser = api::model_ref::lane)]
    lane: String,
    /// Source format, such as h3.lora. Usually detected.
    #[arg(long = "source-format", value_name = "FORMAT")]
    format: Option<String>,
    /// Mirror an author's model (admins). Optionally set its upstream URL.
    #[arg(long, value_name = "URL", num_args = 0..=1, default_missing_value = "")]
    mirror: Option<String>,
    /// Base component the LoRA was trained on, if absent from its metadata.
    #[arg(long = "trained-on", value_name = "COMPONENT")]
    trained_on: Option<String>,
    /// Override the LoRA's trigger words.
    #[arg(long)]
    trigger: Option<String>,
    /// Recommended LoRA strength.
    #[arg(long, allow_hyphen_values = true)]
    strength: Option<f64>,
    /// Base model or commit. Add :component,… to replace selected parts and keep the rest.
    #[arg(long, value_name = "MODEL[:COMPONENT,…]")]
    base: Option<String>,
    #[command(flatten)]
    placement: crate::run::Placement,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Quantize {
    /// Model release or commit to read.
    model: String,
    /// Quantization lane declared by the package.
    #[arg(long, value_parser = api::model_ref::lane)]
    lane: String,
    /// Package with the quantization recipe, [org/]name[@release] or ./dir.
    #[arg(long)]
    package: String,
    #[command(flatten)]
    placement: crate::run::Placement,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Publish {
    /// org/name.
    model: String,
    /// New, increasing release version (PEP 440).
    version: String,
    /// Commit hashes, one per lane.
    #[arg(required = true, value_name = "COMMIT")]
    commits: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Delete {
    /// org/name, org/name@version, or org/name@sha256:<commit>.
    model: String,
}

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
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
        Verb::Edit(args) => catalog::edit(ctx, args).await,
        Verb::Publish(args) => releases::publish(ctx, args).await,
        Verb::Delete(args) => releases::delete(ctx, args).await,
        Verb::Download(args) => machine::download(ctx, args).await,
        Verb::Upload(args) => machine::upload(ctx, args).await,
        Verb::Quantize(args) => quantize::quantize(ctx, args).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct ModelCommand { #[command(flatten)] args: Args }

    #[test]
    fn catalog_paging_and_filters_parse_together() {
        let parsed = ModelCommand::try_parse_from(["model", "ls", "fox", "--family", "sdxl", "--tag", "art", "--no-tag", "nsfw", "--offset", "20", "--limit", "10"]).unwrap();
        let Verb::Ls(args) = parsed.args.verb else { panic!("expected catalog") };
        assert_eq!((args.offset, args.limit), (20, 10));
        assert_eq!(args.query, ["fox"]);
        assert_eq!(args.family.as_deref(), Some("sdxl"));
        assert_eq!(args.tags, ["art"]);
        assert_eq!(args.no_tags, ["nsfw"]);
        assert!(ModelCommand::try_parse_from(["model", "info", "author/model"]).is_err());
        assert!(ModelCommand::try_parse_from(["model", "ls", "--offset", "-1"]).is_err());
    }
}
