//! The legacy hard cut (cl-1003; Paul 11-15: "you can delete all previous run history", "please just hardcut we don't need these
//! anymore"). A store a 0.24/0.25 cozy made (version 7), or a 0.26 RC's with its old project history fenced (8), is written again
//! as version 9 before Calcifer opens it: a new file holding only what is live (machines not ended and any a live run names, runs
//! outside a project not settled with their rows, open rental asks, the project index, settings and admissions) takes the old
//! one's place. Every old run of a project, every settled run outside one and every ended machine stay behind in the old file,
//! which goes whole. Only what is live is read, so the start it happens on is slower by at most about a second on a cold disk, and
//! nothing is vacuumed. A project's own files are untouched; what one left on `local` that only this computer's TensorFS store
//! holds is held, then copied into it, first (`keep_project_files`). One-time code: it goes in the release after.
use super::*;

/// What is filtered here, or left behind: the run tables, machines, retry answers, the index, an old store's caches, and the
/// measurement cursor, whose old run number would pass over the new store's first runs (it starts again at zero).
const NOT_COPIED: [&str; 13] = ["runs", "outputs", "run_logs", "asks", "run_placements", "after_steps", "machines", "answers", "projects",
    "unseen", "sessions", "session_codes", "measurements_dropped"];
/// A version 7 store's caches of its own shape, made again empty.
const OLD_CACHES: [&str; 3] = ["headers", "rendered_here", "vault_operations"];

fn has(conn: &Connection, schema: &str, table: &str) -> Result<bool> {
    Ok(conn.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {schema}.sqlite_schema WHERE type = 'table' AND name = ?1)"), [table], |row| row.get(0))?)
}

/// `schema.table`'s columns. The schema is the pragma's argument: `old.pragma_table_info(t)` reads `main.t` when there is one.
fn columns(conn: &Connection, schema: &str, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?1, ?2)")?;
    Ok(stmt.query_map([table, schema], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?)
}

/// `main.<table>`'s rows from `old.<table>`'s that `which` keeps, by the columns both have.
fn copy(conn: &Connection, table: &str, which: &str) -> Result<()> {
    if !has(conn, "old", table)? { return Ok(()); }
    let have = columns(conn, "old", table)?;
    let both = columns(conn, "main", table)?.into_iter().filter(|column| have.contains(column)).collect::<Vec<_>>().join(",");
    conn.execute_batch(&format!("INSERT OR IGNORE INTO main.{table}({both}) SELECT {both} FROM old.{table} WHERE {which}"))?;
    Ok(())
}

/// Paul's answer on the old landing folder of runs outside a project (`~/.cozy/outputs`, cl-1036): true deletes it with the cut.
const DELETE_OLD_OUTPUTS: bool = false;

fn cut(why: impl std::fmt::Display) -> StoreError { StoreError::Cut(why.to_string()) }

/// A path as one key, whatever its separators: `/a/b/` and `/a/b` are one.
fn path_key(path: &Path) -> String { path.components().collect::<PathBuf>().to_string_lossy().into_owned() }

/// Where a locked project's files left on `local` are held in this computer's TensorFS store until its next unlock copies them
/// in (`project::keep_left_on_local`): a header of just those objects, beside the store in `kept/`, which TensorFS roots while
/// the file is there, whatever its uses' ages. Named by the project's folder, so nothing else records which project it is.
pub(crate) fn kept_path(calcifer: &Path, root: &Path) -> PathBuf {
    calcifer.join("kept").join(format!("{}.cozytensors", key_digest(&format!("cozy: files a locked project left on local\n{}", path_key(root)))))
}

/// `objects` held by TensorFS under `at` (`kept_path`) until that file is removed.
fn hold(store: &tensorfs::store::Store, at: &Path, objects: Vec<tensorfs::format::ObjectRef>) -> std::result::Result<(), String> {
    let header = tensorfs::format::Header::new(objects, None, vec![], Default::default()).map_err(|error| error.to_string())?;
    let bytes = header.to_cbor().map_err(|error| error.to_string())?;
    let dir = at.parent().expect("kept/");
    platform::durable::private_dir(dir).map_err(|error| error.to_string())?;
    platform::durable::replace(at, &bytes, 0o600).map_err(|error| error.to_string())?;
    store.root_file(at, &tensorfs::format::ObjectRef::of(&bytes)).map_err(|error| error.to_string())
}

/// `main.<to>`'s rows from `old.<from>`'s that `which` keeps, by the columns both have.
fn copy_from(conn: &Connection, from: &str, to: &str, which: &str) -> Result<()> {
    if !has(conn, "old", from)? { return Ok(()); }
    let have = columns(conn, "old", from)?;
    let both = columns(conn, "main", to)?.into_iter().filter(|column| have.contains(column)).collect::<Vec<_>>().join(",");
    conn.execute_batch(&format!("INSERT OR IGNORE INTO main.{to}({both}) SELECT {both} FROM old.{from} WHERE {which}"))?;
    Ok(())
}

/// The store at `path`, at version `from` (7 or 8), written again as this version's (`VERSION`) and put in its place. Anything
/// that fails before the new file is named leaves the old one as it was, and the next start goes again.
pub(super) fn rebuild(path: &Path, from: i64) -> Result<()> {
    let (locked, lost) = keep_project_files(path, from)?;
    let next = path.with_extension("db.cut");
    let _ = std::fs::remove_file(&next);
    {
        let conn = Connection::open(&next)?;
        // Before its first table, as a new store's (cl-1010): pages its prunes free go back to the disk.
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        register_owner(&conn, None)?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        // What is read of the old runs stays in memory, never in a temporary file outside the home.
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        conn.execute("ATTACH ?1 AS old", [path.to_string_lossy()])?;
        conn.execute_batch("BEGIN IMMEDIATE;")?;
        conn.execute_batch(SCHEMA)?;
        for tables in [ANSWERS, crate::project::journal::PLACEMENTS, crate::maintenance_source::TABLES, crate::project::vault_operation::TABLES, crate::state::TABLES] {
            conn.execute_batch(tables)?;
        }
        // Runs outside a project still going, found by their own index: a project's never comes (its history is cut), nor a run
        // that settled.
        let indexed = if conn.query_row("SELECT EXISTS(SELECT 1 FROM old.sqlite_schema WHERE type = 'index' AND name = 'runs_owed')", [], |row| row.get::<_, bool>(0))? { "INDEXED BY runs_owed" } else { "" };
        conn.execute_batch("CREATE TEMP TABLE live (id TEXT PRIMARY KEY, key TEXT NOT NULL);")?;
        let found: Vec<(String, String, String, Vec<u8>)> = {
            let mut stmt = conn.prepare(&format!("SELECT id, idempotency_key, out_dir, substr(spec, 1, 64) FROM old.runs {indexed} WHERE state <> 'settled'"))?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default())))?.collect::<rusqlite::Result<_>>()?
        };
        for (id, key, out_dir, spec) in found {
            let a_project = vault::is_payload(&spec) || crate::project::of_landing(Path::new(&out_dir)).is_some();
            if !a_project { conn.execute("INSERT INTO temp.live VALUES (?1, ?2)", params![id, key])?; }
        }
        // Every open rental order stays (one the Hub may have filled already bills a pod): with its run when its body is sealed to
        // the run's project, else on its own. A 0.26 RC fenced a project's with its runs (`legacy_project_*`).
        let mut sources = vec![];
        for source in [("asks", "runs", "outputs", "run_logs"), ("legacy_project_asks", "legacy_project_runs", "legacy_project_outputs", "legacy_project_run_logs")] {
            if has(&conn, "old", source.0)? && has(&conn, "old", source.1)? { sources.push(source); }
        }
        for (asks, runs, _, _) in &sources {
            let sealed: Vec<(String, Vec<u8>)> = {
                let mut stmt = conn.prepare(&format!("SELECT run, body FROM old.{asks} WHERE state <> 'done' AND run IS NOT NULL AND run NOT IN (SELECT id FROM temp.live)"))?;
                stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<rusqlite::Result<_>>()?
            };
            for (run, _) in sealed.into_iter().filter(|(_, body)| vault::is_payload(body)) {
                conn.execute(&format!("INSERT OR IGNORE INTO temp.live SELECT id, idempotency_key FROM old.{runs} WHERE id = ?1"), [&run])?;
            }
        }
        let asked = sources.iter().map(|(asks, _, _, _)| format!(" OR name IN (SELECT machine FROM old.{asks} WHERE state <> 'done' AND machine IS NOT NULL)")).collect::<String>();
        let ran = sources.iter().map(|(_, runs, _, _)| format!(" OR name IN (SELECT machine FROM old.{runs} WHERE id IN (SELECT id FROM temp.live))")).collect::<String>();
        copy(&conn, "machines", &format!("state <> 'ended'{ran}{asked}"))?;
        // An ended one kept for what names it is kept 90 days from now (`prune_machines`), as one that ended before its end was kept.
        conn.execute("UPDATE main.machines SET ended_at = ?1 WHERE state = 'ended' AND ended_at IS NULL", [now_ms()])?;
        for (_, runs, outputs, logs) in &sources {
            copy_from(&conn, runs, "runs", "id IN (SELECT id FROM temp.live)")?;
            copy_from(&conn, outputs, "outputs", "run IN (SELECT id FROM temp.live)")?;
            copy_from(&conn, logs, "run_logs", "run IN (SELECT id FROM temp.live)")?;
        }
        if sources.is_empty() { copy(&conn, "runs", "id IN (SELECT id FROM temp.live)")?; }
        copy(&conn, "run_placements", "run IN (SELECT id FROM temp.live)")?;
        for (asks, _, _, _) in &sources {
            let have = columns(&conn, "old", asks)?;
            let both: Vec<String> = columns(&conn, "main", "asks")?.into_iter().filter(|column| have.contains(column)).collect();
            let picked = both.iter().map(|column| if column == "run" { "CASE WHEN run IN (SELECT id FROM temp.live) THEN run END".to_owned() } else { column.clone() }).collect::<Vec<_>>();
            conn.execute_batch(&format!("INSERT OR IGNORE INTO main.asks({}) SELECT {} FROM old.{asks} WHERE state <> 'done'", both.join(","), picked.join(",")))?;
        }
        if has(&conn, "old", "after_steps")? {
            let keys: Vec<String> = {
                let mut stmt = conn.prepare("SELECT key FROM temp.live")?;
                stmt.query_map([], |row| row.get::<_, String>(0))?.map(|key| key.map(|key| key_digest(&key))).collect::<rusqlite::Result<_>>()?
            };
            let keys = serde_json::to_string(&keys).map_err(|error| StoreError::Corrupt(error.to_string()))?;
            conn.execute("INSERT OR IGNORE INTO main.after_steps SELECT key_sha256, step FROM old.after_steps WHERE key_sha256 IN (SELECT value FROM json_each(?1))", [keys])?;
        }
        if has(&conn, "old", "projects")? {
            let code = if columns(&conn, "old", "projects")?.iter().any(|column| column == "code") { "code" } else { "''" };
            conn.execute_batch(&format!("INSERT OR IGNORE INTO main.projects (id, path, code) SELECT id, path, {code} FROM old.projects"))?;
        }
        // The rest as it is: settings, admissions, the project keys and gone list, and what a later change added.
        let rest: Vec<(String, String)> = {
            let mut stmt = conn.prepare("SELECT name, sql FROM old.sqlite_schema WHERE type = 'table' AND sql IS NOT NULL
                AND name NOT LIKE 'legacy\\_%' ESCAPE '\\' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'")?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<rusqlite::Result<_>>()?
        };
        for (table, sql) in rest {
            if NOT_COPIED.contains(&table.as_str()) || (from == 7 && OLD_CACHES.contains(&table.as_str())) { continue; }
            if !has(&conn, "main", &table)? { conn.execute_batch(&sql)?; }
            // A KEY-N name of a project no longer in the index is no one's; a machine's kept lines go with its row (cl-1012).
            let which = match table.as_str() {
                "project_keys" => "id IN (SELECT id FROM main.projects)",
                "machine_logs" => "machine IN (SELECT name FROM main.machines)",
                _ => "1",
            };
            copy(&conn, &table, which)?;
        }
        conn.execute_batch(LOST)?;
        for (root, files) in &lost {
            conn.execute("INSERT OR REPLACE INTO main.lost_before_upgrade (path, files) VALUES (?1, ?2)", params![root.to_string_lossy(), *files as i64])?;
        }
        conn.pragma_update(None, "user_version", VERSION)?;
        conn.execute_batch("COMMIT; DETACH old;")?;
    }
    std::fs::File::open(&next).and_then(|file| file.sync_all()).map_err(|error| cut(format!("the new store couldn't be written ({error}): free space on that disk, then start cozy again")))?;
    private(&next);
    // The old file's WAL belongs to it: wholly checkpointed into it by its last connection, then gone before the file is replaced.
    let (busy, log, done): (i64, i64, i64) = Connection::open(path)?.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    if busy != 0 || log != done {
        let _ = std::fs::remove_file(&next);
        return Err(cut("another program has Calcifer's store open: close it (an sqlite3 shell, a backup), then start cozy again"));
    }
    for suffix in ["-wal", "-shm"] {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        let _ = std::fs::remove_file(side);
    }
    std::fs::rename(&next, path).map_err(|error| cut(format!("the new store couldn't take the old one's place ({error})")))?;
    if let Some(home) = path.parent() {
        let _ = platform::durable::sync_dir(home);
        if DELETE_OLD_OUTPUTS && let Some(cozy) = home.parent() { let _ = std::fs::remove_dir_all(cozy.join("outputs")); }
    }
    tracing::info!(target: "cozy.service_health", from, locked, "the legacy cut left every old run behind; locked projects' files left on local are kept until their next unlock");
    Ok(())
}

/// Before anything of the old store is left behind, every project's files left on `local` that only this computer's TensorFS store
/// holds are held there (`kept_path`), whatever their uses' ages: a readable project's as its lock names them, a locked one's (an
/// encrypted project's lock can't be read while it is locked, nor an older plain project's before its first open migrates it) as
/// the old store says its runs left them, never exported. Then every readable project's are copied into it, each checked and noted
/// (a copy that fails stops the cut), and only once all are in, each readable project's hold and copies are let go of: the store
/// lets an object go only when no project's hold still names it. A locked project's are copied in as it opens. A file of a locked
/// project's runs this store no longer names (one imported already) can't be known before then. Answers how many locked projects
/// wait for an open, and each plain project that had lost files before, how many (`cozy doctor` and its page say so).
fn keep_project_files(path: &Path, from: i64) -> Result<(usize, Vec<(PathBuf, u64)>)> {
    let calcifer = path.parent().unwrap_or(Path::new("."));
    let conn = Connection::open(path)?;
    if !has(&conn, "main", "projects")? { return Ok((0, vec![])); }
    let paths: Vec<String> = {
        let mut stmt = conn.prepare("SELECT path FROM projects")?;
        stmt.query_map([], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    let (mut locked, mut readable): (Vec<PathBuf>, Vec<(PathBuf, Vec<tensorfs::format::ObjectRef>)>) = (vec![], vec![]);
    for indexed in paths {
        let root = vault::logical(Path::new(&indexed));
        if !root.is_dir() { continue; }
        match (!vault::encrypted(&root)).then(|| crate::project::left_on_local(&root)).flatten() {
            Some(left) => readable.push((root, left)),
            None => locked.push(root),
        }
    }
    hold_locked(&conn, calcifer, from, &locked)?;
    if readable.iter().any(|(_, left)| !left.is_empty()) {
        let store = crate::runs::files::store().map_err(|error| cut(format!("this computer's TensorFS store: {}", error.message)))?;
        for (root, left) in &readable {
            let present: Vec<_> = left.iter().filter(|object| store.path(object).is_ok_and(|at| at.is_file())).cloned().collect();
            if present.is_empty() { continue; }
            hold(store, &kept_path(calcifer, root), present).map_err(|why| cut(format!("a project's files left on local couldn't be held ({why}): free space on that disk, then start cozy again")))?;
        }
    }
    let (mut lost, mut copied) = (vec![], vec![]);
    for (root, _) in readable {
        match crate::project::copy_left_on_local(&root, calcifer) {
            Ok(Some((_, 0))) => copied.push(root),
            Ok(Some((_, gone))) => {
                tracing::warn!(target: "cozy.service_health", code = "legacy_files_lost", gone, "files a project left on local were gone from this computer's store already");
                lost.push((root.clone(), gone as u64));
                copied.push(root);
            }
            // Unreadable since it was looked at: held as a locked one's are, and copied in as it opens.
            Ok(None) => { hold_locked(&conn, calcifer, from, std::slice::from_ref(&root))?; locked.push(root); }
            Err(why) => return Err(cut(format!("{}: {why}", root.display()))),
        }
    }
    for root in copied { crate::project::release_left(&root, calcifer); }
    Ok((locked.len(), lost))
}

/// What the old store's runs left on `local`, never exported, held for each of `locked` by the project its run names.
fn hold_locked(conn: &Connection, calcifer: &Path, from: i64, locked: &[PathBuf]) -> Result<()> {
    let (runs, outputs) = if from == 8 && has(conn, "main", "legacy_project_runs")? { ("legacy_project_runs", "legacy_project_outputs") } else { ("runs", "outputs") };
    if locked.is_empty() || !has(conn, "main", outputs)? { return Ok(()); }
    let left: Vec<(String, i64, Vec<u8>, String)> = {
        let mut stmt = conn.prepare(&format!("SELECT o.digest, o.length, substr(r.spec, 1, 4096), r.out_dir FROM {outputs} o JOIN {runs} r ON r.id = o.run
            WHERE r.machine = 'local' AND o.path IS NULL"))?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, Option<Vec<u8>>>(2)?.unwrap_or_default(), row.get(3)?)))?.collect::<rusqlite::Result<_>>()?
    };
    let store = crate::runs::files::store().map_err(|error| cut(format!("this computer's TensorFS store: {}", error.message)))?;
    for root in locked {
        let key = path_key(root);
        let kept: Vec<tensorfs::format::ObjectRef> = left.iter().filter(|(_, _, spec, out_dir)| {
            let owner = vault::payload_root(spec).ok().flatten().or_else(|| crate::project::of_landing(Path::new(out_dir)).map(Path::to_path_buf));
            owner.is_some_and(|owner| path_key(&vault::logical(&owner)) == key)
        }).map(|(digest, length, _, _)| tensorfs::format::ObjectRef { sha256: digest.trim_start_matches("sha256:").to_owned(), length: (*length).max(0) as u64 }).collect();
        if kept.is_empty() { continue; }
        hold(store, &kept_path(calcifer, root), kept).map_err(|why| cut(format!("a locked project's files left on local couldn't be held ({why}): free space on that disk, then start cozy again")))?;
    }
    Ok(())
}
