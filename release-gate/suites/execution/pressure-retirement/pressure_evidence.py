"""Hard-retirement evidence; a killed executor need not emit cleanup or Invoke statistics."""

import json


def documents(text):
    decoder = json.JSONDecoder()
    while text.strip():
        value, end = decoder.raw_decode(text.lstrip())
        yield value
        text = text.lstrip()[end:]


def memory_notes(text, began_ns):
    notes = []
    for row in documents(text):
        stamp, payload = row.get("text", "").split(" ", 1)
        seconds, dot, fraction = stamp.partition(".")
        at_ns = int(seconds) * 10**9 + int(fraction[:9].ljust(9, "0") if dot else 0)
        if at_ns >= began_ns:
            notes.append({"at_ns": at_ns, **json.loads(payload)})
    return notes


def qualify(receipt, run, notes):
    assert run["id"] == receipt["run_id"] and run["status"] == "canceled", run
    assert not receipt.get("manual_cleanup_cancel", False)
    request = receipt["managed_ready"]["executor_request_id"]
    cancellations = [
        row
        for row in notes
        if row.get("event") == "gpu_pressure_cancel" and row.get("root") == request
    ]
    assert len(cancellations) == 1, cancellations
    observed = next(
        row
        for row in notes
        if row.get("event") == "gpu_pressure"
        and row.get("blocked")
        and row["at_ns"] >= receipt["foreign_ready"]["growth_started_ns"]
    )
    cleared = next(
        row
        for row in notes
        if row.get("event") == "gpu_pressure"
        and row.get("blocked") is False
        and row["at_ns"] > observed["at_ns"]
    )
    assert receipt["executor_exit"]["pidfd_readable"]
    assert receipt["executor_exit"]["at_ns"] >= observed["at_ns"]
    assert receipt["foreign_exit"] == 0 and receipt["foreign_result"]["passed"]
    assert receipt["foreign_ready"]["parent_pid"] == receipt["controller_pid"]
    assert receipt["foreign_result"]["additional_used_bytes"] >= 128 << 20
    freed = (
        receipt["foreign_result"]["observed_free_after_yield"]
        - receipt["foreign_ready"]["free_bytes"]
    )
    assert freed >= 256 << 20, freed
    assert observed["own_bytes"] - cleared["own_bytes"] >= 256 << 20
    assert cleared["foreign_bytes"] <= cleared["baseline_foreign_bytes"] + (64 << 20)
    successor = receipt["successor"]
    assert successor["status"] == "completed" and successor["result"]["passed"]
    result = json.loads(successor["result"]["detail"])
    assert result["pid"] != receipt["managed_ready"]["pid"] and result["value"] == 32640
    return {
        "automatic_cancel_actor": "machine:gpu_pressure",
        "executor_request_id": request,
        "observed_driver_bytes_released": freed,
        "observation_to_exact_exit_ms": (
            receipt["executor_exit"]["at_ns"] - observed["at_ns"]
        )
        / 1e6,
        "admission_cleared_ns": cleared["at_ns"],
        "successor_pid": result["pid"],
    }
