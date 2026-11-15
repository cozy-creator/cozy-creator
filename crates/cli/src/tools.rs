//! The tools this computer has of those cozy runs and never bundles, as `cozy doctor` shows them.
//! Calcifer fetches the ones cozy installs itself, on first need.
use std::path::PathBuf;

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

