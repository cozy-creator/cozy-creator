//! Exact source cleanup debt. The only durable plan/result is the protected project receipt.
use super::*;
mod caches;
use tensorfs::format::ObjectRef;

fn accepted(receipt: &Receipt, scope: &LocalPrivateStorage) -> io::Result<Vec<Execution>> {
    let table = receipt
        .original
        .iter()
        .find(|table| table.name == "executions")
        .ok_or_else(|| db_error("migration has no execution snapshot"))?;
    let column = table.column("record")?;
    table
        .rows
        .iter()
        .map(|row| {
            let bytes = private_journal::inspect_for_migration(
                row[column]
                    .text()
                    .ok_or_else(|| db_error("legacy record is missing"))?
                    .as_bytes(),
                scope,
            )?;
            serde_json::from_slice(&bytes).map_err(db_error)
        })
        .collect()
}
fn owned_refs(receipt: &Receipt) -> io::Result<BTreeMap<String, u64>> {
    let mut objects = BTreeMap::new();
    for table in &receipt.owned {
        if let (Ok(sha), Ok(length)) = (table.column("sha256"), table.column("length")) {
            for row in &table.rows {
                if let (Some(sha), Cell::Integer(length)) = (row[sha].text(), &row[length]) {
                    if *length >= 0 {
                        objects.insert(sha.to_owned(), *length as u64);
                    }
                }
            }
        }
    }
    Ok(objects)
}
fn product_sources(
    receipt: &Receipt,
    scope: &LocalPrivateStorage,
) -> io::Result<Vec<crate::stored::NativeByteRetentionRequest>> {
    let mut sources = vec![];
    let mut add = |product: crate::stored::RunProduct| {
        sources.extend(product.source);
        for part in product.parts {
            sources.extend(part.source);
        }
    };
    for table in &receipt.owned {
        let column = match table.name.as_str() {
            "run_products" => table.column("product").ok(),
            "public_terminals" => table.column("events").ok(),
            _ => None,
        };
        let Some(column) = column else { continue };
        for row in &table.rows {
            let raw = match &row[column] {
                Cell::Text(text) => text.as_bytes(),
                Cell::Blob(bytes) => bytes.as_slice(),
                _ => continue,
            };
            let bytes = private_journal::inspect_for_migration(raw, scope)?;
            if table.name == "run_products" {
                if let Ok(product) = serde_json::from_slice(&bytes) {
                    add(product);
                }
            } else if let Ok(page) = serde_json::from_slice::<crate::stored::LogPage>(&bytes) {
                for event in page.events {
                    if let Some(product) = event.product {
                        add(product);
                    }
                }
            } else if let Ok(events) =
                serde_json::from_slice::<Vec<crate::stored::LogEvent>>(&bytes)
            {
                for event in events {
                    if let Some(product) = event.product {
                        add(product);
                    }
                }
            }
        }
    }
    Ok(sources)
}
pub(super) fn prepare(
    source: &Connection,
    receipt: &Receipt,
    scope: &LocalPrivateStorage,
    store: &crate::storage::Store,
) -> io::Result<CleanupDebt> {
    let records = accepted(receipt, scope)?;
    let mut actors = BTreeSet::new();
    let mut preparations = BTreeSet::new();
    let mut computations = BTreeSet::new();
    let mut generations = BTreeSet::new();
    let mut refs = owned_refs(receipt)?;
    let mut keys = BTreeSet::new();
    let mut owners = BTreeSet::new();
    for record in &records {
        if !record.invocation.generation.is_empty() {
            generations.insert(record.invocation.generation.clone());
        }
        if let Some(submission) = &record.submission {
            actors.insert(submission.actor.clone());
            if !submission.preparation_id.is_empty() {
                preparations.insert(submission.preparation_id.clone());
            }
            if !record.memo.computation.is_empty() {
                computations.insert(record.memo.computation.clone());
            }
        }
        if let Some(result) = &record.result {
            for artifact in &result.artifacts {
                refs.insert(artifact.sha256.clone(), artifact.length);
            }
        }
    }
    for native in product_sources(receipt, scope)? {
        if !native.retention_id.is_empty() {
            keys.insert(crate::trees::name(&native.retention_id));
            owners.insert(native.retention_id);
        }
        if let Some(manifest) = native.source.and_then(|source| source.manifest) {
            let object = ObjectRef {
                sha256: tensorfs::sha256::hex(&manifest.digest),
                length: manifest.length,
            };
            refs.insert(object.sha256.clone(), object.length);
            if let Some(children) = crate::trees::roots(store, &object) {
                for child in children {
                    refs.insert(child.sha256, child.length);
                }
            }
        }
    }
    let mut debt = CleanupDebt {
        generations: generations.into_iter().collect(),
        prepared: true,
        ..Default::default()
    };
    for actor in actors {
        for (name, key, selected) in [
            ("preparations", "id", &preparations),
            ("held_memos", "computation", &computations),
            ("held_memo_objects", "computation", &computations),
            ("native_outputs", "owner", &owners),
        ] {
            let mut table = Table::read(source, name, "actor=?1", &actor)?;
            let column = table.column(key)?;
            table
                .rows
                .retain(|row| row[column].text().is_some_and(|key| selected.contains(key)));
            if table.name == "held_memo_objects" {
                let sha = table.column("sha256")?;
                for row in &table.rows {
                    let sha = row[sha]
                        .text()
                        .ok_or_else(|| db_error("memo object key is invalid"))?;
                    if !refs.contains_key(sha) {
                        let object = ObjectRef {
                            sha256: sha.to_owned(),
                            length: 0,
                        };
                        let path = store.path(&object).map_err(crate::storage::io_error)?;
                        let length = fs::metadata(path)?.len();
                        refs.insert(sha.to_owned(), length);
                    }
                }
            }
            if !table.rows.is_empty() {
                debt.metadata.push(table);
            }
        }
        let mut objects = Table::read(source, "objects", "actor=?1", &actor)?;
        let sha = objects.column("sha256")?;
        objects
            .rows
            .retain(|row| row[sha].text().is_some_and(|sha| refs.contains_key(sha)));
        if !objects.rows.is_empty() {
            debt.metadata.push(objects);
        }
        let mut installations = Table::read(source, "installations", "actor=?1", &actor)?;
        let record = installations.column("record")?;
        installations.rows.retain(|row| {
            row[record]
                .text()
                .and_then(|body| serde_json::from_str::<Installation>(body).ok())
                .is_some_and(|installation| debt.generations.contains(&installation.generation))
        });
        let alias = installations.column("alias")?;
        let aliases = installations
            .rows
            .iter()
            .filter_map(|row| row[alias].text())
            .collect::<BTreeSet<_>>();
        let mut warm = Table::read(source, "warm_sets", "actor=?1", &actor)?;
        let record = warm.column("record")?;
        warm.rows.retain(|row| {
            row[record]
                .text()
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                .and_then(|value| value["installation"].as_str().map(str::to_owned))
                .is_some_and(|name| aliases.contains(name.as_str()))
        });
        if !warm.rows.is_empty() {
            debt.metadata.push(warm);
        }
        if !installations.rows.is_empty() {
            debt.metadata.push(installations);
        }
        let mut resolutions = Table::read(source, "resolutions", "actor=?1", &actor)?;
        let preparation = resolutions.column("preparation")?;
        resolutions.rows.retain(|row| {
            row[preparation]
                .text()
                .is_some_and(|id| preparations.contains(id))
        });
        if !resolutions.rows.is_empty() {
            debt.metadata.push(resolutions);
        }
    }
    // Preserve the exact unscoped usage hints too; they are retired only after no
    // authoritative source owner remains, and become project-scoped rows at the target.
    let mut hints = Table::read(source, "object_uses", "?1 IS NOT NULL", "all")?;
    let sha = hints.column("sha256")?;
    hints
        .rows
        .retain(|row| row[sha].text().is_some_and(|sha| refs.contains_key(sha)));
    if !hints.rows.is_empty() {
        debt.metadata.push(hints);
    }
    // Header/tree candidates retain their complete native closure, not merely the document.
    let roots = refs
        .iter()
        .map(|(sha256, length)| ObjectRef {
            sha256: sha256.clone(),
            length: *length,
        })
        .collect::<Vec<_>>();
    for root in roots {
        for object in store.closure(&root).map_err(crate::storage::io_error)? {
            refs.insert(object.sha256, object.length);
        }
    }
    for sha in refs.keys() {
        keys.insert(sha.clone());
    }
    for key in keys {
        let path = store.root().join("uses").join(&key);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let text = std::str::from_utf8(&bytes).map_err(db_error)?;
                if text.lines().all(|sha| refs.contains_key(sha)) {
                    debt.uses.insert(key, tensorfs::sha256::hex_digest(&bytes));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    debt.objects = refs
        .into_iter()
        .map(|(sha256, length)| ObjectRef { sha256, length })
        .collect();
    Ok(debt)
}

pub(super) fn copy_metadata(
    target: &Connection,
    receipt: &mut Receipt,
    scope: &LocalPrivateStorage,
) -> io::Result<()> {
    for source in &receipt.cleanup.metadata {
        let mut table = source.clone();
        if table.name == "object_uses" {
            table.name = "private_object_uses".into();
            table.columns.insert(0, "project".into());
            for row in &mut table.rows {
                row.insert(0, Cell::Text(scope.project_id.clone()));
            }
            table.insert(target)?;
            receipt.owned.push(table);
            continue;
        }
        let columns = table.columns.clone();
        let column = |name: &str| {
            columns
                .iter()
                .position(|column| column == name)
                .ok_or_else(|| db_error("cleanup schema column is missing"))
        };
        for row in &mut table.rows {
            let actor_at = column("actor")?;
            let actor = row[actor_at]
                .text()
                .ok_or_else(|| db_error("cleanup actor is invalid"))?
                .to_owned();
            let private_actor = crate::private_io::resource_key(&actor);
            let context = match table.name.as_str() {
                "preparations" => Some((
                    "record",
                    format!(
                        "preparation:{actor}:{}",
                        row[column("id")?]
                            .text()
                            .ok_or_else(|| db_error("preparation identity is invalid"))?
                    ),
                )),
                "held_memos" => Some((
                    "record",
                    format!(
                        "memo:{private_actor}:{}",
                        row[column("computation")?]
                            .text()
                            .ok_or_else(|| db_error("memo identity is invalid"))?
                    ),
                )),
                "native_outputs" => {
                    let owner_at = column("owner")?;
                    let owner = row[owner_at]
                        .text()
                        .ok_or_else(|| db_error("native owner is invalid"))?
                        .to_owned();
                    row[owner_at] = Cell::Text(crate::private_io::resource_key(&owner));
                    Some((
                        "source",
                        format!(
                            "native:{private_actor}:{}",
                            crate::private_io::resource_key(&owner)
                        ),
                    ))
                }
                "warm_sets" => Some((
                    "record",
                    format!(
                        "warm:{actor}:{}",
                        match row[column("position")?] {
                            Cell::Integer(n) => n,
                            _ => return Err(db_error("invalid warm position")),
                        }
                    ),
                )),
                "resolutions" => Some((
                    "package",
                    format!(
                        "resolution:{actor}:{}:package",
                        row[column("key")?]
                            .text()
                            .ok_or_else(|| db_error("resolution key is invalid"))?
                    ),
                )),
                _ => None,
            };
            let scope_actor = table.name != "installations"
                || row[column("record")?]
                    .text()
                    .and_then(|text| serde_json::from_str::<Installation>(text).ok())
                    .is_some_and(|installation| installation.package.starts_with("local/"));
            if scope_actor {
                row[actor_at] = Cell::Text(private_actor);
            }
            if let Some((payload_column, context)) = context {
                let at = column(payload_column)?;
                let bytes = match &row[at] {
                    Cell::Text(text) => text.as_bytes(),
                    Cell::Blob(bytes) => bytes.as_slice(),
                    _ => return Err(db_error("cleanup payload type is invalid")),
                };
                let protected = private_journal::write(Some(scope), &context, bytes)?;
                row[at] = if table.name == "warm_sets" {
                    let warm: Value = serde_json::from_slice(bytes).map_err(db_error)?;
                    Cell::Text(serde_json::json!({"installation":warm["installation"],"_private_payload":String::from_utf8(protected).map_err(db_error)?}).to_string())
                } else if table.name == "preparations" {
                    let preparation: Preparation =
                        serde_json::from_slice(bytes).map_err(db_error)?;
                    Cell::Text(serde_json::json!({"installation":preparation.installation,"_private_payload":String::from_utf8(protected).map_err(db_error)?}).to_string())
                } else if matches!(row[at], Cell::Text(_)) {
                    Cell::Text(String::from_utf8(protected).map_err(db_error)?)
                } else {
                    Cell::Blob(protected)
                };
            }
        }
        // Reuse a semantically identical existing binding instead of manufacturing a new
        // reference filename; a different canonical value is an actual conflict.
        let keys = match table.name.as_str() {
            "objects" => vec!["actor", "sha256"],
            "warm_sets" => vec!["actor", "position"],
            "preparations" => vec!["actor", "id"],
            "held_memos" => vec!["actor", "computation"],
            "held_memo_objects" => vec!["actor", "computation", "sha256"],
            "native_outputs" => vec!["actor", "owner"],
            "resolutions" => vec!["actor", "key"],
            "installations" => vec!["actor", "alias"],
            _ => return Err(db_error("unrecognized related cleanup table")),
        };
        for row in &mut table.rows {
            let positions = keys
                .iter()
                .map(|key| column(key))
                .collect::<io::Result<Vec<_>>>()?;
            let where_sql = keys
                .iter()
                .enumerate()
                .map(|(i, key)| format!("{key} IS ?{}", i + 1))
                .collect::<Vec<_>>()
                .join(" AND ");
            let values = positions
                .iter()
                .map(|at| row[*at].sql())
                .collect::<Vec<_>>();
            let mut query = target
                .prepare(&format!("SELECT * FROM {} WHERE {where_sql}", table.name))
                .map_err(db_error)?;
            let mut rows = query
                .query(rusqlite::params_from_iter(values.iter()))
                .map_err(db_error)?;
            if let Some(existing) = rows.next().map_err(db_error)? {
                let existing = (0..table.columns.len())
                    .map(|at| existing.get_ref(at).map_err(db_error).and_then(Cell::read))
                    .collect::<io::Result<Vec<_>>>()?;
                for (left, right) in existing.iter().zip(row.iter()) {
                    let plain = |value: &Cell| -> io::Result<Vec<u8>> {
                        match value {
                            Cell::Text(text) => {
                                private_journal::inspect_for_migration(text.as_bytes(), scope)
                            }
                            Cell::Blob(bytes) => {
                                private_journal::inspect_for_migration(bytes, scope)
                            }
                            _ => serde_json::to_vec(value).map_err(db_error),
                        }
                    };
                    if plain(left)? != plain(right)? {
                        return Err(db_error(
                            "project related binding conflicts with canonical legacy data",
                        ));
                    }
                }
                *row = existing;
            }
        }
        table.insert(target)?;
        receipt.owned.push(table);
    }
    Ok(())
}

/// All other source records remain authoritative even when their project is not mounted.
fn other_roots(source: &Connection) -> io::Result<Vec<ObjectRef>> {
    let mut statement=source.prepare("SELECT DISTINCT sha256,length FROM run_objects UNION SELECT sha256,length FROM objects UNION SELECT sha256,length FROM private_object_uses").map_err(db_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(ObjectRef {
                sha256: row.get(0)?,
                length: row.get(1)?,
            })
        })
        .map_err(db_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
}
fn referenced(source: &Connection, table: &Table, row: &[Cell]) -> io::Result<bool> {
    let text = |name: &str| -> io::Result<&str> {
        row[table.column(name)?]
            .text()
            .ok_or_else(|| db_error("legacy cleanup key is invalid"))
    };
    let exists = |sql: &str, parameters: &[&dyn rusqlite::ToSql]| -> io::Result<bool> {
        source
            .query_row(sql, parameters, |row| row.get(0))
            .map_err(db_error)
    };
    match table.name.as_str() {
        "objects" => exists(
            "SELECT EXISTS(SELECT 1 FROM run_objects WHERE sha256=?1)",
            &[&text("sha256")?],
        ),
        "object_uses" => exists(
            "SELECT EXISTS(SELECT 1 FROM run_objects WHERE sha256=?1 UNION SELECT 1 FROM objects WHERE sha256=?1 UNION SELECT 1 FROM private_object_uses WHERE sha256=?1)",
            &[&text("sha256")?],
        ),
        "warm_sets" => {
            let warm: Value = serde_json::from_str(text("record")?).map_err(db_error)?;
            let alias = warm["installation"]
                .as_str()
                .ok_or_else(|| db_error("invalid warm installation"))?;
            exists(
                "SELECT EXISTS(SELECT 1 FROM executions WHERE json_extract(invocation,'$.generation') IN (SELECT json_extract(record,'$.generation') FROM installations WHERE actor=?1 AND alias=?2))",
                &[&text("actor")?, &alias],
            )
        }
        "preparations" | "held_memos" | "held_memo_objects" => {
            let key = if table.name == "preparations" {
                "id"
            } else {
                "computation"
            };
            let value = text(key)?;
            exists(
                "SELECT EXISTS(SELECT 1 FROM executions WHERE json_extract(record,'$.submission.preparation_id')=?1 OR json_extract(record,'$.memo.computation')=?1)",
                &[&value],
            )
        }
        "resolutions" => exists(
            "SELECT EXISTS(SELECT 1 FROM executions WHERE json_extract(record,'$.submission.preparation_id')=?1)",
            &[&text("preparation")?],
        ),
        "installations" => {
            let installation: Installation =
                serde_json::from_str(text("record")?).map_err(db_error)?;
            if !installation.package.starts_with("local/") {
                return Ok(true);
            }
            exists(
                "SELECT EXISTS(SELECT 1 FROM executions WHERE json_extract(invocation,'$.generation')=?1 UNION SELECT 1 FROM warm_sets WHERE actor=?2 AND json_extract(record,'$.installation')=?3)",
                &[
                    &installation.generation,
                    &installation.actor,
                    &installation.alias,
                ],
            )
        }
        "native_outputs" => {
            let native: crate::stored::NativeByteRetentionRequest =
                serde_json::from_slice(match &row[table.column("source")?] {
                    Cell::Blob(bytes) => bytes.as_slice(),
                    Cell::Text(text) => text.as_bytes(),
                    _ => return Err(db_error("native retention has an invalid type")),
                })
                .map_err(db_error)?;
            let Some(manifest) = native.source.and_then(|source| source.manifest) else {
                return Ok(true);
            };
            let sha = tensorfs::sha256::hex(&manifest.digest);
            exists(
                "SELECT EXISTS(SELECT 1 FROM run_objects WHERE sha256=?1)",
                &[&sha],
            )
        }
        _ => Ok(true),
    }
}
fn same_key_exists(source: &Connection, table: &Table, row: &[Cell]) -> io::Result<bool> {
    let mut schema = source
        .prepare(&format!("PRAGMA table_info({})", table.name))
        .map_err(db_error)?;
    let keys = schema
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    let keys = keys
        .into_iter()
        .filter(|(_, pk)| *pk > 0)
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    if keys.is_empty() {
        return Err(db_error("cleanup table has no canonical key"));
    }
    let predicate = keys
        .iter()
        .enumerate()
        .map(|(i, key)| format!("{key} IS ?{}", i + 1))
        .collect::<Vec<_>>()
        .join(" AND ");
    let values = keys
        .iter()
        .map(|key| table.column(key).map(|at| row[at].sql()))
        .collect::<io::Result<Vec<_>>>()?;
    source
        .query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM {} WHERE {predicate})",
                table.name
            ),
            rusqlite::params_from_iter(values.iter()),
            |row| row.get(0),
        )
        .map_err(db_error)
}

fn verify_object(store: &crate::storage::Store, object: &ObjectRef) -> io::Result<()> {
    use sha2::Digest as _;
    use std::io::Read as _;
    let mut file = store.file(object).map_err(crate::storage::io_error)?;
    let mut hash = sha2::Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    if tensorfs::sha256::hex(&hash.finalize()) != object.sha256 {
        return Err(db_error(
            "protected cleanup object differs from its verified digest",
        ));
    }
    file.sync_all()?;
    Ok(())
}

pub(super) fn retire_metadata(
    source: &Connection,
    target: &Connection,
    receipt: &mut Receipt,
    scope: &LocalPrivateStorage,
    engine: &Path,
    store: &crate::storage::Store,
) -> io::Result<()> {
    if !receipt.cleanup.prepared {
        receipt.cleanup = prepare(source, receipt, scope, store)?;
        save(target, receipt)?;
        checkpoint(target)?;
    }
    let mut keep = vec![];
    let mut pending = vec![];
    for table in &receipt.cleanup.metadata {
        table.validate_schema(source)?;
        for row in &table.rows {
            if referenced(source, table, row)? {
                continue;
            }
            let exact = Table {
                name: table.name.clone(),
                columns: table.columns.clone(),
                rows: vec![row.clone()],
            };
            let present = exact.verify(source).is_ok();
            if present {
                exact.retire(source)?;
            } else if same_key_exists(source, table, row)? {
                pending.push(format!("legacy_metadata_changed:{}", table.name));
            }
        }
    }
    // Raw account grants/use hints are now retired only when no source request needs them.
    keep.extend(other_roots(source)?);
    let private = crate::private_io::store(scope)?;
    for object in &receipt.cleanup.objects {
        if !private.holds(object) {
            let file = match store.file(object) {
                Ok(file) => file,
                Err(_) => {
                    pending.push(format!("legacy_cleanup_payload_missing:{}", object.sha256));
                    continue;
                }
            };
            crate::private_io::retain(scope, std::slice::from_ref(object))?;
            private
                .put(file, Some(object))
                .map_err(crate::storage::io_error)?;
        }
        verify_object(&private, object)?;
    }
    let mut uses = vec![];
    for (name, digest) in &receipt.cleanup.uses {
        match std::fs::read(store.root().join("uses").join(name)) {
            Ok(bytes) if tensorfs::sha256::hex_digest(&bytes) == *digest => uses.push(name.clone()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            _ => pending.push(format!("legacy_use_changed:{name}")),
        }
    }
    if pending.is_empty() {
        let result = store
            .retire_objects(&receipt.cleanup.objects, &uses, &keep)
            .map_err(crate::storage::io_error)?;
        let mut independent = BTreeSet::new();
        for root in &keep {
            independent.extend(
                store
                    .closure(root)
                    .map_err(crate::storage::io_error)?
                    .into_iter()
                    .map(|object| object.sha256),
            );
        }
        for object in result.retained {
            if !independent.contains(&object.sha256) {
                pending.push(format!(
                    "legacy_retention_reader_or_independent_root:{}",
                    object.sha256
                ));
            }
        }
    }
    pending.extend(caches::retire(source, target, receipt, scope, engine)?);
    receipt.cleanup.pending = pending;
    save(target, receipt)?;
    checkpoint(target)?;
    // Retiring derived source rows creates SQLite pages too; finish with secure compaction.
    source
        .execute_batch("PRAGMA secure_delete=ON; VACUUM main")
        .map_err(db_error)?;
    checkpoint(source)?;
    Ok(())
}
