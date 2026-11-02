//! The tools this computer has of those cozy runs and never bundles, as `cozy doctor` shows them.
//! Fetching the ones cozy installs itself on first need is `calcifer::tools`, which features call
//! through `ffmpeg` and `uv` here.
use std::{
    fs::File,
    path::{Path, PathBuf},
};

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use platform::tools::{self, Class, Found, Side, TOOLS};
use protocol::error::{Error, Kind};
use serde::Serialize;

use crate::{Context, output};

/// One tool as `cozy doctor` shows it.
#[derive(Serialize)]
pub(crate) struct Row {
    pub name: &'static str,
    pub side: &'static str,
    pub class: &'static str,
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
    pub cost: &'static str,
    /// What installs it here: a cozy verb, the OS's own command, or what to install.
    pub install: String,
}

/// Every tool this computer uses, probed now.
pub(crate) fn rows(ctx: &Context, side: Side) -> Vec<Row> {
    let path = path();
    TOOLS
        .iter()
        .filter(|tool| tool.side == side && tools::used_here(tool))
        .map(|tool| {
            let found: Option<Found> = (tool.probe)(&ctx.home.tools(), &path);
            Row {
                name: tool.name,
                side: match tool.side {
                    Side::Computer => "computer",
                    Side::Machine => "machine",
                },
                class: match tool.class {
                    Class::Essential => "essential",
                    Class::Optional => "optional",
                },
                found: found.is_some(),
                version: found
                    .as_ref()
                    .map(|f| f.version.clone())
                    .unwrap_or_default(),
                path: found.map(|f| f.path),
                cost: tool.cost,
                install: install_hint(tool, &path),
            }
        })
        .collect()
}

use calcifer::tools::install_hint;

/// A row as a line of `cozy doctor`.
pub(crate) fn cells(row: &Row) -> [String; 3] {
    match row.found {
        true => [
            row.name.into(),
            match row.version.is_empty() {
                true => "found".into(),
                false => row.version.clone(),
            },
            row.path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        ],
        false => [
            row.name.into(),
            format!("missing: {}", row.cost),
            row.install.clone(),
        ],
    }
}

fn path() -> Vec<PathBuf> {
    calcifer::tools::path()
}

pub use calcifer::tools::Ffmpeg;

/// ffmpeg for a feature that needs it (`calcifer::tools::ffmpeg`), what its fetch says printed.
pub async fn ffmpeg(ctx: &Context) -> Result<Ffmpeg, Error> {
    calcifer::tools::ffmpeg(&ctx.home, &|line| output::say(ctx, line)).await
}

/// uv for authoring (`calcifer::tools::uv`), what its fetch says printed.
pub async fn uv(ctx: &Context) -> Result<PathBuf, Error> {
    calcifer::tools::uv(&ctx.home, &|line| output::say(ctx, line)).await
}

/// The refusal a feature gives when an essential tool is missing: what its absence costs, what
/// installs it, and `cozy doctor`.
pub fn missing(ctx: &Context, name: &str) -> Error {
    let tool = tools::tool(name);
    Error::new(
        Kind::Unavailable,
        format!("{name}_unavailable"),
        format!("{name} not found: {}", tool.cost),
    )
    .next(install_hint(tool, &path()))
    .next(doctor(ctx))
}

fn doctor(ctx: &Context) -> &'static str {
    match ctx.json {
        true => "cozy doctor --json",
        false => "cozy doctor",
    }
}

/// Downloads `url` into `part` (`calcifer::tools::download`), showing the bytes moved on a terminal.
pub(crate) async fn download(ctx: &Context, url: &str, part: &Path) -> Result<(File, String), String> {
    let bar = match !ctx.json && ctx.term.stderr {
        true => ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr()),
        false => ProgressBar::hidden(),
    };
    bar.set_style(ProgressStyle::with_template("{spinner} {bytes} at {bytes_per_sec}").expect("a template"));
    let mut first = true;
    let shown = bar.clone();
    let answer = calcifer::tools::download(url, part, &mut |at| {
        if std::mem::take(&mut first) && at > 0 {
            shown.println(format!("resuming at {}", output::bytes(at)));
        }
        shown.set_position(at);
    })
    .await;
    bar.finish_and_clear();
    answer
}
