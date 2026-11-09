//! The run output log's products: what a run has made so far, as it makes it (wire 65).
//! `Outputs.publish` commits the asset's bytes into the run objects' store (`cas`), binds them to the run's
//! actor and journals one `product` on the run's log, in that order, so every product a
//! client reads can be fetched with ReadByteTreeObject. A single output's product is replaced
//! by each publish (SET); a list output grows (APPEND). The returned result publishes only
//! what the log does not already show. Runtime checkpoint outputs carry immutable TensorFS
//! references only; TensorD records them without adopting or uploading their bytes.
use crate::{
    stored,
    catalog::Generation,
    device_executor::{Answer, CheckpointItem, Publish, SavedFile},
    execution::Engine,
    journal::StoredProduct,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};
use tensorfs::format::ObjectRef;
use tensorfs::sha256;

const MAX_PARTS: usize = 128;

fn storage(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

/// Retain one byte string as a one-file tree named by `owner`, bound to `actor` so a reader
/// with that actor's capability can fetch it, and to `run`, so nothing evicts it while that goes on.
/// Idempotent per owner.
pub(crate) fn retain(
    engine: &Engine,
    actor: &str,
    run: &str,
    owner: &str,
    source: &mut File,
    object: &ObjectRef,
) -> io::Result<stored::NativeByteRetentionRequest> {
    // Bytes already held (a product published while the run made it, a memoized or child
    // result) are not read again.
    let cas = engine.cas();
    if !cas.reuse(object)? {
        cas.put(source, Some(object))?;
    }
    let tree = crate::trees::put(&cas, owner, [("payload".to_string(), object.clone())].into())?;
    let native = stored::NativeByteRetentionRequest {
        source: Some(stored::NativeByteTreeRef {
            producer_root_id: tree.hold,
            receipt_digest: unhex(&tree.manifest.sha256)?,
            manifest: Some(stored::Ref {
                digest: unhex(&tree.manifest.sha256)?,
                length: tree.manifest.length,
            }),
            content_bytes: object.length,
        }),
        retention_id: owner.into(),
    };
    engine.bind_native_output(actor, owner, &crate::stored::encode(&native))?;
    // The signer's own, so a later run of it may name these bytes by their hash, with nothing written.
    engine.with_journal(|j| j.adopt_object(actor, run, object))?;
    Ok(native)
}

fn unhex(text: &str) -> io::Result<Vec<u8>> {
    if text.len() != 64 {
        return Err(storage("digest length differs"));
    }
    (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(storage))
        .collect()
}

/// A published asset's bytes: a regular file of the attempt's spool, at most one directory
/// down (a join's parts). Never a symlink or a path outside the spool.
fn spool_file(spool: &Path, name: &str) -> io::Result<PathBuf> {
    let refuse = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the published file names none in this attempt's spool",
        )
    };
    let relative = Path::new(name);
    let steps: Vec<_> = relative.components().collect();
    if steps.is_empty()
        || steps.len() > 2
        || steps.iter().any(|step| {
            !matches!(step, Component::Normal(part) if !part.to_string_lossy().starts_with('.'))
        })
    {
        return Err(refuse());
    }
    let path = spool.join(relative);
    if steps.len() == 2
        && spool
            .join(steps[0])
            .symlink_metadata()?
            .file_type()
            .is_symlink()
    {
        return Err(refuse());
    }
    let metadata = path.symlink_metadata().map_err(|_| refuse())?;
    if !metadata.file_type().is_file() {
        return Err(refuse());
    }
    Ok(path)
}

/// A file its attempt saved, as a caller passes it to a child: a regular file of its spool whose
/// bytes hash here to the digest it is named by, so only a holder of the bytes passes, whatever
/// digest it names. No tree: its manifest would name objects the caller need not hold.
pub(crate) fn saved(spool: &Path, file: &SavedFile) -> io::Result<(PathBuf, ObjectRef)> {
    if file.media_type == api::tree::MEDIA {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a tree cannot be passed as a saved file"));
    }
    let path = spool_file(spool, &spool_name(&file.asset_ref))?;
    let object = hash_file(&path)?;
    if format!("sha256:{}", object.sha256) != file.digest {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} does not hold the bytes of {}", file.asset_ref, file.digest),
        ));
    }
    Ok((path, object))
}

/// A saved asset's spool file, named as the SDK's post phase names it:
/// `attempt:<request>/<kind>/<n>` lives at `<kind>-<n>`. A file a child returned is named by
/// its digest, `sha256:<hex>`, and linked into its parent's spool as `sha256-<hex>`
/// (`calls::receive`), so a caller can publish it as it is.
fn spool_name(asset_ref: &str) -> String {
    if let Some(hex) = asset_ref
        .strip_prefix("sha256:")
        .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return format!("sha256-{hex}");
    }
    let tail: Vec<_> = asset_ref.rsplitn(3, '/').collect();
    if tail.len() == 3 {
        format!("{}-{}", tail[1], tail[0])
    } else {
        String::new()
    }
}

fn hash_file(path: &Path) -> io::Result<ObjectRef> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    let mut length = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        length += count as u64;
    }
    Ok(ObjectRef {
        sha256: sha256::hex(&hash.finalize()),
        length,
    })
}

/// An optional's present type (`T | None`, `{"union": [T, "null"]}`: `T`), else `schema` itself.
pub(crate) fn present(schema: &Value) -> &Value {
    let null = |member: &Value| member.as_str() == Some("null");
    match schema.get("union").and_then(Value::as_array).map(Vec::as_slice) {
        Some([a, b]) if null(a) != null(b) => if null(a) { b } else { a },
        _ => schema,
    }
}

/// How a declared result field takes products: a single asset is SET, a list of assets grows
/// by APPEND, optional or not. Anything else is not a publishable output.
fn output_op(result: &Value, output: &str) -> Option<stored::RunProductOp> {
    let mut schema = result;
    for name in output.split('.') {
        schema = present(schema)
            .get("fields")?
            .as_array()?
            .iter()
            .find(|field| field.get("name").and_then(Value::as_str) == Some(name))?
            .get("type")?;
    }
    let schema = present(schema);
    if schema.get("asset").is_some() {
        Some(stored::RunProductOp::Set)
    } else if schema.get("list")?.get("asset").is_some() {
        Some(stored::RunProductOp::Append)
    } else {
        None
    }
}

pub(crate) fn decode(stored: &StoredProduct) -> io::Result<stored::RunProduct> {
    crate::stored::decode(stored.product.as_slice())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "journaled product is corrupt"))
}

/// The SDK's `publish` request during a running attempt: answers `Published`.
pub(crate) fn publish(engine: &Engine, generation: &Generation, id: &str, spool: &Path, seq: u64, publish: &Publish) -> Answer {
    let mut answer = Answer::unavailable(seq);
    match commit(engine, generation, id, spool, publish) {
        Ok((content, sequence)) => {
            answer.ok = true;
            answer.code.clear();
            answer.detail.clear();
            answer.digest = format!("sha256:{}", sha256::hex(&content.digest));
            answer.length = content.length;
            answer.sequence = sequence;
        }
        Err(error) => {
            answer.code = match error.kind() {
                io::ErrorKind::InvalidInput => "output_undeclared",
                _ => "publish_refused",
            }
            .into();
            answer.detail = error.to_string();
        }
    }
    answer
}

/// The running execution's submission and its entrypoint's declaration, in the App it invokes
/// (`generation`'s own, or a callee's it holds).
fn declared(engine: &Engine, generation: &Generation, id: &str) -> io::Result<(crate::journal::SubmissionContext, Value)> {
    let record = engine.get(id)?;
    let context = record
        .submission
        .ok_or_else(|| io::Error::other("only a public execution has an output log"))?;
    let (_, _, interface) = generation
        .app(&record.invocation.module)
        .ok_or_else(|| io::Error::other("held application interface absent"))?;
    let entry = api::interface::entrypoint(interface, &record.invocation.entrypoint)
        .cloned()
        .ok_or_else(|| io::Error::other("declared entrypoint absent"))?;
    Ok((context, entry))
}

/// Record the commit the Runtime wrote for a weights output, in lane `item.output`; with a Hub
/// destination, `publish` uploads it and the output names the destination's commit instead.
#[allow(clippy::type_complexity)]
pub(crate) fn checkpoint(engine: &Engine, generation: &Generation, id: &str, seq: u64, item: &CheckpointItem,
    publish: &dyn Fn(&str) -> Result<Option<(String, tensorfs::format::ObjectRef)>, crate::objects::Refused>) -> Answer {
    let record = || -> io::Result<u64> {
        let (_, entry) = declared(engine, generation, id)?;
        if !api::interface::weights_outputs(&entry).any(|(output, _)| output == item.output) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                format!("{:?} is not a declared weights output", item.output)));
        }
        if item.repo_id.is_empty() {
            return Err(storage("a checkpoint reference needs its repository"));
        }
        crate::storage::check_digest("checkpoint revision", &item.revision, true).map_err(storage)?;
        let (repo_id, revision) = match publish(&item.revision).map_err(|refused| io::Error::other(format!("{}: {}", refused.code, refused.message)))? {
            Some((destination, commit)) => (destination, commit.id()),
            None => (item.repo_id.clone(), item.revision.clone()),
        };
        let reference = api::machine::CheckpointRef { repo_id, revision, lane: item.output.clone() };
        let product = stored::RunProduct {
            output: item.output.clone(), checkpoint: Some(reference.clone()),
            op: stored::RunProductOp::Set as i32, ..Default::default()
        };
        engine.append_product(id, |prior| {
            let prior = prior.iter().map(decode).collect::<io::Result<Vec<_>>>()?;
            let held = prior.iter().rev().find(|p| p.output == product.output)
                .is_some_and(|p| p.checkpoint.as_ref() == Some(&reference));
            Ok((!held).then(|| encode(&product)))
        })
    };
    match record() {
        Ok(sequence) => { let mut answer = Answer::ok(seq); answer.sequence = sequence; answer }
        Err(error) => crate::device_executor::refused(seq,
            if error.kind() == io::ErrorKind::InvalidInput { "output_undeclared" } else { "checkpoint_refused" },
            error.to_string()),
    }
}

fn commit(
    engine: &Engine,
    generation: &Generation,
    id: &str,
    spool: &Path,
    publish: &Publish,
) -> io::Result<(stored::Ref, u64)> {
    let (context, entry) = declared(engine, generation, id)?;
    let result = api::interface::result(&entry).ok_or_else(|| io::Error::other("declared result schema absent"))?;
    let op = output_op(result, &publish.output).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{:?} is not a file field of this function's result",
                publish.output
            ),
        )
    })?;
    if publish.parts.len() > MAX_PARTS {
        return Err(storage(format!("a product has at most {MAX_PARTS} parts")));
    }
    let retain_file = |path: &Path| -> io::Result<(stored::Ref, stored::NativeByteRetentionRequest)> {
        let object = hash_file(path)?;
        // One hold per distinct byte string of the run; TensorFS names owners by digest.
        let named = format!(
            "cozy.product/1\0{}\0{}\0{}",
            engine.workspace_id(),
            context.request_id,
            object.sha256
        );
        let owner = format!("sha256:{}", sha256::hex(&sha256::digest(named.as_bytes())));
        let source = retain(
            engine,
            &context.actor,
            id,
            &owner,
            &mut File::open(path)?,
            &object,
        )?;
        Ok((
            stored::Ref {
                digest: unhex(&object.sha256)?,
                length: object.length,
            },
            source,
        ))
    };
    let mut product = stored::RunProduct {
        output: publish.output.clone(),
        op: op as i32,
        media_type: publish.media_type.clone(),
        label: publish.label.clone(),
        ..Default::default()
    };
    if publish.parts.is_empty() {
        let path = spool_file(spool, &spool_name(&publish.asset_ref))?;
        let (content, source) = retain_file(&path)?;
        product.content = Some(content);
        product.source = Some(source);
    } else {
        let mut whole = Sha256::new();
        let mut length = 0;
        for part in &publish.parts {
            let path = spool_file(spool, &part.local)?;
            let mut file = File::open(&path)?;
            length += io::copy(&mut file, &mut whole)?;
            let (content, source) = retain_file(&path)?;
            product.parts.push(stored::RunProductPart {
                content: Some(content),
                source: Some(source),
                duration_us: part.duration_us,
            });
        }
        product.content = Some(stored::Ref {
            digest: whole.finalize().to_vec(),
            length,
        });
    }
    let content = product.content.clone().unwrap_or_default();
    let sequence = engine.append_product(id, |prior| {
        let prior = prior.iter().map(decode).collect::<io::Result<Vec<_>>>()?;
        if op == stored::RunProductOp::Append {
            let items = prior
                .iter()
                .filter(|p| p.output == product.output && p.op == op as i32)
                .count();
            product.index = items as u32;
        } else if prior
            .iter()
            .rev()
            .find(|p| p.output == product.output && p.op == op as i32)
            .is_some_and(|current| current.content == product.content && current.checkpoint == product.checkpoint)
        {
            return Ok(None); // the log already shows these bytes
        }
        Ok(Some(encode(&product)))
    })?;
    Ok((content, sequence))
}

/// The canonical body of one product event.
pub(crate) fn document(product: &stored::RunProduct) -> io::Result<Value> {
    let missing = || io::Error::new(io::ErrorKind::InvalidData, "product reference absent");
    let reference = |content: &stored::Ref| json!({"digest":format!("sha256:{}",sha256::hex(&content.digest)),"length":content.length});
    let source = |source: &stored::NativeByteRetentionRequest| -> io::Result<Value> {
        let tree = source.source.as_ref().ok_or_else(missing)?;
        let manifest = tree.manifest.as_ref().ok_or_else(missing)?;
        Ok(
            json!({"retention_id":source.retention_id,"source":{"producer_root_id":tree.producer_root_id,"receipt_digest":format!("sha256:{}",sha256::hex(&tree.receipt_digest)),"manifest":reference(manifest),"content_bytes":tree.content_bytes}}),
        )
    };
    if let Some(checkpoint) = &product.checkpoint {
        return Ok(json!({"format":"cozy.machine.product/1", "output":product.output, "op":product.op,
            "checkpoint":checkpoint}));
    }
    let mut document = json!({"format":"cozy.machine.product/1","output":product.output,"op":product.op,
        "content":reference(product.content.as_ref().ok_or_else(missing)?),"media_type":product.media_type});
    if let Some(held) = &product.source {
        document["source"] = source(held)?;
    }
    if product.index != 0 {
        document["index"] = json!(product.index);
    }
    if !product.label.is_empty() {
        document["label"] = json!(product.label);
    }
    if !product.parts.is_empty() {
        document["parts"] = product
            .parts
            .iter()
            .map(|part| {
                let mut value =
                    json!({"content":reference(part.content.as_ref().ok_or_else(missing)?)});
                if let Some(held) = &part.source {
                    value["source"] = source(held)?;
                }
                if part.duration_us != 0 {
                    value["duration_us"] = json!(part.duration_us);
                }
                Ok(value)
            })
            .collect::<io::Result<Vec<_>>>()?
            .into();
    }
    Ok(document)
}

/// Whether the log already shows a returned product: the same bytes as the last SET of that
/// output, or an APPEND at an index the log already holds.
pub(crate) fn shown(log: &[stored::RunProduct], product: &stored::RunProduct) -> bool {
    if product.op == stored::RunProductOp::Append as i32 {
        log.iter()
            .any(|p| p.output == product.output && p.op == product.op && p.index == product.index)
    } else {
        log.iter()
            .rev()
            .find(|p| p.output == product.output && p.op == product.op)
            .is_some_and(|current| current.content == product.content && current.checkpoint == product.checkpoint)
    }
}

/// The product bytes held by the execution journal.
pub(crate) fn encode(product: &stored::RunProduct) -> Vec<u8> { crate::stored::encode(product) }
