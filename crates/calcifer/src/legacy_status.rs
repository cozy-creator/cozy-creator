//! How much history from before projects kept their own is still to import (cl-984): a project's, and all Calcifer's store
//! holds, by the folder each old run names. Paul's projects are verified fully migrated by these before the hard cut.
use std::{collections::BTreeMap, path::Path};

use api::calcifer::{LegacyFolder, LegacyHistory, LegacyRows};

use crate::{project::{vault, ProjectStore}, store::{Result, Store}};

fn has_table(conn: &rusqlite::Connection, name: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)", [name], |row| row.get(0))?)
}

/// The project at `root` (`id`): runs its journal imported, and runs Calcifer's store still holds that are its own, each
/// decided as an import decides it (an unreadable record counts: the import waits on it).
pub(crate) fn project(store: &Store, journal: &ProjectStore, root: &Path, id: &str) -> Result<LegacyHistory> {
    let imported = journal.read(|conn| {
        if !has_table(conn, "journal_migrations")? { return Ok(0); }
        Ok(conn.query_row("SELECT count(*) FROM journal_migrations WHERE source LIKE 'controller-v7:run:%'", [], |row| row.get::<_, i64>(0))?)
    })? as u64;
    let root = vault::logical(root);
    let landing = crate::project::landing(&root);
    let pending = store.read(|conn| {
        if !has_table(conn, "legacy_project_runs")? || !has_table(conn, "legacy_run_owners")? { return Ok(0); }
        let candidates = crate::store::legacy_run_candidates(conn, &root, id)?;
        Ok(candidates.iter().filter(|(_, out_dir, spec)| match vault::payload_root(spec) {
            Ok(Some(owner)) => owner == root,
            Ok(None) => Path::new(out_dir) == landing,
            Err(_) => true,
        }).count() as u64)
    })?;
    Ok(LegacyHistory { imported, pending })
}

/// Every old project run Calcifer's store still holds, by the folder it names.
pub(crate) fn store(store: &Store) -> Result<LegacyRows> {
    store.read(|conn| {
        let mut rows = LegacyRows::default();
        if !has_table(conn, "legacy_project_runs")? || !has_table(conn, "legacy_run_owners")? { return Ok(rows); }
        let mut folders: BTreeMap<String, u64> = BTreeMap::new();
        {
            let mut statement = conn.prepare("SELECT o.owner_root, count(*) FROM legacy_run_owners o JOIN legacy_project_runs r ON r.id = o.id GROUP BY o.owner_root")?;
            for row in statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))? {
                let (root, runs) = row?;
                folders.insert(root, runs as u64);
            }
        }
        // `store.rs`'s owner of a record it could not read.
        rows.unreadable = folders.remove("*").unwrap_or(0);
        for (root, runs) in folders {
            let folder = LegacyFolder { path: root.into(), runs };
            if folder.path.is_dir() { rows.existing.push(folder) } else { rows.gone.push(folder) }
        }
        Ok(rows)
    })
}
