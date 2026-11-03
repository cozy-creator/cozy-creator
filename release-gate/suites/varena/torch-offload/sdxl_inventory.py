"""Pinned real stock SDXL metadata topology; no weights, inference or native resources.

Optionally compare every parameter path/boundary to a preserved actual stock census.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
from pathlib import Path

import torch
from sdxl_components import components
from tensorfs.varena._offload_auto import _REVIEWED_FORWARDS, _declared_frontier
from tensorfs.varena._offload_inventory import _walk
from tensorfs.varena._offload_sdxl import module_state, reviewed_call, reviewed_forward


def inventory(recorded=None):
    models = components(reduced=False, device="meta")
    declarations = {}
    parameter_paths = []
    for root, model in models.items():
        paths = []
        for path, child, declared in _walk(model, root, frozenset()):
            if any(kind.__name__ in declared for kind in type(child).__mro__):
                paths.append(path)
            if isinstance(child, (torch.nn.ModuleList, torch.nn.Sequential)):
                children = [
                    (name, item)
                    for name, item in child._modules.items()
                    if item is not None
                ]
                if len(children) > 1 and len({type(item) for _, item in children}) == 1:
                    paths.extend(path + "." + name for name, _ in children)
        declarations.update(
            {
                path: model.get_submodule(path[len(root) + 1 :])
                for path in paths or [root]
            }
        )
        parameter_paths.extend(
            root + "." + name
            for name, _ in model.named_parameters(remove_duplicate=False)
        )
    frontier, nested = _declared_frontier(models, declarations)
    assert len(declarations) == 178 and len(frontier) == 83
    assert len(parameter_paths) == 2641
    matched = False
    snapshot_sha = None
    if recorded:
        data = recorded.read_bytes()
        snapshot = json.loads(data)
        actual_paths = [
            path for row in snapshot["current_versions"] for path in row["paths"]
        ]
        assert sorted(actual_paths) == sorted(parameter_paths)
        assert sorted(snapshot["initial_inventory"]["boundaries"]) == sorted(
            declarations
        )
        matched = True
        snapshot_sha = hashlib.sha256(data).hexdigest()
    rows = []
    for boundary in frontier:
        module = boundary.module
        reasons = []
        for child in module.modules():
            if (
                type(child) not in _REVIEWED_FORWARDS
                and reviewed_forward(child) is None
            ):
                reasons.append("unreviewed_module_effects")
            if not reviewed_call(child):
                reasons.append("unreviewed_module_call")
            reason, _ = module_state(child)
            if reason:
                reasons.append(reason)
        assert not reasons, (boundary.name, reasons)
        # Exact stock call effects, not inferred from the meta execution (none
        # occurs here). The real component client checks installed output hooks.
        if type(module).__name__ == "CLIPEncoderLayer":
            status = "eager_framework_output_collector"
        elif type(module).__name__ == "Transformer2DModel":
            status = "candidate_return_dict_false_only"
        else:
            status = "candidate_reviewed_eval"
        rows.append(
            {
                "path": boundary.name,
                "class": type(module).__module__ + "." + type(module).__name__,
                "stock_call_status": status,
                "module_configuration_reviewed": True,
            }
        )
    count = collections.Counter(row["stock_call_status"] for row in rows)
    assert count == {
        "eager_framework_output_collector": 44,
        "candidate_return_dict_false_only": 4,
        "candidate_reviewed_eval": 35,
    }
    return {
        "scope": "real full-width stock config meta models and source/config review; no inference, Source adoption, GPU, pool or automatic replay",
        "model": "stabilityai/stable-diffusion-xl-base-1.0",
        "revision": "462165984030d82259a11f4367a4eed129e94a7b",
        "parameter_paths": len(parameter_paths),
        "parameter_paths_sha256": hashlib.sha256(
            json.dumps(sorted(parameter_paths), separators=(",", ":")).encode()
        ).hexdigest(),
        "matches_preserved_stock_census": matched,
        "recorded_snapshot_sha256": snapshot_sha,
        "declared": len(declarations),
        "frontier": len(frontier),
        "subsumed": len(nested),
        "by_component": dict(
            collections.Counter(row["path"].split(".")[0] for row in rows)
        ),
        "by_class": dict(collections.Counter(row["class"] for row in rows)),
        "stock_call_status": dict(count),
        "boundaries": rows,
        "nested": nested,
        "cuda_initialized": torch.cuda.is_initialized(),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--recorded-snapshot", type=Path)
    args = parser.parse_args()
    torch.set_num_threads(1)
    torch.set_num_interop_threads(1)
    print(json.dumps(inventory(args.recorded_snapshot), indent=2))
