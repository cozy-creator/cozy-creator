//! `cozy package install|remove|ls --on`: runs on a machine through Calcifer. The machine resolves
//! releases from the Hub and owns its environments; the CLI reads only the release it names and
//! that release's card, which the next `cozy run` binds against.
use std::process::ExitCode;

use protocol::{
    calcifer::{Environment, Machine, MachineState, On, Run, Status, Submitted},
    error::{Error, Kind},
    hub::{AfterRun, PackageInstall, PackageRemoval, PackageRemovals},
};
use serde::{Deserialize, Serialize};

use crate::{
    Context,
    machine::{Followed, Notices, route, until_ready},
    output::{self, Table},
    run,
};

/// Waits for `run` to settle, quietly: the verb reports the outcome itself.
async fn settle(ctx: &Context, run: Run) -> Result<Run, Error> {
    let calcifer = ctx.calcifer().await?;
    let mut events = calcifer.events(run.id.as_str(), 0);
    while let Some(event) = events.next().await {
        if event?.is_terminal() {
            break;
        }
    }
    calcifer.run(run.id.as_str()).await
}

/// Why a settled run did not succeed, as `machine: reason (code)`.
fn failure(run: &Run) -> String {
    let machine = run.machine.as_deref().unwrap_or("the machine");
    match &run.reason {
        Some(reason) => format!("{machine}: {} ({})", reason.message, reason.code),
        None => format!("{machine}: the run ended {:?}", run.status).to_lowercase(),
    }
}

#[derive(Deserialize)]
struct Installed {
    #[serde(default)]
    package: String,
    #[serde(default)]
    release: String,
    #[serde(default)]
    interface: serde_json::Value,
    #[serde(default)]
    after: AfterRun,
}

/// Installs `package` (`org/name`, else a package directory's or script's absolute path) on a
/// machine through Calcifer: its environment, no weights. Calcifer names the newest release when
/// none is given, and retires the older ones the machine holds once it is in.
pub async fn install(ctx: &Context, package: String, release: Option<String>, on: On) -> Result<ExitCode, Error> {
    let mut ask = PackageInstall { hub: Some(ctx.hub().to_string()), package, release, on: on.clone(), agreed: None };
    let submitted: Submitted = crate::ask::start(
        ctx,
        "/v1/package-installs",
        "/v1/package-install-quotes",
        &on,
        &mut ask,
        |a, agreed| a.agreed = Some(agreed),
    )
    .await?;
    let settled = run::follow(ctx, submitted.run).await?;
    if ctx.json || settled.status != Status::Completed {
        return Ok(run::exit_code(&settled));
    }
    let installed: Option<Installed> = settled.result.clone().and_then(|r| serde_json::from_value(r).ok());
    let machine = settled.machine.clone().unwrap_or_default();
    let mut next = vec![format!("cozy package ls --on {machine}")];
    if let Some(installed) = installed {
        let package = &installed.package;
        output::say(ctx, &format!("{package}@{} is installed on {machine}", installed.release));
        for retired in &installed.after.retired {
            match &retired.reason {
                None => output::say(ctx, &format!("{package}@{} left {machine}", retired.release)),
                Some(why) => output::say(ctx, &format!("{package}@{} stays on {machine}: {why}", retired.release)),
            }
        }
        let first = protocol::interface::Interface::parse(&installed.interface)
            .ok()
            .and_then(|interface| interface.public().first().map(|name| name.to_string()));
        if let Some(callable) = first {
            next.insert(0, format!("cozy run {package}/{callable} --describe"));
        }
    }
    output::next(ctx, &next.iter().map(String::as_str).collect::<Vec<_>>());
    Ok(ExitCode::SUCCESS)
}

/// The machines this command reads: `on` alone, read from the machine now (so a run that just
/// settled shows), or every ready one as Calcifer last saw it. A booting `on` is waited for, as a
/// run sent to it waits.
async fn machines(ctx: &Context, on: Option<&On>) -> Result<Vec<Machine>, Error> {
    let calcifer = ctx.calcifer().await?;
    let Some(on) = on else {
        let all = calcifer.machines().await?.machines;
        return Ok(all
            .into_iter()
            .filter(|m| matches!(m.state, MachineState::Ready | MachineState::Stopped))
            .collect());
    };
    let mut machine: Machine = calcifer
        .get(&route(&named(on)?.to_ascii_lowercase())?)
        .await?;
    if machine.state == MachineState::Booting {
        let name = machine.name.clone();
        output::say(ctx, &format!("waiting for {name} to start"));
        machine = match until_ready(ctx, machine, Notices::default()).await? {
            Followed::Ready(machine) => machine,
            Followed::Unfollowed => {
                let message = format!("{name} has not started; nothing was sent to it");
                return Err(Error::new(Kind::Unavailable, "machine_booting", message));
            }
        };
    }
    let path = route(&machine.name.to_ascii_lowercase())?;
    let machine: Machine = calcifer.get_query(&path, &[("fresh", "true")]).await?;
    Ok(vec![machine])
}

/// The machine `--on` names, whose packages can be read.
pub(crate) fn named(on: &On) -> Result<&str, Error> {
    match on {
        On::Machine(name) => Ok(name),
        _ => Err(Error::new(
            Kind::Usage,
            "machine_name_required",
            format!("--on {on} names no machine whose packages can be read"),
        )),
    }
}

/// A machine's installations.
fn environments(machine: &Machine) -> Vec<&Environment> {
    machine.status.iter().flat_map(|s| &s.environments).collect()
}

/// Where an installation came from: `local` code, the Hub, or another Hub by its URL.
fn source(ctx: &Context, hub: &str) -> String {
    match (hub.is_empty(), ctx.other_hub(hub)) {
        (true, _) => "local".into(),
        (_, Some(other)) => other.into(),
        _ => "hub".into(),
    }
}

pub(crate) const HEADERS: [&str; 5] = ["MACHINE", "PACKAGE", "RELEASE", "LEVEL", "SOURCE"];

/// Each package `machines` hold.
pub(crate) fn rows<'a>(ctx: &Context, machines: impl IntoIterator<Item = &'a Machine>) -> Vec<Held<'a>> {
    let mut rows = vec![];
    for machine in machines {
        for env in environments(machine) {
            rows.push(Held {
                machine: &machine.name,
                package: &env.package,
                release: &env.release,
                level: &env.level,
                hub: &env.hub,
                source: source(ctx, &env.hub),
                warning: &env.warning,
            });
        }
    }
    rows
}

#[derive(Serialize)]
pub(crate) struct Held<'a> {
    machine: &'a str,
    package: &'a str,
    release: &'a str,
    level: &'a str,
    hub: &'a str,
    /// `local`, `hub`, or another Hub's URL.
    source: String,
    /// Why the environment does not run the machine's own Runtime; empty when it does.
    warning: &'a str,
}

impl Held<'_> {
    pub(crate) fn cells(&self) -> Vec<String> {
        let (machine, package, release) = (self.machine, self.package, self.release);
        vec![
            machine.into(),
            package.into(),
            release.into(),
            self.level.into(),
            self.source.clone(),
        ]
    }
}

pub async fn list(ctx: &Context, on: Option<On>) -> Result<ExitCode, Error> {
    if !ctx.json && ctx.term.stdout {
        let view = super::live::Packages::open(ctx, on).await?;
        crate::tui::show(ctx, crate::tui::Screen::Packages(view)).await?;
        return Ok(ExitCode::SUCCESS);
    }
    let machines = machines(ctx, on.as_ref()).await?;
    let rows = rows(ctx, &machines);
    if ctx.json {
        output::json(&serde_json::json!({ "packages": rows }));
        return Ok(ExitCode::SUCCESS);
    }
    if rows.is_empty() {
        println!("No packages installed.");
        output::next(
            ctx,
            &["cozy package ls", "cozy package install <[org/]name>"],
        );
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = Table::new(ctx, HEADERS);
    for row in &rows {
        table = table.row(row.cells());
    }
    for row in rows.iter().filter(|row| !row.warning.is_empty()) {
        table = table.note(format!("{}@{}: {}", row.package, row.release, row.warning));
    }
    table.print();
    Ok(ExitCode::SUCCESS)
}

#[derive(Deserialize, Default)]
struct Removal {
    #[serde(default)]
    removed: Vec<Removed>,
    #[serde(default)]
    reclaimed_bytes: u64,
    #[serde(default)]
    retained: Vec<String>,
}

#[derive(Deserialize, Serialize)]
struct Removed {
    package: String,
    release: String,
    #[serde(default)]
    hub: String,
}

/// Removes every release of each package from a machine: Calcifer sends one remove run per
/// package and Hub the machine holds it from; each is followed quietly and summed here.
pub async fn remove(ctx: &Context, names: &[String], on: On) -> Result<ExitCode, Error> {
    let ask = PackageRemoval {
        hub: Some(ctx.hub().to_string()),
        packages: names.to_vec(),
        machine: named(&on)?.to_ascii_lowercase(),
    };
    let calcifer = ctx.calcifer().await?;
    let removals: PackageRemovals = calcifer.post("/v1/package-removals", &ask, Some(&run::mint_key())).await?;
    let (mut removed, mut reclaimed, mut retained, mut failed) = (vec![], 0, false, vec![]);
    for ran in removals.runs {
        let ran = settle(ctx, ran).await?;
        if ran.status != Status::Completed {
            failed.push(format!("{}: {}", ran.target, failure(&ran)));
            continue;
        }
        let result: Removal = ran.result.and_then(|r| serde_json::from_value(r).ok()).unwrap_or_default();
        reclaimed += result.reclaimed_bytes;
        retained |= !result.retained.is_empty();
        removed.extend(result.removed);
    }
    let machine = removals.machine;
    if ctx.json {
        output::json(&serde_json::json!({
            "machine": machine, "removed": removed, "reclaimed_bytes": reclaimed, "failed": failed,
        }));
    } else {
        let mut table = Table::new(ctx, ["PACKAGE", "RELEASE", "SOURCE"]);
        for row in &removed {
            table = table.row([row.package.clone(), row.release.clone(), source(ctx, &row.hub)]);
        }
        table = table.note(format!("{} reclaimed on {machine}", output::bytes(reclaimed)));
        if retained {
            table = table.note("Files a running executor still uses stay until it ends.");
        }
        for failure in &failed {
            table = table.note(format!("not removed: {failure}"));
        }
        table.print();
    }
    Ok(if failed.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
