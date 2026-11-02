"""Current offload Versions plus private native residency metadata; no GPU work.

Call snapshot(owner) outside the existing timed generation. This never creates a
Region, clones a tensor, reads tensor storage, collects garbage, trims, or waits.
Old native ownership remains unknown; retired mapped entries are reported by Rust.
"""

from __future__ import annotations

import weakref


def snapshot(owner):
    from tensorfs import _ext

    with owner._owner.lock:
        if owner._owner.gpu is None:
            raise ValueError("residency census requires an existing GPU offload owner")
        versions = {}
        for component, root in owner._roots.items():
            for path, parameter in root.named_parameters(remove_duplicate=False):
                version = parameter._version_owner
                key = id(version)
                if key not in versions:
                    versions[key] = {
                        "reference": weakref.ref(version),
                        "paths": [],
                        "dtype": version.source.dtype,
                        "source_bytes": version.source.nbytes,
                        "region_handle": (
                            version.region._handle if version.region is not None else None
                        ),
                    }
                versions[key]["paths"].append(f"{component}.{path}")
        # Do not retain the loop's last Parameter or Version across observation.
        version = parameter = root = None
        handles = sorted(
            {row["region_handle"] for row in versions.values() if row["region_handle"]}
        )
        result = _ext._varena_public_residency_snapshot(
            owner._owner.gpu._handle, handles
        )
        links = {row["handle"]: row for row in result["region_handles"]}
        current = []
        for identity, row in versions.items():
            if row.pop("reference")() is None:
                raise RuntimeError("current Version changed during census")
            link = links.get(row["region_handle"])
            current.append(
                {
                    "python_version_identity": identity,
                    **row,
                    "arena_id": link["arena_id"] if link else None,
                    "region_id": link["region_id"] if link else None,
                }
            )
        result["current_versions"] = current
        result["initial_inventory"] = owner.facts
        result["intent_pending_refresh"] = owner._intent_dirty
        result["old_owner_attribution"] = "unknown"
        return result
