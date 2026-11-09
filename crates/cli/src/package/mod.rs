//! `cozy package`: find packages on a Hub, keep them on machines, and publish your own.
//!
//! Install, update and remove are runs on a machine through Calcifer; the machine resolves and
//! installs releases. `lock` and `publish` send the package by content: Calcifer locks and builds
//! its copy with uv, never running package code, and a lock comes back for the CLI to write.
use std::{path::PathBuf, process::ExitCode};

use clap::Subcommand;
use api::error::{Error, Kind};

use crate::{Context, output, run::Placement};
use api::target::Published;

mod author;
mod catalog;
pub(crate) mod live;
mod machine;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// The Hub's packages; words search. `--on` lists what machines have installed instead, live on
    /// a terminal.
    #[command(alias = "list")]
    Ls {
        /// Words to match; none lists every package.
        query: Vec<String>,
        /// At most this many results (the Hub lists up to 200 at once).
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// What machines hold instead of the Hub's packages: `local`, a machine's name, or `all`.
        #[arg(long, value_name = "MACHINE")]
        on: Option<String>,
    },
    /// A published package's releases, newest first; one release, with what its publish warned
    /// of (its owner sees the warnings).
    Info {
        /// [org/]name[@release]
        package: String,
    },
    /// Install a published package ([org/]name[@release]), a directory or a script on a machine:
    /// its environment, no weights. Without a release, the newest; over an installed package, it moves
    /// to that release and its older ones there go: a run with no version stays on what is installed
    /// until this. Follows the install; Ctrl-C stops following, never it.
    Install {
        /// [org/]name[@release], a directory such as `.` or `./project`, or a `script.py`.
        #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::local))]
        package: String,
        #[command(flatten)]
        placement: Placement,
    },
    /// Remove packages from a machine.
    Remove {
        /// [org/]name or local/name.
        #[arg(required = true)]
        packages: Vec<String>,
        #[command(flatten)]
        placement: Placement,
    },
    /// Lock this package's dependencies for the Hub and account it publishes to, writing its uv.lock.
    Lock {
        /// The package's folder, if not the one you are in.
        #[arg(short = 'p', value_name = "PATH")]
        path: Option<PathBuf>,
        /// Let this dependency move to its newest compatible release; repeat for each.
        #[arg(long = "upgrade-package", value_name = "PACKAGE")]
        upgrade: Vec<String>,
        /// For the root namespace (first-party, Hub admins), not your account.
        #[arg(long)]
        root: bool,
    },
    /// Publish the package in this folder as <account>/<name>@<version>, or <name>@<version> with --root.
    Publish {
        /// At the root namespace: first-party, Hub admins only.
        #[arg(long)]
        root: bool,
        /// Published yanked (a gated candidate): only an exact pin installs it, and it is never the
        /// newest release, until `cozy package yank --undo`.
        #[arg(long)]
        yanked: bool,
    },
    /// Delete a package from the Hub with every release, for good: its name and versions are free
    /// again. Machines that installed it keep their copies.
    Delete {
        /// [org/]name
        package: String,
    },
    /// Yank a release: machines that hold it keep it, only an exact pin installs it.
    Yank {
        /// [org/]name@release
        release: String,
        /// Un-yank it instead: the unversioned default again (a gated candidate's promotion).
        #[arg(long)]
        undo: bool,
    },
}

pub(crate) async fn exec(ctx: &Context, args: Args) -> Result<ExitCode, Error> {
    match args.command {
        Command::Ls { query, limit, on } => match on {
            Some(on) => machine::list(ctx, (on != "all").then(|| on.into()), &query).await,
            None => catalog::search(ctx, &query.join(" "), output::at_most("--limit", limit, 200)).await,
        },
        Command::Info { package } => catalog::info(ctx, &PackageRef::parse(&package)?).await,
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
        Command::Remove {
            packages,
            placement,
        } => machine::remove(ctx, &names(&packages)?, placement.on()?).await,
        Command::Lock { path, upgrade, root } => author::lock(ctx, path, &upgrade, root).await,
        Command::Publish { root, yanked } => author::publish(ctx, root, yanked).await,
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
        let invalid = || Error::new(Kind::Usage, "package_ref_invalid", format!("{text:?} is not [org/]name[@release]")).next("cozy package ls");
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
        Some((_, Some(function))) => Err(Error::new(Kind::Usage, "package_ref_invalid", format!("{text} names a function, {function}: a package is its folder or script"))),
        None => Ok(None),
    }
}

/// Names to remove: `[org/]name` (or `local/name`), with no release.
fn names(texts: &[String]) -> Result<Vec<String>, Error> {
    texts
        .iter()
        .map(|text| match PackageRef::parse(text)? {
            parsed => {
                if parsed.release.is_some() {
                    output::warn(&format!("{text}: a machine holds one release of a package: removing {}", parsed.package()));
                }
                Ok(parsed.package())
            }
        })
        .collect()
}
