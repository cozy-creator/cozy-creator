"""Real source, current-choice execution and measured eager prefetch ownership."""
from __future__ import annotations

import gc
import hashlib
import json
import os
import traceback
from importlib.metadata import version
from pathlib import Path

import msgspec
import tensorfs
import torch
from cozy_runtime.author import App, Context
from tensorfs.varena import _runtime as varena

app = App()
MIB = 1 << 20


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    runtime_modules: dict[str, str]
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    tensorfs_torch_sha256: str
    gpu_uuid: str


class Cleanup(Probe, kw_only=True):
    expected_pid: int
    keys: list[str]


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def actual(payload, ctx):
    import cozy_runtime
    from cozy_runtime.internal.residency import _REQUEST, Request
    from tensorfs import _ext
    from tensorfs.varena import torch as vt

    assert version('cozy-runtime') == payload.runtime_version
    root = Path(cozy_runtime.__file__).resolve().parent
    assert set(payload.runtime_modules) == {
        'internal/residency.py', 'internal/execution/phase.py',
        'internal/execution/committed_linear.py', 'internal/execution/catch.py',
        'internal/int8_linear.py', 'internal/prebuilt.py', 'lora.py',
        'internal/execution/native_split.py', 'internal/execution/native_linear_split.py',
        'internal/execution/native_residual_ffn.py', 'internal/encoding/leaves.py',
        'author/kernels.py', 'author/_split.py', '_kernels/_gemm.abi3.so',
    }
    for name, expected in payload.runtime_modules.items():
        assert digest(root / name) == expected, name
    assert version('tensorfs') == payload.tensorfs_version
    assert digest(_ext.__file__) == payload.extension_sha256
    assert digest(Path(tensorfs.__file__).resolve().parent / '_varena_torch.so') == payload.shim_sha256
    assert digest(Path(tensorfs.__file__).resolve().parent / 'varena/torch.py') == payload.tensorfs_torch_sha256
    assert hasattr(vt, 'commit_execution')
    assert str(torch.__version__) == '2.14.0+cu130'
    assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
    assert ctx.device.type == 'cuda' and torch.cuda.device_count() == 1
    uuid = str(torch.cuda.get_device_properties(0).uuid)
    assert uuid.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    request = _REQUEST.get()
    assert isinstance(request, Request) and request.id == ctx.request_id and not request.idle
    assert not request.memory.models and request.memory.budget_bytes is None
    assert request.memory.phase_plan_supported and varena.supports_phase_plans()
    return request, vt, {**msgspec.to_builtins(payload), 'pid': os.getpid(),
                         'request_id': ctx.request_id, 'gpu_uuid': uuid}


def settle(vt, device):
    gc.collect()
    torch.cuda.synchronize(device)
    vt.trim(device)
    torch.cuda.synchronize(device)
    vt.trim(device)



def report(result, run):
    try:
        run(result)
        result['passed'] = True
    except Exception:  # noqa: BLE001 — preserve the complete failed proof in its Report.
        result['passed'] = False
        result['error'] = traceback.format_exc()
    return Report(result['passed'], json.dumps(result, sort_keys=True))


@app.entrypoint(accelerator=True)
def cleanup(payload: Cleanup, ctx: Context) -> Report:
    request, vt, provenance = actual(payload, ctx)
    assert os.getpid() == payload.expected_pid and not request.working_phase_keys
    removed = {key: vt.phase_forget(ctx.device.index or 0, key) for key in payload.keys}
    assert removed and not any(removed.values()), removed
    return Report(True, json.dumps({'passed': True, 'provenance': provenance,
                                   'prior_keys_already_retired': removed}, sort_keys=True))


from . import consumer, protocol  # noqa: F401
