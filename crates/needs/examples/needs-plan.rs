//! `needs-plan <interface.json> <package>/<function> [<benchmarks.json> <card.json> [<runs.json>]]`, a request on stdin: the function's
//! plan of it as Calcifer and TensorD make it (cl-1286): what its rules need, its size and, with its package's rows and a card
//! (`{"gpu": "NVIDIA GeForce RTX 4090", "gpus": 1, "memory_bytes": 25757220864, "lane": "int8-convrot"}`, any field left out; no
//! `gpu`, a machine without one), what it costs there and the row that stands on; and the interface's estimate faults, as cozy
//! refuses a package for them. With past runs of it on that card (`[{"size": {"prompt_words": 9, …}, "ms": 20600}]`, newest first),
//! its time there by them, as Calcifer's quote of that machine gives it (`api::estimate::quote`): each run scaled to the size asked as
//! its rows are, by its formula's times at both sizes where it declares one, else by its rows' exponents (`Shape`); and the default's
//! time at each run's size, which a run scales as.
use std::io::Read as _;

use serde_json::{Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(path), Some(function)) = (args.first(), args.get(1)) else {
        eprintln!("usage: needs-plan <interface.json> <package>/<function> [<benchmarks.json> <card.json>] < request.json");
        std::process::exit(2);
    };
    let read = |what: &str, text: Result<String, std::io::Error>| -> Value {
        match text.map_err(|error| error.to_string()).and_then(|text| serde_json::from_str(&text).map_err(|error| error.to_string())) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("{what}: {error}");
                std::process::exit(1);
            }
        }
    };
    let document = read(path, std::fs::read_to_string(path));
    let mut sent = String::new();
    let request = read("the request", std::io::stdin().read_to_string(&mut sent).map(|_| sent));
    let (package, entrypoint) = function.rsplit_once('/').unwrap_or(("", function));
    let plan = needs::plan(&document, &[], entrypoint, Some(&request));
    let mut said = json!({
        "needed": plan.wants.needed, "extras": plan.wants.extras, "size": plan.cost.size, "size_unanswered": plan.cost.size_unanswered,
        "faults": needs::estimate_faults(&document),
    });
    if let (Some(rows), Some(card)) = (args.get(2), args.get(3)) {
        let rows: Vec<api::estimate::Benchmark> = serde_json::from_value(read(rows, std::fs::read_to_string(rows))).unwrap_or_default();
        let card = read(card, std::fs::read_to_string(card));
        let text = |name: &str| card[name].as_str().unwrap_or_default().to_owned();
        let count = |name: &str| card[name].as_u64().unwrap_or(0);
        let card = needs::Card {
            gpu: text("gpu"), gpus: count("gpus") as u32, memory_bytes: count("memory_bytes"), lane: text("lane"), model: text("model"),
            picked: card["picked"].as_bool().unwrap_or(false), cpu: text("cpu"), cores: count("cores") as u32, threads: count("threads") as u32,
        };
        let mut references = api::estimate::References::default();
        references.add(package, &rows);
        let estimated = plan.cost.on(function, &card, &references);
        // The row it stands on, as the platform's estimate picks it (`References::estimate`), for a formula and the default alike.
        let picked = references.estimate(function, &card.gpu, card.memory_bytes, &plan.cost.size, card.runs(), true);
        said["row"] = json!(picked.map(|pick| json!({
            "gpu": pick.row.gpu, "lane": pick.row.lane, "request": pick.row.request, "warm_ms": pick.row.warm_ms, "budget_bytes": pick.row.budget_bytes,
            "scale": pick.scale, "default_ms": pick.ms, "basis": pick.basis,
        })));
        said["estimate"] = json!({
            "ms": estimated.ms, "cold_ms": estimated.cold_ms, "peak_device_bytes": estimated.peak_device_bytes,
            "peak_host_bytes": estimated.peak_host_bytes, "basis": estimated.basis, "declared": estimated.declared,
            "unanswered": estimated.unanswered,
        });
        if let Some(path) = args.get(4) {
            let runs: Vec<api::machine::TimedRun> = serde_json::from_value(read(path, std::fs::read_to_string(path))).unwrap_or_default();
            said["past"] = past(function, &plan.cost, &card, &references, &runs, estimated.ms.filter(|_| estimated.declared));
        }
    }
    println!("{said}");
}

/// `function`'s time on `card` by `runs` there, as Calcifer's quote of a machine of that card that ran them gives it: a job of
/// `cost` with its formula's time (`ms`) and that formula's times at the runs' sizes, as `placement::formula` gives them.
fn past(function: &str, cost: &needs::Cost, card: &needs::Card, references: &api::estimate::References, runs: &[api::machine::TimedRun], ms: Option<f64>) -> Value {
    use api::machine::{EstimateJob, Gpu, StatusFrame, Timed};
    let named = format!("cozy/{function}");
    let formula_at = match ms {
        Some(_) => cost.formula_at(function, card, references, runs.iter().map(|run| &run.size)),
        None => vec![],
    };
    let gpus = if card.gpu.is_empty() { vec![] } else { vec![Gpu { index: 0, name: card.gpu.clone(), memory_bytes: card.memory_bytes, ..Default::default() }] };
    let frame = StatusFrame {
        phase: "ready".into(),
        serving_gpus: if card.gpu.is_empty() { 0 } else { card.gpus.max(1) },
        gpus,
        times: Some(vec![Timed { function: named.clone(), runs: runs.to_vec(), ..Default::default() }]),
        ..Default::default()
    };
    let job = EstimateJob { key: "asked".into(), function: named, size: cost.size.clone(), formula_ms: ms, formula_basis: ms.map(|_| api::machine::Basis::Reference), formula_at: formula_at.clone(), ..Default::default() };
    let quote = api::estimate::quote(&frame, 0, std::slice::from_ref(&job), references).remove(0);
    let at_runs: Vec<Value> = (runs.iter())
        .map(|run| {
            let default = references.estimate(function, &card.gpu, card.memory_bytes, &run.size, card.runs(), true).map(|pick| pick.ms);
            json!({"size": run.size, "ms": run.ms, "default_ms": default, "formula_ms": formula_at.iter().find(|(size, _)| size.values == run.size.values).map(|(_, ms)| *ms)})
        })
        .collect();
    json!({"ms": quote.finish_ms.zip(quote.start_ms).map(|(finish, start)| finish - start), "basis": quote.basis, "runs": at_runs})
}
