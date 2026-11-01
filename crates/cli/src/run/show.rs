//! `cozy run show`: one run whole, with what its machine measured of it and of each call it
//! made: where its execution time went (executor start, construction, each stage and its steps,
//! and what no measurement covers), the models it ran as resolved (release and lane), each
//! component's attention and GEMM kernels, every GPU's attention kernels (the one that served,
//! the others and why not), each GPU's memory (bytes moved by source, per step, and what make
//! room took), its calls, and its log.
use std::process::ExitCode;

use protocol::{
    calcifer::{Event, Run},
    error::Error,
    report::{self, Measured},
};

use super::follow;
use crate::{Context, output};

/// A run not yet settled is followed to its end in every mode (Ctrl-C stops following, never the
/// run): with `--json`, Calcifer's events as NDJSON, then the run's whole record as the last line.
/// `once`: a snapshot instead. The exit code is the run's: 1 when it failed or was canceled.
pub async fn exec(ctx: &Context, id: &str, once: bool) -> Result<ExitCode, Error> {
    let calcifer = ctx.calcifer().await?;
    let full = format!("/v1/runs/{id}");
    let mut run: Run = calcifer.get_query(&full, &[("detail", "full")]).await?;
    if !once && !follow::settled(&run) {
        let ended = follow::follow(ctx, run).await?;
        if !ctx.json {
            return Ok(follow::exit_code(&ended));
        }
        run = calcifer.get_query(&format!("/v1/runs/{}", ended.id), &[("detail", "full")]).await?;
        output::ndjson(&run);
        return Ok(follow::exit_code(&run));
    }
    if ctx.json {
        output::json(&run);
        return Ok(follow::exit_code(&run));
    }
    let measured = run.report.as_ref().map_or(&[][..], |report| &report.measured);
    let mut fields = follow::fields(ctx, &run);
    if let Some(report) = run.report.as_ref().filter(|r| !r.attention_pins.is_empty()) {
        fields = fields.field("attention kernel", report.attention_pins.join(", "));
    }
    fields.print(ctx);
    grid(&["STAGE", "TIME", "DETAIL"], measured, stage_rows).print(ctx, "stages");
    grid(&["SLOT", "MODEL", "LANE"], measured, model_rows).print(ctx, "models");
    grid(&["COMPONENT", "ATTENTION", "GEMM"], measured, component_rows).print(ctx, "components");
    let gpus = grid(&["GPU", "ARCH", "KERNEL", "STATE", "COMPILE", "DETAIL"], measured, gpu_rows);
    gpus.print(ctx, "attention kernels");
    let memory = grid(
        &["GPU", "SCOPE", "KEPT", "LEASED", "STORE", "DERIVED", "STALL", "PEAK", "TORCH", "COMPUTE", "EVICTED", "LEFTOVER"],
        measured,
        memory_rows,
    );
    memory.print(ctx, "memory");
    let steps = grid(&["GPU", "STEP", "CAPACITY", "HIT", "KEPT", "STORE", "DERIVED", "PREFETCHED", "EVICTED"], measured, step_rows);
    steps.print(ctx, "memory by step");
    grid(&["GPU", "STEP", "FOR", "NEED", "FREED", "WAITED"], measured, make_room_rows).print(ctx, "make room");
    if !run.calls.is_empty() {
        anstream::println!("\ncalls");
        let mut table =
            output::Table::new(ctx, ["#", "FUNCTION", "LABEL", "STATUS", "TIME", "REASON"]);
        for call in &run.calls {
            let ms = (call.finished_at_ms - call.called_at_ms).max(0) as f64;
            let status = match (call.memoized, follow::keyed(&call.memo_basis)) {
                (true, keyed) => format!("{} (memoized{keyed})", call.status),
                (false, "") => call.status.clone(),
                (false, keyed) => format!("{} (computed{keyed})", call.status),
            };
            let reason = call
                .reason
                .as_ref()
                .map(|r| r.message.clone())
                .unwrap_or_default();
            table = table.row([
                call.index.to_string(),
                call.function.clone(),
                call.label.clone(),
                status,
                duration(ms),
                reason,
            ]);
        }
        table.print();
    }
    if follow::settled(&run) {
        // A settled run's stream replays its log and closes.
        let mut events = calcifer.events(run.id.as_str(), 0);
        let mut lines = Vec::new();
        // Each stage the run entered, at its time since the run was accepted: what a run whose
        // machine was lost has to say about where it got to.
        let start = run.accepted_at_ms.unwrap_or(run.created_at_ms);
        while let Some(event) = events.next().await {
            match event? {
                Event::Log { level, text, at_ms, .. } if level == "stage" => {
                    lines.push(format!("  stage: {text} (at {})", duration((at_ms - start).max(0) as f64)));
                }
                Event::Log { level, text, .. } => lines.push(format!("  {level}: {text}")),
                _ => {}
            }
        }
        if !lines.is_empty() {
            anstream::println!("\nlog");
            for line in lines {
                anstream::println!("{line}");
            }
        }
    }
    if !follow::settled(&run) {
        follow::next(ctx, &run);
    }
    Ok(follow::exit_code(&run))
}

/// A table's rows before they meet the terminal. With more than one measured, each row leads
/// with who measured it.
struct Grid {
    header: Vec<&'static str>,
    rows: Vec<Vec<String>>,
}

fn grid(
    header: &[&'static str],
    measured: &[Measured],
    rows_of: fn(&Measured) -> Vec<Vec<String>>,
) -> Grid {
    let lead = measured.len() > 1;
    let header = lead
        .then_some("CALL")
        .into_iter()
        .chain(header.iter().copied());
    let rows = measured.iter().flat_map(|m| {
        rows_of(m).into_iter().map(move |row| {
            let who = lead.then(|| m.by.clone());
            who.into_iter().chain(row).collect::<Vec<_>>()
        })
    });
    Grid {
        header: header.collect(),
        rows: rows.collect(),
    }
}

impl Grid {
    fn print(self, ctx: &Context, title: &str) {
        if self.rows.is_empty() {
            return;
        }
        anstream::println!("\n{title}");
        let table = output::Table::new(ctx, self.header);
        self.rows
            .into_iter()
            .fold(table, output::Table::row)
            .print();
    }
}

/// Where the span's time went, in the order it ran, and last what no measurement covers.
fn stage_rows(measured: &Measured) -> Vec<Vec<String>> {
    let row = |stage: &report::Stage| {
        let detail = match (&stage.steps, stage.legs.is_empty(), stage.unaccounted) {
            (_, _, true) => "no measurement covers it".to_owned(),
            (Some(steps), ..) => format!("{} × {}", steps.count, duration(steps.each_ms)),
            (None, false, _) => {
                let legs: Vec<String> = stage.legs.iter().map(|leg| format!("{} {}", leg.name, duration(leg.ms))).collect();
                legs.join(", ")
            }
            (None, true, _) => String::new(),
        };
        vec![stage.name.clone(), duration(stage.ms), detail]
    };
    measured.stages.iter().map(row).collect()
}

/// Each model slot's checkpoint: its repository and release, or its hash when it was passed
/// by value, and the lane it was resolved in with its header's precision family.
fn model_rows(measured: &Measured) -> Vec<Vec<String>> {
    let row = |m: &report::Model| {
        let mut model = match (m.repository.is_empty(), m.release.is_empty()) {
            (true, _) => m.manifest.chars().take(19).collect(),
            (false, true) => m.repository.clone(),
            (false, false) => format!("{}@{}", m.repository, m.release),
        };
        for a in &m.adapters {
            model += &format!(" + {},{} on {}", a.model, a.strength, a.components.join(", "));
        }
        let lane = match m.precision.is_empty() {
            true => or_dash(&m.lane),
            false => format!("{} ({})", or_dash(&m.lane), m.precision),
        };
        vec![m.slot.clone(), model, lane]
    };
    measured.models.iter().map(row).collect()
}

/// Each component's attention and GEMM kernels (a 16-bit lane's components have no GEMM row),
/// a GEMM that served below its lane with what it wanted and why.
fn component_rows(measured: &Measured) -> Vec<Vec<String>> {
    let gemm = |g: &report::Gemm| match &g.fallback {
        None => format!("{} ({})", g.kernel, g.format),
        Some(f) => {
            let served = if f.served.is_empty() { String::new() } else { format!("{}; ", f.served) };
            format!("{} ({served}wanted {}: {})", g.kernel, f.wanted, f.reason)
        }
    };
    let row = |c: &report::Component| {
        let gemms: Vec<String> = c.gemm.iter().map(gemm).collect();
        vec![c.component.clone(), or_dash(&c.attention.join("; ")), or_dash(&gemms.join("; "))]
    };
    measured.components.iter().map(row).collect()
}

/// Every GPU's attention kernels: the one that served, and the others with why not.
fn gpu_rows(measured: &Measured) -> Vec<Vec<String>> {
    let row = |k: &report::Kernel| {
        let state = match (k.served, k.progress) {
            (true, _) => "served".to_owned(),
            (false, Some(p)) => format!("{} ({:.0}%)", k.state, p * 100.0),
            (false, None) => k.state.clone(),
        };
        let compile = if k.compile_ms > 0.0 { duration(k.compile_ms) } else { "-".into() };
        let gpu = k.gpu.map_or("?".into(), |n| n.to_string());
        vec![gpu, k.arch.clone(), k.kernel.clone(), state, compile, or_dash(&k.detail)]
    };
    measured.kernels.iter().map(row).collect()
}

/// Each GPU's whole request: the bytes moved by source at their rate against the measured
/// ceiling, the stall, the most it held, torch's own peak, the compute bytes, what make_room took
/// and what it still held at the end.
fn memory_rows(measured: &Measured) -> Vec<Vec<String>> {
    let bytes = |n: Option<u64>| n.map_or_else(|| "-".into(), output::bytes);
    let row = |m: &report::Memory| {
        vec![
            m.gpu.to_string(),
            m.scope.clone(),
            moved(&m.kept),
            moved(&m.leased),
            moved(&m.store),
            moved(&m.derived),
            duration(m.stall_ms),
            bytes(m.peak),
            bytes(m.torch_peak),
            bytes(m.compute),
            taken(&m.evicted),
            bytes(m.leftover),
        ]
    };
    measured.memory.iter().map(row).collect()
}

/// Bytes and, when they took time, their rate and its share of the ceiling: `3.2 GiB @ 11.4 GB/s (48%)`.
fn moved(moved: &report::Moved) -> String {
    if moved.bytes == 0 {
        return "-".into();
    }
    let mut text = output::bytes(moved.bytes);
    if let Some(gbps) = moved.gbps {
        text.push_str(&format!(" @ {gbps:.1} GB/s"));
        if let Some(share) = moved.ceiling_share {
            text.push_str(&format!(" ({:.0}%)", 100.0 * share));
        }
    }
    text
}

/// What make_room took, by class: `F 1.2 GiB, S 300 MiB`.
fn taken(taken: &[report::Taken]) -> String {
    let said: Vec<String> = taken.iter().map(|t| format!("{} {}", t.class, output::bytes(t.bytes))).collect();
    or_dash(&said.join(", "))
}

/// Each GPU's steps: the free bytes at its start, the bytes its binds found mapped (hit), adopted
/// (kept), read (store) and refilled from a derived entry, how many were prefetched, and what
/// make_room took in it.
fn step_rows(measured: &Measured) -> Vec<Vec<String>> {
    let bytes = |n: u64| if n == 0 { "-".to_string() } else { output::bytes(n) };
    let row = |s: &report::Step| {
        vec![
            s.gpu.to_string(),
            s.step.to_string(),
            s.capacity.map_or("-".into(), output::bytes),
            bytes(s.hit),
            bytes(s.kept),
            bytes(s.store),
            bytes(s.derived),
            bytes(s.prefetched),
            s.evicted.as_deref().map_or("-".into(), taken),
        ]
    };
    measured.steps.iter().map(row).collect()
}

/// Each make_room that had to take bytes: its step, what asked, the bytes asked and taken, and the
/// release events it waited on.
fn make_room_rows(measured: &Measured) -> Vec<Vec<String>> {
    let row = |m: &report::MakeRoom| {
        let (need, freed) = (output::bytes(m.need), output::bytes(m.freed));
        vec![m.gpu.to_string(), m.step.to_string(), m.trigger.clone(), need, freed, m.waited.to_string()]
    };
    measured.make_room.iter().map(row).collect()
}

fn or_dash(text: &str) -> String {
    match text.is_empty() {
        true => "-".into(),
        false => text.into(),
    }
}

/// A time as people read it: `850ms`, `7.6s`, `3m48.7s`.
fn duration(ms: f64) -> String {
    match ms {
        ms if ms < 1000.0 => format!("{ms:.0}ms"),
        ms if ms < 60_000.0 => format!("{:.1}s", ms / 1000.0),
        ms => format!(
            "{}m{:.1}s",
            (ms / 60_000.0).floor(),
            (ms % 60_000.0) / 1000.0
        ),
    }
}

