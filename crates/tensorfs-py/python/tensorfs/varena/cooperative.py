"""Explicit Linux GPU peer service; no allocation hook or background policy is installed.

This staged integration requires one UID and user/PID/mount namespace, a shared tmpfs
directory and a typed GPU/MIG UUID that the native driver verifies. Samples are observations,
not allocation permissions. TensorD's existing policy is not switched by constructing this owner.
"""
from __future__ import annotations

import json
import os
import time
from typing import Any

from .. import _ext


class _Deferred(RuntimeError):
    """Fallback type for platforms/older extensions without automatic owner support."""


CooperationDeferredError = getattr(_ext, "CooperationDeferredError", _Deferred)
CooperationStaleSampleError = getattr(_ext, "CooperationStaleSampleError", _Deferred)


class Cooperation:
    @staticmethod
    def capability() -> int:
        """CPU-only owner-protocol capability; initializes no CUDA or participant."""
        return int(getattr(getattr(_ext, "_VarenaPeer", None), "OWNER_PROTOCOL", 0))

    @staticmethod
    def initialize() -> None:
        """Install metadata-only fork handling before concurrent fork producers start."""
        native = getattr(_ext, "_VarenaPeer", None)
        if native is None:
            raise NotImplementedError("GPU cooperation requires the Linux native ledger")
        native.initialize()

    def __init__(self, device: int, uuid: str, directory: str | os.PathLike[str], *,
                 shared: bool = True, priority: int = 0) -> None:
        native = getattr(_ext, "_VarenaPeer", None)
        if native is None:
            raise NotImplementedError("GPU cooperation requires the Linux native ledger")
        self._native = native(device, uuid, os.fspath(directory), shared, priority)

    def publish(self, *, used: bool = False, last_use_ns: int | None = None) -> dict[str, Any]:
        """Publish actual native backing; used marks an actual owner use, not a polling tick.

        A safe refresh queries submitted completion events without waiting. During capture or
        the replay guard it reports metadata only, with refreshed=False and no CUDA calls.
        """
        # An asynchronous owner can carry the actual use boundary's timestamp; repeated
        # publication must not make an idle donor appear recently used.
        stamp = last_use_ns if last_use_ns is not None else time.monotonic_ns() if used else 0
        return json.loads(self._native.publish(stamp))

    def configure(self, *, shared: bool, priority: int) -> None:
        """Update lifecycle categories, never a byte limit."""
        self._native.configure(shared, priority)

    def sample(self) -> dict[str, Any]:
        """Observe real driver free/total and commit optimistically. A stale observation must
        be taken again; its old numbers cannot be republished with a newer generation.
        """
        return json.loads(self._native.sample(time.monotonic_ns()))

    def ask(self, sample_generation: int, wanted: int) -> dict[str, Any]:
        return json.loads(self._native.ask(sample_generation, wanted))

    def service(self) -> dict[str, Any]:
        """One completed-only donor pass; live, unknown, pinned and pending storage stays.

        No context/event wait or Python shrink callback occurs. Completed bytes count only
        successful physical release, and the requester still retries its actual allocation.
        """
        return json.loads(self._native.service())

    def snapshot(self) -> dict[str, Any]:
        """Last committed shared metadata; neither a live-owner promise nor free capacity."""
        return json.loads(self._native.snapshot())

    def cuda_calls(self) -> int:
        """Metadata-only native driver activity counter, including context and sample calls."""
        return self._native.cuda_calls()

    def close(self) -> None:
        """End this participant outside application/capture locks. Final arena teardown may
        wait for GPU work; service ticks retain their arena owners and never do this teardown.
        """
        self._native.close()
