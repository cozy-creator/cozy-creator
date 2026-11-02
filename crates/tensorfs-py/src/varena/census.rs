//! Private metadata-only census; no GPU-owning objects cross back into Python.
use pyo3::{
    prelude::*,
    types::{PyDict, PyList},
};

#[pyfunction]
#[pyo3(signature = (handle, region_handles=Vec::new()))]
fn _varena_public_residency_snapshot<'py>(
    py: Python<'py>,
    handle: u64,
    region_handles: Vec<u64>,
) -> PyResult<Bound<'py, PyDict>> {
    let snapshot = py
        .detach(|| varena::abi::residency_snapshot(handle, &region_handles))
        .map_err(|error| super::raise(py, error))?;
    let result = PyDict::new(py);
    result.set_item("device", snapshot.device)?;
    result.set_item("anchor_arena_id", snapshot.anchor_arena_id)?;
    result.set_item(
        "inventory_generation_before",
        snapshot.inventory_generation_before,
    )?;
    result.set_item(
        "inventory_generation_after",
        snapshot.inventory_generation_after,
    )?;
    result.set_item("consistency", "sequential_arenas_not_atomic")?;
    let arenas = PyList::empty(py);
    for arena in snapshot.arenas {
        let row = PyDict::new(py);
        row.set_item("id", arena.id)?;
        row.set_item("granularity", arena.granularity)?;
        row.set_item(
            "inventory_generation_before",
            arena.inventory_generation_before,
        )?;
        row.set_item(
            "inventory_generation_after",
            arena.inventory_generation_after,
        )?;
        row.set_item("consistency", "state_locked_entry_atomics_sampled")?;
        row.set_item("fills", arena.fills)?;
        row.set_item("hits", arena.hits)?;
        row.set_item("evictions", arena.evictions)?;
        let entries = PyList::empty(py);
        for entry in arena.entries {
            let values = PyDict::new(py);
            values.set_item("id", entry.id)?;
            values.set_item("requested_bytes", entry.requested_bytes)?;
            values.set_item("reserved_bytes", entry.reserved_bytes)?;
            values.set_item("mapped_bytes", entry.mapped_bytes)?;
            values.set_item("contents", entry.contents)?;
            values.set_item("pins", entry.pins)?;
            values.set_item("unjoined", entry.unjoined)?;
            entries.append(values)?;
        }
        row.set_item("entries", entries)?;
        arenas.append(row)?;
    }
    result.set_item("arenas", arenas)?;
    let finalizing = PyList::empty(py);
    for arena in snapshot.finalizing {
        let row = PyDict::new(py);
        row.set_item("id", arena.id)?;
        row.set_item("physical_bytes", arena.physical_bytes)?;
        row.set_item("consistency", "physical_bytes_atomic_sample")?;
        finalizing.append(row)?;
    }
    result.set_item("finalizing", finalizing)?;
    let links = PyList::empty(py);
    for link in snapshot.region_handles {
        let row = PyDict::new(py);
        row.set_item("handle", link.handle)?;
        row.set_item("arena_id", link.arena_id)?;
        row.set_item("region_id", link.region_id)?;
        links.append(row)?;
    }
    result.set_item("region_handles", links)?;
    Ok(result)
}

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(_varena_public_residency_snapshot, module)?)
}
