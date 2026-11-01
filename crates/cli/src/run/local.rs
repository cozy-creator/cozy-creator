//! Local code: `cozy run ./dir[/fn]` and `cozy run ./script.py[/fn]`, a package nobody published. The
//! machine installs it and answers its interface; the CLI binds the terms against that and calls
//! it, just as it calls a release. None of its code runs here. The interface is the source's (a
//! static description) as its machine's Runtime reads it, so it is kept by the source manifest's
//! digest and that Runtime: a source sent before to a machine on the same Runtime is called at
//! once, and its machine makes its environment ready inside the call. `--describe` reads it from
//! the source here, as a publish does, and installs nothing.
use std::{
    path::Path,
    process::ExitCode,
};

use calcifer::local_code::{self, Source};
use protocol::{
    calcifer::{On, Run},
    error::{Error, Kind},
};

use super::{
    RunArgs, describe,
    follow::{settle, submit},
    interface::Interface,
    spec,
};
use crate::Context;

/// What the machine installed: the package's interface and where the call must go.
struct Installed {
    interface: Interface,
    /// The machine's name for the installation; none for a kept interface.
    installation: Option<String>,
    /// Where it was installed; none for a kept interface.
    machine: Option<String>,
}

/// Local code made callable: installed on its machine, or its interface kept from an earlier
/// install of this exact source.
pub(crate) struct Prepared {
    local: Source,
    installed: Installed,
    on: On,
}

impl Prepared {
    /// `local/<name>`.
    pub(crate) fn package(&self) -> &str {
        &self.local.package
    }

    pub(crate) fn interface(&self) -> &Interface {
        &self.installed.interface
    }

    /// The source manifest's digest: this exact code.
    pub(crate) fn source(&self) -> &str {
        &self.local.source.manifest
    }

    /// Calls `function`. The call goes where the code was installed, with its files once more:
    /// a machine that dropped them meanwhile is sent only what it lacks, and installs them in
    /// the call.
    pub(super) async fn call(&self, ctx: &Context, function: &str, args: RunArgs) -> Result<Run, Error> {
        let code = super::Code {
            package: &self.local.package,
            interface: &self.installed.interface,
            release: None,
            source: spec::Source::Local {
                source: &self.local.source,
                installation: self.installed.installation.as_deref(),
            },
            files: &self.local.inputs,
        };
        let on = self.installed.machine.clone().map_or_else(|| self.on.clone(), On::Machine);
        super::call_code(ctx, code, function, on, args).await
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
    let installed = match kept.and_then(|document| Some(Installed { interface: Interface::parse(&document).ok()?, installation: None, machine: None })) {
        Some(installed) => installed,
        None => install(ctx, hub, on.clone(), &local, runtime.as_deref()).await?,
    };
    Ok(Prepared { local, installed, on })
}

pub(super) async fn run(
    ctx: &Context,
    path: &Path,
    function: Option<String>,
    args: RunArgs,
) -> Result<ExitCode, Error> {
    if args.describe {
        return described(ctx, path, function, &args).await;
    }
    let prepared = prepare(ctx, path, args.placement.on()?, args.warm.is_some()).await?;
    let Some(function) = chosen(ctx, prepared.package(), prepared.interface(), function, &args)? else {
        return Ok(ExitCode::SUCCESS);
    };
    let run = prepared.call(ctx, &function, args).await?;
    Ok(super::exit_code(&run))
}

/// `--describe`: the function's contract, read from the source as a publish reads it. Nothing
/// installs, and no machine is asked.
async fn described(ctx: &Context, path: &Path, function: Option<String>, args: &RunArgs) -> Result<ExitCode, Error> {
    let (package, described) = crate::package::local::describe(ctx, path).await?;
    let interface = &described.interface;
    let Some(function) = chosen(ctx, &package, interface, function, args)? else {
        return Ok(ExitCode::SUCCESS);
    };
    let callable = super::callable(&package, interface, &function)?;
    if ctx.json {
        crate::output::json(&super::catalog::request(&described.document, &callable.name));
    } else {
        let target = format!("{package}/{}", callable.name);
        anstream::print!("{}", describe::contract(&target, interface, callable));
    }
    Ok(ExitCode::SUCCESS)
}

/// The function a run of local code names, else its one callable: local code is its author's to
/// run, a recipe too. None when it names none of several, which are listed.
fn chosen(ctx: &Context, package: &str, interface: &Interface, function: Option<String>, args: &RunArgs)
    -> Result<Option<String>, Error> {
    let callables: Vec<&str> = (interface.callables.iter()).filter(|c| !c.internal).map(|c| c.name.as_str()).collect();
    match (function, callables.as_slice()) {
        (Some(function), _) => Ok(Some(function)),
        (None, [only]) => Ok(Some((*only).to_owned())),
        (None, _) => {
            super::unnamed(package, None, interface, args)?;
            super::functions_of(ctx, package, ("local", ""), None, interface);
            Ok(None)
        }
    }
}

/// Installs the code on its machine: a WARM run naming no function, whose result is the
/// installation and its interface, then kept. It is named for what the machine does: an install
/// when the environment's inputs (metadata, lock, path dependencies) are new here, else a sync of
/// the changed source.
async fn install(ctx: &Context, hub: &protocol::config::Origin, on: On, local: &Source, runtime: Option<&str>) -> Result<Installed, Error> {
    let submitted = submit(ctx, local_code::install_run(&ctx.home, hub.as_str(), on, local), None).await?;
    let run = settle(ctx, submitted.run).await?;
    let installed = local_code::installed(run)?;
    if let Some(runtime) = runtime {
        local_code::keep(&ctx.home, local, runtime, &installed.interface);
    }
    let interface = Interface::parse(&installed.interface).map_err(|why| {
        Error::new(Kind::Unavailable, "local_interface_unreadable", format!("the machine's description of this code is unreadable: {why}"))
    })?;
    Ok(Installed { interface, installation: installed.installation, machine: installed.machine })
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
