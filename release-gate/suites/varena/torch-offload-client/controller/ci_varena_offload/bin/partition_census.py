"""Offline scalar-only partitions of saved residency snapshots; no framework import."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def partition(snapshot):
    anchor = snapshot["anchor_arena_id"]
    versions = snapshot["current_versions"]
    current = {
        (row["arena_id"], row["region_id"])
        for row in versions
        if row["region_id"] is not None
    }
    groups = {
        name: {
            "regions": 0,
            "requested_bytes": 0,
            "reserved_bytes": 0,
            "mapped_bytes": 0,
            "mapped_requested_bytes": 0,
            "mapped_padding_bytes": 0,
        }
        for name in (
            "current", "noncurrent_ready", "retired", "noncurrent_other", "other_arenas"
        )
    }
    seen = set()
    current_retired = []
    arenas = []
    for arena in snapshot["arenas"]:
        arenas.append({key: arena[key] for key in (
            "id", "granularity", "inventory_generation_before",
            "inventory_generation_after", "consistency", "fills", "hits", "evictions",
        )})
        for entry in arena["entries"]:
            key = (arena["id"], entry["id"])
            assert key not in seen, f"duplicate native Region id: {key}"
            seen.add(key)
            requested, reserved, mapped = (
                entry["requested_bytes"], entry["reserved_bytes"], entry["mapped_bytes"]
            )
            assert 0 <= requested <= reserved and mapped in (0, reserved), entry
            if key in current and entry["contents"] == "retired":
                current_retired.append(list(key))
            if arena["id"] != anchor:
                name = "other_arenas"
            elif entry["contents"] == "retired":
                name = "retired"
            elif key in current:
                name = "current"
            elif entry["contents"] == "ready":
                name = "noncurrent_ready"
            else:
                name = "noncurrent_other"
            group = groups[name]
            group["regions"] += 1
            group["requested_bytes"] += requested
            group["reserved_bytes"] += reserved
            group["mapped_bytes"] += mapped
            if mapped:
                group["mapped_requested_bytes"] += requested
                group["mapped_padding_bytes"] += mapped - requested
    finalizing = {
        "arenas": snapshot["finalizing"],
        "physical_bytes": sum(row["physical_bytes"] for row in snapshot["finalizing"]),
        "requested_bytes": None,
        "reserved_bytes": None,
    }
    return {
        "anchor_arena_id": anchor,
        "inventory_generation_before": snapshot["inventory_generation_before"],
        "inventory_generation_after": snapshot["inventory_generation_after"],
        "inventory_changed": snapshot["inventory_generation_before"] != snapshot["inventory_generation_after"],
        "consistency": snapshot["consistency"],
        "matching_generations_do_not_make_state_atomic": True,
        "arena_observations": arenas,
        "groups": groups,
        "finalizing": finalizing,
        "sampled_mapped_total": sum(g["mapped_bytes"] for g in groups.values()) + finalizing["physical_bytes"],
        "current_versions": len(versions),
        "current_source_bytes": sum(row["source_bytes"] for row in versions),
        "current_unreserved_versions": sum(row["region_id"] is None for row in versions),
        "current_regions_not_in_snapshot": [list(key) for key in sorted(current - seen)],
        "current_retired_overlap": current_retired,
        "current_regions_outside_anchor": [list(key) for key in sorted(current) if key[0] != anchor],
        "initial_host_storage_bytes": snapshot["initial_inventory"]["host_storage_bytes"],
        "intent_pending_refresh": snapshot["intent_pending_refresh"],
        "old_owner_attribution": "unknown",
        "notes": [
            "Each native Region counted once; requested bytes are not physical residency.",
            "Other arenas are separate from anchor ownership; finalizing logical sizes are unknown.",
            "Noncurrent ready/other entries do not identify Plan, export, or retirement owners.",
            "Per-arena state locks and atomic samples are not an atomic device-wide observation.",
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshots", type=Path, nargs="+")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = {str(path): partition(json.loads(path.read_text())) for path in args.snapshots}
    with args.output.open("x") as output:
        output.write(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
