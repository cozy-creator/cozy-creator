"""Fast source/metadata checks only; imports no Torch, TensorFS, or model."""
from __future__ import annotations

import ast
import hashlib
import json
import runpy
from pathlib import Path

ROOT = Path(__file__).resolve().parent
BIN = ROOT / "controller/ci_varena_offload/bin"


def tree(path):
    return ast.parse(path.read_text())


def normalized(node):
    return ast.dump(node, include_attributes=False)


def main():
    contract = json.loads((ROOT / "evidence/census-source/baseline-contract.json").read_text())
    after = tree(BIN / "sdxl_census.py")
    def node_sha(node):
        return hashlib.sha256(normalized(node).encode()).hexdigest()
    for name, expected in contract["functions"].items():
        current = next(n for n in after.body if isinstance(n, ast.FunctionDef) and n.name == name)
        assert node_sha(current) == expected, name
    after_asserts = {node_sha(n) for n in ast.walk(after) if isinstance(n, ast.Assert)}
    assert set(contract["assertions"]) <= after_asserts
    for attribute, key in (("assert_allclose", "allclose_calls"), ("from_pretrained", "model_load_calls")):
        assert [node_sha(n) for n in ast.walk(after) if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr == attribute] == contract[key]
    assert sum(isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id == "run" for n in ast.walk(after)) == 2
    assert hashlib.sha256((BIN / "residency_census.py").read_bytes()).hexdigest() == contract["census_helper_sha256"]
    loops = [n for n in after.body if isinstance(n, ast.For) and normalized(n.iter) == normalized(ast.parse("range(2)", mode="eval").body)]
    assert len(loops) == 2
    for loop in loops:
        run = next(i for i, n in enumerate(loop.body) if isinstance(n, ast.Assign) and isinstance(n.value, ast.Call) and isinstance(n.value.func, ast.Name) and n.value.func.id == "run")
        assert isinstance(loop.body[run - 1], ast.Assign) and normalized(loop.body[run - 1].value) == normalized(ast.parse("time.perf_counter()", mode="eval").body)
        assert isinstance(loop.body[run + 1], ast.Assign) and isinstance(loop.body[run + 1].value, ast.BinOp)
    assert sum(isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id == "capture_census" for n in ast.walk(loops[0])) == 0
    assert sum(isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id == "capture_census" for n in ast.walk(loops[1])) == 2
    assert sum(isinstance(n, ast.Expr) and isinstance(n.value, ast.Call) and isinstance(n.value.func, ast.Name) and n.value.func.id == "capture_census" for n in after.body) == 1
    after_sync = [node_sha(n) for n in ast.walk(after) if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr == "synchronize"]
    assert contract["synchronizations"] == after_sync
    assert hashlib.sha256((BIN / "sdxl_gpu.py").read_bytes()).hexdigest() == "c553ce93e371843a367da4565c7cbaf0306dd0bca37db7145355cf4bc57e9db1"
    assert hashlib.sha256((BIN / "model-layout.json").read_bytes()).hexdigest() == "720a7556edda49ff7d1e195d58f24a8c3faf906dea0e0f299252a5c52ab1275f"
    for path in [ROOT / "prepare.py", *ROOT.glob("controller/ci_varena_offload/*.py"), *BIN.glob("*.py")]:
        tree(path)

    partition = runpy.run_path(str(BIN / "partition_census.py"))["partition"]
    def entry(i, requested, reserved, mapped, contents):
        return {"id": i, "requested_bytes": requested, "reserved_bytes": reserved, "mapped_bytes": mapped, "contents": contents}
    def arena(i, entries):
        return {"id": i, "granularity": 8, "inventory_generation_before": 10, "inventory_generation_after": 11, "consistency": "state_locked_entry_atomics_sampled", "fills": 5, "hits": 20, "evictions": 2, "entries": entries}
    sample = {
        "anchor_arena_id": 1, "inventory_generation_before": 10, "inventory_generation_after": 11,
        "consistency": "sequential_arenas_not_atomic", "intent_pending_refresh": True,
        "initial_inventory": {"host_storage_bytes": 9},
        "current_versions": [{"arena_id": 1, "region_id": 1, "source_bytes": 5}, {"arena_id": None, "region_id": None, "source_bytes": 4}],
        "arenas": [arena(1, [entry(1, 5, 8, 8, "ready"), entry(2, 2, 8, 8, "retired"), entry(3, 3, 8, 8, "ready"), entry(4, 1, 8, 0, "cold")]), arena(2, [entry(5, 2, 8, 8, "ready")])],
        "finalizing": [{"id": 3, "physical_bytes": 16, "consistency": "physical_bytes_atomic_sample"}],
    }
    result = partition(sample)
    assert result["groups"]["current"]["mapped_padding_bytes"] == 3
    assert result["groups"]["retired"]["mapped_bytes"] == 8
    assert result["groups"]["noncurrent_ready"]["requested_bytes"] == 3
    assert result["groups"]["noncurrent_other"]["mapped_bytes"] == 0
    assert result["groups"]["other_arenas"]["mapped_bytes"] == 8
    assert result["sampled_mapped_total"] == 48
    assert result["finalizing"]["requested_bytes"] is None
    assert result["inventory_changed"] and result["matching_generations_do_not_make_state_atomic"]
    assert result["old_owner_attribution"] == "unknown" and result["current_unreserved_versions"] == 1
    print(json.dumps({"source_assertions_preserved": True, "strict_stock_script_unchanged": True, "fixed_baseline_calls": 2, "fixed_adopted_calls": 2, "census_snapshots_outside_timers": 5, "extra_synchronization": False, "offline_scalar_partition": "pass synthetic metadata only", "torch_or_model_imported": False, "gpu_executed": False}, sort_keys=True))


if __name__ == "__main__":
    main()
