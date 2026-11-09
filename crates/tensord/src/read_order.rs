//! The order a call first filled its components, as its last run on this machine recorded it
//! (varena's fill trace): what a cold call reads ahead first. One file under TensorD's root,
//! each `manifest/component` by its rank in its last run.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static ORDER: OnceLock<(PathBuf, Mutex<HashMap<String, u64>>)> = OnceLock::new();

/// Read the recorded order under `root`, once per process.
pub fn open(root: &Path) {
    let path = root.join("read-order.json");
    let order = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    let _ = ORDER.set((path, Mutex::new(order)));
}

/// A component's key, as a plan slot (`sha256:…`) and a varena set (`…/sha256:…/component`)
/// both name it.
pub fn key(manifest: &str, component: &str) -> String {
    format!("{}/{component}", manifest.rsplit("sha256:").next().unwrap_or(manifest))
}

/// Where `key` stood among its last run's first fills.
pub fn rank(key: &str) -> Option<u64> {
    ORDER.get()?.1.lock().unwrap_or_else(|e| e.into_inner()).get(key).copied()
}

/// A settled request's varena rows (`memory.varena`): its sets in the order they were first
/// filled replace what the machine knew of them.
pub fn record(varena: &serde_json::Value) {
    let Some((path, order)) = ORDER.get() else { return };
    let mut fills: Vec<(u64, &str)> = varena.as_array().into_iter().flatten()
        .filter_map(|row| row.pointer("/stats/trace")?.as_array())
        .flatten()
        .filter_map(|fill| Some((fill.get("sequence")?.as_u64()?, fill.get("set")?.as_str()?)))
        .collect();
    fills.sort_unstable();
    let mut first: Vec<String> = Vec::new();
    for (_, set) in fills {
        if let Some((manifest, component)) = set.rsplit_once('/') {
            let key = key(manifest, component);
            if !first.contains(&key) {
                first.push(key);
            }
        }
    }
    if first.is_empty() {
        return;
    }
    let bytes = {
        let mut order = order.lock().unwrap_or_else(|e| e.into_inner());
        order.extend(first.into_iter().zip(0..));
        serde_json::to_vec(&*order)
    };
    let written = bytes.map_err(std::io::Error::other).and_then(|bytes| {
        let staged = path.with_extension("json.new");
        std::fs::write(&staged, bytes)?;
        std::fs::rename(&staged, path)
    });
    if let Err(error) = written {
        tracing::warn!("read-order.json: {error}");
    }
}
