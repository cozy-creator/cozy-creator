//! `cozy model quantize`: a new commit in a lane, written from a commit (or a release's original
//! weights) by the recipe a named package declares for that lane. The package is always named, a
//! release or a local directory: packages that serve the same model can each declare a recipe
//! for it. The package's `quantize` (the Runtime's) runs it on the machine, which uploads the
//! output as a commit of the model in that lane.
use std::path::PathBuf;
use std::process::ExitCode;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hub_client::types::ModelCard;
use protocol::calcifer::{Run, Status};
use protocol::config::Hub;
use protocol::error::{Error, Kind};
use tensorfs::format::{Header, ObjectRef};
use tensorfs::{Config, Endpoint};

use super::refs::{ModelRef, Repo};
use super::Quantize;
use crate::Context;
use crate::context::{hub_answer, hub_error};
use crate::output::{self, Record};
use crate::run::Callee;
use crate::run::interface::{Interface, Recipe, RECIPES};

/// Original weights: the plain dtypes a 4-bit lane may be written from.
const ORIGINAL: [&str; 3] = ["bf16", "fp16", "f32"];

/// The commit a quantize reads: spelled for the run, its repository, and the components its
/// header holds.
struct Source {
    spelled: String,
    repo: Repo,
    components: Vec<String>,
}

/// Whether a header of `kinds` holds only original weights.
fn original(kinds: &[String]) -> bool {
    !kinds.is_empty() && kinds.iter().all(|kind| ORIGINAL.contains(&kind.as_str()))
}

/// The commit `model` names, as the model's card lists it (its org's members see every commit):
/// a commit by id, or a release's lane of original weights (its only lane when it has one). A
/// 4-bit lane is written only from original weights (owner, 2026-10-22): from a commit holding
/// none it would stack two rounds of 4-bit error.
async fn commit(ctx: &Context, hub: &Hub, model: &ModelRef, lane: &str) -> Result<(Source, ObjectRef), Error> {
    let (repo, release, id) = match model {
        ModelRef::Hub { repo, release } => (repo, release.as_deref(), None),
        ModelRef::Commit { repo, commit } => (repo, None, Some(commit.as_str())),
        ModelRef::Local(_) | ModelRef::Provider(_) => return Err(not_on_hub(model)),
    };
    let card: ModelCard = hub_answer(ctx.hub_client(hub)?.get_model(&repo.org, &repo.name).await)?;
    let absent = |what: String| Error::new(Kind::NotFound, "model_commit_not_found", what).next(format!("cozy model info {repo}"));
    let (commit, header, kinds) = match id {
        Some(id) => {
            let found = card.commits.iter().find(|c| c.id == id)
                .ok_or_else(|| absent(format!("{repo} has no commit {id} you may read")))?;
            (ObjectRef { sha256: found.id.trim_start_matches("sha256:").into(), length: found.length as u64 }, found.header.clone(), found.kinds.clone())
        }
        None => {
            let found = match release {
                Some(version) => card.releases.iter().find(|r| r.version == version),
                None => card.releases.first(),
            };
            let found = found.ok_or_else(|| absent(format!("{repo} has no release {}", release.unwrap_or("yet"))))?;
            let originals: Vec<_> = found.lanes.values().filter(|l| original(&l.kinds)).collect();
            let picked = match (originals.as_slice(), found.lanes.len()) {
                ([one], _) => *one,
                (_, 1) => found.lanes.values().next().expect("one lane"),
                _ => return Err(Error::new(Kind::Usage, "model_quantize_source", format!(
                    "{repo}@{} has lanes {:?}: name the commit to quantize", found.version, found.lanes.keys().collect::<Vec<_>>()))
                    .next(format!("cozy model quantize {repo}@<commit> --lane {lane} --package <package>"))),
            };
            let commit = ObjectRef { sha256: picked.commit.trim_start_matches("sha256:").into(), length: picked.length as u64 };
            (commit, picked.header.clone(), picked.kinds.clone())
        }
    };
    if matches!(lane, "nvfp4" | "w4a8" | "svdq-nvfp4") && !original(&kinds) {
        return Err(Error::new(Kind::Usage, "model_quantize_source_quantized", format!(
            "{repo}@{} holds no original weights ({}): a {lane} lane from them stacks two rounds of 4-bit error", commit.id(), kinds.join(", ")))
            .remedy(format!("upload the original weights (cozy model upload <path> {repo} --lane bf16) and quantize those")));
    }
    let header = ObjectRef { sha256: header.sha256, length: header.length as u64 };
    let read = endpoint(ctx, hub).await?.document(&header).await.map_err(|f| Error::new(Kind::Unavailable, "model_header_unread", f.detail))?;
    let components = Header::from_cbor(&read).map_err(|e| Error::new(Kind::Validation, "model_header_invalid", e.to_string()))?
        .components().iter().map(|c| c.name.clone()).collect();
    let spelled = format!("{repo}@{}", commit.id());
    Ok((Source { spelled, repo: repo.clone(), components }, commit))
}

/// `hub`'s distribution endpoint as this computer's signed-in user, trusting the private CA the
/// Hub names (a test Hub's).
async fn endpoint(ctx: &Context, hub: &Hub) -> Result<Endpoint, Error> {
    let client = ctx.hub_signed_in(hub)?;
    let hub_client::Credential::User(session) = client.credential() else {
        unreachable!("a signed-in client");
    };
    let ca = hub_answer(client.get_execution_environment().await)?.environment.tensorhub_ca_der_b64url;
    let token = session.bearer(&client.anonymous()).await.map_err(hub_error)?.token;
    let config = Config {
        token: Some(tensorfs::bearer(token)),
        ca_der: ca.and_then(|ca| URL_SAFE_NO_PAD.decode(ca.trim()).ok()),
        ..Config::from_env()
    };
    Endpoint::with_config(&hub.origin.to_string(), &config).map_err(|f| Error::new(Kind::Validation, "endpoint_invalid", f.detail))
}

fn not_on_hub(model: &ModelRef) -> Error {
    Error::new(Kind::Usage, "model_quantize_source", format!("{model} is not on the Hub: quantize reads a Hub model"))
        .next(format!("cozy model upload {model} <org/name> --lane <lane>"))
}

/// The package named for the recipe: a release on the Hub, or local code nobody published.
enum Named {
    /// `org/name` and the release asked for, if any.
    Hub(String, Option<String>),
    /// A package directory, called as `cozy run ./dir` calls it.
    Local(PathBuf),
}

fn package(named: &str) -> Result<Named, Error> {
    if crate::run::target::local_path(named) {
        let cwd = std::env::current_dir()
            .map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
        return Ok(Named::Local(cwd.join(named.trim())));
    }
    let named = crate::package::PackageRef::parse(named)?;
    Ok(Named::Hub(named.package(), named.release))
}

/// The recipes of `lane` that read a commit holding `components`.
fn recipes<'i>(interface: &'i Interface, lane: &str, components: &[String]) -> Vec<&'i Recipe> {
    (interface.recipes.iter()).filter(|r| r.lane == lane && r.reads(components)).collect()
}

/// When one of `found` calibrates on the model itself: `model.<slot>=<source>` for each model the
/// package's `quantize` constructs (on demand). Else nothing: another recipe's model is never bound.
fn calibration(interface: &Interface, found: &[&Recipe], source: &str) -> Vec<String> {
    if found.iter().all(|r| r.calibrate.is_empty()) {
        return vec![];
    }
    (interface.callables.iter()).filter(|c| c.name == RECIPES).flat_map(|c| &c.models)
        .filter(|slot| !slot.weights).map(|slot| format!("model.{}={source}", slot.param)).collect()
}

/// What a package declares, for a refusal: each recipe's flag and the model it reads.
fn declared(interface: &Interface) -> String {
    if interface.recipes.is_empty() {
        return "it declares no recipes; name the package that does with --package org/name".into();
    }
    let each: Vec<String> = (interface.recipes.iter())
        .map(|r| {
            let mut reads: Vec<&str> = r.model.component_use.values().flatten().map(String::as_str).collect();
            reads.sort_unstable();
            reads.dedup();
            let reads = if reads.is_empty() { "any checkpoint".to_owned() } else { reads.join(", ") };
            format!("{} ({}: {reads})", r.flag(), r.model.class)
        })
        .collect();
    format!("its recipes: {}", each.join("; "))
}

pub(crate) async fn quantize(ctx: &Context, args: Quantize) -> Result<ExitCode, Error> {
    let named = package(&args.package)?;
    let model = ModelRef::parse(&args.model)?;
    let hub = ctx.hub()?;
    let (card, prepared);
    let callee = match &named {
        Named::Hub(package, release) => {
            card = crate::run::resolve(ctx, &hub, package, release.as_deref()).await?.card;
            Callee::Release { package, card: &card }
        }
        Named::Local(path) => {
            prepared = crate::run::local::prepare(ctx, path, args.placement.on(), false).await?;
            Callee::Local(&prepared)
        }
    };
    let interface = callee.interface();
    let (source, _) = commit(ctx, &hub, &model, &args.lane).await?;
    let found = recipes(interface, &args.lane, &source.components);
    if found.is_empty() {
        return Err(Error::new(Kind::NotFound, "model_quantizer_not_found", format!(
            "{callee} has no {} recipe for this model ({})", args.lane, source.components.join(", ")))
            .remedy(declared(interface)));
    }
    // The commit goes to the source's own model, in the lane; local code's records its package
    // as `local/<name>` and its release as the source's commit: an unpublished recipe, exactly.
    let mut terms = vec![source.spelled.clone(), source.repo.to_string(), format!("lane={}", args.lane),
        format!("package={}", callee.package()), format!("release={}", callee.release())];
    terms.extend(calibration(interface, &found, &source.spelled));
    let run = crate::run::call(ctx, &callee, RECIPES, terms, &args.placement).await?;
    if !ctx.json && run.status == Status::Completed
        && let Some(written) = written(&run, &args.lane)
    {
        Record::new().field("commit", format!("{}@{}", written.repo_id, written.revision)).field("lane", &written.lane).print(ctx);
        output::next(ctx, &[&format!("cozy model publish {} <version> {}", written.repo_id, written.revision)]);
    }
    Ok(crate::run::exit_code(&run))
}

/// The commit the run's `lane` output became.
fn written<'a>(run: &'a Run, lane: &str) -> Option<&'a protocol::machine::CheckpointRef> {
    run.output.iter().filter(|item| item.name == lane).find_map(|item| item.checkpoint.as_ref())
}
