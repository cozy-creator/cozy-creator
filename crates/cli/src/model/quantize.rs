//! `cozy model quantize`: a new commit in a lane, written from a commit (or a release's original
//! weights) by the recipe a named package declares for that lane, through Calcifer
//! (CALCIFER-API.md "Models"). The package is always named, a release or a local directory:
//! packages that serve the same model can each declare a recipe for it. Its `quantize` (the
//! Runtime's) runs on the machine, which uploads the output as a commit of the model in that lane.
use std::process::ExitCode;

use protocol::calcifer::{On, Run, RunQuote, Status, Submitted};
use protocol::error::{Error, Kind};
use protocol::hub::ModelQuantize;

use super::Quantize;
use crate::Context;
use crate::output::{self, Record};
use crate::run::{exit_code, follow, mint_key};

pub(crate) async fn quantize(ctx: &Context, args: Quantize) -> Result<ExitCode, Error> {
    let on = args.placement.on()?;
    let cwd = std::env::current_dir()
        .map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    let package = match protocol::target::local(&args.package, &cwd, &ctx.user_home) {
        Some(path) => crate::ask::absolute(&path.to_string_lossy())?,
        None => args.package,
    };
    let mut ask = ModelQuantize {
        hub: Some(ctx.hub().to_string()),
        model: args.model,
        lane: args.lane.clone(),
        package,
        on: on.clone(),
        agreed: None,
    };
    let calcifer = ctx.calcifer().await?;
    if matches!(on, On::New(_)) {
        let quoted: RunQuote = calcifer.post("/v1/model-quantization-quotes", &ask, None).await?;
        if !crate::machine::agree(ctx, &quoted)? {
            return Err(Error::new(Kind::Usage, "quote_declined", "nothing was rented and the run was not sent"));
        }
        ask.agreed = Some(quoted.quote.agreed());
    }
    // Local code not yet ready on its machine is installed first, then asked again.
    let key = mint_key();
    let run = loop {
        let submitted: Submitted = calcifer.post("/v1/model-quantizations", &ask, Some(&key)).await?;
        if !submitted.preparing {
            break follow(ctx, submitted.run).await?;
        }
        crate::run::settle(ctx, submitted.run).await?;
    };
    if !ctx.json && run.status == Status::Completed
        && let Some(written) = written(&run, &args.lane)
    {
        Record::new().field("commit", format!("{}@{}", written.repo_id, written.revision)).field("lane", &written.lane).print(ctx);
        output::next(ctx, &[&format!("cozy model publish {} <version> {}", written.repo_id, written.revision)]);
    }
    Ok(exit_code(&run))
}

/// The commit the run's `lane` output became.
fn written<'a>(run: &'a Run, lane: &str) -> Option<&'a protocol::machine::CheckpointRef> {
    run.output.iter().filter(|item| item.name == lane).find_map(|item| item.checkpoint.as_ref())
}
