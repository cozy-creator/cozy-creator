//! Print the authenticated local browser URL for the current project.
use std::process::ExitCode;

use api::{calcifer::SessionCode, error::Error};
use serde_json::json;

use crate::{Context, output, project};

#[derive(clap::Args, Debug)]
#[command(after_help = "Example:\n  cd my-project && cozy open")]
pub(crate) struct Args {}

pub(crate) async fn exec(ctx: &Context, _args: Args) -> Result<ExitCode, Error> {
    let project = project::current(ctx).await?;
    let calcifer = ctx.calcifer().await?;
    calcifer.text("/").await?;
    let code: SessionCode = calcifer.post("/v1/session-codes", &json!({}), None).await?;
    let link = format!("{}&to=/projects/{}", code.url, project.id);
    output::document(ctx, &json!({ "url": link, "expires_ms": code.expires_ms }));
    Ok(ExitCode::SUCCESS)
}
