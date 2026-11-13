//! `cozy package`: find packages on a Hub, keep them on machines, and publish your own.
//!
//! Install, update and uninstall are runs on a machine through Calcifer; the machine resolves and
//! installs releases. Publishing sends the package by content: Calcifer locks and builds
//! its copy with uv, never running package code, and a lock comes back for the CLI to write.
use std::{path::PathBuf, process::ExitCode};

use clap::Subcommand;
use api::error::{Error, Kind};

use crate::{Context, output, run::Placement};
use api::target::Published;

pub(crate) mod author;
mod catalog;
pub(crate) mod live;
pub(crate) mod browse;
mod machine;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Search the Hub's packages, or list installed packages with --on.
    #[command(alias = "list")]
    Ls {
        /// Search words.
        query: Vec<String>,
        /// Maximum packages to show.
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// Skip this many matching catalog packages.
        #[arg(long, default_value_t = 0)]
        offset: u32,
        /// List installed packages on local, a named machine, or all machines.
        #[arg(long, value_name = "MACHINE")]
        on: Option<String>,
    },
    /// Install a package, directory or script on a machine.
    Install {
        /// Package reference, ./directory or script.py.
        #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::local))]
        package: String,
        #[command(flatten)]
        placement: Placement,
    },
    /// Uninstall packages from a machine.
    Uninstall {
        /// [org/]name or local/name.
        #[arg(required = true)]
        packages: Vec<String>,
        #[command(flatten)]
        placement: Placement,
    },
    /// Publish the package in the current directory.
    #[command(hide = true)]
    Publish {
        /// Use the first-party root namespace (Hub admins only).
        #[arg(long)]
        root: bool,
    },
    /// Permanently delete your package and all its releases from the Hub.
    Delete {
        /// [org/]name.
        package: String,
    },
    /// Yank a release so new unpinned installs skip it.
    Yank {
        /// [org/]name@release
        release: String,
        /// Restore the yanked release.
        #[arg(long)]
        undo: bool,
    },
}

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    match args.command {
        Command::Ls { query, limit, offset, on } => match on {
            Some(on) => machine::list(ctx, (on != "all").then(|| on.into()), &query).await,
            None => catalog::search(ctx, &query.join(" "), output::at_most("--limit", limit, 200), offset).await,
        },
        Command::Install { package, placement } => match local(ctx, &package)? {
            Some(path) => {
                let on = placement.on()?;
                let code = crate::send::Sender::new(ctx).await?.code(&path).await?;
                machine::install(ctx, String::new(), Some(code), None, on).await
            }
            None => {
                let named = PackageRef::parse(&package)?;
                machine::install(ctx, named.package(), None, named.release, placement.on()?).await
            }
        },
        Command::Uninstall {
            packages,
            placement,
        } => machine::uninstall(ctx, &names(&packages)?, placement.on()?).await,
        Command::Publish { root } => author::publish(ctx, root).await,
        Command::Delete { package } => author::delete(ctx, &PackageRef::parse(&package)?).await,
        Command::Yank { release, undo } => {
            author::yank(ctx, &PackageRef::parse(&release)?, undo).await
        }
    }
}

/// `[org/]name[@release]`, lowercased: names are one identity in any case. No org is the root's
/// (first-party), org [`ROOT`] on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackageRef {
    pub org: String,
    pub name: String,
    pub release: Option<String>,
}

impl PackageRef {
    /// A package target without a function (`run::target`'s grammar): `x/y` is account x's
    /// package y.
    pub(crate) fn parse(text: &str) -> Result<Self, Error> {
        let invalid = || Error::new(Kind::Usage, "package_ref_invalid", format!("Invalid package reference: {text:?}"))
            .remedy("Use [org/]name[@release]").next("cozy package ls");
        let (package, release) = match Published::parse(text.trim()).map_err(|_| invalid())? {
            Published { package, function: None, release, .. } => (package, release),
            Published { owned: Some(owned), release, .. } => (owned, release),
            _ => return Err(invalid()),
        };
        let (org, name) = package.split_once('/').ok_or_else(invalid)?;
        Ok(Self { org: org.into(), name: name.into(), release })
    }

    pub(crate) fn package(&self) -> String {
        format!("{}/{}", self.org, self.name)
    }
}

impl std::fmt::Display for PackageRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.release {
            Some(release) => write!(f, "{}/{}@{release}", self.org, self.name),
            None => write!(f, "{}/{}", self.org, self.name),
        }
    }
}

/// The package local code `text` names here (`run::local`), which goes by content.
pub(crate) fn local(ctx: &Context, text: &str) -> Result<Option<PathBuf>, Error> {
    let cwd = std::env::current_dir().map_err(|e| Error::new(Kind::Internal, "cwd", format!("the working directory: {e}")))?;
    match crate::run::local(ctx, text, &cwd)? {
        Some((package, None)) => Ok(Some(package)),
        Some((_, Some(function))) => Err(Error::new(Kind::Usage, "package_ref_invalid", format!("{text} selects function {function}; use the package directory or script"))
            .next("cozy package --help")),
        None => Ok(None),
    }
}

/// Names to uninstall: `[org/]name` (or `local/name`), with no release.
fn names(texts: &[String]) -> Result<Vec<String>, Error> {
    texts
        .iter()
        .map(|text| match PackageRef::parse(text)? {
            parsed => {
                if parsed.release.is_some() {
                    output::warn(&format!("{text}: a machine holds one release of a package: uninstalling {}", parsed.package()));
                }
                Ok(parsed.package())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[derive(Debug, Parser)]
    struct Cli {
        #[command(flatten)]
        args: Args,
    }

    #[test]
    fn uninstall_is_the_only_public_spelling() {
        let parsed = Cli::try_parse_from(["package", "uninstall", "local/example", "--on", "local"]).unwrap();
        let Command::Uninstall { packages, .. } = parsed.args.command else { panic!("expected uninstall") };
        assert_eq!(packages, ["local/example"]);
        let command = Cli::command();
        assert!(command.get_subcommands().any(|command| command.get_name() == "uninstall"));
        assert!(!command.get_subcommands().any(|command| command.get_name() == "remove"));
        assert_eq!(Cli::try_parse_from(["package", "remove", "local/example"]).unwrap_err().kind(), clap::error::ErrorKind::InvalidSubcommand);
        assert!(Cli::try_parse_from(["package", "delete", "example"]).is_ok());
        assert!(Cli::try_parse_from(["package", "lock", "--help"]).is_err());
        assert!(Cli::try_parse_from(["package", "info", "example"]).is_err());
    }
}
