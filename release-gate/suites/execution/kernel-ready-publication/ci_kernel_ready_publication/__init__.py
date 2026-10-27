"""The installed Runtime's kernel readiness publication under two real openers of one kernel.

CPU only: each opener is a real Torch matrix product, a failure is a real read of a file that
does not exist, and the registry is reached only through its public calls in a real request.
"""

import importlib.metadata
import json
import tempfile
import threading
import uuid
from collections.abc import Callable
from functools import partial
from pathlib import Path

import msgspec
import torch
from cozy_runtime.author import App, Context
from cozy_runtime.author import _request_path as request_path
from cozy_runtime.internal import kernel_ready

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    mode: str = "late-success"
    seed: int = 202610270201


class Report(msgspec.Struct):
    passed: bool
    detail: str


def _late(name: str, mode: str, product: Callable[[], torch.Tensor], missing: Path) -> dict:
    """A background opener finishes after an explicit wait made the kernel serve."""
    owner = threading.get_ident()
    entered, release = threading.Event(), threading.Event()
    background: list[threading.Thread] = []

    def opener(wait: bool) -> torch.Tensor:
        value = product()
        if threading.get_ident() != owner:
            background.append(threading.current_thread())
            entered.set()
            if not release.wait(5):
                raise RuntimeError("the background opener was never released")
            if mode == "late-error":
                missing.read_bytes()
        return value

    try:
        assert kernel_ready.ready(name, opener) is None, "in a request it opens in the background"
        assert entered.wait(5), "the background opener did not start"
        explicit = kernel_ready.wait(name, opener)
        assert kernel_ready.ready(name) is explicit, "the explicit wait's kernel does not serve"
        before = kernel_ready.epoch()
        release.set()
        background[0].join(5)
        assert not background[0].is_alive(), "the background opener did not finish"
        after, failure = kernel_ready.ready(name), kernel_ready.failed(name)
        return {
            "openers": 2,
            "ready_before": True,
            "ready_after": after is not None,
            "same_kernel_after": after is explicit,
            "failure": type(failure).__name__ if failure else None,
            "epoch_before": before,
            "epoch_after": kernel_ready.epoch(),
        }
    finally:
        release.set()


def _dropped(name: str, product: Callable[[], torch.Tensor], missing: Path) -> dict:
    """The waiter that opened the slot fails while a second waiter of it succeeds."""
    entered, fail = threading.Event(), threading.Event()
    first: list[BaseException] = []

    def failing(wait: bool) -> torch.Tensor:
        product()
        entered.set()
        if not fail.wait(5):
            raise RuntimeError("the first waiter was never let fail")
        missing.read_bytes()
        return product()

    def first_waiter() -> None:
        try:
            kernel_ready.wait(name, failing)
        except FileNotFoundError as exc:
            first.append(exc)

    thread = threading.Thread(target=first_waiter, name="ci-kernel-ready first waiter")
    thread.start()

    def opener(wait: bool) -> torch.Tensor:
        value = product()
        fail.set()
        thread.join(5)
        if thread.is_alive():
            raise RuntimeError("the first waiter did not fail")
        return value

    try:
        assert entered.wait(5), "the first waiter did not start"
        before = kernel_ready.epoch()
        served = kernel_ready.wait(name, opener)
        after, failure = kernel_ready.ready(name), kernel_ready.failed(name)
        return {
            "openers": 2,
            "first_waiter_failure": type(first[0]).__name__ if first else None,
            "ready_after": after is not None,
            "same_kernel_after": after is served,
            "failure": type(failure).__name__ if failure else None,
            "epoch_before": before,
            "epoch_after": kernel_ready.epoch(),
        }
    finally:
        fail.set()
        thread.join(5)


@app.entrypoint(accelerator=False)
def cpu(payload: Probe, ctx: Context) -> Report:
    assert payload.mode in ("late-success", "late-error", "dropped-slot"), payload.mode
    assert request_path.in_flight(), "needs the real Cozy request scope"
    torch.set_num_threads(1)
    generator = torch.Generator(device="cpu").manual_seed(payload.seed)
    left, right = (torch.randn(64, 64, generator=generator) for _ in range(2))
    expected = torch.mm(left, right)
    name = f"ci-kernel-ready {payload.mode} {payload.seed} {uuid.uuid4().hex}"
    with tempfile.TemporaryDirectory(prefix="cozy-kernel-ready-") as tmp:
        missing = Path(tmp) / "never-written"
        product = partial(torch.mm, left, right)
        if payload.mode == "dropped-slot":
            facts = _dropped(name, product, missing)
        else:
            facts = _late(name, payload.mode, product, missing)
    served = kernel_ready.ready(name)
    facts.update(
        mode=payload.mode,
        runtime=importlib.metadata.version("cozy-runtime"),
        torch=str(torch.__version__),
        exact=served is not None and torch.equal(served, expected),
        cuda_initialized=torch.cuda.is_initialized(),
    )
    print("KERNEL-READY " + json.dumps(facts, sort_keys=True), flush=True)
    assert not facts["cuda_initialized"], facts
    grew = facts["epoch_after"] - facts["epoch_before"]
    if payload.mode == "late-success":
        ok = facts["same_kernel_after"] and facts["exact"] and grew == 0 and not facts["failure"]
        why = "an older background success replaced or demoted the serving kernel"
    elif payload.mode == "late-error":
        ok = not facts["ready_after"] and facts["failure"] == "FileNotFoundError" and grew == 1
        why = "a late real opener failure did not stop the kernel serving, or the epoch missed it"
    else:
        ok = (
            facts["first_waiter_failure"] == "FileNotFoundError"
            and facts["same_kernel_after"]
            and facts["exact"]
            and grew == 1
            and not facts["failure"]
        )
        why = "a waiter's verified kernel did not serve after the slot's opener failed"
    assert ok, (why, facts)
    return Report(True, json.dumps(facts, sort_keys=True))
