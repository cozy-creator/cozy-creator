//! Inspect snapshots produced by real `cozy project` operations and exercise their actual CRDT
//! format across offline native replicas. No daemon, mock project store or media merge is involved.
use std::{collections::BTreeSet, fs, path::Path};

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
fn new_version(doc: &mut AutoCommit, asset_id: &str, id: &str, number: u64, source: &Value, words: &str) -> Result<(), String> {
    let assets = object(doc, &ROOT, "assets")?;
    let asset = object(doc, &assets, asset_id)?;
    let versions = object(doc, &asset, "versions")?;
    let version = doc.put_object(&versions, id, ObjType::Map).map_err(|e| e.to_string())?;
    doc.put(&version, "present", true).map_err(|e| e.to_string())?;
    doc.put(&version, "discarded", false).map_err(|e| e.to_string())?;
    let record = doc.put_object(&version, "record", ObjType::Map).map_err(|e| e.to_string())?;
    doc.put(&record, "id", id).map_err(|e| e.to_string())?;
    doc.put(&record, "number", number).map_err(|e| e.to_string())?;
    doc.put(&record, "text", words).map_err(|e| e.to_string())?;
    let files = doc.put_object(&record, "files", ObjType::Map).map_err(|e| e.to_string())?;
    for (role, hash) in source["files"].as_object().into_iter().flatten() {
        doc.put(&files, role.as_str(), hash.as_str().ok_or("non-string media reference")?)
            .map_err(|e| e.to_string())?;
    }
    doc.commit();
    Ok(())
}
fn run(before: &Path, renamed: &Path) -> Result<Value, String> {
    let seed_bytes = fs::read(before).map_err(|e| e.to_string())?;
    let mut seed = AutoCommit::load(&seed_bytes).map_err(|e| e.to_string())?;
    let after = AutoCommit::load(&fs::read(renamed).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let initial = value(&seed);
    require(initial["schema"] == 1, "unexpected logical schema")?;
    let box_id = named(&initial, "kyogre")?;
    require(named(&value(&after), "whale")? == box_id, "rename replaced stable entity identity")?;
    let (asset_id, asset) = initial["assets"]
        .as_object()
        .into_iter()
        .flatten()
        .find(|(_, item)| item["present"] == true && item["versions"].as_object().is_some_and(|versions| !versions.is_empty()))
        .ok_or("no real CLI-created asset versions in fixture")?;
    let versions = asset["versions"].as_object().ok_or("versions are not a stable-ID map")?;
    for (id, item) in versions {
        require(item["record"]["id"] == *id, "version ID differs from its CRDT key")?;
    }
    let original: BTreeSet<String> = versions.keys().cloned().collect();
    let source = versions.values().next().ok_or("no version")?["record"].clone();
    let display = versions
        .values()
        .filter_map(|item| item["record"]["number"].as_u64())
        .max()
        .unwrap_or(0)
        + 1;
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
    new_version(&mut replicas[0], asset_id, "e2e-version-a", display, &source, "independent dawn")?;
    new_version(&mut replicas[1], asset_id, "e2e-version-b", display, &source, "independent dusk")?;
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
    let actual: BTreeSet<String> = merged["assets"][asset_id]["versions"]
        .as_object()
        .ok_or("lost version map")?
        .keys()
        .cloned()
        .collect();
    require(
        original.is_subset(&actual)
            && actual.contains("e2e-version-a")
            && actual.contains("e2e-version-b")
            && actual.len() == original.len() + 2,
        "same display number lost an immutable version",
    )?;
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
        json!({"passed":true,"native_replicas":3,"stable_rename_id":box_id,"same_number_versions_preserved":2,"original_versions":original.len(),"prompt_alternatives":alternatives,"duplicate_reordered_delivery":true,"snapshot_reload":true}),
    )
}
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
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
