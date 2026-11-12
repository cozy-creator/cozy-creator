//! `cozy machine ls` and `show`: Calcifer's picture of every machine, which never waits on a Hub.
use std::process::ExitCode;

use api::{
    calcifer::{LiveRentals, LiveRun, Machine, MachineKind, MachineList, MachineState, MachineStatus, Rental},
    error::Error,
};

use super::{Notices, clock, machine, now_ms, usd};
use crate::{Context, output, run, tui};

const IDLE_RULE: &str = "Rentals end themselves after 15 minutes idle (no queued or running work).";

pub(crate) const HEADERS: [&str; 11] = [
    "MACHINE", "KIND", "GPUS", "FAMILY", "STATE", "$/HOUR", "SPENT", "UPTIME", "RUNNING", "QUEUED",
    "ENDS",
];

/// What `--json` keeps of a machine in a list (`--full`: all of it): the table's facts, by the record's names.
pub(crate) const FIELDS: &[&str] = &[
    "rentals", "unanswered", "signed_out", "machines.name", "machines.placeholder", "machines.kind", "machines.state", "machines.hub", "machines.rental_id",
    "machines.ordered", "machines.unverified", "machines.offline", "machines.unreachable", "machines.boot", "machines.reason", "machines.end_failed",
    "machines.running", "machines.queued", "machines.rental.sku", "machines.rental.accelerator", "machines.rental.gpus",
    "machines.rental.usd_micros_per_hour", "machines.rental.spend_usd_micros", "machines.rental.spend_basis",
    "machines.rental.rented_at_ms", "machines.status.version", "machines.status.phase", "machines.status.gpus.index",
    "machines.status.gpus.name", "machines.status.gpus.memory_bytes", "machines.status.idle_deadline_ms",
];
/// The columns of `HEADERS` a view shows without `--full`.
const SHOWN: [usize; 8] = [0, 1, 2, 4, 5, 6, 8, 10];

pub(crate) async fn list(ctx: &Context) -> Result<ExitCode, Error> {
    if !ctx.json && ctx.term.stdout {
        let view = super::live::Machines::open(ctx).await?;
        crate::tui::show(ctx, crate::tui::Screen::Machines(view)).await?;
        return Ok(ExitCode::SUCCESS);
    }
    let list: MachineList = ctx
        .calcifer()
        .await?
        .get("/v1/machines?refresh=true")
        .await?;
    let code = if list.unanswered.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    };
    if ctx.json {
        output::view(ctx, &list, FIELDS);
        return Ok(code);
    }
    print(ctx, &list);
    Ok(code)
}

/// The machines as a view prints them: what the rentals cost, a row each, then what to know of them.
pub(crate) fn print(ctx: &Context, list: &MachineList) {
    let live = || {
        list.machines
            .iter()
            .filter(|m| m.state != MachineState::Ended)
    };
    let rentals = &list.rentals;
    if rentals.count > 0 {
        println!(
            "Rentals running: {} · {}/hour now{}",
            rentals.count,
            usd(rentals.usd_micros_per_hour),
            accrued(rentals)
        );
    }
    for problem in &list.unanswered {
        println!(
            "hub {} did not answer: {}; its rentals may still be billing",
            problem.hub, problem.message
        );
    }
    for hub in &list.signed_out {
        println!("hub {hub}: not logged in; `{}`", ctx.config.login(hub));
    }
    let rows = rows(ctx, list);
    if rows.is_empty() {
        println!("machines: none");
        output::next(ctx, &["cozy machine rent"]);
    } else if ctx.full {
        rows.into_iter().fold(output::Table::new(ctx, HEADERS), output::Table::row).print();
    } else {
        let table = output::Table::new(ctx, SHOWN.map(|i| HEADERS[i]));
        rows.into_iter().map(|cells| SHOWN.map(|i| cells[i].clone())).fold(table, output::Table::row).print();
    }
    if let Some(note) = super::local_note() {
        println!("{note}");
    }
    for note in live().filter_map(offline) {
        println!("{note}");
    }
    for note in live().filter_map(end_failed) {
        println!("{note}");
    }
    for m in live() {
        for note in Notices::default().fresh(m) {
            println!("{}: {note}", m.called());
        }
        if unreachable_since(m).is_some() {
            println!("{}: `cozy machine end {}` ends it", m.called(), m.handle());
        }
    }
    if rentals.count > 0 && ctx.full {
        println!("{IDLE_RULE}");
    }
}

pub(crate) fn rows(ctx: &Context, list: &MachineList) -> Vec<Vec<String>> {
    let live = list
        .machines
        .iter()
        .filter(|m| m.state != MachineState::Ended);
    live.map(|m| {
        let rental = m.rental.as_ref();
        vec![
            m.called().to_owned(),
            kind_cell(ctx, m),
            gpus_cell(m),
            family_cell(m.status.as_ref()),
            state_cell(m),
            rental.map_or("—".into(), |r| usd(r.usd_micros_per_hour)),
            rental.map_or("—".into(), spent),
            uptime(m),
            m.running.to_string(),
            m.queued.to_string(),
            ends_cell(m),
        ]
    })
    .collect()
}

/// One machine: a live view on a terminal while it lives, else its record once (or JSON).
pub(crate) async fn show(ctx: &Context, name: &str) -> Result<ExitCode, Error> {
    let m = machine(ctx, name).await?;
    if !ctx.json && ctx.term.stdout && m.state != MachineState::Ended {
        let view = super::live::Shown::open(ctx, name).await?;
        tui::show(ctx, tui::Screen::Machine(Box::new(view))).await?;
        return Ok(ExitCode::SUCCESS);
    }
    if ctx.json {
        output::json(&m);
        return Ok(ExitCode::SUCCESS);
    }
    record(ctx, &m).print(ctx);
    Ok(ExitCode::SUCCESS)
}

/// `cozy machine show`'s record of `m`, printed once or drawn live.
pub(crate) fn record(ctx: &Context, m: &Machine) -> output::Record {
    let mut record = output::Record::new()
        .field("machine", m.called())
        .field("kind", kind(m))
        .field("state", state_cell(m));
    if let Some(alias) = m.alias.as_deref().filter(|a| *a != m.name) {
        record = record.field("alias", alias);
    }
    if let Some(hub) = ctx.other_hub(&m.hub) {
        record = record.field("hub", hub);
    }
    if let Some(id) = &m.rental_id {
        record = record.field("rental", id);
    }
    if let Some(address) = &m.address {
        record = record.field("address", address);
    }
    if let Some(fingerprint) = &m.fingerprint {
        record = record.field("fingerprint", fingerprint);
    }
    if let Some(reach) = &m.reach {
        record = record.field("add it at", reach);
    }
    if let Some(key) = &m.controller_key {
        record = record
            .field("controller key", key)
            .note(format!("another computer lets this one use its machine: cozy machine authorize {key}"));
    }
    if let Some(r) = &m.rental {
        record = record
            .field("sku", &r.sku)
            .field("gpus", gpus_of_rental(r))
            .field("$/hour", usd(r.usd_micros_per_hour))
            .field("spent", spent(r))
            .field("disk", format!("{} GB", r.disk_gb))
            .field(
                "rented",
                format!("{} ago", clock(now_ms() - r.rented_at_ms)),
            );
        if let Some(ended) = r.ended_at_ms {
            record = record.field("ended", format!("{} ago", clock(now_ms() - ended)));
        }
        if let Some(failure) = &r.failure {
            record = record.field("failure", failure);
        }
        if let Some(note) = &r.placement_note {
            record = record.field("placement", note);
        }
        if let Some(digest) = &r.base_worker_image_digest {
            record = record.field("image", digest);
        }
        if let Some(ssh) = &r.ssh {
            record = record.field("ssh", format!("{ssh} (cozy machine ssh {})", m.handle()));
        }
        if let Some(provider) = &r.provider {
            let at = r.datacenter.as_deref().map_or(String::new(), |d| format!(" {d}"));
            let drive = if r.nfs_drive { "mounted" } else { "none" };
            record = record.field("provider", format!("{provider}{at}")).field("nfs drive", drive);
        }
    }
    if let Some(since) = unreachable_since(m) {
        record = record.note(format!(
            "the Hub cannot reach it since {} ago, though its provider reports it running; \
             `cozy machine end {}` ends it",
            clock(now_ms() - since),
            m.handle()
        ));
    }
    if let Some(reason) = &m.reason {
        record = record.field("reason", reason);
    }
    if m.state != MachineState::Ended {
        record = record.field("running", m.running).field("queued", m.queued);
    }
    if let Some(why) = &m.unreachable {
        record = record.note(format!(
            "it did not answer just now ({why}); its facts are from its last report"
        ));
    }
    match &m.status {
        Some(status) => {
            for (key, value) in status_fields(status) {
                record = record.field(&key, value);
            }
            // The warm set: each member at its level, or what it holds instead and why.
            for item in &status.warm {
                let held = match item.holds.as_str() {
                    holds if holds.is_empty() || holds == item.level => item.level.clone(),
                    holds => format!("{}, holds {holds}: {}", item.level, item.held_back),
                };
                record = record.field(&format!("warm {}/{}", item.source, item.entrypoint), held);
            }
            record = record.field("ends", ends_cell(m));
        }
        None if m.state == MachineState::Ready => {
            record = record.note("the machine has not reported its status yet");
        }
        None => {}
    }
    if let Some(note) = offline(m) {
        record = record.note(note);
    }
    if let Some(note) = end_failed(m) {
        record = record.note(note);
    }
    record
}

/// A run the machine holds: its state there, target, stage and why it waits. One whose record
/// here ended is an executor left behind.
fn live_run(held: &LiveRun) -> (String, String) {
    let Some(run) = &held.run else {
        return (format!("run {}", held.id), format!("{}; no record of it here", held.state));
    };
    let progress = Some(run::list::progress(run)).filter(|p| !p.is_empty()).unwrap_or_else(|| held.waiting.clone());
    let cells = [held.state.clone(), run::list::target(run), progress];
    let mut cell = cells.into_iter().filter(|c| !c.is_empty()).collect::<Vec<_>>().join(" · ");
    if run::settled(run) {
        cell += &format!("; {} here, left behind on the machine", run::list::status(run));
    }
    (format!("run {}", run.number), cell)
}

/// What a machine reports of itself, one fact per line, GPUs by number.
pub(crate) fn status_fields(status: &MachineStatus) -> Vec<(String, String)> {
    let mut fields = vec![
        (
            "version".into(),
            match status.dev {
                true => format!("{} (dev)", status.version),
                false => status.version.clone(),
            },
        ),
        ("platform".into(), status.platform.clone()),
        ("phase".into(), status.phase.clone()),
    ];
    if !status.update.is_empty() {
        fields.push(("update".into(), status.update.clone()));
    }
    for stuck in &status.stuck {
        let wchan = Some(&stuck.wchan).filter(|w| !w.is_empty()).map(|w| format!(" in {w}")).unwrap_or_default();
        fields.push((
            "waiting on".into(),
            format!(
                "pid {} ({}), killed, in D state{wchan} since {} ago: it holds what it held until the kernel lets it exit",
                stuck.pid, stuck.command, clock(now_ms() - stuck.since_unix_ms)
            ),
        ));
    }
    let commit = [("commit", &status.commit)];
    for (key, value) in commit
        .into_iter()
        .chain([("runtime", &status.runtime), ("tensorfs", &status.tensorfs), ("torch", &status.torch)])
    {
        if !value.is_empty() {
            fields.push((key.into(), value.clone()));
        }
    }
    if status.gpus.is_empty() {
        fields.push(("gpus".into(), "none: runs use the CPU".into()));
    }
    for gpu in &status.gpus {
        let mut cell = format!("{} · {} GiB", gpu.name, gpu.memory_bytes >> 30);
        for part in [&gpu.family, &gpu.memory_class].into_iter().flatten() {
            cell += &format!(" · {part}");
        }
        if !gpu.driver.is_empty() {
            cell += &format!(" · driver {}", gpu.driver);
        }
        fields.push((format!("GPU {}", gpu.index), cell));
    }
    // Beside each card's own size, so a limited card is not mistaken for a smaller one.
    let limited: Vec<_> = status.gpus.iter().filter(|g| g.memory_limit_bytes > 0).collect();
    if let Some(first) = limited.first() {
        let cards: Vec<String> = limited.iter().map(|g| format!("GPU {}'s {} GiB", g.index, g.memory_bytes >> 30)).collect();
        let limit = crate::output::bytes(first.memory_limit_bytes);
        fields.push(("gpu memory limit".into(), format!("{limit} of {}", cards.join(", "))));
    }
    if status.disk_total_bytes > 0 {
        fields.push((
            "disk free".into(),
            format!(
                "{:.1} of {:.1} GiB",
                status.disk_free_bytes as f64 / (1u64 << 30) as f64,
                status.disk_total_bytes as f64 / (1u64 << 30) as f64
            ),
        ));
        // What fills it, so a full machine says what: the rest is other (the image).
        let kinds = [("models", status.disk_models_bytes), ("package environments", status.disk_environments_bytes), ("Runtime", status.disk_runtime_bytes), ("run files", status.disk_runs_bytes)];
        let measured: u64 = kinds.iter().map(|(_, bytes)| bytes).sum();
        if measured > 0 {
            let other = (status.disk_total_bytes.saturating_sub(status.disk_free_bytes)).saturating_sub(measured);
            let parts: Vec<String> = kinds.into_iter().filter(|(kind, bytes)| *kind != "run files" || *bytes > 0).chain([("other", other)]).map(|(kind, bytes)| format!("{kind} {}", crate::output::bytes(bytes))).collect();
            fields.push(("disk use".into(), parts.join(" · ")));
        }
    }
    fields.extend(status.runs.iter().map(live_run));
    if !status.mode.is_empty() {
        fields.push(("mode".into(), status.mode.clone()));
    }
    if !status.driver_floor.is_empty() {
        let old: Vec<String> = status
            .gpus
            .iter()
            .filter(|gpu| older(&gpu.driver, &status.driver_floor))
            .map(|gpu| {
                format!(
                    "GPU {}'s driver {} is older: update it",
                    gpu.index, gpu.driver
                )
            })
            .collect();
        let floor = match old.is_empty() {
            true => format!("{} (met)", status.driver_floor),
            false => format!("{}; {}", status.driver_floor, old.join("; ")),
        };
        fields.push(("driver floor".into(), floor));
    }
    fields.extend(status.gpu_problems.iter().map(|problem| ("GPU problem".to_owned(), problem.clone())));
    if let Some(toolchain) = &status.toolchain {
        let cell = match toolchain.state.as_str() {
            "ready" => format!("ready: {} · {}", toolchain.nvcc, toolchain.host_compiler),
            "installing" => "installing".into(),
            state => {
                let mut cell = format!(
                    "{state}: missing {}; cards without a prebuilt kernel use the slower SDPA/kitchen path",
                    toolchain.missing.join(", ")
                );
                if !toolchain.detail.is_empty() {
                    cell += &format!(" ({})", toolchain.detail);
                }
                cell += "; it is fetched when a kernel first needs it";
                if !toolchain.install.is_empty() {
                    cell += &format!(", and for a host compiler: {}", toolchain.install);
                }
                cell
            }
        };
        fields.push(("toolchain".into(), cell));
    }
    for kernel in &status.kernels {
        let verdict = match kernel.ok {
            true => format!("verified: {}", kernel.detail),
            false => format!("refused: {}", kernel.detail),
        };
        fields.push((
            format!("kernel {} sm{}", kernel.kernel, kernel.sm),
            format!("{verdict} (driver {})", kernel.driver),
        ));
    }
    fields
}

/// Whether driver `version` is older than `floor`, by their dotted numbers; unreadable is not.
fn older(version: &str, floor: &str) -> bool {
    let parts =
        |v: &str| -> Option<Vec<u32>> { v.trim().split('.').map(|p| p.parse().ok()).collect() };
    matches!((parts(version), parts(floor)), (Some(v), Some(f)) if v < f)
}

fn kind(m: &Machine) -> &'static str {
    match (m.kind, m.ordered) {
        (MachineKind::Local, _) => "local",
        (MachineKind::Added, _) => "added",
        (MachineKind::Rental, true) => "rental",
        (MachineKind::Rental, false) => "rental (other client)",
    }
}

pub(crate) fn state_cell(m: &Machine) -> String {
    let mut cell = match m.state {
        _ if m.offline.is_some() => format!("offline: {}", m.offline.as_deref().unwrap_or_default()),
        MachineState::Booting => match &m.boot {
            Some(boot) => format!("booting · {boot}"),
            None => "booting".into(),
        },
        MachineState::Ready => match unreachable_since(m) {
            Some(since) => format!(
                "unreachable from the Hub since {} ago",
                clock(now_ms() - since)
            ),
            None => "ready".into(),
        },
        MachineState::Ending => "ending".into(),
        MachineState::Ended => "ended".into(),
        MachineState::Stopped => "stopped · starts when used".into(),
        MachineState::Unknown => "unknown".into(),
    };
    if m.unverified {
        cell += " (unverified)";
    }
    match m.unreachable_since_ms {
        Some(at) if m.state == MachineState::Ready && m.offline.is_none() && unreachable_since(m).is_none() => {
            cell += &format!(" (unreachable for {})", clock(now_ms() - at))
        }
        _ => {}
    }
    cell
}

/// The GPUs as the machine reports them, else as the rental was bought.
fn gpus_cell(m: &Machine) -> String {
    match (&m.status, &m.rental) {
        (Some(status), _) if !status.gpus.is_empty() => {
            format!("{}× {}", status.gpus.len(), status.gpus[0].name)
        }
        (_, Some(rental)) => gpus_of_rental(rental),
        _ => "—".into(),
    }
}

fn gpus_of_rental(r: &Rental) -> String {
    if r.gpus == 0 || r.accelerator.eq_ignore_ascii_case("cpu") {
        "—".into()
    } else {
        format!("{}× {}", r.gpus, r.accelerator)
    }
}

fn family_cell(status: Option<&MachineStatus>) -> String {
    let gpu = status.and_then(|s| s.gpus.first());
    let parts: Vec<&str> = gpu
        .into_iter()
        .flat_map(|g| [g.family.as_deref(), g.memory_class.as_deref()])
        .flatten()
        .collect();
    if parts.is_empty() {
        "—".into()
    } else {
        parts.join(" · ")
    }
}

fn spent(r: &Rental) -> String {
    match (r.spend_usd_micros, r.spend_basis.as_deref()) {
        (Some(micros), Some("charged")) => usd(micros),
        (Some(micros), Some("unmetered")) => format!("{} unmetered", usd(micros)),
        (Some(micros), _) => format!("{} est.", usd(micros)),
        (None, _) => "—".into(),
    }
}

/// The rentals' accrued total, on its weakest basis; one whose Hub said nothing makes it a floor.
fn accrued(rentals: &LiveRentals) -> String {
    let Some(total) = rentals.accrued_usd_micros else {
        return String::new();
    };
    let suffix = match rentals.accrued_basis.as_deref() {
        Some("charged") => "",
        Some("unmetered") => " unmetered",
        _ => " est.",
    };
    let floor = if rentals.accrued_floor { "at least " } else { "" };
    format!(" · accrued {floor}{}{suffix}", usd(total))
}

fn uptime(m: &Machine) -> String {
    let since = m
        .rental
        .as_ref()
        .map(|r| r.rented_at_ms)
        .or(m.status.as_ref().map(|s| s.started_at_ms));
    since.map_or("—".into(), |at| clock(now_ms() - at))
}

/// When an idle machine ends itself: a rental with work names no deadline and ends after it.
fn ends_cell(m: &Machine) -> String {
    match (m.status.as_ref().map(|s| s.idle_deadline_ms), m.kind) {
        (Some(Some(at)), _) if at > now_ms() => format!("in {}", clock(at - now_ms())),
        (Some(Some(_)), _) => "now".into(),
        (Some(None), MachineKind::Rental) => "after its jobs".into(),
        _ => "—".into(),
    }
}

/// Since when the Hub cannot reach a live rental its provider reports running.
fn unreachable_since(m: &Machine) -> Option<i64> {
    let since = m.rental.as_ref()?.unreachable_since_ms?;
    (m.state == MachineState::Ready).then_some(since)
}

/// A machine whose end failed: why, and that it was not ended.
fn end_failed(m: &Machine) -> Option<String> {
    let why = m.end_failed.as_deref()?;
    let billing = if m.kind == MachineKind::Rental { " and may still be billing" } else { "" };
    Some(format!("{} was not shut down{billing}: {why}. `cozy machine end {}` asks again.", m.called(), m.handle()))
}

/// What an offline machine's owner can do: it is kept, never replaced, and refuses runs meanwhile.
fn offline(m: &Machine) -> Option<String> {
    let r = m.rental.as_ref().filter(|_| m.offline.is_some())?;
    Some(format!(
        "{} is offline and refuses runs: it returns if its provider resumes it, and ends after 15 \
         minutes idle. To keep working: `cozy machine rent {}`, or `cozy run … --on new`.",
        m.name, r.sku
    ))
}

/// Its kind, `dev` for a dev build, and the Hub it is from when that is not this one.
fn kind_cell(ctx: &Context, m: &Machine) -> String {
    let dev = match m.status.as_ref().is_some_and(|s| s.dev) {
        true => " dev",
        false => "",
    };
    match ctx.other_hub(&m.hub) {
        Some(hub) => format!("{}{dev} at {hub}", kind(m)),
        None => format!("{}{dev}", kind(m)),
    }
}
