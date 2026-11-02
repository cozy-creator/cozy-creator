"""Actual graph definitions and Runtime native continuations validate replay observations."""
from __future__ import annotations

import gc
from pathlib import Path
import json
import importlib.util
import sys
import time

import torch
from tensorfs import varena
import tensorfs.varena.torch as vt
from cozy_runtime._kernels import _execution

# The fresh child uses -I, so load the reviewed sibling by its exact frozen path.
_probe_spec = importlib.util.spec_from_file_location("varena_fixture_probe", Path(__file__).with_name("probe.py"))
assert _probe_spec is not None and _probe_spec.loader is not None
probe = importlib.util.module_from_spec(_probe_spec)
sys.modules[_probe_spec.name] = probe
_probe_spec.loader.exec_module(probe)

MIB = probe.MIB


class Definition:
    def __init__(self, pool, stream, *, sleep=0):
        self.pool, self.stream = pool, stream
        self.output = torch.zeros(MIB, dtype=torch.uint8, device="cuda")
        self.index = torch.zeros((), dtype=torch.int64, device="cuda")
        self.graph = torch.cuda.CUDAGraph(keep_graph=True)
        self.footprint = vt.capture_footprint(0, pool)
        self.native = None
        with vt.graph(self.graph, pool=pool, stream=stream, reserve_bytes=16 * MIB,
                      footprint=self.footprint):
            before = probe.figures()
            scratch = torch.empty(MIB, dtype=torch.uint8, device="cuda")
            scratch.fill_(1)
            self.output.add_(scratch)
            if sleep:
                torch.cuda._sleep(sleep)
            del scratch
            after = probe.figures()
            assert all(after[name] == before[name] for name in probe.DRIVER), (before, after)
        self.facts = self.footprint.facts()
        assert self.facts["sealed"] and not self.facts["retired"], self.facts
        assert MIB <= self.facts["traced_bytes"] < 16 * MIB, self.facts
        assert self.facts["traced_bytes"] <= self.facts["backing_bytes"] < 16 * MIB, self.facts
        self.native = _execution.ResidentRepeat(
            self.graph.raw_cuda_graph(), self.index.data_ptr(), 0, 1,
            [self.graph, self.output, self.index, self.stream, self.footprint],
        )

    def sequence(self, controller, key, *, footprints=None, repeats=1, native=None):
        self.output.zero_()
        program = self.native if native is None else native
        capsule, _ = program.continuation(torch.cuda.current_stream().cuda_stream,
                                          reusable=repeats > 1)
        wrapped = controller.wrap(capsule, None, (),
                                  footprints=[self.footprint] if footprints is None else footprints,
                                  execution_key=key)
        args = ([wrapped], 0) if repeats == 1 else ([wrapped], 0, (0, 1, repeats))
        return _execution.NativeSequence(*args)

    def close(self):
        if self.native is not None:
            self.native.close_owner()
            self.native = None
        self.graph.reset()
        self.output = self.index = None


def rows(controller):
    return controller.facts()["observations"]


def run(definition, controller, key, *, footprints=None, repeats=1, native=None):
    seq = definition.sequence(controller, key, footprints=footprints, repeats=repeats, native=native)
    assert not rows(controller) or all(not row.get("pending", False) for row in rows(controller))
    seq.start()
    completed, canceled = seq.wait()
    assert not canceled and completed == repeats, (completed, canceled)
    torch.cuda.synchronize()
    assert int(definition.output[0]) == int(definition.output[-1]) == repeats
    seq.close_owner()
    return rows(controller)[-1]


def definition_scope_and_shared_pool():
    pool, stream = torch.cuda.graph_pool_handle(), torch.cuda.Stream()
    arena = varena.Arena(0)
    controller = varena.PhaseController(arena)
    first = Definition(pool, stream)
    second = Definition(pool, stream)
    name = "p2-replay:shared-definition"
    assert rows(controller) == [], "capture alone published a replay measurement"
    try:
        initial = run(first, controller, name, repeats=3)
        assert initial["published"] and initial["mode"] == "replay", initial
        assert initial["successful_passes"] == 3, initial
        assert initial["definitions"] == [first.facts["definition"]]
        assert initial["backing_peak_bytes"] == first.facts["backing_bytes"], initial
        # Two definitions sharing scratch must union actual ranges, not count the pool twice.
        prefix_body = _execution.ResidentRepeat(
            second.graph.raw_cuda_graph(), second.index.data_ptr(), 0, 1,
            [first.graph, first.output, second.graph, second.output, second.index, stream],
            prefix_graph=first.graph.raw_cuda_graph(),
        )
        try:
            combined = run(second, controller, name, footprints=[first.footprint, second.footprint],
                           native=prefix_body)
        finally:
            prefix_body.close_owner()
        assert combined["published"] and combined["successful_passes"] == 1, combined
        assert combined["backing_peak_bytes"] < 16 * MIB, combined
        assert combined["backing_peak_bytes"] < first.facts["backing_bytes"] + second.facts["backing_bytes"], "shared-pool scratch was counted twice"
        first.graph.reset()
        assert first.footprint.facts()["retired"]
        assert not second.footprint.facts()["retired"] and vt.stats(0)["pools"] > 0
        refused = run(second, controller, name, footprints=[first.footprint])
        assert not refused["published"] and refused["reason"] == "graph_footprint_unqualified", refused
        other = run(second, controller, name)
        assert other["published"] and other["successful_passes"] == 1, other
        # The same semantic key with another exact definition cannot reuse the first profile.
        assert other["definitions"] != initial["definitions"]
        assert controller.facts()["transitions"] == 0, "measurement-only stage changed weight preferences"
        assert vt.phase_forget(0, name)
        forgotten = run(second, controller, name)
        assert forgotten["successful_passes"] == 1, forgotten
        return {"first": initial, "combined": combined, "retired_definition": refused,
                "fresh_definition": other, "after_forget": forgotten,
                "trace_a": first.facts, "trace_b": second.facts}
    finally:
        first.close()
        second.close()
        del first, second
        gc.collect()
        probe.clean()


def failed_capture_and_unknown_key():
    pool, stream = torch.cuda.graph_pool_handle(), torch.cuda.Stream()
    graph = torch.cuda.CUDAGraph(keep_graph=True)
    footprint = vt.capture_footprint(0, pool)
    caught = False
    try:
        with vt.graph(graph, pool=pool, stream=stream, reserve_bytes=4 * MIB, footprint=footprint):
            temporary = torch.empty(MIB, dtype=torch.uint8, device="cuda")
            del temporary
            raise RuntimeError("intentional fixture capture abort")
    except RuntimeError as error:
        caught = str(error) == "intentional fixture capture abort"
    assert caught and footprint.facts()["retired"] and not footprint.facts()["sealed"]
    graph.reset()
    del graph
    probe.clean()
    # All owners of the abandoned Torch host pool are gone; a new capture gets a new handle.
    definition = Definition(torch.cuda.graph_pool_handle(), stream)
    controller = varena.PhaseController(varena.Arena(0))
    try:
        unknown = run(definition, controller, None)
        assert not unknown["published"] and unknown["reason"] == "execution_key_unknown", unknown
        return {"failed_capture": footprint.facts(), "unknown": unknown}
    finally:
        definition.close()


def legacy_preference_wrapper():
    definition = Definition(torch.cuda.graph_pool_handle(), torch.cuda.Stream())
    controller = varena.PhaseController(varena.Arena(0))
    capsule, _ = definition.native.continuation(torch.cuda.current_stream().cuda_stream)
    # Omitted footprints must keep the old weight-preference-only contract, including
    # compatibility with the earlier Runtime's outer non-publishing replay observation.
    wrapped = controller.wrap(capsule, "legacy-preference", ())
    sequence = _execution.NativeSequence([wrapped], 0)
    token = vt.phase_enter(0, "legacy-outer", mode="replay")
    try:
        sequence.start()
        completed, canceled = sequence.wait()
        assert completed == 1 and not canceled
        observed = vt.phase_leave(0, token, completed=True)
        facts = controller.facts()
        assert not observed["published"] and facts["observations"] == [], facts
        assert facts["transitions"] == 1
        sequence.close_owner()
        return {"outer": observed, "legacy_controller": facts}
    finally:
        sequence.close_owner()
        definition.close()


def callback_interval_across_rearm():
    pool, stream = torch.cuda.graph_pool_handle(), torch.cuda.Stream()
    # A real GPU sleep makes the two native iterations long enough for an observed host
    # interleave. The test checks that interleave actually happened; elapsed time is not proof.
    definition = Definition(pool, stream, sleep=2_000_000_000)
    controller = varena.PhaseController(varena.Arena(0))
    name = "p2-replay:callback"
    sequence = definition.sequence(controller, name, repeats=2)
    try:
        with vt.external_callback(0):
            sequence.start()
            deadline = time.monotonic() + 15
            while True:
                facts = controller.facts()
                if facts["observations"] and facts["active_observations"]:
                    break
                assert not sequence.poll()[2], "native stages finished before the requested interleave"
                assert time.monotonic() < deadline, "no checked first-stage completion within fixture deadline"
                time.sleep(0.001)
            first = dict(facts["observations"][-1])
            assert first["published"] and first["successful_passes"] == 1, first
            temporary = torch.empty(4096, dtype=torch.uint8, device="cuda")
            del temporary
            completed, canceled = sequence.wait()
        assert completed == 2 and not canceled
        second = rows(controller)[-1]
        assert not second["published"] and second["reason"] == "external_callback_memory", second
        assert second["successful_passes"] == 1, "invalidated callback replaced a prior successful sample"
        sequence.close_owner()
        return {"cpu_only_first_iteration": first, "memory_second_iteration": second,
                "guard_spanned_observe_and_rearm": True}
    finally:
        sequence.close_owner()
        definition.close()
        vt.phase_forget(0, name)


CASES = {
    "definition-scope-and-shared-pool": definition_scope_and_shared_pool,
    "failed-capture-and-unknown-key": failed_capture_and_unknown_key,
    "legacy-preference-wrapper": legacy_preference_wrapper,
    "callback-interval-across-rearm": callback_interval_across_rearm,
}


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
