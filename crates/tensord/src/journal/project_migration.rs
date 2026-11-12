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
    #[serde(default)]
    attributed_ids: Vec<u64>,
}
fn snapshot(conn: &Connection, project: &str, attributed: &[u64]) -> io::Result<Vec<Table>> {
    let prefix = format!("local-private:{}:{}:", project.len(), project);
    let selection = if attributed.is_empty() {
        "json_extract(invocation,'$.local_private.project_id')=?1".to_owned()
    } else {
        let ids = attributed
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        format!("(json_extract(invocation,'$.local_private.project_id')=?1 OR id IN ({ids}))")
    };

    let mut tables = vec![Table::read(conn, "executions", &selection, project)?];
    for name in EXECUTION_TABLES {
        tables.push(Table::read(
            conn,
            name,
            &format!("execution IN (SELECT id FROM executions WHERE {selection})"),
            project,
        )?);
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
fn copy_run_directory(source: &Path, target: &Path) -> io::Result<()> {
    use std::io::{Read as _, Write as _};
    let metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(db_error(
            "legacy run directory is not a proven regular directory",
        ));
    }
    if let Ok(metadata) = fs::symlink_metadata(target) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(db_error("project run directory has an unsafe type"));
        }
    }
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let destination = target.join(entry.file_name());
        let kind = fs::symlink_metadata(&path)?;
        if kind.file_type().is_symlink() {
            return Err(db_error(
                "legacy run closure contains an unproven symbolic link",
            ));
        }
        if kind.is_dir() {
            copy_run_directory(&path, &destination)?;
        } else if kind.is_file() {
            let mut input =
                platform::durable::open_nofollow(&path, fs::OpenOptions::new().read(true))?;
            let mut bytes = Vec::new();
            input.read_to_end(&mut bytes)?;
            if destination.exists() {
                let mut old = platform::durable::open_nofollow(
                    &destination,
                    fs::OpenOptions::new().read(true),
                )?;
                let mut previous = Vec::new();
                old.read_to_end(&mut previous)?;
                if previous != bytes {
                    return Err(db_error(
                        "project run file conflicts with canonical legacy bytes",
                    ));
                }
            } else {
                let mut output = platform::durable::open_nofollow(
                    &destination,
                    fs::OpenOptions::new().write(true).create_new(true),
                )?;
                output.write_all(&bytes)?;
                output.sync_all()?;
            }
        } else {
            return Err(db_error("legacy run closure contains an unknown object"));
        }
    }
    platform::durable::sync_dir(target)?;
    Ok(())
}
fn copy_attributed_run_files(
    source_engine: &Path,
    scope: &LocalPrivateStorage,
    ids: &[u64],
) -> io::Result<Vec<String>> {
    let target = crate::private_io::root(scope)?;
    let mut pending = vec![];
    for id in ids {
        for kind in ["results", "staging"] {
            let source = source_engine.join(kind).join(id.to_string());
            if source.exists() {
                copy_run_directory(&source, &target.join(kind).join(id.to_string()))?;
                // Canonical data is copied and durable. Deletion needs source-reader/folder
                // retirement proof; keep the exact remaining source explicit until then.
                pending.push(format!(
                    "legacy_run_files_copied_source_retirement_pending:{}",
                    source.display()
                ));
            }
        }
    }
    Ok(pending)
}

fn copy_attributed_objects(
    tables: &[Table],
    scope: &LocalPrivateStorage,
    source: &crate::storage::Store,
    source_connection: &Connection,
    actor: &str,
) -> io::Result<Vec<String>> {
    let target = crate::private_io::store(scope)?;
    let mut todo = BTreeMap::new();
    for table in tables.iter().filter(|table| table.name == "run_objects") {
        let sha = table.column("sha256")?;
        let length = table.column("length")?;
        for row in &table.rows {
            if let (Some(sha), Cell::Integer(length)) = (row[sha].text(), &row[length]) {
                if *length >= 0 {
                    todo.insert(sha.to_owned(), *length as u64);
                }
            }
        }
    }
    let mut visited = BTreeSet::new();
    let mut pending = vec![];
    while let Some((sha256, length)) = todo.pop_first() {
        if !visited.insert(sha256.clone()) {
            continue;
        }
        let object = tensorfs::format::ObjectRef {
            sha256: sha256.clone(),
            length,
        };
        crate::private_io::retain(scope, std::slice::from_ref(&object))?;
        if !target.holds(&object) {
            let input = match source.file(&object) {
                Ok(input) => input,
                Err(_) => {
                    pending.push(format!("legacy_required_object_unavailable:{sha256}"));
                    continue;
                }
            };
            target
                .put(input, Some(&object))
                .map_err(crate::storage::io_error)?;
        }
        // Uploaded code/resource manifests were checked against this actor's grants on the
        // original acceptance. Never use an authored digest as authority for a foreign blob.
        if object.length <= 4 << 20 {
            let bytes = target.read(&object).map_err(crate::storage::io_error)?;
            if let Ok(manifest) = serde_json::from_slice::<crate::local_source::Manifest>(&bytes) {
                for member in manifest.files.into_iter().chain(manifest.wheels) {
                    let allowed: bool = source_connection.query_row("SELECT EXISTS(SELECT 1 FROM objects WHERE actor=?1 AND sha256=?2 AND length=?3)", params![actor,member.digest,member.length], |row| row.get(0)).map_err(db_error)?;
                    if allowed {
                        todo.insert(member.digest, member.length);
                    } else {
                        pending.push("legacy_code_member_grant_unproven".into());
                    }
                }
            }
            if let Ok(members) = crate::trees::parse(&bytes) {
                for member in members.into_values() {
                    let allowed: bool = source_connection.query_row("SELECT EXISTS(SELECT 1 FROM objects WHERE actor=?1 AND sha256=?2 AND length=?3 UNION ALL SELECT 1 FROM run_objects WHERE sha256=?2 AND length=?3)", params![actor,member.sha256,member.length], |row| row.get(0)).map_err(db_error)?;
                    if allowed {
                        todo.insert(member.sha256, member.length);
                    } else {
                        pending.push("legacy_tree_member_grant_unproven".into());
                    }
                }
            }
        }
    }
    Ok(pending)
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
    let retained = receipt
        .pending
        .iter()
        .filter(|item| {
            item.starts_with("legacy_required_")
                || item.starts_with("legacy_code_member_")
                || item.starts_with("legacy_tree_member_")
                || item.starts_with("legacy_run_files_")
                || item.starts_with("legacy_unscoped_dependency_")
        })
        .cloned()
        .collect::<Vec<_>>();
    receipt.pending = receipt.inventory.pending.clone();
    receipt.pending.extend(retained);
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
    if finalized && !receipt.pending.is_empty() {
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
    pub(crate) fn lookup_project_legacy_bindings(
        &self,
        actor: &str,
        project: &str,
        requests: &[String],
    ) -> io::Result<api::machine::ProjectExecutionLegacyLookupAnswer> {
        if requests.len() > 256 {
            return Err(db_error("legacy binding lookup exceeds its protocol bound"));
        }
        let mut answer = api::machine::ProjectExecutionLegacyLookupAnswer::default();
        for request in requests {
            if request.is_empty() || request.len() > 256 || request.chars().any(char::is_control) {
                return Err(db_error("invalid legacy request identity"));
            }
            let row: Option<(u64, String, String)> = self
                .connection
                .query_row(
                    "SELECT id,invocation,record FROM executions WHERE actor=?1 AND request_id=?2",
                    params![actor, request],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(db_error)?;
            let (source_number, invocation, record) = row.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "canonical legacy request has no acceptance at this registered worker",
                )
            })?;
            let invocation: Invocation = serde_json::from_str(&invocation).map_err(db_error)?;
            if invocation
                .local_private
                .as_ref()
                .is_some_and(|scope| scope.project_id != project)
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "legacy request is tagged to another project",
                ));
            }
            let record = decoded_execution(&record)?;
            let accepted = record
                .submission
                .ok_or_else(|| db_error("legacy request has no accepted binding"))?;
            if accepted.actor != actor
                || accepted.request_id != *request
                || accepted.invocation_digest.is_empty()
            {
                return Err(db_error("legacy accepted binding is incomplete"));
            }
            answer
                .bindings
                .push(api::machine::ProjectExecutionLegacyBinding {
                    source_number,
                    binding: api::machine::ProjectExecutionMigrationBinding {
                        request_id: request.clone(),
                        source_workspace_id: self.workspace_id.clone(),
                        invocation_spec_digest: accepted.invocation_digest,
                    },
                });
        }
        Ok(answer)
    }

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
        actor: &str,
        explicit: &[api::machine::ProjectExecutionMigrationBinding],
    ) -> io::Result<()> {
        if explicit.len() > 256 {
            return Err(db_error("legacy adoption exceeds its protocol bound"));
        }
        if explicit.is_empty() && !self.needs_project_migration(target, &scope.project_id)? {
            return Ok(());
        }
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
                let current =
                    snapshot(&self.connection, &scope.project_id, &receipt.attributed_ids)?;
                if current.iter().any(|table| !table.rows.is_empty()) {
                    if digest(&current)? != receipt.digest {
                        return Err(io::Error::new(
                            io::ErrorKind::ResourceBusy,
                            "legacy scope changed during a pending migration",
                        ));
                    }
                    if !receipt.attributed_ids.is_empty() {
                        receipt.pending = copy_attributed_objects(
                            &receipt.owned,
                            scope,
                            store,
                            &self.connection,
                            actor,
                        )?;
                    }
                    let required = verify_request_objects(&receipt.owned, scope)?;
                    if !required.is_empty()
                        || !receipt
                            .pending
                            .iter()
                            .filter(|pending| {
                                pending.starts_with("legacy_required_")
                                    || pending.starts_with("legacy_code_member_")
                                    || pending.starts_with("legacy_tree_member_")
                                    || pending.starts_with("legacy_run_files_")
                            })
                            .collect::<Vec<_>>()
                            .is_empty()
                    {
                        receipt.pending.extend(required);
                        save(&target.connection, &receipt)?;
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "legacy required payload closure is incomplete",
                        ));
                    }
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
        let mut attributed = BTreeSet::new();
        for binding in explicit {
            if binding.source_workspace_id != self.workspace_id {
                return Err(db_error(
                    "legacy acceptance belongs to another source workspace",
                ));
            }
            let row: Option<(u64, String, String)> = transaction
                .query_row(
                    "SELECT id,invocation,record FROM executions WHERE actor=?1 AND request_id=?2",
                    params![actor, binding.request_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(db_error)?;
            let (id, invocation, record) = row.ok_or_else(|| {
                db_error("verified legacy acceptance is no longer available at this source")
            })?;
            let invocation: Invocation = serde_json::from_str(&invocation).map_err(db_error)?;
            if invocation
                .local_private
                .as_ref()
                .is_some_and(|old| old.project_id != scope.project_id)
            {
                return Err(db_error(
                    "legacy acceptance is already tagged to another project",
                ));
            }
            let record = decoded_execution(&record)?;
            let accepted = record
                .submission
                .as_ref()
                .ok_or_else(|| db_error("legacy acceptance has no source context"))?;
            if accepted.actor != actor
                || accepted.request_id != binding.request_id
                || accepted.invocation_digest != binding.invocation_spec_digest
                || accepted.invocation_digest.is_empty()
            {
                return Err(db_error(
                    "legacy acceptance differs from the canonical verified binding",
                ));
            }
            if invocation.local_private.is_none() {
                attributed.insert(id);
            }
        }
        // Exact lineage only: a selected parent's internally brokered children belong to its
        // closure. Different tagged projects/actors are refused rather than adopted.
        loop {
            if attributed.is_empty() {
                break;
            }
            if attributed.len() > 10000 {
                return Err(db_error(
                    "legacy lineage exceeds its migration batch boundary",
                ));
            }
            let ids = attributed
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = transaction.prepare(&format!("SELECT id,actor,invocation FROM executions WHERE CAST(json_extract(invocation,'$.parent') AS INTEGER) IN ({ids})")).map_err(db_error)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_error)?;
            let before = attributed.len();
            for (id, child_actor, invocation) in rows {
                let invocation: Invocation = serde_json::from_str(&invocation).map_err(db_error)?;
                if child_actor.as_deref().is_some_and(|owner| owner != actor)
                    || invocation
                        .local_private
                        .as_ref()
                        .is_some_and(|old| old.project_id != scope.project_id)
                {
                    return Err(db_error("legacy child lineage crosses an unproven owner"));
                }
                if invocation.local_private.is_none() {
                    attributed.insert(id);
                }
            }
            if attributed.len() == before {
                break;
            }
        }
        let attributed_ids = attributed.into_iter().collect::<Vec<_>>();
        let original = snapshot(&transaction, &scope.project_id, &attributed_ids)?;
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
        let mut needed_preparations = BTreeSet::new();
        let mut actors = BTreeSet::new();
        let mut generations = BTreeSet::new();
        let actor_column = executions.column("actor")?;
        for row in &executions.rows {
            let record = decoded_execution(
                row[record_column]
                    .text()
                    .ok_or_else(|| db_error("legacy execution has no record"))?,
            )?;
            let scoped = record
                .invocation
                .local_private
                .as_ref()
                .is_some_and(|descriptor| {
                    descriptor.project_id == scope.project_id
                        && descriptor.generation > 0
                        && descriptor.generation <= scope.generation
                });
            let attributed = record.invocation.local_private.is_none()
                && record
                    .id
                    .parse::<u64>()
                    .ok()
                    .is_some_and(|id| attributed_ids.contains(&id));
            if (!scoped && !attributed)
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
            if indexed.local_private != record.invocation.local_private {
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
                if !submission.preparation_id.is_empty() {
                    needed_preparations
                        .insert((submission.actor.clone(), submission.preparation_id.clone()));
                }
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
        {
            let executions = &mut owned[0];
            for row in &mut executions.rows {
                let mut record = decoded_execution(
                    row[record_column]
                        .text()
                        .ok_or_else(|| db_error("legacy record is missing"))?,
                )?;
                if record.invocation.local_private.is_none() {
                    record.invocation.local_private = Some(scope.clone());
                    row[invocation_column] = Cell::Text(invocation_index(&record.invocation)?);
                    row[record_column] = Cell::Text(encoded_execution(&record)?);
                }
            }
        }
        // Unscoped payloads become protected references in the copy, preserving canonical bytes.
        for table in &mut owned {
            if !EXECUTION_TABLES.contains(&table.name.as_str()) {
                continue;
            }
            let execution = table.column("execution")?;
            for row in &mut table.rows {
                let Cell::Integer(id) = row[execution] else {
                    return Err(db_error("legacy child row has no execution identity"));
                };
                if !attributed_ids.contains(&(id as u64)) {
                    continue;
                }
                let sequence = table
                    .columns
                    .iter()
                    .position(|column| column == "sequence")
                    .and_then(|at| match row[at] {
                        Cell::Integer(sequence) => Some(sequence),
                        _ => None,
                    });
                for (column, value) in table.columns.iter().zip(row.iter_mut()) {
                    let slot = match (table.name.as_str(), column.as_str()) {
                        ("public_terminals", "events") => Some("terminal".into()),
                        ("run_products", "product") => {
                            sequence.map(|sequence| format!("product:{sequence}"))
                        }
                        ("run_measurements", "measurements") => Some("measurements".into()),
                        ("run_logs", "record") => {
                            sequence.map(|sequence| format!("log:{sequence}"))
                        }
                        ("run_calls", "record") => {
                            sequence.map(|sequence| format!("call:{sequence}"))
                        }
                        ("triage", "record") => Some("triage".into()),
                        ("run_specs", "spec") => Some("spec".into()),
                        _ => None,
                    };
                    if let Some(slot) = slot {
                        let bytes = match value {
                            Cell::Text(text) => text.as_bytes(),
                            Cell::Blob(bytes) => bytes.as_slice(),
                            _ => return Err(db_error("legacy payload has the wrong type")),
                        };
                        let protected = private_journal::write(
                            Some(scope),
                            &payload_context(&id.to_string(), &slot),
                            bytes,
                        )?;
                        *value = if matches!(value, Cell::Text(_)) {
                            Cell::Text(String::from_utf8(protected).map_err(db_error)?)
                        } else {
                            Cell::Blob(protected)
                        };
                    }
                }
            }
        }

        for (owner, id) in needed_preparations {
            let mut dependency = Table::read(&transaction, "preparations", "actor=?1", &owner)?;
            let id_column = dependency.column("id")?;
            dependency
                .rows
                .retain(|row| row[id_column].text() == Some(id.as_str()));
            if !dependency.rows.is_empty() {
                let actor_column = dependency.column("actor")?;
                let record_column = dependency.column("record")?;
                for row in &mut dependency.rows {
                    let bytes = row[record_column]
                        .text()
                        .ok_or_else(|| db_error("legacy preparation is invalid"))?
                        .as_bytes();
                    let preparation: Preparation =
                        serde_json::from_slice(bytes).map_err(db_error)?;
                    let protected = private_journal::write(
                        Some(scope),
                        &format!("preparation:{owner}:{id}"),
                        bytes,
                    )?;
                    row[actor_column] = Cell::Text(crate::private_io::resource_key(&owner));
                    row[record_column] = Cell::Text(serde_json::json!({"installation":preparation.installation,"_private_payload":String::from_utf8(protected).map_err(db_error)?}).to_string());
                }
                owned.push(dependency);
            }
        }
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

        if !attributed_ids.is_empty() {
            let object_rows = owned
                .iter()
                .find(|table| table.name == "run_objects")
                .cloned();
            if let Some(object_rows) = object_rows {
                let sha = object_rows.column("sha256")?;
                let required: BTreeSet<String> = object_rows
                    .rows
                    .iter()
                    .filter_map(|row| row[sha].text().map(str::to_owned))
                    .collect();
                let mut grants = Table::read(&transaction, "objects", "actor=?1", actor)?;
                let sha = grants.column("sha256")?;
                let owner = grants.column("actor")?;
                grants
                    .rows
                    .retain(|row| row[sha].text().is_some_and(|sha| required.contains(sha)));
                for row in &mut grants.rows {
                    row[owner] = Cell::Text(crate::private_io::resource_key(actor));
                }
                if !grants.rows.is_empty() {
                    owned.push(grants);
                }
            }
        }
        let raw_dependencies = if attributed_ids.is_empty() {
            vec![]
        } else {
            vec!["legacy_unscoped_dependency_metadata_requires_reference_retirement".to_owned()]
        };
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
        let mut object_pending = if attributed_ids.is_empty() {
            vec![]
        } else {
            copy_attributed_objects(&owned, scope, store, &transaction, actor)?
        };
        if !attributed_ids.is_empty() {
            object_pending.extend(copy_attributed_run_files(
                &engine.join("execution"),
                scope,
                &attributed_ids,
            )?);
        }
        let mut receipt = Receipt {
            source_workspace: self.workspace_id.clone(),
            digest: digest(&original)?,
            phase: "copied".into(),
            original,
            owned,
            requests,
            execution_ids,
            pending: object_pending.into_iter().chain(raw_dependencies).collect(),
            footprint,
            inventory: InventoryReport::default(),
            bindings,
            attributed_ids,
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
        let required = verify_request_objects(&receipt.owned, scope)?;
        if !required.is_empty()
            || receipt.pending.iter().any(|pending| {
                pending.starts_with("legacy_required_")
                    || pending.starts_with("legacy_code_member_")
                    || pending.starts_with("legacy_tree_member_")
            })
        {
            receipt.pending.extend(required);
            save(&target.connection, &receipt)?;
            checkpoint(&target.connection)?;
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "legacy required payload closure is incomplete",
            ));
        }
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
