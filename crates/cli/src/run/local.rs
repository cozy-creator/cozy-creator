//! Local code: `cozy run ./dir[/fn]` and `cozy run ./script.py[/fn]`, a package nobody published. The
//! machine installs it and answers its interface; the CLI binds the terms against that and calls
//! it, just as it calls a release. None of its code runs here. The interface is the source's (a
//! static description) as its machine's Runtime reads it, so it is kept by the source manifest's
//! digest and that Runtime: a source sent before to a machine on the same Runtime is called at
//! once, and its machine makes its environment ready inside the call. `--describe` reads it from
//! the source here, as a publish does, and installs nothing.
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use platform::{durable, paths::Home};

use protocol::{
    calcifer::{Kind as RunKind, On, Run, StagedInput, Status, SubmitRun},
    error::{Error, Kind},
    machine as v1,
    project::LocalCode,
};

use super::{
    RunArgs, describe,
    follow::{self, settle, submit},
    interface::Interface,
    spec,
};
use crate::{Context, package::local::Local};

/// What the machine installed: the package's interface and where the call must go.
struct Installed {
    interface: Interface,
    /// As the machine answered it.
    document: serde_json::Value,
    /// The machine's name for the installation; none for a kept interface.
    installation: Option<String>,
    /// Where it was installed; none for a kept interface.
    machine: Option<String>,
}

/// Local code made callable: installed on its machine, or its interface kept from an earlier
/// install of this exact source.
pub(crate) struct Prepared {
    local: Local,
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

    /// This code as a project's render block names it (`with`), calling `function`.
    pub(crate) fn code(&self, with: &str, function: &str) -> Result<LocalCode, Error> {
        let function = super::callable(self.package(), self.interface(), function)?.name.clone();
        let manifest = |input: &&StagedInput| input.digest.to_string() == self.local.source.manifest;
        Ok(LocalCode {
            with: with.into(),
            on: self.on.clone(),
            function,
            package: self.local.package.clone(),
            source: self.local.source.clone(),
            manifest: self.local.manifest.clone(),
            files: self.local.inputs.iter().filter(|input| !manifest(input)).cloned().collect(),
            interface: self.installed.document.clone(),
        })
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
    let local = crate::package::local::source(ctx, path, &on).await?;
    let runtime = runtime(ctx, &on).await;
    let kept = runtime.as_deref().filter(|_| !warm).and_then(|runtime| kept(&ctx.home, &local, runtime));
    let installed = match kept {
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
async fn install(ctx: &Context, hub: &str, on: On, local: &Local, runtime: Option<&str>) -> Result<Installed, Error> {
    let submitted = submit(ctx, install_run(&ctx.home, hub, on, local), None).await?;
    let run = settle(ctx, submitted.run).await?;
    let result = run.result.clone().unwrap_or_default();
    let installed = installed(run)?;
    if let Some(runtime) = runtime {
        keep(&ctx.home, local, runtime, &result["interface"]);
    }
    Ok(installed)
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

/// The run that installs local code `on` a machine: a WARM run naming no function.
pub(crate) fn install_run(home: &Home, hub: &str, on: On, local: &Local) -> SubmitRun {
    let target = match synced(home, local) {
        true => format!("sync {} (source changed)", local.package),
        false => format!("install {}", local.package),
    };
    let spec = v1::RunSpec {
        kind: v1::RunKind::Warm,
        source: Some(v1::Source::Local(local.source.clone())),
        ..Default::default()
    };
    SubmitRun {
        agreed: None,
        on,
        hub: hub.to_owned(),
        kind: RunKind::Warm,
        target,
        spec,
        inputs: local.inputs.clone(),
        out: None,
        warnings: vec![],
    }
}

/// Where local code's interfaces are kept, beside release cards: one file per source manifest
/// and Runtime that described it, named for its environment's inputs first, so a changed source
/// with the same environment is seen.
fn dir(home: &Home, local: &Local) -> PathBuf {
    let mut dir = home.root().join("packages");
    dir.extend(local.package.split('/'));
    dir
}

fn file(home: &Home, local: &Local, runtime: &str) -> PathBuf {
    let manifest = local.source.manifest.trim_start_matches("sha256:");
    let runtime = &hex(&protocol::ids::Digest::of(runtime.as_bytes()))[..16];
    dir(home, local).join(format!(
        "{}-{manifest}-{runtime}.json",
        &hex(&local.environment)[..16]
    ))
}

fn hex(digest: &protocol::ids::Digest) -> String {
    digest.to_string().trim_start_matches("sha256:").to_owned()
}

/// The interface kept for this exact source, as `runtime` described it.
fn kept(home: &Home, local: &Local, runtime: &str) -> Option<Installed> {
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(file(home, local, runtime)).ok()?).ok()?;
    Some(Installed {
        interface: Interface::parse(&document).ok()?,
        document,
        installation: None,
        machine: None,
    })
}

/// Whether some kept source of this package has the same environment inputs.
fn synced(home: &Home, local: &Local) -> bool {
    let prefix = format!("{}-", &hex(&local.environment)[..16]);
    std::fs::read_dir(dir(home, local))
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with(&prefix))
}

/// Keeps `interface` for this source as `runtime` described it, and only the package's newest 16.
fn keep(home: &Home, local: &Local, runtime: &str, interface: &serde_json::Value) {
    const KEPT: usize = 16;
    let dir = dir(home, local);
    let kept = durable::private_dir(&dir).and_then(|()| {
        durable::replace(&file(home, local, runtime), interface.to_string().as_bytes(), 0o600)
    });
    // An interface that cannot be kept is asked again next time; the run goes on.
    drop(kept);
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, old) in files.into_iter().skip(KEPT) {
        let _ = std::fs::remove_file(old);
    }
}

fn installed(run: Run) -> Result<Installed, Error> {
    if run.status != Status::Completed {
        let (code, message) = run.reason.map_or_else(
            || {
                (
                    "local_install_unfinished".to_owned(),
                    format!("its install is {}", follow::status_word(run.status)),
                )
            },
            |r| (r.code, r.message),
        );
        return Err(Error::new(
            Kind::Unavailable,
            "local_install_failed",
            format!("the machine did not install this code: {message}"),
        )
        .cause(code)
        .next(format!("cozy run show {}", run.number)));
    }
    let mut result = run.result.unwrap_or_default();
    let interface = Interface::parse(&result["interface"]).map_err(|why| {
        Error::new(
            Kind::Unavailable,
            "local_interface_unreadable",
            format!("the machine's description of this code is unreadable: {why}"),
        )
    })?;
    let installation = result["installation"].as_str().map(str::to_owned);
    let machine = run.machine.ok_or_else(|| {
        Error::new(
            Kind::Internal,
            "local_install_unplaced",
            "the install ran on no machine",
        )
    })?;
    Ok(Installed {
        interface,
        document: result["interface"].take(),
        installation,
        machine: Some(machine),
    })
}
