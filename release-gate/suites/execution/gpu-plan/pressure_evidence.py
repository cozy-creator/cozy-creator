"""Validate real CLI/machine receipts using distinct Hub and executor request IDs."""

import json


def memory_notes(text, began_ns):
    notes = []
    for line in text.splitlines():
        try:
            stamp, payload = json.loads(line)["text"].split(" ", 1)
            note = json.loads(payload)
            at_ns = int(float(stamp) * 1e9)
            if at_ns >= began_ns:
                notes.append({"at_ns": at_ns, **note})
        except (ValueError, KeyError, TypeError):
            continue
    return notes


def qualify(receipt, run, notes):
    assert run["id"] == receipt["run_id"] and run["status"] == "canceled"
    assert not receipt.get("manual_cleanup_cancel", False)
    measurements = run["measurements"]
    software = measurements["software"]
    assert software["tensord_commit"] == receipt["machine"]["commit"]
    for name in ("runtime", "tensorfs"):
        assert software[name] == receipt["machine"][name]
    assert any(
        gpu["pid"] == receipt["managed_ready"]["pid"]
        for gpu in measurements["execution"]["gpus"]
    )
    requests = {
        str(row["stats"]["request"])
        for row in measurements["memory"]["stats"]["varena"]
    }
    assert len(requests) == 1, requests
    request = requests.pop()
    cancellations = [
        n
        for n in notes
        if n.get("event") == "gpu_pressure_cancel" and n.get("root") == request
    ]
    assert len(cancellations) == 1, cancellations
    observed = next(
        n
        for n in notes
        if n.get("event") == "gpu_pressure"
        and n.get("blocked")
        and n["at_ns"] >= receipt["foreign_ready"]["growth_started_ns"]
    )
    cleared = next(
        n
        for n in notes
        if n.get("event") == "gpu_pressure"
        and n.get("blocked") is False
        and n["at_ns"] > observed["at_ns"]
    )
    cleanup = receipt["managed_cleanup"]
    assert cleanup["pinned_bytes"] == 0 and cleanup["freed_bytes"] >= 256 << 20
    assert cleanup["at_ns"] >= observed["at_ns"]
    assert receipt["foreign_exit"] == 0
    assert receipt["foreign_ready"]["parent_pid"] == receipt["controller_pid"]
    assert receipt["foreign_result"]["passed"]
    assert receipt["foreign_result"]["additional_used_bytes"] >= 128 << 20
    assert cleared["foreign_bytes"] <= cleared["baseline_foreign_bytes"] + (64 << 20)
    assert observed["own_bytes"] - cleared["own_bytes"] >= 256 << 20
    return {
        "executor_request_id": request,
        "observation_to_native_release_ms": (cleanup["at_ns"] - observed["at_ns"])
        / 1e6,
        "observed_owned_bytes_released": observed["own_bytes"] - cleared["own_bytes"],
        # Logged after engine.cancel returns, potentially after the native cleanup.
        "cancel_ack_logged_ns": cancellations[0]["at_ns"],
        "admission_cleared_ns": cleared["at_ns"],
    }
