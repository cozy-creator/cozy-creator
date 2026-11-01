"""Actual norm and first-use attention-output native admission."""
from __future__ import annotations

import gc
import hashlib
import json
import os
import traceback
from importlib.metadata import version
from pathlib import Path

import msgspec
import torch
from cozy_runtime.author import App, Context
from minimax_h3 import glue, producer_kernels

app = App()


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    runtime_modules: dict[str, str]
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str
    package_modules: dict[str, str]
    kernel_identities: dict[str, str]
    seed: int = 13829


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
    assert set(payload.runtime_modules) == {
        'author/kernels.py', 'internal/triton_kernels.py', 'internal/attention.py',
        'internal/kernel_sources.py', 'internal/kernel_ready.py',
        'author/_split.py', 'internal/execution/native_sparse_producer.py',
        'internal/execution/native_rms_norm.py', 'internal/execution/cuts.py',
        'internal/int8_linear.py',
        'internal/execution/native_linear_split.py', 'internal/execution/phase.py',
        'internal/residency.py', 'internal/producers.py',
        'rows.py', 'lora.py', 'internal/encoding/leaves.py', 'internal/prebuilt.py',
        'internal/execution/native_heads.py', 'internal/execution/native_residual_ffn.py',
        '_kernels/_gemm.abi3.so',
    }
    for name, expected in payload.runtime_modules.items():
        assert digest(root / name) == expected, name
    root = Path(glue.__file__).resolve().parent
    assert set(payload.package_modules) == {
        '__init__.py', 'glue.py', 'adaln_pruned.py', 'overlay.py', 'producer_kernels.py', 'producer.py',
    }
    for name, expected in payload.package_modules.items():
        assert digest(root / name) == expected, name
    assert payload.kernel_identities == {
        s.name: s.identity for s in (glue.GLUE, glue.GLUE_FP8, producer_kernels.STATS)}
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
                         'request_id': ctx.request_id, 'gpu_uuid': uuid}


def settle(vt, device):
    gc.collect()
    torch.cuda.synchronize(device)
    vt.trim(device)
    torch.cuda.synchronize(device)
    vt.trim(device)



def report(result, operation):
    try:
        operation(result)
        result['passed'] = True
    except Exception:  # noqa: BLE001 -- preserve the complete failed proof.
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
                                   'prior_request_keys_already_retired': removed}, sort_keys=True))


from . import norm, projection  # noqa: F401
