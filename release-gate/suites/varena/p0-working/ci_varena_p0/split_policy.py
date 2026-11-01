"""Real CUDA split admission and deferred exact-choice sampling; no mock driver or operation retry."""
from __future__ import annotations

from contextlib import suppress
import importlib.util
import json
from pathlib import Path
import sys

import torch
import tensorfs.varena.torch as vt
from tensorfs import varena

spec = importlib.util.spec_from_file_location("varena_split_probe_helpers", Path(__file__).with_name("probe.py"))
assert spec is not None and spec.loader is not None
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)
MIB = probe.MIB
MENU = [("batch:4", [32 * MIB, 32 * MIB]),
        ("batch:2", [32 * MIB, 16 * MIB]),
        ("batch:1", [32 * MIB, 8 * MIB])]
COPY = {"batch:4": 32 * MIB, "batch:2": 16 * MIB, "batch:1": 8 * MIB}


def finish_phase(owner, *, success=True):
    # One real enclosing completion proof, never one synchronization per chunk or leaf.
    torch.cuda.current_stream().synchronize()
    return vt.phase_leave(0, owner, completed=success)


def run_variant(key, cap, expected, *, preview=False):
    vt.set_limit(0, cap)
    owner = vt.phase_enter(0)
    chosen = vt.split_select(0, key, MENU, preferred="batch:4", stream=torch.cuda.current_stream().cuda_stream)
    assert chosen["choice_id"] == expected, chosen
    observation = vt.split_begin(0, key, expected, stream=torch.cuda.current_stream().cuda_stream, completion_owner=owner)
    out = torch.empty(32 * MIB, dtype=torch.uint8, device="cuda")
    width = COPY[expected]
    for at in range(0, out.numel(), width):
        scratch = torch.empty(width, dtype=torch.uint8, device="cuda")
        scratch.fill_(1 + at // width)
        out[at:at + width].copy_(scratch)
        del scratch
    submitted = vt.split_end(0, observation, submitted=True)
    assert submitted["pending"] and not submitted["published"], submitted
    if preview:
        # Capacity is temporarily ample for a read-only selection query. Actual GPU completion
        # may already have occurred, but no owning completion proof has published the sample.
        vt.set_limit(0, 128 * MIB)
        before_completion = vt.split_select(0, key, MENU, preferred="batch:4", stream=torch.cuda.current_stream().cuda_stream)
        assert before_completion["basis"] == "declared", before_completion
        vt.set_limit(0, cap)
    ended = finish_phase(owner)
    assert not ended["published"] and ended["reason"] == "execution_key_unknown"
    assert len(ended["split_observations"]) == 1
    leaf = ended["split_observations"][0]
    assert leaf["published"] and not leaf["pending"], leaf
    assert leaf["choice_id"] == expected and leaf["backing_peak_bytes"] == 32 * MIB + width, leaf
    for at in range(0, out.numel(), width):
        assert int(out[at]) == int(out[at + width - 1]) == 1 + at // width
    del out
    probe.clean()
    return {"selection": chosen, "submitted": submitted, "completed": ended}


def exact_choices():
    key = "split-proof:closed-32MiB-output"
    try:
        whole = run_variant(key, 64 * MIB, "batch:4", preview=True)
        half = run_variant(key, 48 * MIB, "batch:2")
        narrow = run_variant(key, 40 * MIB, "batch:1")
        repeated = run_variant(key, 48 * MIB, "batch:2")
        assert whole["selection"]["basis"] == half["selection"]["basis"] == narrow["selection"]["basis"] == "declared"
        assert repeated["selection"]["basis"] == "measured"
        return {"whole": whole, "half": half, "narrow": narrow, "repeat": repeated}
    finally:
        vt.set_limit(0, None)
        vt.phase_forget(0, key)


def preexecution_exhaustion():
    key = "split-proof:minimum"
    vt.set_limit(0, 39 * MIB)
    calls = 0
    refused = None
    try:
        try:
            vt.split_select(0, key, MENU, preferred="batch:4", stream=torch.cuda.current_stream().cuda_stream)
            calls += 1
            output = torch.empty(32 * MIB, dtype=torch.uint8, device="cuda")
            scratch = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
            output.zero_()
            scratch.zero_()
            del output, scratch
        except torch.OutOfMemoryError as error:
            assert getattr(error, "varena_no_retry", False)
            assert error.required_bytes == 40 * MIB and error.available_bytes < error.required_bytes
            refused = {"required_bytes": error.required_bytes, "available_bytes": error.available_bytes,
                       "type": type(error).__name__, "varena_no_retry": error.varena_no_retry}
        assert refused is not None and calls == 0
        return {"exhausted": refused, "operation_invocations": calls}
    finally:
        vt.set_limit(0, None)
        vt.phase_forget(0, key)


def outside_holdings_and_slab_floor():
    key = "split-proof:outside"
    outside = torch.empty(32 * MIB, dtype=torch.uint8, device="cuda")
    try:
        vt.set_limit(0, 80 * MIB)
        selected = vt.split_select(0, key, MENU, preferred="batch:4", stream=torch.cuda.current_stream().cuda_stream)
        assert selected["choice_id"] == "batch:2" and selected["available_bytes"] == 48 * MIB, selected
    finally:
        del outside
        probe.clean()
        vt.set_limit(0, None)
        vt.phase_forget(0, key)
    key = "split-proof:small-classes"
    try:
        vt.set_limit(0, 3 * MIB)
        refused = False
        try:
            vt.split_select(0, key, [("two-slabs", [512, 1024])], preferred="two-slabs", stream=torch.cuda.current_stream().cuda_stream)
        except torch.OutOfMemoryError as error:
            refused = error.required_bytes == 4 * MIB
        assert refused, "two tiny size classes require two native slabs"
        return {"outside_selection": selected, "two_class_required_bytes": 4 * MIB}
    finally:
        vt.set_limit(0, None)
        vt.phase_forget(0, key)


def ownership_and_abort():
    key = "split-proof:ownership"
    choices = [("only", [8 * MIB])]
    vt.split_select(0, key, choices, preferred="only", stream=torch.cuda.current_stream().cuda_stream)
    parent = vt.phase_enter(0)
    leaf = vt.split_begin(0, key, "only", stream=torch.cuda.current_stream().cuda_stream, completion_owner=parent)
    value = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
    vt.split_end(0, leaf, submitted=True)
    ended = finish_phase(parent)
    assert ended["split_observations"][0]["published"]
    del value
    probe.clean()
    parent = vt.phase_enter(0)
    leaf = vt.split_begin(0, key, "only", stream=torch.cuda.current_stream().cuda_stream, completion_owner=parent)
    context = vt.split_context(0)
    assert context == {"eligible": False, "reason": "leaf_active"}, context
    nested = False
    try:
        vt.split_begin(0, key, "only", stream=torch.cuda.current_stream().cuda_stream, completion_owner=parent)
    except RuntimeError:
        nested = True
    assert nested
    value = torch.empty(4 * MIB, dtype=torch.uint8, device="cuda")
    vt.split_end(0, leaf, submitted=False)
    aborted = finish_phase(parent, success=False)
    assert not aborted["split_observations"][0]["published"]
    del value
    probe.clean()
    kept = vt.split_select(0, key, choices, preferred="only", stream=torch.cuda.current_stream().cuda_stream)
    assert kept["basis"] == "measured" and kept["required_bytes"] == 8 * MIB, kept
    vt.phase_forget(0, key)
    return {"baseline": ended, "aborted": aborted, "retained": kept, "nested_rejected": nested}


def caught_allocation_failure():
    key = "split-proof:failed-declaration"
    menu = [("only", [4 * MIB])]
    vt.set_limit(0, 8 * MIB)
    parent = vt.phase_enter(0)
    vt.split_select(0, key, menu, preferred="only", stream=torch.cuda.current_stream().cuda_stream)
    leaf = vt.split_begin(0, key, "only", stream=torch.cuda.current_stream().cuda_stream, completion_owner=parent)
    value = torch.empty(4 * MIB, dtype=torch.uint8, device="cuda")
    vt.split_end(0, leaf, submitted=True)
    initial = finish_phase(parent)
    assert initial["split_observations"][0]["published"]
    del value
    probe.clean()
    parent = vt.phase_enter(0)
    leaf = vt.split_begin(0, key, "only", stream=torch.cuda.current_stream().cuda_stream, completion_owner=parent)
    partial = torch.empty(2 * MIB, dtype=torch.uint8, device="cuda")
    partial.fill_(5)
    failed = False
    try:
        # Deliberately violate the declaration once. A caught real allocation failure must
        # invalidate the sample even when its caller claims submission/phase success.
        torch.empty(16 * MIB, dtype=torch.uint8, device="cuda")
    except torch.OutOfMemoryError:
        failed = True
    ended = vt.split_end(0, leaf, submitted=True)
    result = finish_phase(parent)
    assert failed and not result["split_observations"][0]["published"], result
    assert ended["reason"] == "allocation_error", ended
    del partial
    probe.clean()
    kept = vt.split_select(0, key, menu, preferred="only", stream=torch.cuda.current_stream().cuda_stream)
    assert kept["basis"] == "measured" and kept["required_bytes"] == 4 * MIB, kept
    vt.set_limit(0, None)
    vt.phase_forget(0, key)
    return {"prior": initial, "caught_failure": result, "retained": kept}


def invalidation_without_detail():
    key = "split-proof:empty-invalidation"
    parent = vt.phase_enter(0, key)
    value = torch.empty(4 * MIB, dtype=torch.uint8, device="cuda")
    torch.cuda.current_stream().synchronize()
    prior = vt.phase_leave(0, parent, completed=True)
    assert prior["published"] and prior["peak_bytes"] == 4 * MIB
    del value
    probe.clean()
    parent = vt.phase_enter(0, key)
    value = torch.empty(2 * MIB, dtype=torch.uint8, device="cuda")
    vt.phase_invalidate(0, parent, reason="")
    torch.cuda.current_stream().synchronize()
    result = vt.phase_leave(0, parent, completed=True)
    assert result["invalidated"] and not result["published"], result
    kept = vt.phase_stats(0, key)
    assert kept["known"] and kept["peak_bytes"] == 4 * MIB and kept["successful_passes"] == 1, kept
    del value
    probe.clean()
    vt.phase_forget(0, key)
    return {"prior": prior, "invalidated": result, "retained": kept}


def caught_admission_refusal(at):
    """A caught pre-leaf capacity refusal cannot replace a successful named parent sample."""
    key = "split-proof:caught-" + at + ":parent"
    site = "split-proof:caught-" + at + ":site"
    menu = [("only", [4 * MIB])]
    stream = torch.cuda.current_stream().cuda_stream
    parent = leaf = None
    value = None
    report = None
    try:
        vt.set_limit(0, 8 * MIB)
        parent = vt.phase_enter(0, key)
        value = torch.empty(4 * MIB, dtype=torch.uint8, device="cuda")
        value.fill_(19)
        prior = finish_phase(parent)
        parent = None
        assert prior["published"] and prior["peak_bytes"] == 4 * MIB, prior
        assert int(value[0]) == int(value[-1]) == 19
        value = None
        probe.clean()
        prior_stats = vt.phase_stats(0, key)
        assert prior_stats["known"] and prior_stats["successful_passes"] == 1, prior_stats
        assert prior_stats["peak_bytes"] == prior_stats["backing_peak_bytes"] == 4 * MIB, prior_stats

        parent = vt.phase_enter(0, key)
        value = torch.empty(2 * MIB, dtype=torch.uint8, device="cuda")
        value.fill_(23)
        selected = None
        if at == "begin":
            selected = vt.split_select(0, site, menu, preferred="only", stream=stream)
            assert selected["choice_id"] == "only" and selected["required_bytes"] == 4 * MIB, selected
        # The existing 2 MiB stays live: the 4 MiB choice cannot fit in the 2 MiB left.
        vt.set_limit(0, 4 * MIB)
        refusal = None
        try:
            if at == "select":
                vt.split_select(0, site, menu, preferred="only", stream=stream)
            else:
                leaf = vt.split_begin(0, site, "only", stream=stream, completion_owner=parent)
        except torch.OutOfMemoryError as error:
            assert getattr(error, "varena_no_retry", False), error
            assert error.required_bytes == 4 * MIB and error.available_bytes == 2 * MIB, error
            refusal = {"required_bytes": error.required_bytes, "available_bytes": error.available_bytes,
                       "type": type(error).__name__, "varena_no_retry": error.varena_no_retry}
        assert refusal is not None, "fixture failed to produce the real admission refusal"
        assert vt.split_context(0)["eligible"], "a refused begin must not leave an active leaf"
        # Deliberately catch the refusal and claim success, as an application may do.
        ended = finish_phase(parent)
        parent = None
        assert int(value[0]) == int(value[-1]) == 23
        kept = vt.phase_stats(0, key)
        report = {"stage": at, "prior": prior, "prior_stats": prior_stats, "selected": selected,
                  "refusal": refusal, "caught_parent": ended, "retained": kept}
    finally:
        if leaf is not None:
            with suppress(Exception):
                vt.split_end(0, leaf, submitted=False)
        if parent is not None:
            with suppress(Exception):
                vt.phase_leave(0, parent, completed=False)
        value = None
        probe.clean()
        vt.set_limit(0, None)
        vt.phase_forget(0, key)
        vt.phase_forget(0, site)
    retired = probe.figures()
    assert retired["allocated_bytes"] == retired["mapped_bytes"] == retired["pending_bytes"] == 0, retired
    assert retired["pools"] == retired["event_handles"] == 0, retired
    report["retired_before_verdict"] = retired
    # Preserve the complete failing evidence and successful cleanup before raising the gate.
    probe.emit({"caught_split_refusal": report})
    ended, kept = report["caught_parent"], report["retained"]
    assert ended["invalidated"] and not ended["published"], report
    assert ended["reason"] == "split_" + at + "_error", report
    assert kept["known"] and kept["successful_passes"] == 1, report
    assert kept["peak_bytes"] == kept["backing_peak_bytes"] == 4 * MIB, report
    return report


def no_completion_owner():
    key = "split-proof:unjoined-owner"
    choices = [("only", [8 * MIB])]
    vt.split_select(0, key, choices, preferred="only", stream=torch.cuda.current_stream().cuda_stream)
    parent = vt.phase_enter(0)
    stream = torch.cuda.Stream()
    with torch.cuda.stream(stream):
        leaf = vt.split_begin(0, key, "only", stream=torch.cuda.current_stream().cuda_stream, completion_owner=None)
        value = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
        value.fill_(9)
        ended = vt.split_end(0, leaf, submitted=True)
    assert not ended["observed"] and ended["peak_bytes"] is None
    assert not ended["pending"] and not ended["published"]
    parent_result = finish_phase(parent)
    assert parent_result["split_observations"] == []
    stream.synchronize()
    assert int(value[-1]) == 9
    del value, stream
    probe.clean()
    selected = vt.split_select(0, key, choices, preferred="only", stream=torch.cuda.current_stream().cuda_stream)
    assert selected["basis"] == "declared", selected
    vt.phase_forget(0, key)
    with varena.replay(0):
        assert vt.split_context(0) == {"eligible": False, "reason": "replay_active"}
    return {"leaf": ended, "parent": parent_result, "unlearned": selected}


def stream_geometry(case):
    key = "split-proof:stream-geometry:" + case
    default = torch.cuda.default_stream()
    other = torch.cuda.Stream()
    old_pointer = None
    blocker = None
    parent = leaf = None
    done = torch.cuda.Event()
    # CUDA lazy module/event initialization can synchronize. Warm these exact operations
    # BEFORE establishing the pending interval; the pending event is still checked afterward.
    with torch.cuda.stream(default):
        warmed = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
        warmed.fill_(7)
        torch.cuda._sleep(1)
        done.record(default)
    default.synchronize()
    del warmed
    probe.clean()
    if case == "completed-cache":
        with torch.cuda.stream(other):
            cached = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
            old_pointer = cached.data_ptr()
            cached.fill_(7)
            del cached
        other.synchronize()
        vt.retire_stream(other)
        assert vt.stats(0)["reclaimable_bytes"] == 8 * MIB
    else:
        with torch.cuda.stream(default):
            cached = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
            old_pointer = cached.data_ptr()
            torch.cuda._sleep(2_000_000_000)
            cached.fill_(7)
            done.record(default)
            del cached
        assert not done.query(), "fixture did not retain pending work"
        if case == "unique-slot":
            # The real slow path polls the deferred block while its event is still pending.
            # Its free index may contain repeated entries, but it is still ONE allocation.
            blocker = torch.empty(MIB, dtype=torch.uint8, device="cuda")
            assert not done.query(), "pending work completed before duplicate-credit check"
    cap = (42 if blocker is not None else 40) * MIB
    vt.set_limit(0, cap)
    stream = other if case == "cross-stream" else default
    allocations = [32 * MIB, 8 * MIB] + ([8 * MIB] if case == "unique-slot" else [])
    before = vt.stats(0)
    try:
        if case in ("cross-stream", "unique-slot"):
            refused = False
            try:
                vt.split_select(0, key, [("only", allocations)], preferred="only", stream=stream.cuda_stream)
            except torch.OutOfMemoryError as error:
                refused = bool(getattr(error, "varena_no_retry", False))
            assert refused, "pending storage was credited on the wrong stream or more than once"
            assert not done.query(), "exclusion requires an actually pending consumer"
            return {"refused": True, "managed_limit": cap, "pending_bytes": before["pending_bytes"]}
        selected = vt.split_select(0, key, [("only", allocations)], preferred="only", stream=stream.cuda_stream)
        assert selected["allocation_backing_bytes"] == 40 * MIB, selected
        assert selected["required_bytes"] == 32 * MIB and selected["reused_capacity_bytes"] == 8 * MIB, selected
        parent = vt.phase_enter(0)
        leaf = vt.split_begin(0, key, "only", stream=stream.cuda_stream, completion_owner=parent)
        admitted = vt.stats(0)
        assert admitted["mapped_bytes"] == before["mapped_bytes"], (before, admitted)
        for field in ("event_records", "event_queries", "context_joins"):
            assert admitted[field] == before[field], (field, before, admitted)
        if case != "completed-cache":
            assert not done.query(), "same-stream admission unnecessarily waited for completion"
        with torch.cuda.stream(stream):
            output = torch.empty(32 * MIB, dtype=torch.uint8, device="cuda")
            scratch = torch.empty(8 * MIB, dtype=torch.uint8, device="cuda")
            assert scratch.data_ptr() == old_pointer, "compatible cache was unmapped instead of reused"
            scratch.fill_(29)
            for at in range(0, output.numel(), scratch.numel()):
                output[at:at + scratch.numel()].copy_(scratch)
            del scratch
        ended = vt.split_end(0, leaf, submitted=True)
        leaf = None
        stream.synchronize()
        completed = vt.phase_leave(0, parent, completed=True)
        parent = None
        assert completed["split_observations"][0]["published"], completed
        assert int(output[0]) == int(output[-1]) == 29
        del output
        return {"selection": selected, "submitted": ended, "completed": completed,
                "same_pointer_reused": True, "managed_limit": cap}
    finally:
        if leaf is not None:
            with suppress(Exception):
                vt.split_end(0, leaf, submitted=False)
        if parent is not None:
            with suppress(Exception):
                vt.phase_leave(0, parent, completed=False)
        default.synchronize()
        other.synchronize()
        del blocker
        probe.clean()
        vt.set_limit(0, None)
        vt.phase_forget(0, key)


CASES = {
    "same-stream-pending": lambda: stream_geometry("same-stream"),
    "cross-stream-pending-exclusion": lambda: stream_geometry("cross-stream"),
    "unique-reusable-slot": lambda: stream_geometry("unique-slot"),
    "compatible-completed-cache": lambda: stream_geometry("completed-cache"),
    "exact-choices": exact_choices,
    "preexecution-exhaustion": preexecution_exhaustion,
    "outside-holdings-and-slab-floor": outside_holdings_and_slab_floor,
    "ownership-and-abort": ownership_and_abort,
    "caught-allocation-failure": caught_allocation_failure,
    "caught-select-refusal": lambda: caught_admission_refusal("select"),
    "caught-begin-refusal": lambda: caught_admission_refusal("begin"),
    "invalidation-without-detail": invalidation_without_detail,
    "no-completion-owner": no_completion_owner,
}


if __name__ == "__main__":
    try:
        probe.CASES = CASES
        probe.main(json.loads(sys.argv[1]))
    except BaseException as error:
        probe.emit({"result": {"passed": False, "error": f"{type(error).__name__}: {error}"}})
        raise
