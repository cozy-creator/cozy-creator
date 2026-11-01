"""A real fused norm, inside the ordinary executor's Request and eager phase."""
from __future__ import annotations

import gc
import hashlib
import json
import os
from functools import partial
from importlib.metadata import version
from pathlib import Path

import msgspec
import torch
import torch.nn.functional as F
from cozy_runtime import fusion
from cozy_runtime.author import App, Context, denoise_loop
from cozy_runtime.author.kernels import Plan
from torch import nn

app = App()
MIB = 1 << 20
SHAPE = (4, 64, 256, 256)


class FixtureChannelNorm(nn.Module):
    """The declared channel RMS operation served by the real Runtime fusion kernel."""

    channel_first = True
    scale = 8.0
    bias = 0.0

    def __init__(self, device):
        super().__init__()
        self.gamma = nn.Parameter(torch.ones((64, 1, 1), device=device, dtype=torch.bfloat16),
                                  requires_grad=False)

    def forward(self, x):
        normalized = F.normalize(x.float(), dim=1, eps=1e-12).to(x.dtype)
        return normalized * self.scale * self.gamma


# Discovered by the normal executor's kernel preparation, not compiled by this handler.
LAYOUT = fusion.ChannelNorm(FixtureChannelNorm, dtype="bf16")


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    runtime_split_sha256: str
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str
    seed: int = 13817


class Cleanup(Probe, kw_only=True):
    expected_pid: int
    keys: list[str]


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def actual_request(payload: Probe, ctx: Context):
    import tensorfs
    from cozy_runtime.internal.execution import native_split
    from cozy_runtime.internal.residency import _REQUEST, Request
    from tensorfs import _ext
    from tensorfs.varena import torch as vt

    assert version('cozy-runtime') == payload.runtime_version
    assert digest(native_split.__file__) == payload.runtime_split_sha256
    assert version('tensorfs') == payload.tensorfs_version
    assert digest(_ext.__file__) == payload.extension_sha256
    shim = Path(tensorfs.__file__).resolve().parent / '_varena_torch.so'
    assert digest(shim) == payload.shim_sha256
    assert str(torch.__version__) == '2.14.0+cu130'
    assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
    assert version('triton') == '3.8.0'
    assert ctx.device.type == 'cuda' and torch.cuda.device_count() == 1
    actual_uuid = str(torch.cuda.get_device_properties(0).uuid)
    assert actual_uuid.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    request = _REQUEST.get()
    assert isinstance(request, Request) and request.id == ctx.request_id and not request.idle
    assert request in request.memory.requests and request.memory.budget_bytes is None
    assert not request.memory.models, 'this bounded fixture requires a componentless executor'
    assert request.memory.native.stats()['resident_bytes'] == 0
    provenance = {**msgspec.to_builtins(payload), 'pid': os.getpid(), 'request_id': ctx.request_id,
                  'torch': str(torch.__version__), 'torch_git': torch.version.git_version,
                  'triton': version('triton'), 'gpu_uuid': actual_uuid,
                  'request_type': f'{type(request).__module__}.{type(request).__qualname__}',
                  'allocator_backend': torch.cuda.get_allocator_backend()}
    return request, vt, provenance


def trim(vt, device):
    torch.cuda.synchronize(device)
    vt.trim(device)
    torch.cuda.synchronize(device)
    vt.trim(device)


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime.internal import triton_kernels
    from cozy_runtime.internal.triton_kernels import _Launcher

    request, vt, provenance = actual_request(payload, ctx)
    device = ctx.device.index or 0
    torch.set_num_threads(1)
    trim(vt, device)
    initial = vt.stats(device)
    plan_reports = []
    plan = Plan(ctx.device, partial(triton_kernels.load, device=ctx.device),
                report=lambda name, row: plan_reports.append({'name': name, **row}))
    rows = []
    reference = None
    holder = {}
    with torch.inference_mode():
        norm = FixtureChannelNorm(ctx.device)
        identity = fusion.install_norms(plan, LAYOUT, norm)
        assert identity and getattr(type(norm), '_cozy_fused', None) == 'channel'
        launcher = norm._cozy_norm.switch.launch
        assert type(launcher) is _Launcher and launcher.key
        provenance.update(launcher_type=f'{type(launcher).__module__}.{type(launcher).__qualname__}',
                          launcher_key=launcher.key, fusion_identity=identity,
                          kernel_source=LAYOUT.entries[0].kernel.cache_key)
        cpu_input = torch.randn((4, 256, 256, 64), dtype=torch.bfloat16,
                                generator=torch.Generator().manual_seed(payload.seed))
        x = cpu_input.permute(0, 3, 1, 2).to(ctx.device)
        assert tuple(x.shape) == SHAPE and not x.is_contiguous()
        marker = torch.zeros(1, dtype=torch.int64, device=ctx.device)
        try:
            for headroom, expected in ((64, 'batch:4'), (48, 'batch:2'), (40, 'batch:1'), (39, None)):
                holder.clear()
                observation_before = len(request.working_phases)

                def step(index, marker, x, gamma, *, norm=norm, headroom=headroom, expected=expected):
                    del index, marker
                    assert gamma is norm.gamma
                    # Fixture setup is deliberately quiescent. Product execution adds no
                    # per-leaf synchronization; denoise_loop supplies its existing end sync.
                    trim(vt, device)
                    before = vt.stats(device)
                    assert before['reclaimable_bytes'] == 0
                    vt.set_limit(device, before['mapped_bytes'] + headroom * MIB)
                    try:
                        holder['output'] = norm(x)
                    except torch.OutOfMemoryError as error:
                        if expected is not None:
                            raise
                        after = vt.stats(device)
                        assert getattr(error, 'varena_no_retry', False)
                        assert error.required_bytes > error.available_bytes
                        assert after['allocations'] == before['allocations'], (
                            'minimum refusal allocated an output or copy', before, after,
                        )
                        holder['refusal'] = {'required_bytes': error.required_bytes,
                                             'available_bytes': error.available_bytes,
                                             'no_retry': True, 'allocation_delta': 0}
                    finally:
                        vt.set_limit(device, None)

                denoise_loop(step, range(1), state=(marker,), inputs=(x, norm.gamma),
                             key=('varena-real-channel-norm', SHAPE))
                assert len(request.working_phases) == observation_before + 1
                observed = request.working_phases[-1]
                assert not observed['published'], 'unknown parent must not publish its watermark'
                if expected is None:
                    assert 'output' not in holder and 'refusal' in holder
                    rows.append({'headroom_mib': headroom, 'refusal': holder['refusal'],
                                 'phase': observed})
                    continue
                assert 'output' in holder
                result = holder.pop('output')
                cpu = result.cpu()
                assert torch.isfinite(cpu).all()
                leaves = observed['split_observations']
                assert len(leaves) == 1 and leaves[0]['published'] and not leaves[0]['pending'], leaves
                assert leaves[0]['choice_id'] == expected, leaves
                key = leaves[0]['key']
                selected = request.working_splits[key]
                assert selected['selection']['choice_id'] == expected
                if reference is None:
                    reference = cpu
                    delta = 0.0
                else:
                    torch.testing.assert_close(cpu, reference)
                    delta = float((cpu.float() - reference.float()).abs().max())
                rows.append({'headroom_mib': headroom, 'choice_id': expected,
                             'selection': dict(selected['selection']), 'phase': observed,
                             'shape': list(cpu.shape), 'dtype': str(cpu.dtype),
                             'output_sha256': hashlib.sha256(cpu.view(torch.uint16).numpy().tobytes()).hexdigest(),
                             'max_absolute_delta': delta, 'exact': torch.equal(cpu, reference)})
                del result, cpu
            keys = sorted(request.working_phase_keys)
            assert keys and set(keys) == set(request.working_splits)
        finally:
            vt.set_limit(device, None)
        del step, norm, x, marker, cpu_input, launcher
        holder.clear()
    gc.collect()
    trim(vt, device)
    final = vt.stats(device)
    assert final['allocated_bytes'] == initial['allocated_bytes'], (initial, final)
    assert final['pending_bytes'] == 0 and final['pools'] == initial['pools']
    report = {'passed': True, 'provenance': provenance, 'cases': rows,
              'initial': initial, 'final': final, 'plan_reports': plan_reports,
              'request_owned_keys': keys,
              'scope': 'real componentless Request/denoise_loop phase and actual compiled fused norm; no mocked context or model package bypass',
              'numeric_policy': 'Torch dtype-default assert_close; exact bytes reported descriptively',
              'cleanup_scope': 'owned tensor allocations return to pre-fixture live baseline; follow cleanup entrypoint in same PID to verify request-owned keys retired'}
    return Report(True, json.dumps(report, sort_keys=True))


@app.entrypoint(accelerator=True)
def cleanup(payload: Cleanup, ctx: Context) -> Report:
    request, vt, provenance = actual_request(payload, ctx)
    assert os.getpid() == payload.expected_pid, 'another executor is not cleanup evidence'
    assert not request.working_phase_keys
    removed = {key: vt.phase_forget(ctx.device.index or 0, key) for key in payload.keys}
    assert removed and not any(removed.values()), removed
    return Report(True, json.dumps({'passed': True, 'provenance': provenance,
                                   'prior_request_keys_already_retired': removed}, sort_keys=True))
