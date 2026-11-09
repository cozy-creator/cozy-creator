//! `cozy run play N`: a link that plays a run's output in any browser, straight from the machine
//! that made it, local or rented, live as it grows. Calcifer reads the machine's WebRTC listener,
//! signs a capability for that one output and answers the link.
use std::process::ExitCode;

use api::{calcifer::Play, error::Error};

use crate::{Context, output};

pub(crate) async fn exec(
    ctx: &Context,
    run: &str,
    output: &str,
    expires: &str,
) -> Result<ExitCode, Error> {
    let calcifer = ctx.calcifer().await?;
    let run = calcifer.run(run).await?;
    let play: Play = calcifer
        .get_query(
            &format!("/v1/runs/{}/play", run.id),
            &[("output", output), ("expires", expires)],
        )
        .await?;
    if ctx.json {
        output::json(
            &serde_json::json!({"run": run.number, "output": output, "expires_s": play.expires_s, "link": play.url}),
        );
    } else {
        output::Record::new()
            .field("run", run.number)
            .field("output", output)
            .field("expires", expires)
            .field("link", &play.url)
            .print(ctx);
    }
    Ok(ExitCode::SUCCESS)
}
