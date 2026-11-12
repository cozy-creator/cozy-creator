//! `cozy model`: the Hub's model catalog, and models on machines.
use std::{io::IsTerminal as _, process::ExitCode};

use clap::Subcommand;
use api::error::Error;

use crate::{Context, output};

mod catalog;
mod machine;
pub(crate) mod live;
mod quantize;
mod releases;
mod show;

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
    /// Show what each lane's checkpoint holds: its factors, α, adapter record and files.
    Show(Show),
    /// Install a LoRA on a machine.
    Install(Download),
    /// Uninstall downloaded copies from this machine.
    Uninstall(Remove),
    /// Update a LoRA's family, tags, trigger words or recommended strength.
    Edit(LoraEdit),
    /// Upload a LoRA as a private checkpoint.
    #[command(hide = true)]
    Upload(LoraUpload),
    /// Publish checkpoints as a LoRA release.
    #[command(hide = true)]
    Publish(Publish),
    /// Make a release private while keeping its checkpoints.
    Unpublish(Unpublish),
    /// Delete a LoRA, release or checkpoint from TensorHub.
    Delete(Delete),
}

pub(crate) async fn lora_exec(ctx: &Context, args: LoraArgs) -> Result<ExitCode, Error> {
    match args.verb {
        LoraVerb::Ls(args) => list(ctx, args, "lora").await,
        LoraVerb::Show(args) => show::show(ctx, args, "lora").await,
        LoraVerb::Install(args) => machine::download(ctx, args).await,
        LoraVerb::Uninstall(args) => machine::remove(ctx, &args.model).await,
        LoraVerb::Edit(mut args) => {
            args.edit.trigger = args.trigger;
            args.edit.strength = args.strength;
            catalog::edit(ctx, args.edit).await
        }
        LoraVerb::Upload(mut args) => {
            args.upload.trained_on = args.trained_on;
            args.upload.trigger = args.trigger;
            args.upload.strength = args.strength;
            machine::upload(ctx, args.upload).await
        }
        LoraVerb::Publish(args) => releases::publish(ctx, args, "lora").await,
        LoraVerb::Unpublish(args) => releases::unpublish(ctx, args, "lora").await,
        LoraVerb::Delete(args) => releases::delete(ctx, args, "lora").await,
    }
}

#[derive(Subcommand, Debug)]
enum Verb {
    /// Search models on the Hub, or list downloaded models with --on.
    #[command(alias = "list")]
    Ls(Ls),
    /// Show what each lane's checkpoint holds: components, configs, files and its recorded base.
    Show(Show),
    /// Update a model's family or tags.
    Edit(Edit),
    /// Install a model on a machine.
    Install(Download),
    /// Uninstall downloaded copies from this machine.
    Uninstall(Remove),
    /// Upload weights as a private model checkpoint.
    #[command(hide = true)]
    Upload(Upload),
    /// Create a model checkpoint in a package-defined quantization lane.
    #[command(hide = true)]
    Quantize(Quantize),
    /// Publish checkpoints as a model release.
    #[command(hide = true)]
    Publish(Publish),
    /// Make a release private while keeping its checkpoints.
    Unpublish(Unpublish),
    /// Delete a model, release or checkpoint from TensorHub.
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
    #[arg(long, default_value_t = 0)]
    offset: u32,
    /// List cached models on local, a named machine, or all machines.
    #[arg(long, value_name = "MACHINE")]
    on: Option<String>,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Show {
    /// org/name, org/name@release or org/name@sha256:<checkpoint>. No release: the newest.
    #[arg(value_name = "MODEL")]
    model: String,
    /// Only this lane.
    #[arg(long, value_parser = api::model_ref::lane)]
    lane: Option<String>,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Edit {
    /// org/name.
    #[arg(value_name = "MODEL")]
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
    #[arg(skip)]
    trigger: Option<String>,
    #[arg(skip)]
    strength: Option<Strength>,
}

#[derive(clap::Args, Debug)]
struct LoraEdit {
    #[command(flatten)]
    edit: Edit,
    /// Set trigger words. An empty value clears them.
    #[arg(long)]
    trigger: Option<String>,
    /// Set recommended strength, or none to clear.
    #[arg(long, value_parser = strength)]
    strength: Option<Strength>,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Remove {
    /// org/name. Removes all downloaded releases and lanes, keeping the Hub model.
    #[arg(value_name = "MODEL")]
    model: String,
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
    /// Model, checkpoint, provider URL, or local weights/checkpoint file.
    #[arg(value_name = "REFERENCE")]
    model: String,
    /// Local name for the downloaded copy. The local/ prefix is optional.
    #[arg(value_name = "NAME", value_parser = local_alias)]
    alias: Option<String>,
    /// Source format, such as h3.lora. Usually detected.
    #[arg(long = "source-format", value_name = "FORMAT")]
    format: Option<String>,
    #[command(flatten)]
    placement: crate::run::Placement,
}

fn local_alias(text: &str) -> Result<String, String> {
    let text = text.trim();
    let name = match text.split_once('/') {
        Some((org, name)) if org.eq_ignore_ascii_case("local") => name,
        Some(_) => return Err("Use a local name; installing does not create a Hub model".into()),
        None => text,
    };
    let model_id = api::model_ref::ModelId::parse(&format!("local/{name}")).map_err(|error| error.message)?;
    Ok(model_id.to_string())
}

#[derive(clap::Args, Debug)]
pub(crate) struct Upload {
    /// Weights file, CozyTensors folder, local alias, model source, or run:N[#output].
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::local))]
    source: String,
    /// Destination model, org/name.
    #[arg(value_name = "MODEL")]
    destination: String,
    /// Checkpoint lane, such as bf16 or fp8.
    #[arg(long, value_parser = api::model_ref::lane)]
    lane: String,
    /// Source format, such as h3.lora. Usually detected.
    #[arg(long = "source-format", value_name = "FORMAT")]
    format: Option<String>,
    /// Mirror an author's model (admins). Optionally set its upstream URL.
    #[arg(long, value_name = "URL", num_args = 0..=1, default_missing_value = "")]
    mirror: Option<String>,
    #[arg(skip)]
    trained_on: Option<String>,
    #[arg(skip)]
    trigger: Option<String>,
    #[arg(skip)]
    strength: Option<f64>,
    /// Base model or checkpoint. A Hub checkpoint takes only what it lacks from it; a converted source's parts replace the base's. Add :component,… to place them.
    #[arg(long, value_name = "MODEL[:COMPONENT,…]")]
    base: Option<String>,
    /// Add a file as a component's asset, COMPONENT/PATH=FILE. Repeat for several.
    #[arg(long = "add", value_name = "COMPONENT/PATH=FILE")]
    add: Vec<String>,
    #[command(flatten)]
    placement: crate::run::Placement,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Quantize {
    /// Model release or checkpoint to read.
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
struct LoraUpload {
    #[command(flatten)]
    upload: Upload,
    /// Base model or checkpoint this LoRA was trained on.
    #[arg(long)]
    trained_on: Option<String>,
    /// Override the LoRA's trigger words.
    #[arg(long)]
    trigger: Option<String>,
    /// Recommended LoRA strength.
    #[arg(long, allow_hyphen_values = true)]
    strength: Option<f64>,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Publish {
    /// org/name.
    #[arg(value_name = "MODEL")]
    model: String,
    /// New, increasing release version (PEP 440).
    version: String,
    /// Checkpoint hashes, one per lane.
    #[arg(required = true, value_name = "CHECKPOINT")]
    checkpoints: Vec<String>,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Unpublish {
    /// Named model release, org/name@version.
    #[arg(value_name = "MODEL@VERSION")]
    model: String,
}

#[derive(clap::Args, Debug)]
pub(crate) struct Delete {
    /// org/name, org/name@version, or org/name@sha256:<checkpoint>.
    #[arg(value_name = "MODEL")]
    model: String,
}

async fn list(ctx: &Context, mut args: Ls, role: &str) -> Result<ExitCode, Error> {
    match args.on.clone() {
        Some(on) => {
            if args.family.is_some() || !args.tags.is_empty() || !args.no_tags.is_empty() {
                output::warn("--family, --tag and --no-tag search the Hub; with --on they are ignored");
                args.family = None;
                args.tags.clear();
                args.no_tags.clear();
            }
            if on == "local" && !ctx.json && ctx.term.stdout && std::io::stdin().is_terminal() {
                catalog::local(ctx, args, role).await
            } else {
                machine::list(ctx, (on != "all").then_some(on), &args.query, role, args.limit, args.offset).await
            }
        }
        None => catalog::search(ctx, args, role).await,
    }
}

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    match args.verb {
        Verb::Ls(args) => list(ctx, args, "model").await,
        Verb::Show(args) => show::show(ctx, args, "model").await,
        Verb::Edit(args) => catalog::edit(ctx, args).await,
        Verb::Publish(args) => releases::publish(ctx, args, "model").await,
        Verb::Unpublish(args) => releases::unpublish(ctx, args, "model").await,
        Verb::Delete(args) => releases::delete(ctx, args, "model").await,
        Verb::Install(args) => machine::download(ctx, args).await,
        Verb::Uninstall(args) => machine::remove(ctx, &args.model).await,
        Verb::Upload(args) => machine::upload(ctx, args).await,
        Verb::Quantize(args) => quantize::quantize(ctx, args).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory as _, Parser};

    #[derive(Parser, Debug)]
    struct ModelCommand { #[command(flatten)] args: Args }
    #[derive(Parser, Debug)]
    struct LoraCommand { #[command(flatten)] args: LoraArgs }

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

    #[test]
    fn download_names_are_local_without_an_explicit_prefix() {
        for name in ["my-model", "local/my-model"] {
            let model = ModelCommand::try_parse_from(["model", "install", "hf://owner/model", name]).unwrap();
            let Verb::Install(model) = model.args.verb else { panic!("expected download") };
            assert_eq!(model.alias.as_deref(), Some("local/my-model"));
            let lora = LoraCommand::try_parse_from(["lora", "install", "civitai://123", name]).unwrap();
            let LoraVerb::Install(lora) = lora.args.verb else { panic!("expected download") };
            assert_eq!(lora.alias.as_deref(), Some("local/my-model"));
        }
        assert!(ModelCommand::try_parse_from(["model", "install", "hf://owner/model", "owner/model"]).is_err());
    }

    #[test]
    fn adapter_metadata_is_only_exposed_in_lora_commands() {
        for (flag, value) in [("--trigger", "word"), ("--strength", "0.5")] {
            let error = ModelCommand::try_parse_from(["model", "edit", "author/model", flag, value]).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
        }
        for (flag, value) in [("--trigger", "word"), ("--strength", "0.5"), ("--trained-on", "author/base")] {
            let error = ModelCommand::try_parse_from(["model", "upload", "file", "author/model", "--lane", "bf16", flag, value]).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
        }
        assert!(LoraCommand::try_parse_from(["lora", "edit", "author/adapter", "--trigger", "word", "--strength", "0.5"]).is_ok());
        LoraCommand::try_parse_from(["lora", "upload", "file", "author/adapter", "--lane", "bf16", "--trained-on", "base", "--strength", "0.5"]).unwrap_or_else(|error| panic!("{error}"));
        assert!(ModelCommand::try_parse_from(["model", "uninstall", "author/model"]).is_ok());
        assert!(LoraCommand::try_parse_from(["lora", "uninstall", "author/adapter"]).is_ok());
        assert!(LoraCommand::try_parse_from(["lora", "info", "author/adapter"]).is_err());
    }

    #[test]
    fn model_and_lora_help_list_their_public_controls() {
        let mut cli = crate::dev::hide(crate::Cli::command());
        cli.build();
        let names = |name: &str| cli.find_subcommand(name).unwrap().get_subcommands().filter(|command| !command.is_hide_set()).map(|command| command.get_name().to_owned()).collect::<Vec<_>>();
        assert_eq!(names("model"), ["ls", "show", "edit", "install", "uninstall", "unpublish", "delete"]);
        assert_eq!(names("lora"), ["ls", "show", "install", "uninstall", "edit", "unpublish", "delete"]);
    }
}
