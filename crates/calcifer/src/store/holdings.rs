//! What Calcifer's own (project-less) runs put on each machine: the checkpoints they asked for or made, the installations they
//! completed and the warm items they set. A machine's picture shows only these of what it holds (8d424127c). Folded from the
//! journal once, then from what is new or unsettled since, and only after a commit: a picture never reads a machine's history.
use std::collections::{BTreeMap, HashMap, HashSet};

use api::{calcifer::Status, machine as v1};
use rusqlite::params;
use serde_json::Value;

use super::{RUN_COLUMNS, Result, RunRow, State, Store, run_row};

/// How many runs one read folds: the reader is let go between reads.
const CHUNK: i64 = 256;

#[derive(Clone, Default)]
pub(crate) struct Held {
    pub manifests: HashSet<(String, String)>,
    pub installations: HashSet<String>,
    /// Its runs' warm items, without what a Status says of them.
    warm: Vec<v1::WarmItem>,
}

impl Held {
    fn add(&mut self, other: &Held) {
        self.manifests.extend(other.manifests.iter().cloned());
        self.installations.extend(other.installations.iter().cloned());
        for item in &other.warm {
            if !self.warm.contains(item) {
                self.warm.push(item.clone());
            }
        }
    }

    fn of(row: &RunRow) -> Held {
        let mut held = Held::default();
        let set = row.spec.set.iter().flat_map(|set| &set.items);
        for choice in row.spec.models.iter().chain(set.clone().flat_map(|item| &item.models)) {
            if !choice.model.is_empty() && !choice.manifest.is_empty() {
                held.manifests.insert((choice.model.clone(), choice.manifest.clone()));
            }
        }
        for item in set {
            let item = bare(item);
            if !held.warm.contains(&item) {
                held.warm.push(item);
            }
        }
        let result = (row.status == Some(Status::Completed)).then(|| row.outcome.as_ref()?.result.as_ref()).flatten();
        if let Some(result) = result.and_then(|result| serde_json::from_str::<Value>(result.get()).ok()) {
            if let Some(id) = result.get("installation").and_then(Value::as_str) {
                held.installations.insert(id.to_owned());
            }
            for model in result.get("models").and_then(Value::as_array).into_iter().flatten() {
                let text = |pointer: &str| model.pointer(pointer).and_then(Value::as_str).map(str::to_owned);
                if let (Some(name), Some(commit)) = (text("/repository"), text("/checkpoint")) {
                    held.manifests.insert((name, commit));
                }
                // What the run made: an install's local alias, or an upload's commit, held by its destination.
                if let (Some(name), Some(commit)) = (text("/published/destination"), text("/published/checkpoint")) {
                    held.manifests.insert((name, commit));
                }
            }
        }
        held
    }

    /// The items of `items` its runs set.
    pub(crate) fn warm(&self, items: Vec<v1::WarmItem>) -> Vec<v1::WarmItem> {
        items.into_iter().filter(|item| self.warm.contains(&bare(item))).collect()
    }
}

fn bare(item: &v1::WarmItem) -> v1::WarmItem {
    let mut item = item.clone();
    item.holds.clear();
    item.held_back.clear();
    item
}

/// The journal folded so far. A settled run's machine, spec and result don't change but by a machine's move, which starts the
/// fold over; an unsettled one is read again until it settles. A run that can't be opened (a locked journal's) holds nothing.
#[derive(Default)]
pub(super) struct Fold {
    /// The reader's `data_version` when it was last read; none: nothing read yet.
    version: Option<i64>,
    /// The newest run number read.
    through: i64,
    settled: HashMap<String, Held>,
    unsettled: BTreeMap<i64, (Option<String>, Held)>,
}

impl Fold {
    /// Reads what `store` committed since the last read, a chunk at a time.
    pub(super) fn refresh(&mut self, store: &Store) -> Result<()> {
        // Moves with every commit of another connection: this one only reads.
        let version: i64 = store.read(|conn| Ok(conn.query_row("PRAGMA data_version", [], |r| r.get(0))?))?;
        if self.version == Some(version) {
            return Ok(());
        }
        let unsettled: Vec<i64> = self.unsettled.keys().copied().collect();
        let rows = store.read(|conn| {
            let mut statement = conn.prepare(&format!("SELECT {RUN_COLUMNS} FROM runs WHERE number = ?1"))?;
            let mut rows = vec![];
            for number in &unsettled {
                rows.push((*number, statement.query_map([number], run_row)?.next().and_then(|row| row.ok()?.ok())));
            }
            Ok(rows)
        })?;
        for (number, row) in rows {
            self.unsettled.remove(&number);
            if let Some(row) = row {
                self.fold(&row);
            }
        }
        loop {
            let rows = store.read(|conn| {
                let mut statement = conn.prepare(&format!("SELECT {RUN_COLUMNS} FROM runs WHERE number > ?1 ORDER BY number LIMIT ?2"))?;
                let rows = statement.query_map(params![self.through, CHUNK], |r| Ok((r.get::<_, i64>(1)?, run_row(r).ok().and_then(|row| row.ok()))))?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            })?;
            for (number, row) in &rows {
                self.through = *number;
                if let Some(row) = row {
                    self.fold(row);
                }
            }
            if rows.len() < CHUNK as usize {
                break;
            }
        }
        self.version = Some(version);
        Ok(())
    }

    fn fold(&mut self, row: &RunRow) {
        // A run of a project adapter, from when there were any, is no one's to read (`Store::runs`).
        if row.target.to_ascii_lowercase().contains("/project-adapter-") {
            return;
        }
        let held = Held::of(row);
        match row.state {
            State::Settled => {
                if let Some(machine) = &row.machine {
                    self.settled.entry(machine.clone()).or_default().add(&held);
                }
            }
            _ => {
                self.unsettled.insert(row.number as i64, (row.machine.clone(), held));
            }
        }
    }

    /// What `machine`'s runs put on it.
    pub(super) fn of(&self, machine: &str) -> Held {
        let mut held = self.settled.get(machine).cloned().unwrap_or_default();
        for (_, unsettled) in self.unsettled.values().filter(|(on, _)| on.as_deref() == Some(machine)) {
            held.add(unsettled);
        }
        held
    }
}
