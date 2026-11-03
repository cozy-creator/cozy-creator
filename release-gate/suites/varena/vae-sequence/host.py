"""Actual output owners/views plus immutable schedule protocol; no fake GPU admission."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

import torch

parser = argparse.ArgumentParser()
parser.add_argument("--sequence", type=Path, required=True)
args = parser.parse_args()
spec = importlib.util.spec_from_file_location("vae_sequence_source", args.sequence)
sequence = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = sequence
spec.loader.exec_module(sequence)
torch.set_num_threads(1)
rows = []
with torch.no_grad():
    for batch in (1, 3, 7):
        output = torch.empty(batch, *sequence.PIXEL_SHAPE, dtype=torch.float16)
        assert output.numel() * output.element_size() == sequence.pixel_allocation(
            batch
        )
        assembly = sequence.OutputAssembly(output, batch)
        declared = sequence.variants(batch)
        for variant in declared:
            assert sequence.resolve(declared, variant.identity) is variant
            output.fill_(float("nan"))
            visited = []
            for window in variant.windows():
                target = assembly.view(window)
                assert torch._C._is_alias_of(target, output) and target.is_contiguous()
                assert (
                    target.data_ptr()
                    == output.data_ptr() + window.start * sequence.pixel_allocation(1)
                )
                target.fill_(window.start + 1)
                assembly.validate_result(window, target)
                duplicate = torch.from_dlpack(target)
                assembly.validate_result(
                    window, duplicate
                )  # Same assigned bytes with another owner.
                visited.extend(range(window.start, window.stop))
                expected_functional = (
                    window.count * 1797 * 2048 * 4,
                    window.count * 1797 * 4,
                    window.count * 1797 * 4,
                )
                assert (
                    tuple(
                        size for _, size in sequence.tail_functional_allocations(window)
                    )
                    == expected_functional
                )
            assert visited == list(range(batch))
            assert not bool(torch.isnan(output).any())
            for window in variant.windows():
                assert bool((assembly.view(window) == window.start + 1).all())
            operations = sequence.operations(variant)
            for window in variant.windows():
                local = [op for op in operations if op.window == window]
                assert [op.kind for op in local] == ["post_quant", "prefix"] + [
                    "block"
                ] * 36 + ["tail"]
                assert [op.block for op in local if op.kind == "block"] == list(
                    range(36)
                )
                assert (
                    local[2].input_owner == "hidden16"
                    and local[-1].input_owner == "hidden32_b"
                )
            cursor = sequence.Cursor(variant)
            tickets = []
            for op in operations:
                ticket = cursor.claim()
                assert ticket.operation == op
                cursor.submitted(ticket)
                tickets.append(ticket)
            assert cursor.host_done
            try:
                cursor.claim()
                raise AssertionError("finished schedule resumed")
            except RuntimeError as error:
                assert "repeat" in str(error)
            try:
                cursor.submitted(tickets[-1])
                raise AssertionError("old ticket accepted")
            except RuntimeError as error:
                assert "current invocation" in str(error)
            rows.append(
                {
                    "batch": batch,
                    "variant": variant.identity,
                    "windows": len(variant.windows()),
                    "host_invocations": len(operations),
                    "full_batch_blockwise": variant.outer == batch,
                    "output_bytes": output.numel() * output.element_size(),
                    "views_and_coverage_passed": True,
                }
            )
        del output, assembly
    # Actual Torch failure exercises terminal host state only; no provider is substituted.
    failed = sequence.Cursor(sequence.Variant(1, 1, 1))
    ticket = failed.claim()
    try:
        torch.empty(-1)
    except RuntimeError as error:
        failed.failed(ticket, error)
        assert getattr(error, "varena_no_retry", False)
    assert failed.failure is not None and not failed.host_done
    try:
        failed.claim()
        raise AssertionError("failed schedule resumed")
    except RuntimeError as error:
        assert "failed" in str(error)
assert (
    not torch.cuda.is_initialized()
    and "libcuda.so" not in Path("/proc/self/maps").read_text()
)
print(
    json.dumps(
        {
            "passed": True,
            "torch": torch.__version__,
            "torch_commit": torch.version.git_version,
            "source_sha256": hashlib.sha256(args.sequence.read_bytes()).hexdigest(),
            "cases": rows,
            "cuda_initialized": False,
            "gpu_qualified": False,
            "scope": "metadata scheduling/order plus real CPU pixel owners/views; no model body, query or admission executed",
        },
        indent=2,
    )
)
