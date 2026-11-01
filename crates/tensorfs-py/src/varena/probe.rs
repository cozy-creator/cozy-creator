//! Diagnostic-only CPU metadata bindings. Calls stay on the benchmark's OS thread.
use pyo3::prelude::*;
use serde_json::{json, Value};
use varena::probe;

fn snapshot(value: probe::Snapshot) -> Value {
    json!({
        "schema": "varena.allocator_probe/1", "clock": "std::time::Instant",
        "time_unit": "nanoseconds_since_reset", "stride": probe::STRIDE,
        "capacity": probe::CAPACITY, "points": probe::NAMES,
        "seen": {"alloc": value.seen[0], "free": value.seen[1]},
        "dropped": value.dropped, "nested": value.nested,
        "operations": ["alloc", "free"], "flags": {"success": 1, "slow": 2, "nested": 4},
        "columns": ["operation", "sequence", "requested_bytes", "capacity_bytes",
                    "flags", "present", "duplicate", "ns"],
        "records": value.records.iter().map(|record| json!([
            record.operation, record.sequence, record.requested, record.capacity,
            u8::from(record.success) | (u8::from(record.slow) << 1) | (u8::from(record.nested) << 2),
            record.present, record.duplicate, record.ns,
        ])).collect::<Vec<_>>()
    })
}

#[pyfunction]
fn _varena_allocator_probe_reset(py: Python<'_>) -> PyResult<()> {
    probe::reset().map_err(|error| super::raise(py, error))
}

#[pyfunction]
fn _varena_allocator_probe_read(py: Python<'_>) -> PyResult<String> {
    probe::read()
        .map(|value| snapshot(value).to_string())
        .map_err(|error| super::raise(py, error))
}

#[pyfunction]
fn _varena_allocator_probe_calibrate(py: Python<'_>) -> PyResult<String> {
    let value = probe::calibrate().map_err(|error| super::raise(py, error))?;
    Ok(json!({
        "schema": "varena.allocator_probe_calibration/1",
        "clock_pairs_ns": value.clock_pairs_ns,
        "empty_markers": snapshot(value.empty_markers),
        "empty_markers_selection": "128 forced records per operation, not the real-loop stride",
        "dispatch_iterations": value.dispatch_iterations,
        "dispatch_unit": "one enter/exit callback, not an alloc/free pair; loop overhead included",
        "disabled_dispatch_ns": value.disabled_dispatch_ns,
        "enabled_dispatch_ns": value.enabled_dispatch_ns,
        "empty_bundle_pairs": value.bundle_pairs,
        "disabled_empty_bundle_ns": value.disabled_bundle_ns,
        "enabled_empty_bundle_ns": value.enabled_bundle_ns,
        "empty_bundle_meaning": "each pair runs alloc then free enter/all marks/allocation-metadata/exit in actual point order, including every unsampled helper and occasional sampled clocks; no real allocation; loop overhead included",
        "meaning": "CPU-only empty clock/marker spans and enter/exit loops; no allocator operation; raw overhead, not automatic subtraction",
    }).to_string())
}

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(_varena_allocator_probe_reset, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_allocator_probe_read, module)?)?;
    module.add_function(wrap_pyfunction!(_varena_allocator_probe_calibrate, module)?)?;
    Ok(())
}
