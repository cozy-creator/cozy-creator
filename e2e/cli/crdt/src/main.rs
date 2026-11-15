//! Inspect snapshots produced by real current-format CLI/project API operations and exercise their actual CRDT
//! format across offline native replicas. No daemon, mock project store or media merge is involved.
use std::{fs, path::Path};

use automerge::{ActorId, AutoCommit, AutoSerde, ObjId, ObjType, ROOT, ReadDoc, transaction::Transactable};
use serde_json::{Value, json};

fn object(doc: &AutoCommit, parent: &ObjId, key: &str) -> Result<ObjId, String> {
    doc.get(parent, key)
        .map_err(|e| e.to_string())?
        .map(|(_, id)| id)
        .ok_or_else(|| format!("missing object {key}"))
}
fn value(doc: &AutoCommit) -> Value {
    serde_json::to_value(AutoSerde::from(doc)).expect("native document serializes")
}
fn require(holds: bool, why: &str) -> Result<(), String> {
    holds.then_some(()).ok_or_else(|| why.to_owned())
}
fn named(value: &Value, name: &str) -> Result<String, String> {
    value["boxes"]
        .as_object()
        .into_iter()
        .flatten()
        .find(|(_, item)| item["present"] == true && item["name"] == name)
        .map(|(id, _)| id.clone())
        .ok_or_else(|| format!("no box {name}"))
}
/// Add independent history entries with the same local display number to real CLI state.
/// The caller installs this native merged snapshot as a fixture, then exercises the product's
/// next ordinary CLI transaction before checking that both original identities survive.
fn history_fixture(input: &Path, output: &Path) -> Result<Value, String> {
    let bytes = fs::read(input).map_err(|e| e.to_string())?;
    let seed = AutoCommit::load(&bytes).map_err(|e| e.to_string())?;
    let number = value(&seed)["history"]
        .as_object().into_iter().flatten()
        .filter_map(|(_, record)| record["record"]["id"].as_u64())
        .max().unwrap_or(0) + 1;
    let graph = value(&seed);
    let mut replicas = Vec::new();
    for (actor, uid) in [(201u64, "e2e-history-a"), (202u64, "e2e-history-b")] {
        let mut doc = AutoCommit::load(&bytes).map_err(|e| e.to_string())?.with_actor(ActorId::from(actor.to_be_bytes()));
        let boxes = object(&doc, &ROOT, "boxes")?;
        let held = object(&doc, &boxes, &named(&graph, "whale")?)?;
        let fields = object(&doc, &held, "fields")?;
        doc.put(&fields, "instructions", if actor == 201 { "remote dawn" } else { "remote dusk" }).map_err(|e| e.to_string())?;
        // The outer objects conflict, and each has a concurrent scalar conflict inside a
        // list. A CLI read must report both retained subtrees, including the losing object.
        let details = doc.put_object(&fields, "e2e_details", ObjType::Map).map_err(|e| e.to_string())?;
        let choices = doc.put_object(&details, "choices", ObjType::List).map_err(|e| e.to_string())?;
        doc.insert(&choices, 0, "initial").map_err(|e| e.to_string())?;
        doc.commit();
        let mut other = doc.fork().with_actor(ActorId::from((actor + 1000).to_be_bytes()));
        doc.put(&choices, 0, format!("{uid}-left")).map_err(|e| e.to_string())?;
        other.put(&choices, 0, format!("{uid}-right")).map_err(|e| e.to_string())?;
        doc.merge(&mut other).map_err(|e| e.to_string())?;
        let history = object(&doc, &ROOT, "history")?;
        let entry = doc.put_object(&history, uid, ObjType::Map).map_err(|e| e.to_string())?;
        doc.put(&entry, "present", true).map_err(|e| e.to_string())?;
        let record = doc.put_object(&entry, "record", ObjType::Map).map_err(|e| e.to_string())?;
        doc.put(&record, "uid", uid).map_err(|e| e.to_string())?;
        doc.put(&record, "id", number).map_err(|e| e.to_string())?;
        doc.put(&record, "at_ms", 1i64).map_err(|e| e.to_string())?;
        for (key, text) in [("by", r#"{"via":"api"}"#), ("words", uid), ("edits", "[]"), ("changes", "[]")] {
            doc.put(&record, key, text).map_err(|e| e.to_string())?;
        }
        doc.commit();
        replicas.push(doc);
    }
    let mut merged = replicas.remove(0);
    merged.merge(&mut replicas[0]).map_err(|e| e.to_string())?;
    let heads: Vec<String> = merged.get_heads().iter().map(ToString::to_string).collect();
    fs::write(output, merged.save()).map_err(|e| e.to_string())?;
    Ok(json!({"passed":true,"same_number_history":number,"heads":heads}))
}

fn history_check(input: &Path) -> Result<Value, String> {
    let doc = AutoCommit::load(&fs::read(input).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let value = value(&doc);
    let a = &value["history"]["e2e-history-a"];
    let b = &value["history"]["e2e-history-b"];
    require(a["present"] == true && b["present"] == true, "next CLI write lost a concurrent same-number history entry")?;
    require(a["record"]["uid"] == "e2e-history-a" && b["record"]["uid"] == "e2e-history-b", "history map identity was replaced")?;
    require(a["record"]["id"] == b["record"]["id"], "history fixture no longer exercises colliding local labels")?;
    let boxes = object(&doc, &ROOT, "boxes")?;
    let held = object(&doc, &boxes, &named(&value, "whale")?)?;
    let fields = object(&doc, &held, "fields")?;
    let mut alternatives: Vec<_> = doc.get_all(&fields, "instructions").map_err(|e| e.to_string())?.into_iter()
        .filter_map(|(value, _)| value.as_str().map(str::to_owned)).collect();
    alternatives.sort();
    require(alternatives == ["remote dawn", "remote dusk"], "unrelated CLI edit erased retained prompt alternatives")?;
    let details = doc.get_all(&fields, "e2e_details").map_err(|e| e.to_string())?;
    require(details.len() == 2, "next CLI write lost a conflicting object alternative")?;
    let mut nested = Vec::new();
    for (_, detail) in details {
        let choices = object(&doc, &detail, "choices")?;
        let mut values: Vec<_> = doc.get_all(&choices, 0).map_err(|e| e.to_string())?.into_iter()
            .filter_map(|(value, _)| value.as_str().map(str::to_owned)).collect();
        values.sort();
        require(values.len() == 2, "next CLI write lost a nested list conflict")?;
        nested.push(values);
    }
    nested.sort();
    require(nested == [vec!["e2e-history-a-left", "e2e-history-a-right"], vec!["e2e-history-b-left", "e2e-history-b-right"]], "nested object alternatives changed")?;
    Ok(json!({"passed":true,"same_number_history_preserved_after_cli_commit":2,"conflicts_retained_after_cli_commit":alternatives,"nested_conflicts_retained_after_cli_commit":nested}))
}
fn run(before: &Path, renamed: &Path) -> Result<Value, String> {
    let seed_bytes = fs::read(before).map_err(|e| e.to_string())?;
    let mut seed = AutoCommit::load(&seed_bytes).map_err(|e| e.to_string())?;
    let after = AutoCommit::load(&fs::read(renamed).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let initial = value(&seed);
    require(initial["schema"] == 1, "unexpected logical schema")?;
    let box_id = named(&initial, "kyogre")?;
    require(named(&value(&after), "whale")? == box_id, "rename replaced stable entity identity")?;
    // Files, versions and runs are project.db's rows, never node state (cl-904).
    let outside = ["assets", "files", "runs", "reserved", "makes"].iter().all(|key| initial.get(*key).is_none());
    require(outside, "the document holds files, versions or runs")?;
    let heads = seed.get_heads();
    let mut replicas: Vec<AutoCommit> = (101u64..104)
        .map(|actor| {
            AutoCommit::load(&seed_bytes)
                .expect("fixture reload")
                .with_actor(ActorId::from(actor.to_be_bytes()))
        })
        .collect();
    for (index, doc) in replicas.iter_mut().enumerate() {
        let boxes = object(doc, &ROOT, "boxes")?;
        let held = object(doc, &boxes, &box_id)?;
        let fields = object(doc, &held, "fields")?;
        doc.put(&fields, "instructions", ["dawn", "dusk", "noon"][index])
            .map_err(|e| e.to_string())?;
        doc.commit();
    }
    let packets: Vec<_> = replicas.iter_mut().map(|doc| doc.get_changes(&heads)).collect();
    for doc in &mut replicas {
        for packet in packets.iter().rev() {
            for change in packet.iter().rev() {
                doc.apply_changes([change.clone()]).map_err(|e| e.to_string())?;
            }
        }
        for packet in &packets {
            doc.apply_changes(packet.clone()).map_err(|e| e.to_string())?;
        }
    }
    let merged = value(&replicas[0]);
    require(replicas.iter().all(|doc| value(doc) == merged), "replicas diverged")?;
    let boxes = object(&replicas[0], &ROOT, "boxes")?;
    let held = object(&replicas[0], &boxes, &box_id)?;
    let fields = object(&replicas[0], &held, "fields")?;
    let mut alternatives: Vec<String> = replicas[0]
        .get_all(&fields, "instructions")
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|(value, _)| value.as_str().map(str::to_owned))
        .collect();
    alternatives.sort();
    require(
        alternatives == ["dawn", "dusk", "noon"],
        "concurrent prompt alternatives were not retained",
    )?;
    for doc in &mut replicas {
        let restored = AutoCommit::load(&doc.save()).map_err(|e| e.to_string())?;
        require(value(&restored) == merged, "snapshot restart changed merged state")?;
    }
    Ok(
        json!({"passed":true,"native_replicas":3,"stable_rename_id":box_id,"no_files_versions_runs":outside,"prompt_alternatives":alternatives,"duplicate_reordered_delivery":true,"snapshot_reload":true}),
    )
}
fn canvas_collision(input: &Path, output: &Path) -> Result<Value, String> {
    let mut doc = AutoCommit::load(&fs::read(input).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?.with_actor(ActorId::from(65534u64.to_be_bytes()));
    let id = named(&value(&doc), "whale")?;
    let canvas = object(&doc, &ROOT, "canvas")?;
    for (id, x, y) in [(id.as_str(), 10.0, 20.0), ("zzzz-e2e-canvas-peer", 700.0, 800.0)] {
        let held = doc.put_object(&canvas, id, ObjType::Map).map_err(|e| e.to_string())?;
        doc.put(&held, "present", true).map_err(|e| e.to_string())?;
        doc.put(&held, "key", "whale").map_err(|e| e.to_string())?;
        let record = doc.put_object(&held, "record", ObjType::Map).map_err(|e| e.to_string())?;
        doc.put(&record, "name", "whale").map_err(|e| e.to_string())?;
        doc.put(&record, "seq", 7u64).map_err(|e| e.to_string())?;
        doc.put(&record, "by", "native peer").map_err(|e| e.to_string())?;
        let card = doc.put_object(&record, "card", ObjType::Map).map_err(|e| e.to_string())?;
        doc.put(&card, "x", x).map_err(|e| e.to_string())?;
        doc.put(&card, "y", y).map_err(|e| e.to_string())?;
    }
    let boxes = object(&doc, &ROOT, "boxes")?;
    let whale = object(&doc, &boxes, &id)?;
    let root_extension = doc.put_object(ROOT, "e2e_extension", ObjType::Map).map_err(|e| e.to_string())?;
    let box_extension = doc.put_object(&whale, "e2e_extension", ObjType::Map).map_err(|e| e.to_string())?;
    doc.put(&root_extension, "payload", "root peer data").map_err(|e| e.to_string())?;
    doc.put(&box_extension, "payload", "box peer data").map_err(|e| e.to_string())?;

    let heads: Vec<_> = doc.get_heads().iter().map(ToString::to_string).collect();
    fs::write(output, doc.save()).map_err(|e| e.to_string())?;
    Ok(json!({"id":id,"heads":heads,"selected_record":{"name":"whale","card":{"x":700.0,"y":800.0},"seq":7,"by":"native peer"},"root_extension":root_extension.to_string(),"box_extension":box_extension.to_string()}))
}

fn canvas_check(input: &Path, report: &Path) -> Result<Value, String> {
    let doc = AutoCommit::load(&fs::read(input).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let report: Value = serde_json::from_slice(&fs::read(report).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let state = value(&doc);
    let target = report["id"].as_str().ok_or("missing mapped canvas identity")?;
    require(state["canvas"][target]["record"] == report["selected_record"], "mapped canvas retained an older peer record")?;
    require(state["canvas"][target]["present"] == true && state["canvas"]["zzzz-e2e-canvas-peer"]["present"] == false, "canvas normalization lost mapped identity or failed to retire peer")?;
    let boxes = object(&doc, &ROOT, "boxes")?;
    let whale = object(&doc, &boxes, target)?;
    require(state["e2e_extension"] == json!({"payload":"root peer data"}) && object(&doc, &ROOT, "e2e_extension")?.to_string() == report["root_extension"], "unknown root value or object identity changed")?;
    require(state["boxes"][target]["e2e_extension"] == json!({"payload":"box peer data"}) && object(&doc, &whale, "e2e_extension")?.to_string() == report["box_extension"], "untouched record lost its unknown value or object identity")?;
    Ok(json!({"passed":true,"mapped_canvas_id":target,"selected_record":report["selected_record"],"unknown_objects_retained":true}))
}

/// One independent native writer against actual CLI-created state. The CLI fixture holds
/// SQLite's writer turn, installs this complete native change, then checks stale-base refusal.
fn outside_edit(input: &Path, output: &Path) -> Result<Value, String> {
    let mut bytes = fs::read(input).map_err(|e| e.to_string())?;
    let mut doc = AutoCommit::load(&bytes).map_err(|e| e.to_string())?.with_actor(ActorId::from(65535u64.to_be_bytes()));
    let old_heads = doc.get_heads();
    let boxes = object(&doc, &ROOT, "boxes")?;
    let held = object(&doc, &boxes, &named(&value(&doc), "whale")?)?;
    let fields = object(&doc, &held, "fields")?;
    doc.put(&fields, "e2e_outside", format!("retained native writer {}", output.file_stem().unwrap_or_default().to_string_lossy())).map_err(|e| e.to_string())?;
    doc.commit();
    let change = doc.get_last_local_change().ok_or("no outside native change")?;
    fs::write(output.with_extension("change"), change.raw_bytes()).map_err(|e| e.to_string())?;
    let heads: Vec<_> = doc.get_heads().iter().map(ToString::to_string).collect();
    bytes.extend(doc.save_after(&old_heads));
    let restored = AutoCommit::load(&bytes).map_err(|e| e.to_string())?;
    require(value(&restored) == value(&doc), "appended outside stream changed native state")?;
    fs::write(output, bytes).map_err(|e| e.to_string())?;
    Ok(json!({"heads":heads,"hash":change.hash().to_string(),"actor":change.actor_id().to_hex_string()}))
}

fn scan_fixture(input: &Path, output: &Path) -> Result<Value, String> {
    let mut doc = AutoCommit::load(&fs::read(input).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?
        .with_actor(ActorId::from(65533u64.to_be_bytes()));
    let original = value(&doc);
    let records = original["boxes"].as_object().ok_or("no boxes in CLI fixture")?;
    let start = records.values().filter_map(|row| row["order"].as_u64()).max().unwrap_or(0) + 1;
    let first_collision_order = records.values().filter(|row| row["present"] != false).count();
    let boxes = object(&doc, &ROOT, "boxes")?;
    for (offset, (id, name, text)) in [
        ("e2e-scan-mixed-first", "Mixed_Name", "first mixed"),
        ("e2e-scan-unicode-first", "Étage-A", "first unicode"),
        ("e2e-scan-pivot", "projection_pivot", "between"),
        ("e2e-scan-mixed-last", "MIXED-NAME", "last mixed"),
        ("e2e-scan-unicode-last", "ÉTAGE_A", "last unicode"),
    ].into_iter().enumerate() {
        let row = doc.put_object(&boxes, id, ObjType::Map).map_err(|e| e.to_string())?;
        doc.put(&row, "name", name).map_err(|e| e.to_string())?;
        doc.put(&row, "present", true).map_err(|e| e.to_string())?;
        doc.put(&row, "order", start + offset as u64).map_err(|e| e.to_string())?;
        let fields = doc.put_object(&row, "fields", ObjType::Map).map_err(|e| e.to_string())?;
        doc.put(&fields, "text", text).map_err(|e| e.to_string())?;
    }
    let groudon = object(&doc, &boxes, &named(&original, "groudon")?)?;
    let fields = object(&doc, &groudon, "fields")?;
    let keys = doc.put_object(&fields, "e2e_keys", ObjType::Map).map_err(|e| e.to_string())?;
    for key in ["Ωmega", "zeta", "Éclair", "a", "keep"] {
        doc.put(&keys, key, "kept").map_err(|e| e.to_string())?;
    }
    doc.put(&fields, "e2e_replace", "scalar before replacement").map_err(|e| e.to_string())?;
    let heads: Vec<_> = doc.get_heads().iter().map(ToString::to_string).collect();
    let native_order: Vec<_> = doc.keys(&keys).filter(|key| key != "keep").collect();
    require(native_order == ["a", "zeta", "Éclair", "Ωmega"], "unexpected native map key order")?;
    fs::write(output, doc.save()).map_err(|e| e.to_string())?;
    Ok(json!({"heads":heads,"first_collision_order":first_collision_order}))
}

fn scan_check(before: &Path, after: &Path) -> Result<Value, String> {
    let mut before = AutoCommit::load(&fs::read(before).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let mut after = AutoCommit::load(&fs::read(after).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let old = value(&before);
    let new = value(&after);
    let groudon = named(&old, "groudon")?;
    require(named(&new, "groudon")? == groudon, "map replacement changed the box identity")?;
    let fields = &new["boxes"][&groudon]["fields"];
    require(fields["e2e_keys"] == json!({"keep":"kept","new":["same","same"]}), "reused map did not preserve kept keys and insert equal list items")?;
    require(fields["e2e_replace"] == json!({"branch":[1,1]}), "replacement map did not retain its new nested list")?;
    for id in ["e2e-scan-pivot", "e2e-scan-mixed-first"] {
        require(new["boxes"][id]["present"] == false && new["boxes"][id]["fields"] == old["boxes"][id]["fields"], "deleted box lost its retained fields")?;
    }
    for id in ["e2e-scan-mixed-last", "e2e-scan-unicode-first", "e2e-scan-unicode-last"] {
        require(new["boxes"][id]["present"] == true && new["boxes"][id]["fields"] == old["boxes"][id]["fields"], "a surviving normalized collision lost its native identity or fields")?;
    }
    let removed = ["a", "zeta", "Éclair", "Ωmega"];
    let mut deletes = Vec::new();
    for change in after.get_changes(&before.get_heads()) {
        let decoded = serde_json::to_value(change.decode()).map_err(|e| e.to_string())?;
        deletes.extend(decoded["ops"].as_array().into_iter().flatten().filter_map(|op| {
            let key = op["key"].as_str()?;
            (op["action"] == "del" && removed.contains(&key)).then(|| key.to_owned())
        }));
    }
    require(deletes == removed, "native Unicode map deletes were missing or out of order")?;
    Ok(json!({"passed":true,"deleted_keys":deletes,"retained_box_tombstones":2,"surviving_collision_ids":3,"equal_list_items_retained":true}))
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 4 && (args[1] == "--scan-fixture" || args[1] == "--scan-check") {
        let input = Path::new(&args[2]);
        let output = Path::new(&args[3]);
        let report = if args[1] == "--scan-fixture" { scan_fixture(input, output) } else { scan_check(input, output) };
        match report {
            Ok(report) => println!("{report}"),
            Err(why) => { eprintln!("{why}"); std::process::exit(1); }
        }
        return;
    }
    if args.len() == 4 && ["--outside-edit", "--canvas-collision", "--canvas-check"].iter().any(|mode| args[1] == *mode) {
        let report = if args[1] == "--outside-edit" { outside_edit(Path::new(&args[2]), Path::new(&args[3])) }
            else if args[1] == "--canvas-collision" { canvas_collision(Path::new(&args[2]), Path::new(&args[3])) }
            else { canvas_check(Path::new(&args[2]), Path::new(&args[3])) };
        match report {
            Ok(report) => println!("{report}"),
            Err(why) => { eprintln!("{why}"); std::process::exit(1); }
        }
        return;
    }
    if (args.len() == 4 && args[1] == "--history-fixture") || (args.len() == 3 && args[1] == "--history-check") {
        let report = if args.len() == 4 {
            history_fixture(Path::new(&args[2]), Path::new(&args[3]))
        } else {
            history_check(Path::new(&args[2]))
        };
        match report {
            Ok(report) => println!("{report}"),
            Err(why) => { eprintln!("{why}"); std::process::exit(1); }
        }
        return;
    }
    if matches!(args.len(), 5 | 6) && args[1] == "--identity" {
        let report = (|| -> Result<Value, String> {
            let before = AutoCommit::load(&fs::read(&args[2]).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let after = AutoCommit::load(&fs::read(&args[3]).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let name = args[4].to_str().ok_or("non-UTF8 box name")?;
            let after_name = match args.get(5) {
                Some(name) => name.to_str().ok_or("non-UTF8 new box name")?,
                None => name,
            };
            let id = named(&value(&before), name)?;
            require(
                named(&value(&after), after_name)? == id,
                "redo/undo changed restored entity identity",
            )?;
            Ok(json!({"passed":true,"restored_entity_id":id}))
        })();
        match report {
            Ok(value) => println!("{value}"),
            Err(why) => {
                eprintln!("{why}");
                std::process::exit(1);
            }
        }
        return;
    }
    if args.len() != 3 {
        eprintln!("usage: cozy-project-crdt-proof BEFORE.am RENAMED.am");
        std::process::exit(2);
    }
    match run(Path::new(&args[1]), Path::new(&args[2])) {
        Ok(report) => println!("{}", report),
        Err(why) => {
            eprintln!("project CRDT proof: {why}");
            std::process::exit(1);
        }
    }
}
