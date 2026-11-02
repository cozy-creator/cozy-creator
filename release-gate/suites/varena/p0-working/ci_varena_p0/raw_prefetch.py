"""Real two-pass native raw-source lookahead, with grouped capacity and identity checks."""
from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import time

import tensorfs
from tensorfs import varena
import tensorfs.varena.torch as vt
import torch
from cozy_runtime._kernels import _execution

helper = Path(__file__).with_name("probe.py")
spec = importlib.util.spec_from_file_location("varena_raw_probe_helpers", helper)
assert spec is not None and spec.loader is not None
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)
MIB = probe.MIB


class SourceStage:
    def __init__(self, arena, checkpoint, names, values, stream, pool):
        self.arena = arena
        self.regions = [arena.reserve(2 * MIB) for _ in names]
        groups = [(region, [("net/" + name + "#value", 0, 2 * MIB)])
                  for region, name in zip(self.regions, names, strict=True)]
        slot = varena.LoadSlot.from_regions(checkpoint, groups, threads=1, keep_ready=True)
        assert slot is not None
        inputs = slot.tensors
        self.output = torch.empty(2 * MIB, dtype=torch.uint8, device="cuda")
        self.index = torch.zeros((), dtype=torch.int64, device="cuda")
        self.graph = torch.cuda.CUDAGraph(keep_graph=True)
        self.footprint = vt.capture_footprint(0, pool)
        with vt.graph(self.graph, pool=pool, stream=stream, reserve_bytes=16 * MIB,
                      footprint=self.footprint):
            before = probe.figures()
            self.output.copy_(inputs[0])
            for item in inputs[1:]:
                self.output.add_(item)
            scratch = torch.empty(MIB, dtype=torch.uint8, device="cuda")
            scratch.zero_()
            self.output[:MIB].add_(scratch)
            del scratch
            after = probe.figures()
            assert all(after[name] == before[name] for name in probe.DRIVER)
        # Input tensors are capture aliases only. Native source detachment checks actual Pins.
        if len(inputs) > 1:
            del item
        del inputs
        slot.seal()
        slot._tensor = None
        slot._tensors = None
        self.source = slot._native.detach_source()
        slot.close()
        self.native = _execution.ResidentRepeat(self.graph.raw_cuda_graph(), self.index.data_ptr(),
                                               0, 1, [self.graph, self.output, self.index, stream,
                                                      self.footprint])
        self.expected = sum(values)

    def arm(self, controller, key):
        capsule, _ = self.native.continuation(torch.cuda.current_stream().cuda_stream, reusable=True)
        capsule, job = self.source.continuation(capsule)
        capsule = controller.wrap(capsule, None, (), footprints=[self.footprint], execution_key=key)
        return capsule, job

    def close(self):
        self.native.close_owner()
        self.graph.reset()
        self.source = self.output = self.index = None


def drain(arena):
    deadline = time.monotonic() + 10
    while True:
        probe.clean()
        arena.reclaim(1 << 30)
        row = arena.stats()
        if row["resident_bytes"] == row["pinned_bytes"] == 0:
            return row
        assert time.monotonic() < deadline, row
        time.sleep(0.01)


def pair(case):
    with tempfile.TemporaryDirectory(prefix="varena-raw-lookahead-") as directory:
        writer = tensorfs.Writer(Path(directory) / "source.cozytensors")
        component = writer.component("net")
        for name, value in (("a", 11), ("b", 17), ("c", 23)):
            component.put(name, torch.full((2 * MIB,), value, dtype=torch.uint8))
        checkpoint = writer.commit()
        arena = varena.Arena(0)
        stream, pool = torch.cuda.Stream(), torch.cuda.graph_pool_handle()
        first = SourceStage(arena, checkpoint, ["a"], [11], stream, pool)
        second = SourceStage(arena, checkpoint, ["b", "c"], [17, 23], stream, pool)
        controller = varena.PhaseController(arena)
        sequence = None
        current_key = "raw-lookahead:" + case
        try:
            drain(arena)
            # The old demand window forces a miss on the second pass. New speculation must
            # not change it or use it as an independent admission budget.
            arena.set_streaming_limit(2 * MIB)
            before = probe.figures()
            weights_room = 4 * MIB if case == "complete-group-refusal" else 6 * MIB
            a, job_a = first.arm(controller, current_key)
            b, job_b = second.arm(controller, "raw-lookahead:next:" + case)
            assert job_a.supports_measured_lookahead and job_b.supports_measured_lookahead
            edge_key = "unrelated-key" if case == "wrong-semantic-key" else current_key
            edge_footprints = [second.footprint] if case == "wrong-definition" else [first.footprint]
            assert job_a.link_next(job_b, execution_key=edge_key, footprints=edge_footprints)
            # Pressure changes AFTER linking: admission must use current native capacity.
            vt.set_limit(0, before["mapped_bytes"] + weights_room)
            assert job_b.facts()["prefetch_started"] == 0
            sequence = _execution.NativeSequence([a, b], 0, (0, 2, 2))
            sequence.start()
            completed, canceled = sequence.wait()
            assert completed == 4 and not canceled, (completed, canceled)
            torch.cuda.synchronize()
            assert int(first.output[0]) == int(first.output[-1]) == first.expected
            assert int(second.output[0]) == int(second.output[-1]) == second.expected
            next_facts = job_b.facts()
            observed = controller.facts()
            assert all(row["published"] and row["successful_passes"] == 2
                       for row in observed["observations"]), observed
            assert arena.streaming_limit_bytes == 2 * MIB
            if case == "measured-second-pass":
                assert next_facts["prefetch_skipped"] >= 1, next_facts
                assert next_facts["prefetch_started"] == next_facts["prefetch_used"] == 1, next_facts
                assert next_facts["prefetch_read_bytes"] == 4 * MIB, next_facts
            else:
                assert next_facts["prefetch_started"] == 0, next_facts
                assert next_facts["prefetch_issued_bytes"] == next_facts["prefetch_read_bytes"] == 0, next_facts
                assert next_facts["prefetch_skipped"] >= 2, next_facts
            sequence.close_owner()
            result = {"source_job": next_facts, "observations": observed,
                      "working": probe.figures(), "weights": arena.stats(),
                      "unchanged_streaming_limit": arena.streaming_limit_bytes,
                      "managed_limit": before["mapped_bytes"] + weights_room,
                      "limit_changed_after_link": True,
                      "complete_group_bytes": 4 * MIB, "current_source_bytes": 2 * MIB}
            del a, b, job_a, job_b
            return result
        finally:
            if sequence is not None:
                sequence.close_owner()
            first.close()
            second.close()
            vt.set_limit(0, None)
            drain(arena)
            checkpoint.close()


CASES = {name: (lambda name=name: pair(name)) for name in (
    "measured-second-pass", "complete-group-refusal", "wrong-semantic-key", "wrong-definition",
)}


def main(config):
    probe.emit({"native_execution": {"path": _execution.__file__,
                                     "sha256": probe.digest(_execution.__file__)}})
    probe.CASES = CASES
    probe.main(config)


if __name__ == "__main__":
    try:
        main(json.loads(sys.argv[1]))
    except BaseException as error:
        probe.emit({"result": {"passed": False, "error": f"{type(error).__name__}: {error}"}})
        raise
