"""Offline synthetic census partitions; no framework/native imports or GPU work."""

from __future__ import annotations

import copy
import json

from partition_census import partition


def sample(versions, requested=264):
    return {
        "anchor_arena_id": 1,
        "inventory_generation_before": 1,
        "inventory_generation_after": 1,
        "consistency": "sequential_arenas_not_atomic",
        "arenas": [{
            "id": 1, "granularity": 2 << 20,
            "inventory_generation_before": 1, "inventory_generation_after": 1,
            "consistency": "state_locked_entry_atomics_sampled",
            "fills": 0, "hits": 0, "evictions": 0,
            "entries": [{"id": 7, "requested_bytes": requested,
                         "reserved_bytes": 2 << 20, "mapped_bytes": 2 << 20,
                         "contents": "ready"}],
        }],
        "current_versions": versions,
        "finalizing": [],
        "initial_inventory": {"host_storage_bytes": 12},
        "intent_pending_refresh": False,
    }


versions = [
    {"arena_id": 1, "region_id": 7, "member_offset": 0, "source_bytes": 4},
    {"arena_id": 1, "region_id": 7, "member_offset": 256, "source_bytes": 8},
]
full = partition(sample(versions))
assert full["current_versions"] == 2 and full["current_unique_regions"] == 1
assert full["groups"]["current"]["regions"] == 1
assert full["current_source_bytes"] == full["current_original_storage_logical_bytes"] == 12
assert full["current_group_requested_bytes_observed"] == 264
assert full["current_group_alignment_gap_bytes_known"] == 252
assert full["current_groups_with_unknown_original_layout"] == 0
for remaining, residual in ((versions[:1], 260), (versions[1:], 256)):
    partial = partition(sample(remaining))
    layout = partial["current_region_layouts"][0]
    assert not layout["complete_original_layout_known"]
    assert layout["alignment_gap_bytes"] is None
    assert layout["unattributed_old_member_or_gap_bytes"] == residual
    assert partial["groups"]["current"]["regions"] == 1
historical = copy.deepcopy(versions[:1])
historical[0].pop("member_offset")
assert partition(sample(historical, requested=4))["current_group_alignment_gap_bytes_known"] == 0
missing = sample(versions)
missing["arenas"][0]["entries"] = []
unknown = partition(missing)
assert unknown["current_regions_not_in_snapshot"] == [[1, 7]]
assert unknown["current_region_layouts"][0]["requested_bytes"] is None
print(json.dumps({"passed": True, "scope": "offline metadata only", "checks": [
    "two storage versions share one counted native Region",
    "complete pack reports exact 256-byte alignment gaps",
    "partial casts preserve unknown old-member-or-gap residual",
    "historical singleton and absent native entry remain explicit",
]}))
