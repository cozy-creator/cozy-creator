//! `cozy model quantize`: a new checkpoint in a lane, written from a checkpoint (or a release's original
//! weights) by the recipe a named package declares for that lane, through Calcifer
//! (CALCIFER-API.md "Models"). The package is always named, a release or a local directory:
//! packages that serve the same model can each declare a recipe for it. Its `quantize` (the
//! Runtime's) runs on the machine, which uploads the output as a checkpoint of the model in that lane.
use std::process::ExitCode;

use api::calcifer::{Run, Status};
use api::error::{Error, Kind};
use api::calcifer::hub::{ModelQuantize, job};

use super::Quantize;
use crate::Context;
use crate::output::{self, Record};
use crate::run::{exit_code, follow, settled};

pub(crate) async fn quantize(ctx: &Context, args: Quantize) -> Result<ExitCode, Error> {
    let on = args.placement.on()?;
    let cwd = std::env::current_dir()
        .map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    // Local code goes by content.
    let (package, code) = match crate::run::local(ctx, &args.package, &cwd)? {
        Some((path, _)) => (String::new(), Some(crate::send::Sender::new(ctx).await?.code(&path).await?)),
        None => (args.package, None),
    };
    let ask = ModelQuantize { model: args.model, lane: args.lane.clone(), package };
    let run = crate::run::job(ctx, job::MODEL_QUANTIZE, &ask, on, code).await?;
    // Its install, no longer followed: nothing was called.
    let run = if settled(&run) { run } else { follow(ctx, run).await? };
    if !ctx.json && run.status == Status::Completed
        && let Some(written) = written(&run, &args.lane)
    {
        Record::new().field("checkpoint", format!("{}@{}", written.model_id, written.revision)).field("lane", &written.lane).print(ctx);
        output::next(ctx, &[&format!("cozy model publish {} <version> {}", written.model_id, written.revision)]);
    }
    Ok(exit_code(&run))
}

/// The checkpoint the run's `lane` output became.
fn written<'a>(run: &'a Run, lane: &str) -> Option<&'a api::machine::CheckpointRef> {
    run.output.iter().filter(|item| item.name == lane).find_map(|item| item.checkpoint.as_ref())
}
