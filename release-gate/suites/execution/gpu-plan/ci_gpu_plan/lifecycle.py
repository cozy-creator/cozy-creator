"""Real CUDA work through the ordinary author invocation and TensorFS output lifetime."""

from __future__ import annotations

import hashlib
import json
import sys
import tempfile
import threading
import time
from pathlib import Path

import msgspec
import torch
from cozy_runtime.author import (
    App,
    CapabilityError,
    Context,
    Device,
    Invocation,
    WeightsOutput,
    attempt,
)
from cozy_runtime.compute import PreparedRepeat
from cozy_runtime.weights_store import Derivation, Output
from tensorfs import varena


class Payload(msgspec.Struct):
    action: str


class Done(msgspec.Struct):
    returned: bool


def report(**fields):
    print(json.dumps(fields), flush=True)


def main():
    seed = int(sys.argv[1])
    mode = sys.argv[2] if len(sys.argv) > 2 else "lifecycle"
    varena.install_allocator()
    torch.set_num_threads(1)
    generator = torch.Generator(device="cuda").manual_seed(seed)
    weight = torch.randn(256, 256, device="cuda", generator=generator) * 0.01
    plans, states, tickets, contexts = {}, {}, {}, {}
    for case in ("normal", "raise", "pending", "active"):
        state = torch.randn(16, 256, device="cuda", generator=generator) * 0.01

        def step(index, state=state):
            state.add_(torch.tanh(state @ weight) * 0.00001)

        states[case] = state
        plans[case] = PreparedRepeat.capture(
            step, state=(state,), steps=10000000 if case == "active" else 100000
        )

    pending_entered, pending_release, canceled = (
        threading.Event(),
        threading.Event(),
        threading.Event(),
    )
    active_entered = threading.Event()
    submit_errors, workers = [], []
    closed = []
    app = App()

    class DelayedSubmission:
        def submit(self, context=None):
            pending_entered.set()
            if not pending_release.wait(120):
                raise TimeoutError("pending submit was not released")
            return plans["pending"].submit(context)

    @app.entrypoint(weights=(WeightsOutput("audit", 1 << 20),))
    def handler(payload: Payload, ctx: Context) -> Done:
        contexts[payload.action] = ctx
        opened = ctx.output("audit", Derivation(base=None))
        opened.component("audit").put("sentinel", torch.arange(8))
        if payload.action == "pending":

            def submit():
                try:
                    tickets["pending"] = ctx.plan(DelayedSubmission())
                except BaseException as error:
                    submit_errors.append(error)

            worker = threading.Thread(target=submit)
            workers.append(worker)
            worker.start()
            assert pending_entered.wait(10)
            report(stage="pending-submit-ready", ready_for_task_cancel=True)
            return Done(True)  # closure must own the still-pending submission
        tickets[payload.action] = ctx.plan(plans[payload.action])
        if payload.action == "active":
            active_entered.set()
            assert plans["active"].facts()["host_graph_launches"] == 1
            report(
                stage="active-submit-ready",
                ready_for_task_cancel=True,
                native=plans["active"].facts(),
            )
        if payload.action == "raise":
            raise ValueError("intentional handler unwind after GPU submission")
        return Done(True)  # deliberately no poll/wait/close in the handler

    with tempfile.TemporaryDirectory(prefix="cozy-gpu-plan-lifecycle-") as directory:
        store = Path(directory)
        case = "normal"

        class WatchedOutput(Output):
            def close(self):
                ticket = tickets.get(case)
                ready = (
                    ticket.poll().done
                    if ticket is not None
                    else plans[case].facts()["host_graph_launches"] == 0
                )
                closed.append({"case": case, "gpu_complete_before_tensorfs_close": ready})
                super().close()

        def output(slot, derivation):
            transaction = "sha256:" + hashlib.sha256((case + str(seed)).encode()).hexdigest()
            return WatchedOutput(store / "tensorfs", slot, transaction, derivation, 1 << 20)

        try:
            cases = (
                ("active",)
                if mode == "lifecycle_running_cancel"
                else ("pending",)
                if mode == "lifecycle_cancel"
                else ("normal", "raise", "pending")
            )
            for case in cases:
                if case in ("pending", "active"):

                    def release(case=case):
                        assert (active_entered if case == "active" else pending_entered).wait(10)
                        if mode in ("lifecycle_cancel", "lifecycle_running_cancel"):
                            for line in sys.stdin:
                                if line.strip() == "cancel":
                                    break
                        else:
                            time.sleep(0.03)
                        canceled.set()
                        pending_release.set()

                    control = threading.Thread(target=release)
                    control.start()

                    def heartbeat():
                        while not pending_release.wait(0.5):
                            report(stage="pending-heartbeat")

                    threading.Thread(target=heartbeat, daemon=True).start()
                result, outcome, _ = attempt(
                    app.get("handler"),
                    {"action": case},
                    Invocation(
                        request_id=f"lifecycle-{case}",
                        spool=store / case,
                        deadline=time.monotonic() + 150,
                        device=Device("cuda", 0),
                        cancel=canceled.is_set,
                        tensorfs_output=output,
                    ),
                )
                if outcome.terminal not in ("succeeded", "canceled"):
                    for line in outcome.traceback.splitlines():
                        report(stage="invocation-trace", detail=line)
                    report(stage="invocation-outcome", code=outcome.code, detail=outcome.message)
                if case == "normal":
                    assert outcome.terminal == "succeeded", outcome
                    assert result is not None
                    assert tickets[case].poll().completed_steps == 100000
                elif case == "raise":
                    assert outcome.terminal == "failed" and outcome.origin == "author", outcome
                    progress = tickets[case].poll()
                    assert (
                        progress.done and progress.canceled and progress.completed_steps < 100000
                    ), progress
                elif case == "active":
                    control.join(timeout=10)
                    progress = tickets[case].poll()
                    assert canceled.is_set() and outcome.terminal == "canceled", outcome
                    assert (
                        progress.done
                        and progress.canceled
                        and 0 < progress.completed_steps < 10000000
                    ), progress
                else:
                    control.join(timeout=10)
                    for worker in workers:
                        worker.join(timeout=10)
                        assert not worker.is_alive(), "request closure deadlocked pending submit"
                    assert outcome.terminal == "canceled", outcome
                    assert submit_errors and plans[case].facts()["host_graph_launches"] == 0
                assert closed[-1]["gpu_complete_before_tensorfs_close"], closed[-1]
                launches = plans[case].facts()["host_graph_launches"]
                try:
                    contexts[case].plan(plans[case])
                except CapabilityError as error:
                    assert error.code == "prepared_request_closed", error
                else:
                    raise AssertionError("closed request admitted more GPU work")
                assert plans[case].facts()["host_graph_launches"] == launches
                report(
                    stage="request-lifetime",
                    case=case,
                    terminal=outcome.terminal,
                    native=plans[case].facts(),
                    source_close=closed[-1],
                )
            report(
                result={
                    "passed": True,
                    "request_lifecycle": closed,
                    "actual_task_cancel": mode in ("lifecycle_cancel", "lifecycle_running_cancel"),
                }
            )
        finally:
            pending_release.set()
            for plan in plans.values():
                plan.close()


if __name__ == "__main__":
    main()
