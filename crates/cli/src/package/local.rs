//! Local code's static description here: `cozy run ./dir --describe` reads its interface as a
//! publish does, installing nothing. Everything else about local code is Calcifer's
//! (`calcifer::local_code`).
use std::path::Path;

use calcifer::local_code::{self, Project};
use protocol::error::Error;

use super::author::{Described, described};
use crate::Context;

/// The `local/<name>` package at `path` and its interface, read from its source as a publish
/// reads it: nothing installs, and none of it runs.
pub(crate) async fn describe(ctx: &Context, path: &Path) -> Result<(String, Described), Error> {
    let project = Project::read(local_code::project(&ctx.home, path)?)?;
    Ok((format!("local/{}", project.name), described(&project, ctx).await?))
}
