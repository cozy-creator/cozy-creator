//! Local code, a package nobody published (`./dir`, `./script.py`): `cozy run` sends it to Calcifer
//! as its target (`POST /v1/runs`), which makes it ready on its machine and calls it. What stays
//! here until `cozy model quantize` is Calcifer's (cl-716 D): its `--package ./dir` install, which
//! reads the package's recipes.
use std::path::{Path, PathBuf};

use calcifer::local_code::{self, Source};
use protocol::{
    calcifer::On,
    error::{Error, Kind},
};

use protocol::interface::Interface;

use super::follow::{settle, submit};
use crate::Context;

/// Local code made callable: installed on its machine, or its interface kept from an earlier
/// install of this exact source.
pub(crate) struct Prepared {
    path: PathBuf,
    local: Source,
    interface: Interface,
}

impl Prepared {
    /// `local/<name>`.
    pub(crate) fn package(&self) -> &str {
        &self.local.package
    }

    pub(crate) fn interface(&self) -> &Interface {
        &self.interface
    }

    /// The source manifest's digest: this exact code.
    pub(crate) fn source(&self) -> &str {
        &self.local.source.manifest
    }

    /// Its package directory or script.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// The code at `path` made callable `on` its machine. A warm set names the machine's
/// installation, which only an install answers.
pub(crate) async fn prepare(ctx: &Context, path: &Path, on: On, warm: bool) -> Result<Prepared, Error> {
    let hub = ctx.hub();
    let client = ctx.hub_signed_in(hub).ok();
    let local = local_code::source(&ctx.home, path, &on, client.as_ref().map(|client| (client, hub))).await?;
    let runtime = runtime(ctx, &on).await;
    let kept = runtime.as_deref().filter(|_| !warm).and_then(|runtime| local_code::kept(&ctx.home, &local, runtime));
    let interface = match kept.and_then(|document| Interface::parse(&document).ok()) {
        Some(interface) => interface,
        None => install(ctx, hub, on.clone(), &local, runtime.as_deref()).await?,
    };
    Ok(Prepared { path: path.to_owned(), local, interface })
}

/// Installs the code on its machine: a WARM run naming no function, whose result is the
/// installation and its interface, then kept. It is named for what the machine does: an install
/// when the environment's inputs (metadata, lock, path dependencies) are new here, else a sync of
/// the changed source.
async fn install(ctx: &Context, hub: &protocol::config::Origin, on: On, local: &Source, runtime: Option<&str>) -> Result<Interface, Error> {
    let submitted = submit(ctx, local_code::install_run(&ctx.home, hub.as_str(), on, local), None).await?;
    let run = settle(ctx, submitted.run).await?;
    let installed = local_code::installed(run)?;
    if let Some(runtime) = runtime {
        local_code::keep(&ctx.home, local, runtime, &installed.interface);
    }
    Interface::parse(&installed.interface).map_err(|why| {
        Error::new(Kind::Unavailable, "local_interface_unreadable", format!("the machine's description of this code is unreadable: {why}"))
    })
}

/// The Runtime the machine `on` names describes code with, as it last said; none for a machine
/// not chosen yet, or one that has not answered.
async fn runtime(ctx: &Context, on: &On) -> Option<String> {
    let On::Machine(name) = on else {
        return None;
    };
    let machine = crate::machine::machine(ctx, name).await.ok()?;
    machine.status.map(|status| status.runtime)
}
