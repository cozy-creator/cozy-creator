"""Offline scalar-only partitions of saved residency snapshots; no framework import."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def packed_current_regions(snapshot):
    """Separate original storage payload from unique native Region requests.

    A partial dtype conversion can leave only some original pack members current.
    Such missing bytes include unknown old payload, not just alignment padding.
    """
    members = {}
    for version in snapshot["current_versions"]:
        if version["region_id"] is not None:
            key = version["arena_id"], version["region_id"]
            members.setdefault(key, []).append(version)
    entries = {
        (arena["id"], entry["id"]): entry
        for arena in snapshot["arenas"]
        for entry in arena["entries"]
    }
    result = []
    for key, versions in sorted(members.items()):
        entry = entries.get(key)
        requested = entry["requested_bytes"] if entry else None
        logical = sum(row["source_bytes"] for row in versions)
        # Historical singleton snapshots predate the explicit member offset.
        offsets = [
            row.get("member_offset", 0 if len(versions) == 1 else None)
            for row in versions
        ]
        spans = sorted(
            (offset, row["source_bytes"])
            for offset, row in zip(offsets, versions)
            if offset is not None
        )
        complete = requested is not None and len(spans) == len(versions)
        end, alignment = 0, 0
        for offset, size in spans:
            assert offset >= end and size >= 0, (key, spans)
            aligned = (end + 255) // 256 * 256
            complete = complete and offset == aligned
            alignment += offset - end
            end = offset + size
        complete = complete and end == requested
        if requested is not None:
            assert logical <= requested and end <= requested, (key, requested, spans)
        result.append({
            "arena_id": key[0],
            "region_id": key[1],
            "current_storage_versions": len(versions),
            "current_storage_logical_bytes": logical,
            "requested_bytes": requested,
            "complete_original_layout_known": complete,
            "alignment_gap_bytes": alignment if complete else None,
            "unattributed_old_member_or_gap_bytes": (
                0 if complete else requested - logical if requested is not None else None
            ),
            "current_member_spans": [
                {"offset": offset, "source_bytes": size} for offset, size in spans
            ],
        })
    return result


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
    packed = packed_current_regions(snapshot)
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
        "current_original_storage_logical_bytes": sum(row["source_bytes"] for row in versions),
        "current_unique_regions": len(current),
        "current_region_layouts": packed,
        "current_group_requested_bytes_observed": sum(
            row["requested_bytes"] for row in packed if row["requested_bytes"] is not None
        ),
        "current_group_alignment_gap_bytes_known": sum(
            row["alignment_gap_bytes"] for row in packed if row["alignment_gap_bytes"] is not None
        ),
        "current_groups_with_unknown_original_layout": sum(
            not row["complete_original_layout_known"] for row in packed
        ),
        "current_unreserved_versions": sum(row["region_id"] is None for row in versions),
        "current_regions_not_in_snapshot": [list(key) for key in sorted(current - seen)],
        "current_retired_overlap": current_retired,
        "current_regions_outside_anchor": [list(key) for key in sorted(current) if key[0] != anchor],
        "initial_host_storage_bytes": snapshot["initial_inventory"]["host_storage_bytes"],
        "intent_pending_refresh": snapshot["intent_pending_refresh"],
        "old_owner_attribution": "unknown",
        "notes": [
            "Each native Region counted once; requested bytes are not physical residency.",
            "Current Source bytes count original storages; native group requests include alignment and may retain noncurrent members.",
            "Exact 256-byte alignment gaps require complete original member coverage; partial layouts retain an unknown old-member-or-gap residual.",
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
