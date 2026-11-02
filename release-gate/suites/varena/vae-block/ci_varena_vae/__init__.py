"""Ordinary Cozy entrypoints for the real prepared dense provider."""
from __future__ import annotations

import gc
import hashlib
import json
import os
from importlib.metadata import version
from pathlib import Path

import msgspec
import torch
from cozy_runtime.author import App, Context

app = App()
MODULES = (
    '_kernels/_dense.abi3.so', '_kernels/__init__.py', 'internal/dense_linear.py',
    'internal/execution/phase.py', 'internal/residency.py', 'fusion/__init__.py',
    'fusion/_kernels.py', 'graphs/domain.py', 'internal/accel.py',
)


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    runtime_modules: dict[str, str]
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str
    seed: int = 13839


class Cleanup(Probe, kw_only=True):
    expected_pid: int
    keys: list[str] = msgspec.field(default_factory=list)


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def actual(payload, ctx):
    import cozy_runtime
    import tensorfs
    from cozy_runtime.internal.residency import _REQUEST, Request
    from tensorfs import _ext
    from tensorfs.varena import torch as vt

    assert version('cozy-runtime') == payload.runtime_version
    root = Path(cozy_runtime.__file__).resolve().parent
    assert set(payload.runtime_modules) == set(MODULES)
    assert all(digest(root / name) == expected for name, expected in payload.runtime_modules.items())
    assert version('tensorfs') == payload.tensorfs_version
    assert digest(_ext.__file__) == payload.extension_sha256
    assert digest(Path(tensorfs.__file__).resolve().parent / '_varena_torch.so') == payload.shim_sha256
    assert str(torch.__version__) == '2.14.0+cu130'
    assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
    assert ctx.device.type == 'cuda' and torch.cuda.device_count() == 1
    uuid = str(torch.cuda.get_device_properties(0).uuid)
    assert uuid.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    request = _REQUEST.get()
    assert isinstance(request, Request) and request.id == ctx.request_id and not request.idle
    assert request in request.memory.requests and not request.memory.models
    return request, vt, {**msgspec.to_builtins(payload), 'pid': os.getpid(),
                         'request_id': ctx.request_id, 'gpu_uuid': uuid,
                         'diffusers_version': version('diffusers')}


def settle(vt, device):
    """Completion belongs to this caller's explicit stream, not unrelated GPU work."""
    gc.collect()
    stream = torch.cuda.current_stream(device)
    assert stream.cuda_stream != 0, 'the successor fixture requires its owned work stream'
    done = torch.cuda.Event()
    done.record(stream)
    done.synchronize()
    vt.trim(device)
    done.record(stream)
    done.synchronize()
    vt.trim(device)


@app.entrypoint(accelerator=True)
def cleanup(payload: Cleanup, ctx: Context) -> Report:
    request, vt, provenance = actual(payload, ctx)
    assert os.getpid() == payload.expected_pid and not request.working_phase_keys
    removed = {key: vt.phase_forget(ctx.device.index or 0, key) for key in payload.keys}
    assert removed and not any(removed.values()), removed
    return Report(True, json.dumps({'passed': True, 'provenance': provenance,
                                   'prior_request_keys_already_retired': removed}, sort_keys=True))


from . import dense  # noqa: F401
