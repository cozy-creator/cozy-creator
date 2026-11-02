"""Linux GPU cooperation with an explicit peer or a native automatic owner.

This staged integration requires one UID and user/PID/mount namespace, a shared tmpfs
directory and a typed GPU/MIG UUID that the native driver verifies. Samples are observations,
not allocation permissions. The automatic owner requires a categorically negotiated native
policy; constructing a manual peer does not switch TensorD's existing memory policy.
"""
from __future__ import annotations

import json
import os
import sys
import time
from typing import Any

from .. import _ext

# The native peer module is compiled only on Linux with 64-bit atomics.
# Supported Linux wheels must provide it; missing names there are a broken cohort,
# not a signal to silently adopt an older memory policy.
if sys.platform == "linux":
    CooperationDeferredError = _ext.CooperationDeferredError
    CooperationStaleSampleError = _ext.CooperationStaleSampleError
    _Peer = _ext._VarenaPeer
    _Owner = _ext._VarenaOwner
else:
    class CooperationDeferredError(RuntimeError):
        """No native cooperation operation can run on this platform."""

    class CooperationStaleSampleError(CooperationDeferredError):
        pass

    _Peer = _Owner = None


class Cooperation:
    @staticmethod
    def capability() -> int:
        """CPU-only owner-protocol capability; initializes no CUDA or participant."""
        return 0 if _Peer is None else int(_Peer.OWNER_PROTOCOL)

    @staticmethod
    def initialize() -> None:
        """Install metadata-only fork handling before concurrent fork producers start."""
        native = _Peer
        if native is None:
            raise NotImplementedError("GPU cooperation requires the Linux native ledger")
        native.initialize()

    def __init__(self, device: int, uuid: str, directory: str | os.PathLike[str], *,
                 shared: bool = True, priority: int = 0) -> None:
        native = _Peer
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


class CooperationOwner:
    """One lease on the process/device's native automatic service.

    Creation-only shared/priority defaults apply when no service exists. Joining a
    ready same-domain service inherits its actual settings; inspect snapshot() for
    its row. configure() is an explicit process-wide lifecycle change. close() only
    releases this lease; other clients continue. Final close remains retryable when
    capture excludes native retirement. No call owns a separate memory byte policy.
    """

    @staticmethod
    def capability() -> int:
        return 0 if _Owner is None else int(_Owner.OWNER_PROTOCOL)

    @staticmethod
    def pressure_capability() -> int:
        """CPU-only sampled-pressure capability; no CUDA/NVML initialization."""
        return 0 if _Owner is None else int(_Owner.PRESSURE_PROTOCOL)

    initialize = staticmethod(Cooperation.initialize)

    def __init__(self, device: int, uuid: str, directory: str | os.PathLike[str], *,
                 shared: bool = True, priority: int = 0) -> None:
        native = _Owner
        if native is None:
            raise NotImplementedError("native automatic GPU cooperation is unavailable")
        self._native = native(device, uuid, os.fspath(directory), shared, priority)

    def wake(self, *, used: bool = False) -> None:
        self._native.wake(time.monotonic_ns() if used else 0)

    def request_boundary(self, *, begin: bool) -> None:
        """Actual request lifecycle only; native code observes and decides every byte."""
        self._native.request_boundary(begin)

    def facts(self) -> dict[str, Any]:
        """Per-client active/closed plus service client count and stable row identity.

        A closed lease may retain its last row identity for diagnostics; it is not a
        live-row or free-capacity promise. The final native worker owns retirement.
        """
        return json.loads(self._native.facts())

    def configure(self, *, shared: bool, priority: int) -> None:
        self._native.configure(shared, priority)

    def snapshot(self) -> dict[str, Any]:
        return json.loads(self._native.snapshot())

    def cuda_calls(self) -> int:
        return self._native.cuda_calls()

    def close(self) -> None:
        self._native.close()
