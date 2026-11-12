//! Positive-ownership migration of the legacy field-encrypted journal. The only durable
//! receipt/snapshot is inside the verified project. Global retirement follows a durable copy.
use super::project_inventory::{self, InventoryReport, LegacyFootprint};
use super::*;
use api::machine::LocalPrivateStorage;
use rusqlite::types::{Value as SqlValue, ValueRef};
use std::collections::{BTreeMap, BTreeSet};

const RECEIPTS: &str = "CREATE TABLE IF NOT EXISTS project_custody_migrations(source_workspace TEXT NOT NULL,digest TEXT NOT NULL,receipt BLOB NOT NULL,PRIMARY KEY(source_workspace,digest))";
const EXECUTION_TABLES: &[&str] = &[
    "public_terminals",
    "run_products",
    "run_measurements",
    "run_logs",
    "attention_applied",
    "run_log_lengths",
    "package_log_dropped",
    "run_calls",
    "triage",
    "run_objects",
    "run_specs",
    "root_times",
];
const ACTOR_TABLES: &[&str] = &[
    "update_run_ids",
    "installations",
    "preparations",
    "warm_sets",
    "native_outputs",
    "resolutions",
    "objects",
    "held_memos",
    "held_memo_objects",
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(String),
    Blob(Vec<u8>),
}
impl Cell {
    fn read(value: ValueRef<'_>) -> io::Result<Self> {
        Ok(match value {
            ValueRef::Null => Self::Null,
            ValueRef::Integer(n) => Self::Integer(n),
            ValueRef::Real(n) => Self::Real(n.to_bits()),
            ValueRef::Text(s) => Self::Text(String::from_utf8(s.to_vec()).map_err(db_error)?),
            ValueRef::Blob(b) => Self::Blob(b.to_vec()),
        })
    }
    fn sql(&self) -> SqlValue {
        match self {
            Self::Null => SqlValue::Null,
            Self::Integer(n) => SqlValue::Integer(*n),
            Self::Real(n) => SqlValue::Real(f64::from_bits(*n)),
            Self::Text(s) => SqlValue::Text(s.clone()),
            Self::Blob(b) => SqlValue::Blob(b.clone()),
        }
    }
    fn text(&self) -> Option<&str> {
        if let Self::Text(s) = self {
            Some(s)
        } else {
            None
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Table {
    name: String,
    columns: Vec<String>,
    rows: Vec<Vec<Cell>>,
}
impl Table {
    fn column(&self, name: &str) -> io::Result<usize> {
        self.columns
            .iter()
            .position(|column| column == name)
            .ok_or_else(|| db_error("legacy journal schema is incomplete"))
    }
    fn read(conn: &Connection, name: &str, clause: &str, parameter: &str) -> io::Result<Self> {
        let mut statement = conn
            .prepare(&format!("SELECT * FROM {name} WHERE {clause}"))
            .map_err(db_error)?;
        let columns = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut read = statement.query([parameter]).map_err(db_error)?;
        let mut rows = vec![];
        while let Some(row) = read.next().map_err(db_error)? {
            rows.push(
                (0..columns.len())
                    .map(|i| row.get_ref(i).map_err(db_error).and_then(Cell::read))
                    .collect::<io::Result<Vec<_>>>()?,
            );
        }
        // Sort in memory: no SQLite temporary sort can replicate private metadata outside its view.
        rows.sort_by_cached_key(|row| serde_json::to_vec(row).unwrap_or_default());
        Ok(Self {
            name: name.into(),
            columns,
            rows,
        })
    }
    fn validate_schema(&self, conn: &Connection) -> io::Result<()> {
        if self.name != "executions"
            && self.name != "held_names"
            && self.name != "private_object_uses"
            && !EXECUTION_TABLES.contains(&self.name.as_str())
            && !ACTOR_TABLES.contains(&self.name.as_str())
        {
            return Err(db_error("unrecognized migration table"));
        }
        let statement = conn
            .prepare(&format!("SELECT * FROM {} LIMIT 0", self.name))
            .map_err(db_error)?;
        let expected = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if self.columns != expected || self.rows.iter().any(|row| row.len() != expected.len()) {
            return Err(db_error("migration columns differ from the owned schema"));
        }
        Ok(())
    }
    fn predicate(&self) -> String {
        self.columns
            .iter()
            .enumerate()
            .map(|(i, column)| format!("{column} IS ?{}", i + 1))
            .collect::<Vec<_>>()
            .join(" AND ")
    }
    fn verify(&self, conn: &Connection) -> io::Result<()> {
        self.validate_schema(conn)?;
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE {}",
            self.name,
            self.predicate()
        );
        for row in &self.rows {
            let values = row.iter().map(Cell::sql).collect::<Vec<_>>();
            let count: i64 = conn
                .query_row(&sql, rusqlite::params_from_iter(values.iter()), |row| {
                    row.get(0)
                })
                .map_err(db_error)?;
            if count != 1 {
                return Err(db_error(
                    "project migration target differs from its durable receipt",
                ));
            }
        }
        Ok(())
    }
    fn insert(&self, conn: &Connection) -> io::Result<()> {
        self.validate_schema(conn)?;
        let parameters = (1..=self.columns.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "INSERT OR IGNORE INTO {}({}) VALUES({parameters})",
            self.name,
            self.columns.join(",")
        );
        for row in &self.rows {
            let values = row.iter().map(Cell::sql).collect::<Vec<_>>();
            conn.execute(&sql, rusqlite::params_from_iter(values.iter()))
                .map_err(db_error)?;
        }
        self.verify(conn)
    }
    fn retire(&self, conn: &Connection) -> io::Result<()> {
        self.validate_schema(conn)?;
        let sql = format!("DELETE FROM {} WHERE {}", self.name, self.predicate());
        for row in &self.rows {
            let values = row.iter().map(Cell::sql).collect::<Vec<_>>();
            if conn
                .execute(&sql, rusqlite::params_from_iter(values.iter()))
                .map_err(db_error)?
                != 1
            {
                return Err(db_error("legacy rows changed before exact retirement"));
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Receipt {
    source_workspace: String,
    digest: String,
    phase: String,
    original: Vec<Table>,
    owned: Vec<Table>,
    requests: Vec<String>,
    execution_ids: Vec<String>,
    pending: Vec<String>,
    footprint: LegacyFootprint,
    inventory: InventoryReport,
    bindings: Vec<api::machine::ProjectExecutionMigrationBinding>,
}
fn snapshot(conn: &Connection, project: &str) -> io::Result<Vec<Table>> {
    let prefix = format!("local-private:{}:{}:", project.len(), project);
    let mut tables = vec![Table::read(
        conn,
        "executions",
        "json_extract(invocation,'$.local_private.project_id')=?1",
        project,
    )?];
    for name in EXECUTION_TABLES {
        tables.push(Table::read(conn, name, "execution IN (SELECT id FROM executions WHERE json_extract(invocation,'$.local_private.project_id')=?1)", project)?);
    }
    for name in ACTOR_TABLES {
        tables.push(Table::read(
            conn,
            name,
            "substr(actor,1,length(?1))=?1",
            &prefix,
        )?);
    }
    tables.push(Table::read(
        conn,
        "held_names",
        "substr(name,1,length(?1))=?1",
        &prefix,
    )?);
    tables.push(Table::read(
        conn,
        "private_object_uses",
        "project=?1",
        project,
    )?);
    Ok(tables)
}
fn footprint(
    tables: &[Table],
    scope: &LocalPrivateStorage,
    requests: Vec<String>,
    execution_ids: Vec<String>,
) -> io::Result<LegacyFootprint> {
    fn walk(value: &Value, key: &str, found: &mut LegacyFootprint) {
        if let Some(text) = value.as_str().filter(|text| !text.is_empty()) {
            match key {
                "generation" => found.generations.push(text.into()),
                "construction" | "executor" => found.constructions.push(text.into()),
                "manifest" | "snapshot" | "checkpoint" | "header"
                    if text.trim_start_matches("sha256:").len() == 64 =>
                {
                    found.manifests.push(text.into())
                }
                "model" | "repository" if text.starts_with("local/") => found
                    .provider_models
                    .push(text.trim_end_matches('@').into()),
                "source"
                    if text.starts_with("hf://")
                        || text.starts_with("civitai://")
                        || text.contains("huggingface.co/")
                        || text.contains("civitai.com/") =>
                {
                    found.provider_models.push(text.into())
                }
                _ => (),
            }
        }
        match value {
            Value::Object(values) => {
                if matches!(key, "header" | "manifest" | "checkpoint") {
                    if let Some(sha) = values.get("sha256").and_then(Value::as_str) {
                        found.manifests.push(sha.into());
                    }
                }
                for (key, value) in values {
                    walk(value, key, found);
                }
            }
            Value::Array(values) => {
                for value in values {
                    walk(value, key, found);
                }
            }
            _ => (),
        }
    }
    let mut found = LegacyFootprint::default();
    found.request_ids = requests;
    found.execution_ids = execution_ids;
    for table in tables {
        for row in &table.rows {
            for cell in row {
                let bytes = match cell {
                    Cell::Text(text) => text.as_bytes(),
                    Cell::Blob(bytes) => bytes.as_slice(),
                    _ => continue,
                };
                let bytes = crate::private_journal::inspect_for_migration(bytes, scope)?;
                if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                    walk(&value, "", &mut found);
                }
                if table.name == "preparations" {
                    if let Ok(preparation) = serde_json::from_slice::<Preparation>(&bytes) {
                        let plan = crate::gpu_service::Plan::of(&preparation)?;
                        let value = serde_json::to_value(plan).map_err(db_error)?;
                        walk(&value, "", &mut found);
                    }
                }
            }
        }
    }
    for values in [
        &mut found.request_ids,
        &mut found.execution_ids,
        &mut found.generations,
        &mut found.constructions,
        &mut found.manifests,
        &mut found.provider_models,
    ] {
        values.sort();
        values.dedup();
    }
    Ok(found)
}
fn verify_request_objects(
    tables: &[Table],
    scope: &LocalPrivateStorage,
) -> io::Result<Vec<String>> {
    use sha2::Digest as _;
    use std::io::Read as _;
    let store = crate::private_io::store(scope)?;
    let mut objects = BTreeMap::<String, u64>::new();
    for table in tables.iter().filter(|table| table.name == "run_objects") {
        let sha = table.column("sha256")?;
        let length = table.column("length")?;
        for row in &table.rows {
            let (Some(sha), Cell::Integer(length)) = (row[sha].text(), &row[length]) else {
                return Err(db_error("legacy request object reference is invalid"));
            };
            if *length < 0 {
                return Err(db_error("legacy request object length is negative"));
            }
            if objects
                .insert(sha.to_owned(), *length as u64)
                .is_some_and(|old| old != *length as u64)
            {
                return Err(db_error("legacy request object length is ambiguous"));
            }
        }
    }
    let mut pending = vec![];
    for (sha256, length) in objects {
        let object = tensorfs::format::ObjectRef {
            sha256: sha256.clone(),
            length,
        };
        let mut file = match store.file(&object) {
            Ok(file) => file,
            Err(_) => {
                pending.push(format!("protected_request_object_unavailable:{sha256}"));
                continue;
            }
        };
        let mut hash = sha2::Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        if tensorfs::sha256::hex(&hash.finalize()) != sha256 {
            pending.push(format!("protected_request_object_corrupt:{sha256}"));
        }
    }
    Ok(pending)
}
fn inventory(
    target: &Connection,
    receipt: &mut Receipt,
    scope: &LocalPrivateStorage,
    logs: &Path,
    engine: &Path,
    store: &crate::storage::Store,
    finalized: bool,
) -> io::Result<()> {
    // These explicitly disposable worker diagnostics are not canonical execution history.
    // The global store owner and terminal/exact-Birth checks proved the old writer quiescent;
    // the diagnostics barrier fences current appenders during in-place retirement.
    let mut retirement = super::project_retirement::retire_disposable(logs, engine)?;
    retirement.extend(super::project_retirement::retire_store_diagnostics(
        store.root(),
    )?);
    receipt.inventory = project_inventory::inspect(scope, logs, engine, store, &receipt.footprint)?;
    receipt.pending = receipt.inventory.pending.clone();
    receipt.pending.extend(retirement);
    receipt
        .pending
        .extend(verify_request_objects(&receipt.owned, scope)?);
    receipt.pending.sort();
    receipt.pending.dedup();
    receipt.phase = if finalized {
        if receipt.pending.is_empty() {
            "complete"
        } else {
            "inventory_pending"
        }
    } else {
        "copied"
    }
    .into();
    save(target, receipt)?;
    checkpoint(target)?;
    if !receipt.pending.is_empty() {
        return Err(io::Error::new(io::ErrorKind::WouldBlock, "project_storage_migration_pending: legacy artifacts have unresolved ownership; inspect the protected migration receipt"));
    }
    Ok(())
}
fn digest(tables: &[Table]) -> io::Result<String> {
    Ok(tensorfs::sha256::hex_digest(
        &serde_json::to_vec(tables).map_err(db_error)?,
    ))
}
fn checkpoint(conn: &Connection) -> io::Result<()> {
    let (busy, _, _): (i64, i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(db_error)?;
    if busy != 0 {
        return Err(io::Error::new(
            io::ErrorKind::ResourceBusy,
            "legacy journal readers prevent secure retirement",
        ));
    }
    Ok(())
}
fn save(conn: &Connection, receipt: &Receipt) -> io::Result<()> {
    let bytes = serde_json::to_vec(receipt).map_err(db_error)?;
    conn.execute("INSERT INTO project_custody_migrations(source_workspace,digest,receipt) VALUES(?1,?2,?3) ON CONFLICT(source_workspace,digest) DO UPDATE SET receipt=excluded.receipt", params![receipt.source_workspace, receipt.digest, bytes]).map_err(db_error)?;
    Ok(())
}

impl Journal {
    pub(crate) fn needs_project_migration(
        &self,
        target: &Journal,
        project: &str,
    ) -> io::Result<bool> {
        if self.contains_private_project(project)? {
            return Ok(true);
        }
        let present: bool = target.connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='project_custody_migrations')", [], |row| row.get(0)).map_err(db_error)?;
        if !present {
            return Ok(false);
        }
        let mut statement = target
            .connection
            .prepare("SELECT receipt FROM project_custody_migrations")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .map_err(db_error)?;
        for row in rows {
            let receipt: Receipt =
                serde_json::from_slice(&row.map_err(db_error)?).map_err(db_error)?;
            if receipt.phase != "complete" {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(crate) fn project_migration_receipt(
        &self,
    ) -> io::Result<api::machine::ProjectExecutionMigrationReceipt> {
        let present: bool = self.connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='project_custody_migrations')", [], |row| row.get(0)).map_err(db_error)?;
        let mut answer = api::machine::ProjectExecutionMigrationReceipt::default();
        if !present {
            return Ok(answer);
        }
        let mut statement = self
            .connection
            .prepare("SELECT receipt FROM project_custody_migrations")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .map_err(db_error)?;
        for row in rows {
            let receipt: Receipt =
                serde_json::from_slice(&row.map_err(db_error)?).map_err(db_error)?;
            if receipt.phase != "complete" || !receipt.pending.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "project_storage_migration_pending",
                ));
            }
            if digest(&receipt.original)? != receipt.digest {
                return Err(db_error("migration coverage snapshot is inconsistent"));
            }
            for binding in receipt.bindings {
                if binding.request_id.is_empty()
                    || binding.source_workspace_id != receipt.source_workspace
                    || binding.invocation_spec_digest.is_empty()
                {
                    return Err(db_error("legacy acceptance coverage is incomplete"));
                }
                answer.bindings.push(binding);
            }
        }
        answer.bindings.sort_by(|a, b| {
            (
                &a.source_workspace_id,
                &a.request_id,
                &a.invocation_spec_digest,
            )
                .cmp(&(
                    &b.source_workspace_id,
                    &b.request_id,
                    &b.invocation_spec_digest,
                ))
        });
        answer.bindings.dedup_by(|a, b| {
            a.request_id == b.request_id
                && a.source_workspace_id == b.source_workspace_id
                && a.invocation_spec_digest == b.invocation_spec_digest
        });
        Ok(answer)
    }
    /// Copies only a proof-owned local scope. Unscoped/remote records require independent
    /// canonical attribution and are never guessed from a filename, time or model digest.
    pub(crate) fn migrate_project_into(
        &mut self,
        target: &mut Journal,
        scope: &LocalPrivateStorage,
        logs: &Path,
        engine: &Path,
        store: &crate::storage::Store,
    ) -> io::Result<()> {
        target
            .connection
            .execute_batch(RECEIPTS)
            .map_err(db_error)?;
        self.connection
            .execute_batch("PRAGMA secure_delete=ON; PRAGMA temp_store=MEMORY;")
            .map_err(db_error)?;
        let prior: Vec<Vec<u8>> = {
            let mut statement = target
                .connection
                .prepare("SELECT receipt FROM project_custody_migrations")
                .map_err(db_error)?;
            let rows = statement
                .query_map([], |row| row.get(0))
                .map_err(db_error)?;
            rows.collect::<Result<_, _>>().map_err(db_error)?
        };
        for bytes in prior {
            let mut receipt: Receipt = serde_json::from_slice(&bytes).map_err(db_error)?;
            if receipt.source_workspace != self.workspace_id {
                if receipt.phase == "complete" {
                    continue;
                }
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "project_storage_migration_pending: a prior source workspace has not retired custody"));
            }
            if digest(&receipt.original)? != receipt.digest {
                return Err(db_error(
                    "protected migration receipt does not match its source snapshot",
                ));
            }
            if receipt.phase == "inventory_pending"
                || receipt.phase == "journal_retired"
                || receipt.phase == "complete"
            {
                inventory(
                    &target.connection,
                    &mut receipt,
                    scope,
                    logs,
                    engine,
                    store,
                    true,
                )?;
                continue;
            }
            if receipt.phase != "complete" {
                for table in &receipt.owned {
                    table.verify(&target.connection)?;
                }
                // A crash after target commit but before source commit leaves the exact old
                // rows, or none after source commit. Partial ownership is never inferred.
                let current = snapshot(&self.connection, &scope.project_id)?;
                if current.iter().any(|table| !table.rows.is_empty()) {
                    if digest(&current)? != receipt.digest {
                        return Err(io::Error::new(
                            io::ErrorKind::ResourceBusy,
                            "legacy scope changed during a pending migration",
                        ));
                    }
                    inventory(
                        &target.connection,
                        &mut receipt,
                        scope,
                        logs,
                        engine,
                        store,
                        false,
                    )?;
                    let transaction = self
                        .connection
                        .transaction_with_behavior(TransactionBehavior::Immediate)
                        .map_err(db_error)?;
                    for table in receipt.original.iter().rev() {
                        table.retire(&transaction)?;
                    }
                    transaction.commit().map_err(db_error)?;
                }
                self.finish_migration(target, &mut receipt)?;
                inventory(
                    &target.connection,
                    &mut receipt,
                    scope,
                    logs,
                    engine,
                    store,
                    true,
                )?;
            }
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let original = snapshot(&transaction, &scope.project_id)?;
        if original.iter().all(|table| table.rows.is_empty()) {
            transaction.commit().map_err(db_error)?;
            return Ok(());
        }
        let executions = &original[0];
        let record_column = executions.column("record")?;
        let invocation_column = executions.column("invocation")?;
        let id_column = executions.column("id")?;
        let selected: BTreeSet<String> = executions
            .rows
            .iter()
            .filter_map(|row| match &row[id_column] {
                Cell::Integer(id) => Some(id.to_string()),
                _ => None,
            })
            .collect();
        let mut requests = vec![];
        let mut bindings = vec![];
        let mut actors = BTreeSet::new();
        let mut generations = BTreeSet::new();
        let actor_column = executions.column("actor")?;
        for row in &executions.rows {
            let record = decoded_execution(
                row[record_column]
                    .text()
                    .ok_or_else(|| db_error("legacy execution has no record"))?,
            )?;
            let descriptor = record
                .invocation
                .local_private
                .as_ref()
                .ok_or_else(|| db_error("legacy execution has no positive project scope"))?;
            if descriptor.project_id != scope.project_id
                || descriptor.generation == 0
                || descriptor.generation > scope.generation
                || !record.state.terminal()
                || record.pending_completion.is_some()
                || record
                    .process
                    .as_ref()
                    .is_some_and(|birth| !birth.ended().unwrap_or(false))
            {
                return Err(io::Error::new(
                    io::ErrorKind::ResourceBusy,
                    "legacy executions must be terminal and quiescent before migration",
                ));
            }
            let indexed: Invocation = serde_json::from_str(
                row[invocation_column]
                    .text()
                    .ok_or_else(|| db_error("legacy scope index is missing"))?,
            )
            .map_err(db_error)?;
            if indexed.local_private.as_ref() != Some(descriptor) {
                return Err(db_error(
                    "legacy scope proof differs from its protected payload",
                ));
            }
            for linked in [
                &record.invocation.parent,
                &record.invocation.root,
                &record.invocation.family,
            ] {
                if !linked.is_empty() && !selected.contains(linked) {
                    return Err(db_error(
                        "legacy invocation crosses an unproven project boundary",
                    ));
                }
            }
            generations.insert(record.invocation.generation.clone());
            if let Some(actor) = row[actor_column].text() {
                actors.insert(actor.to_owned());
            }
            if let Some(submission) = record.submission {
                actors.insert(submission.actor);
                bindings.push(api::machine::ProjectExecutionMigrationBinding {
                    request_id: submission.request_id.clone(),
                    source_workspace_id: self.workspace_id.clone(),
                    invocation_spec_digest: submission.invocation_digest,
                });
                requests.push(submission.request_id);
            }
        }
        let mut owned = original.clone();
        // A published package's installation was historically shared by actor, even for a
        // scoped invocation. Copy the exact referenced public dependency without retiring
        // the independent public software registration.
        for actor in actors {
            let mut dependencies = Table::read(&transaction, "installations", "actor=?1", &actor)?;
            let record = dependencies.column("record")?;
            dependencies.rows.retain(|row| {
                row[record]
                    .text()
                    .and_then(|text| serde_json::from_str::<Installation>(text).ok())
                    .is_some_and(|installation| {
                        !installation.package.starts_with("local/")
                            && generations.contains(&installation.generation)
                    })
            });
            if !dependencies.rows.is_empty() {
                owned.push(dependencies);
            }
        }

        let mut references = BTreeMap::new();
        for table in &mut owned {
            for row in &mut table.rows {
                for cell in row {
                    match cell {
                        Cell::Text(text) => {
                            let bytes = crate::private_journal::rebase_for_migration(
                                text.as_bytes(),
                                scope,
                                &mut references,
                            )?;
                            *text = String::from_utf8(bytes).map_err(db_error)?;
                        }
                        Cell::Blob(bytes) => {
                            *bytes = crate::private_journal::rebase_for_migration(
                                bytes,
                                scope,
                                &mut references,
                            )?;
                        }
                        _ => (),
                    }
                }
            }
        }
        let execution_ids = selected.into_iter().collect::<Vec<_>>();
        let footprint = footprint(&owned, scope, requests.clone(), execution_ids.clone())?;
        let mut receipt = Receipt {
            source_workspace: self.workspace_id.clone(),
            digest: digest(&original)?,
            phase: "copied".into(),
            original,
            owned,
            requests,
            execution_ids,
            pending: vec![],
            footprint,
            inventory: InventoryReport::default(),
            bindings,
        };
        let target_empty: bool = target
            .connection
            .query_row("SELECT NOT EXISTS(SELECT 1 FROM executions)", [], |row| {
                row.get(0)
            })
            .map_err(db_error)?;
        if target.workspace_id != self.workspace_id && !target_empty {
            return Err(db_error("project history has another workspace identity; canonical receipt attribution is required"));
        }
        let destination = target
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        for table in &receipt.owned {
            table.insert(&destination)?;
        }
        destination
            .execute(
                "UPDATE machine_metadata SET value=?1 WHERE key='workspace_id'",
                [&self.workspace_id],
            )
            .map_err(db_error)?;
        save(&destination, &receipt)?;
        destination.commit().map_err(db_error)?;
        target.workspace_id = self.workspace_id.clone();
        checkpoint(&target.connection)?;
        // The target and full required object closure/inventory are verified before source retirement.
        inventory(
            &target.connection,
            &mut receipt,
            scope,
            logs,
            engine,
            store,
            false,
        )?;
        for table in receipt.original.iter().rev() {
            table.retire(&transaction)?;
        }
        transaction.commit().map_err(db_error)?;
        self.finish_migration(target, &mut receipt)?;
        inventory(
            &target.connection,
            &mut receipt,
            scope,
            logs,
            engine,
            store,
            true,
        )
    }
    fn finish_migration(&mut self, target: &mut Journal, receipt: &mut Receipt) -> io::Result<()> {
        checkpoint(&self.connection)?;
        self.connection
            .execute_batch("VACUUM main")
            .map_err(db_error)?;
        checkpoint(&self.connection)?;
        for table in &receipt.owned {
            table.verify(&target.connection)?;
        }
        receipt.phase = "journal_retired".into();
        save(&target.connection, receipt)?;
        checkpoint(&target.connection)
    }
}
