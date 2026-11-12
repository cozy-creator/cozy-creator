use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tensorfs::format::{Header, ObjectRef, Tree};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Deserialize)]
struct Inputs {
    inputs: Vec<Input>,
    #[serde(default)]
    processor_source: Value,
}

#[derive(Deserialize)]
struct Input {
    model: String,
    release: String,
    lane: String,
    old_checkpoint: String,
    old_tree: PathBuf,
    old_tree_length: u64,
    old_header: PathBuf,
    old_header_ref: ObjectRef,
}

#[derive(Deserialize)]
struct Normalization {
    source: PathBuf,
    source_sha256: String,
    config: Value,
}

#[derive(Serialize)]
struct Asset {
    name: String,
    object: ObjectRef,
}

fn check(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into().into())
    }
}

fn read_verified(path: &Path, expected: &ObjectRef) -> Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    check(
        ObjectRef::of(&bytes) == *expected,
        format!(
            "{} does not match its recorded digest and length",
            path.display()
        ),
    )?;
    Ok(bytes)
}

fn gather_assets(root: &Path, path: &Path, output: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        check(
            !kind.is_symlink(),
            "processor snapshot must contain regular files, not symlinks",
        )?;
        if kind.is_dir() {
            gather_assets(root, &entry.path(), output)?;
        } else {
            check(
                kind.is_file(),
                "processor snapshot contains a non-file entry",
            )?;
            let relative = entry.path().strip_prefix(root)?.to_path_buf();
            let name = relative
                .to_str()
                .ok_or("processor file name is not UTF-8")?
                .replace('\\', "/");
            output.insert(format!("processor/{name}"), fs::read(entry.path())?);
        }
    }
    Ok(())
}

fn normalization(path: &Path) -> Result<Value> {
    let proposal: Normalization = serde_json::from_slice(&fs::read(path)?)?;
    let source = fs::read(&proposal.source)?;
    check(
        ObjectRef::of(&source).sha256 == proposal.source_sha256,
        "normalization source digest changed",
    )?;
    let source: Value = serde_json::from_slice(&source)?;
    let value = proposal
        .config
        .get("latent_normalization")
        .ok_or("proposal has no latent_normalization")?;
    let source = source
        .get("latent_normalization")
        .ok_or("source has no latent_normalization")?;
    for field in ["enabled", "mean", "std"] {
        check(
            source.get(field) == value.get(field),
            format!("proposed normalization {field} differs from its package source"),
        )?;
    }
    check(
        value.get("enabled").and_then(Value::as_bool) == Some(true),
        "normalization must explicitly be enabled",
    )?;
    for (name, positive) in [("mean", false), ("std", true)] {
        let values = value
            .get(name)
            .and_then(Value::as_array)
            .ok_or("normalization needs mean and std arrays")?;
        check(
            values.len() == 24,
            format!("normalization {name} needs 24 channels"),
        )?;
        check(
            values.iter().all(|v| {
                v.as_f64()
                    .is_some_and(|v| v.is_finite() && (!positive || v > 0.0))
            }),
            format!("normalization {name} contains an invalid value"),
        )?;
    }
    Ok(value.clone())
}

fn closure(header: &Header) -> BTreeSet<ObjectRef> {
    header.objects().cloned().collect()
}

fn rebuilt(header: &Header, components: Vec<tensorfs::format::Component>) -> Result<Header> {
    Ok(Header::new(
        header.chunks.clone(),
        header.pipeline().cloned(),
        components,
        header.metadata.clone(),
    )?)
}

fn record_object(objects: &mut BTreeMap<String, Vec<u8>>, bytes: Vec<u8>) -> Result<ObjectRef> {
    let object = ObjectRef::of(&bytes);
    if let Some(previous) = objects.get(&object.sha256) {
        check(
            previous == &bytes,
            "one digest names conflicting prepared bytes",
        )?;
    } else {
        objects.insert(object.sha256.clone(), bytes);
    }
    Ok(object)
}

fn prepare(
    input: &Input,
    assets: &BTreeMap<String, Vec<u8>>,
    norm: &Value,
    objects: &mut BTreeMap<String, Vec<u8>>,
) -> Result<Value> {
    let old_header_bytes = read_verified(&input.old_header, &input.old_header_ref)?;
    let old_header = Header::from_cbor(&old_header_bytes)?;
    check(
        old_header.to_cbor()? == old_header_bytes,
        "old header does not roundtrip byte-for-byte through the native encoder",
    )?;
    let old_tree_ref = ObjectRef {
        sha256: input
            .old_checkpoint
            .strip_prefix("sha256:")
            .ok_or("checkpoint lacks sha256 prefix")?
            .into(),
        length: input.old_tree_length,
    };
    let old_tree_bytes = read_verified(&input.old_tree, &old_tree_ref)?;
    let old_tree = Tree::from_json(&old_tree_bytes)?;
    check(
        old_tree.to_json()? == old_tree_bytes,
        "old tree does not roundtrip byte-for-byte through the native encoder",
    )?;
    check(
        old_tree.lane == input.lane,
        "tree lane differs from the reviewed lane",
    )?;
    let (header_path, header_ref) = old_tree.header()?;
    check(
        *header_ref == input.old_header_ref,
        "old tree names a different header",
    )?;
    let header_path = header_path.to_owned();
    let mut components = old_header.components().to_vec();
    let mut added = BTreeSet::new();
    let next_release = match input.model.as_str() {
        "minimaxai/minimax-h3" => {
            check(
                input.release == "1.0.0-rc.7",
                "H3 source version is not the reviewed rc.7",
            )?;
            check(
                components.len() == 5,
                "H3 must keep exactly its existing five components",
            )?;
            let encoder = components
                .iter_mut()
                .find(|component| component.name == "text_encoder")
                .ok_or("H3 has no text_encoder component")?;
            for (name, bytes) in assets {
                check(
                    !encoder.assets.contains_key(name),
                    format!("H3 already has asset {name}; refusing replacement"),
                )?;
                let object = record_object(objects, bytes.clone())?;
                encoder.assets.insert(name.clone(), object.clone());
                added.insert(object);
            }
            "1.0.0-rc.8"
        }
        "lbh-123-ai/h3-latent-upscaler" => {
            check(
                input.release == "0.1.0" && input.lane == "bf16",
                "upscaler source differs from reviewed 0.1.0/bf16",
            )?;
            check(
                components.len() == 2,
                "upscaler must keep its existing two components",
            )?;
            let model = components
                .iter_mut()
                .find(|component| component.name == "model")
                .ok_or("upscaler has no root model config")?;
            check(
                model.config.get("architecture").and_then(Value::as_str)
                    == Some("minimax-h3-latent-upscaler-3d-conv-v1"),
                "upscaler architecture changed",
            )?;
            check(
                !model.config.contains_key("latent_normalization"),
                "upscaler already has normalization; refusing replacement",
            )?;
            model
                .config
                .insert("latent_normalization".into(), norm.clone());
            "0.1.1"
        }
        _ => return Err("this client only prepares the two reviewed H3 models".into()),
    };
    let new_header = rebuilt(&old_header, components)?;
    let new_header_bytes = new_header.to_cbor()?;
    check(
        Header::from_cbor(&new_header_bytes)? == new_header,
        "new native header roundtrip changed model data",
    )?;
    check(
        Header::from_cbor(&new_header_bytes)?.to_cbor()? == new_header_bytes,
        "new header encoding is not deterministic",
    )?;

    // Undo only the intended additions. The native bytes must be the exact old header.
    let mut restored = new_header.components().to_vec();
    match input.model.as_str() {
        "minimaxai/minimax-h3" => {
            let encoder = restored
                .iter_mut()
                .find(|component| component.name == "text_encoder")
                .unwrap();
            for name in assets.keys() {
                encoder.assets.remove(name);
            }
        }
        _ => {
            restored
                .iter_mut()
                .find(|component| component.name == "model")
                .unwrap()
                .config
                .remove("latent_normalization");
        }
    }
    check(
        rebuilt(&new_header, restored)?.to_cbor()? == old_header_bytes,
        "repair changed existing chunks, tensors, config, plans, metadata or order",
    )?;
    let old_closure = closure(&old_header);
    let new_closure = closure(&new_header);
    check(
        new_closure == old_closure.union(&added).cloned().collect(),
        "header closure differs beyond the reviewed added assets",
    )?;
    let new_header_ref = record_object(objects, new_header_bytes)?;
    let mut new_tree = old_tree.clone();
    new_tree
        .files
        .insert(header_path.clone(), new_header_ref.clone());
    let new_tree_bytes = new_tree.to_json()?;
    check(
        Tree::from_json(&new_tree_bytes)? == new_tree,
        "new native tree roundtrip changed file references",
    )?;
    let mut restored_tree = new_tree.clone();
    restored_tree
        .files
        .insert(header_path, input.old_header_ref.clone());
    check(
        restored_tree.to_json()? == old_tree_bytes,
        "repair changed a supporting file or tree property",
    )?;
    let new_tree_ref = record_object(objects, new_tree_bytes)?;
    let ancillary: BTreeMap<_, _> = old_tree
        .files
        .iter()
        .filter(|(name, _)| !tensorfs::format::is_header(name))
        .map(|(name, object)| (name.to_owned(), object.clone()))
        .collect();
    Ok(json!({
        "model": input.model, "old_release": input.release, "proposed_release": next_release, "lane": input.lane,
        "old_checkpoint": old_tree_ref, "new_checkpoint": new_tree_ref,
        "old_header": input.old_header_ref, "new_header": new_header_ref,
        "component_count": old_header.components().len(), "tensor_count": old_header.components().iter().map(|component| component.tensors.len()).sum::<usize>(),
        "existing_chunk_count": old_header.chunks.len(), "old_distinct_closure_objects": old_closure.len(),
        "new_distinct_closure_objects": new_closure.len(), "new_asset_objects": added,
        "supporting_files_preserved": ancillary,
        "original_header_reproduced_after_removing_additions": true,
        "original_tree_reproduced_after_restoring_header_ref": true
    }))
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    check(
        args.len() == 4,
        "usage: xs170-artifact-reissue INPUTS_JSON PROCESSOR_DIR NORMALIZATION_JSON NEW_OUTPUT_DIR",
    )?;
    let inputs: Inputs = serde_json::from_slice(&fs::read(&args[0])?)?;
    check(
        !args[3].exists(),
        "output path already exists; use a new directory",
    )?;
    let expected: BTreeSet<_> = ["bf16", "bf16-full", "fp8", "int8", "nvfp4"]
        .into_iter()
        .collect();
    let actual: BTreeSet<_> = inputs
        .inputs
        .iter()
        .filter(|input| input.model == "minimaxai/minimax-h3")
        .map(|input| input.lane.as_str())
        .collect();
    check(
        inputs.inputs.len() == 6
            && actual == expected
            && inputs
                .inputs
                .iter()
                .filter(|input| input.model == "lbh-123-ai/h3-latent-upscaler")
                .count()
                == 1,
        "inventory must contain the five reviewed H3 lanes and one upscaler checkpoint",
    )?;
    let mut assets = BTreeMap::new();
    gather_assets(&args[1], &args[1], &mut assets)?;
    check(
        assets.len() == 5,
        "reviewed processor snapshot must contain exactly five files",
    )?;
    let normalization = normalization(&args[2])?;
    let mut objects = BTreeMap::new();
    let mappings: Vec<_> = inputs
        .inputs
        .iter()
        .map(|input| prepare(input, &assets, &normalization, &mut objects))
        .collect::<Result<_>>()?;
    let assets: Vec<_> = assets
        .iter()
        .map(|(name, bytes)| Asset {
            name: name.clone(),
            object: ObjectRef::of(bytes),
        })
        .collect();
    let total_new_bytes: usize = objects.values().map(Vec::len).sum();
    fs::create_dir_all(args[3].join("objects"))?;
    let mut written = Vec::new();
    for (sha256, bytes) in &objects {
        let relative = PathBuf::from("objects").join(sha256);
        fs::write(args[3].join(&relative), bytes)?;
        written.push(json!({"object": ObjectRef::of(bytes), "path": relative}));
    }
    let manifest = json!({
        "format": "xs170.required-data-reissue/1", "prepared_only": true,
        "weights_downloaded": 0, "production_mutations": 0, "portable_files_materialized": false,
        "native_dependency_source": "fetched origin/rc/0.24.0, recorded by the source receipt",
        "processor_source": inputs.processor_source, "normalization_proposal": args[2],
        "processor_assets": assets, "mappings": mappings,
        "new_unique_objects": written, "new_unique_object_count": objects.len(), "total_new_bytes": total_new_bytes
    });
    fs::write(
        args[3].join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    println!(
        "Prepared {} checkpoint mappings, {} unique new objects, {} bytes; no publication",
        mappings.len(),
        objects.len(),
        total_new_bytes
    );
    Ok(())
}
