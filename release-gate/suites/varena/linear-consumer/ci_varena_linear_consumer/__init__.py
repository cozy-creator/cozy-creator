"""Real NativeLinear/public split through the executor's Request and eager completion."""
from __future__ import annotations

import gc
import hashlib
import json
import os
from importlib.metadata import version
from pathlib import Path

import msgspec
import torch
from cozy_runtime import lora
from cozy_runtime.author import App, Context, Cut, denoise_loop, split

app = App()
ROWS, CHANNELS, WIDTH = 8193, 5376, 7168


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    runtime_modules: dict[str, str]
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str
    seed: int = 13818


class Cleanup(Probe, kw_only=True):
    expected_pid: int
    keys: list[str]


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def actual_request(payload, ctx):
    import cozy_runtime
    import tensorfs
    from cozy_runtime.internal.residency import _REQUEST, Request
    from tensorfs import _ext
    from tensorfs.varena import torch as vt

    assert version('cozy-runtime') == payload.runtime_version
    base = Path(cozy_runtime.__file__).resolve().parent
    assert set(payload.runtime_modules) == {
        'author/kernels.py', 'author/_split.py', 'internal/encoding/leaves.py',
        'internal/prebuilt.py', 'internal/execution/__init__.py',
        'internal/execution/native_linear_split.py', 'internal/execution/native_split.py',
        'lora.py', '_kernels/_gemm.abi3.so',
    }
    for relative, expected in payload.runtime_modules.items():
        assert digest(base / relative) == expected, relative
    assert version('tensorfs') == payload.tensorfs_version
    assert digest(_ext.__file__) == payload.extension_sha256
    assert digest(Path(tensorfs.__file__).resolve().parent / '_varena_torch.so') == payload.shim_sha256
    assert str(torch.__version__) == '2.14.0+cu130'
    assert torch.version.git_version == '08187d9e0fba026dc8217405802ab5381dc88d90'
    assert ctx.device.type == 'cuda' and torch.cuda.device_count() == 1
    assert torch.cuda.get_device_capability(0) == (8, 9), 'this proof is the actual SM89 route'
    actual_uuid = str(torch.cuda.get_device_properties(0).uuid)
    assert actual_uuid.removeprefix('GPU-').lower() == payload.gpu_uuid.removeprefix('GPU-').lower()
    request = _REQUEST.get()
    assert isinstance(request, Request) and request.id == ctx.request_id and not request.idle
    assert request in request.memory.requests and not request.memory.models
    assert request.memory.budget_bytes is None
    return request, vt, {**msgspec.to_builtins(payload), 'pid': os.getpid(),
                         'request_id': ctx.request_id, 'gpu_uuid': actual_uuid}


def trim(vt, device):
    torch.cuda.synchronize(device)
    vt.trim(device)
    torch.cuda.synchronize(device)
    vt.trim(device)


def allocation_menu(native, device, count):
    full = native.int8_plan(device, False, count, CHANNELS, WIDTH, CHANNELS)
    tail_count = ROWS % count or count
    tail = native.int8_plan(device, False, tail_count, CHANNELS, WIDTH, CHANNELS)
    assert full['status'] == tail['status'] == 0
    quantized = max((m for m, p in ((count, full), (tail_count, tail)) if p['kind'] == 0), default=0)
    allocations = [ROWS * CHANNELS * 2]
    if quantized:
        allocations += [quantized * WIDTH, quantized * 4]
    workspace = max(full['workspace_bytes'], tail['workspace_bytes'])
    if workspace:
        allocations.append(workspace)
    return allocations, {'full': full, 'tail': tail, 'tail_rows': tail_count}


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime import _kernels
    from cozy_runtime.internal import int8_linear, prebuilt, verify
    from cozy_runtime.internal.encoding.leaves import Int8ConvRotNativeLeaf

    request, vt, provenance = actual_request(payload, ctx)
    device = ctx.device.index or 0
    on = torch.device(ctx.device.type, device)
    torch.set_num_threads(1)
    before_ready = vt.stats(device)
    assert int8_linear.native(torch, device) is not None
    library = int8_linear.library(device)
    assert library.family == 'mma.sync'
    assert callable(_kernels._gemm.int8_plan)
    gc.collect()
    trim(vt, device)
    initial = vt.stats(device)
    provenance['readiness'] = {'before': before_ready, 'settled': initial}
    generator = torch.Generator().manual_seed(payload.seed)
    rows, held, cases = [], {}, []
    with torch.inference_mode():
        data = torch.randint(-127, 128, (CHANNELS, WIDTH), generator=generator,
                             dtype=torch.int8).to(on)
        scales = torch.full((CHANNELS,), 0.001, dtype=torch.float32, device=on)
        replaced = torch.nn.Linear(WIDTH, CHANNELS, bias=False, device='meta', dtype=torch.bfloat16)
        leaf = Int8ConvRotNativeLeaf().leaf(
            torch, {'weight': data, 'scale': scales}, replaced, torch.bfloat16,
        )
        x = torch.randn((1, ROWS, WIDTH), generator=generator, dtype=torch.bfloat16).to(on)
        marker = torch.zeros(1, dtype=torch.int64, device=on)
        # The ordinary unsplit native call is the numerical reference, with auto dispatch.
        reference = leaf(x).cpu()
        assert type(leaf).__name__ == 'NativeLinear'
        query_execution = []
        for count in (ROWS, (ROWS + 1) // 2, 3):
            info = _kernels._gemm.int8_plan(device, False, count, CHANNELS, WIDTH, CHANNELS)
            assert info['status'] == 0
            values = x.view(ROWS, WIDTH)[:count]
            target = torch.empty((count, CHANNELS), dtype=x.dtype, device=on)
            actual_workspace = []

            def allocate(size, actual_workspace=actual_workspace):
                actual_workspace.append(size)
                return torch.empty(size, dtype=torch.uint8, device=on)

            stream = int(torch.cuda.current_stream(device).cuda_stream)
            if info['kind'] == 1:
                code = _kernels._gemm.gemv(device, values.data_ptr(), False,
                    data.data_ptr(), scales.data_ptr(), 0, target.data_ptr(), False,
                    count, CHANNELS, WIDTH, stream)
            else:
                quantized, row_scale = library.int8_quantize(values)
                # Automatic -1 dispatch executes the original route, independently of the
                # queried config; its real allocator callback proves the workspace answer.
                code = _kernels._gemm.gemm(device, 0, False, quantized.data_ptr(),
                    row_scale.data_ptr(), data.data_ptr(), scales.data_ptr(), 0,
                    target.data_ptr(), CHANNELS, False, count, CHANNELS, WIDTH, -1,
                    allocate, stream)
                del quantized, row_scale
            library._check(code, 'query versus actual automatic dispatch')
            expected = [info['workspace_bytes']] if info['workspace_bytes'] else []
            assert actual_workspace == expected, (info, actual_workspace)
            relative = verify.error(torch, target.cpu(), reference.view(ROWS, CHANNELS)[:count])
            assert relative <= prebuilt.BUDGET
            query_execution.append({'rows': count, 'query': info,
                                    'actual_workspace': actual_workspace, 'relative_l2': relative})
            del values, target
        del allocate
        queries = {}
        for chunk in (ROWS, (ROWS + 1) // 2, ((ROWS + 1) // 2 + 1) // 2, 1):
            allocations, queried = allocation_menu(_kernels._gemm, device, chunk)
            queries[chunk] = (allocations, queried)
        assert queries[ROWS][1]['full']['workspace_bytes'] > 0, 'exercise actual Stream-K'

        for chunk in (ROWS, (ROWS + 1) // 2, ((ROWS + 1) // 2 + 1) // 2, None):
            held.clear()
            prior = len(request.working_phases)

            def step(index, marker, x, *, chunk=chunk, leaf=leaf):
                del index, marker
                trim(vt, device)
                before = vt.stats(device)
                allocations, _ = queries[chunk or 1]
                key = f'fixture.linear-capacity/{chunk}'
                try:
                    exact = vt.split_select(device, key, [('target', allocations)],
                                            preferred='target',
                                            stream=int(torch.cuda.current_stream(device).cuda_stream))
                finally:
                    vt.phase_forget(device, key)
                headroom = exact['required_bytes'] - (1 if chunk is None else 0)
                vt.set_limit(device, before['mapped_bytes'] + headroom)
                try:
                    held['output'] = split(leaf, x, Cut(1))
                except torch.OutOfMemoryError as error:
                    if chunk is not None:
                        raise
                    after = vt.stats(device)
                    assert getattr(error, 'varena_no_retry', False)
                    assert error.required_bytes > error.available_bytes
                    assert after['allocations'] == before['allocations'], (before, after)
                    held['refusal'] = {'required_bytes': error.required_bytes,
                                       'available_bytes': error.available_bytes,
                                       'allocation_delta': 0, 'no_retry': True}
                finally:
                    vt.set_limit(device, None)

            denoise_loop(step, range(1), state=(marker,), inputs=(x,),
                         key=('actual-int8-projection', ROWS, CHANNELS, WIDTH))
            assert len(request.working_phases) == prior + 1
            phase = request.working_phases[-1]
            if chunk is None:
                assert 'output' not in held and 'refusal' in held
                assert phase['invalidated'] and not phase['published'], phase
                cases.append({'refusal': held['refusal'], 'phase': phase})
                continue
            output = held.pop('output').cpu()
            assert torch.isfinite(output).all()
            relative = verify.error(torch, output, reference)
            assert relative <= prebuilt.BUDGET, (relative, prebuilt.BUDGET)
            observations = phase['split_observations']
            assert len(observations) == 1 and observations[0]['published'], observations
            assert phase['cut_choices'] == 1, phase
            sample = observations[0]
            selected = request.working_splits[sample['key']]
            assert selected['row_chunk'] == chunk, selected
            rows.append(sample['key'])
            cases.append({'row_chunk': chunk, 'selection': json.loads(json.dumps(selected)),
                          'phase': phase, 'relative_l2': relative, 'budget': prebuilt.BUDGET,
                          'exact': torch.equal(output, reference), 'query': queries[chunk][1]})
            del output
        # A real nonzero up matrix and zero down matrix make a zero-delta LoRA.
        # Both branches retain the actual Hook, and unbaked execution performs its GEMMs.
        class Factors(torch.nn.Module):
            def __init__(self):
                super().__init__()
                self.down = torch.zeros((1, WIDTH), dtype=torch.bfloat16, device=on)
                self.up = torch.ones((CHANNELS, 1), dtype=torch.bfloat16, device=on)

            def forward(self, value, output):
                lora.accumulate(value, output, self.down, self.up, 1.0, chunk=128)

            def factors(self):
                return self.down, self.up, 1.0

        factors, reads = Factors(), [0]

        def armed(factors=factors):
            reads[0] += 1
            return factors

        hook = lora.Hook(armed)
        handle = leaf.register_forward_hook(hook)
        small = x[:, :129]
        for baked in (True, False):
            hook._cozy_baked = baked
            reads[0] = 0
            held.clear()

            def hooked(index, marker, values, *, leaf=leaf):
                del index, marker
                held['output'] = split(leaf, values, Cut(1))

            denoise_loop(hooked, range(1), state=(marker,), inputs=(small,),
                         key=('actual-int8-hook', baked))
            observed = request.working_phases[-1]
            output = held.pop('output').cpu()
            relative = verify.error(torch, output, reference[:, :129])
            assert relative <= prebuilt.BUDGET
            if baked:
                assert reads[0] == 0 and len(observed['split_observations']) == 1
                assert observed['split_observations'][0]['published']
                rows.append(observed['split_observations'][0]['key'])
            else:
                assert reads[0] > 0 and not observed['split_observations']
            cases.append({'baked_hook': baked, 'factor_reads': reads[0],
                          'relative_l2': relative, 'phase': observed})
            del output
        handle.remove()
        keys = sorted(request.working_phase_keys)
        assert keys and set(keys) == set(rows)
        del hooked, factors, hook, handle, armed, small
        del step, leaf, data, scales, x, marker, replaced, library
        held.clear()
    gc.collect()
    trim(vt, device)
    final = vt.stats(device)
    assert final['allocated_bytes'] == initial['allocated_bytes'], (initial, final)
    assert final['pending_bytes'] == 0 and final['pools'] == initial['pools']
    return Report(True, json.dumps({'passed': True, 'provenance': provenance,
                                   'cases': cases, 'query_execution': query_execution,
                                   'initial': initial, 'final': final,
                                   'request_owned_keys': keys}, sort_keys=True))


@app.entrypoint(accelerator=True)
def cleanup(payload: Cleanup, ctx: Context) -> Report:
    request, vt, provenance = actual_request(payload, ctx)
    assert os.getpid() == payload.expected_pid
    assert not request.working_phase_keys
    removed = {key: vt.phase_forget(ctx.device.index or 0, key) for key in payload.keys}
    assert removed and not any(removed.values()), removed
    return Report(True, json.dumps({'passed': True, 'provenance': provenance,
                                   'prior_request_keys_already_retired': removed}, sort_keys=True))
