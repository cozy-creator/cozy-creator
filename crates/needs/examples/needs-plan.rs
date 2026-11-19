//! `needs-plan <interface.json> <package>/<function> [<benchmarks.json> <card.json>]`, a request on stdin: the function's plan of it
//! as Calcifer and TensorD make it (cl-1286): what its rules need, its size and, with its package's rows and a card
//! (`{"gpu": "NVIDIA GeForce RTX 4090", "gpus": 1, "memory_bytes": 25757220864, "lane": "int8-convrot"}`, any field left out; no
//! `gpu`, a machine without one), what it costs there and the row that stands on; and the interface's estimate faults, as cozy
//! refuses a package for them.
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
    }
    println!("{said}");
}
