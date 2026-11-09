//! The model slots each package's App last loaded on this machine: what its import-only parent
//! builds ahead (their headers, config and `load()`, no device) when it next starts, so a cold
//! call's executor forks with them built. One file under TensorD's root, by package and App.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::catalog::HeldGeneration;
use crate::device_executor::Binding;

/// One construction's slots as its call loaded them (no lane, no interface).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recorded {
    pub construction: String,
    pub slots: Vec<Binding>,
}

static RECORDED: OnceLock<(PathBuf, Mutex<BTreeMap<String, Recorded>>)> = OnceLock::new();

/// Read the record under `root`, once per process.
pub fn open(root: &Path) {
    let path = root.join("build-ahead.json");
    let held = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    let _ = RECORDED.set((path, Mutex::new(held)));
}

fn key(held: &HeldGeneration) -> String {
    format!("{} {}", held.record.package, held.record.application)
}

/// What `held`'s App last loaded.
pub fn recorded(held: &HeldGeneration) -> Option<Recorded> {
    RECORDED.get()?.1.lock().unwrap_or_else(|e| e.into_inner()).get(&key(held)).cloned()
}

/// A call of `held` loads `construction`'s `slots`: its App's parent builds them when it next starts.
pub fn record(held: &HeldGeneration, construction: &str, slots: Vec<Binding>) {
    let Some((path, recorded)) = RECORDED.get() else { return };
    let entry = Recorded { construction: construction.into(), slots };
    let bytes = {
        let mut recorded = recorded.lock().unwrap_or_else(|e| e.into_inner());
        if recorded.get(&key(held)).is_some_and(|was| was.construction == entry.construction && was.slots == entry.slots) {
            return;
        }
        recorded.insert(key(held), entry);
        serde_json::to_vec(&*recorded)
    };
    let written = bytes.map_err(std::io::Error::other).and_then(|bytes| {
        let staged = path.with_extension("json.new");
        std::fs::write(&staged, bytes)?;
        std::fs::rename(&staged, path)
    });
    if let Err(error) = written {
        tracing::warn!("build-ahead.json: {error}");
    }
}
