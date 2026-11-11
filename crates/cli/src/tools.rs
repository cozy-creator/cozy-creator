//! The tools this computer has of those cozy runs and never bundles, as `cozy doctor` shows them.
//! Calcifer fetches the ones cozy installs itself, on first need.
use std::{
    fs::File,
    path::{Path, PathBuf},
};

use platform::tools::{self, Class, Found, Side, TOOLS, install_hint};
use serde::Serialize;

use crate::Context;

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
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default()
}

/// Downloads `url` into `part` (`tensorfs::fetch::get_pinned_into`), showing the bytes moved on a terminal.
pub(crate) async fn download(
    ctx: &Context,
    url: &str,
    part: &Path,
) -> Result<(File, String), String> {
    use crate::progress::{Transfer, TransferCounts};
    let bar = Transfer::stderr(ctx.term.stderr, "Downloading Cozy");
    let mut initial = None;
    let mut position = 0;
    bar.update(TransferCounts {
        files_done: 0,
        files_total: Some(1),
        bytes_done: 0,
        bytes_total: None,
        bytes_moved: 0,
    });
    bar.phase("Downloading and checking archive");
    let answer = tensorfs::fetch::get_pinned_into(url, part, &mut |at| {
        let start = initial.get_or_insert(at);
        if at < *start {
            *start = 0;
        }
        position = at;
        bar.update(TransferCounts {
            files_done: 0,
            files_total: Some(1),
            bytes_done: at,
            bytes_total: None,
            bytes_moved: at.saturating_sub(*start),
        });
    })
    .await;
    if answer.is_ok() {
        bar.update(TransferCounts {
            files_done: 1,
            files_total: Some(1),
            bytes_done: position,
            bytes_total: Some(position),
            bytes_moved: position.saturating_sub(initial.unwrap_or(0)),
        });
    }
    bar.finish_and_clear();
    answer
}
