"""Instrumented actual H3 Heads; all kernels, allocations, and refusals are real."""
from __future__ import annotations

import gc
import hashlib
import json
import os
from collections import Counter
from contextlib import nullcontext
from importlib.metadata import version
from pathlib import Path

import msgspec
import torch
from cozy_runtime.author import App, Context, Cut, Telemetry, denoise_loop, split
from minimax_h3 import glue
from torch.utils._python_dispatch import TorchDispatchMode

app = App()
WIDTH, HEADS, ROWS = 5376, 56, 8195


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    runtime_version: str
    runtime_modules: dict[str, str]
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str
    package_modules: dict[str, str]
    glue_identities: dict[str, str]
    seed: int = 13828


class Cleanup(Probe, kw_only=True):
    expected_pid: int
    keys: list[str]


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def actual_request(payload, ctx):
    import cozy_runtime
    import tensorfs
    from cozy_runtime.internal.residency import _REQUEST, Request
    from tensorfs import _ext
    from tensorfs.varena import torch as vt

    assert version('cozy-runtime') == payload.runtime_version
    base = Path(cozy_runtime.__file__).resolve().parent
    required = {'author/kernels.py', 'author/_split.py', 'rows.py', 'lora.py',
                'internal/attention_sol.py', 'internal/attention_int4.py',
                'internal/residency.py', 'internal/execution/phase.py',
                'internal/execution/__init__.py', 'internal/execution/native_heads.py',
                'internal/execution/native_linear_split.py', 'internal/execution/native_residual_ffn.py',
                'internal/encoding/leaves.py', 'internal/prebuilt.py',
                'internal/triton_kernels.py', '_kernels/_gemm.abi3.so', '_kernels/_attn4.abi3.so'}
    assert set(payload.runtime_modules) == required
    for name, expected in payload.runtime_modules.items():
        assert digest(base / name) == expected, name
    base = Path(glue.__file__).resolve().parent
    assert set(payload.package_modules) == {'__init__.py', 'glue.py', 'adaln_pruned.py', 'overlay.py'}
    for name, expected in payload.package_modules.items():
        assert digest(base / name) == expected, name
    assert payload.glue_identities == {s.name: s.identity for s in (glue.GLUE, glue.GLUE_FP8)}
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
    assert request.memory.budget_bytes is None
    assert request.memory.phase_plan_supported
    return request, vt, {**msgspec.to_builtins(payload), 'pid': os.getpid(),
                         'request_id': ctx.request_id, 'gpu_uuid': uuid}


def trim(vt, device):
    gc.collect()
    vt.trim(device)
    torch.cuda.synchronize(device)
    vt.trim(device)


def make_attention(on, generator, launch, heads=HEADS):
    from cozy_runtime import lora
    from cozy_runtime.internal.encoding.formats import CONVROT_GROUP
    from cozy_runtime.internal.encoding.leaves import Int8ConvRotNativeLeaf
    from diffusers.models.transformers.transformer_minimax_h3 import MiniMaxH3Attention

    with torch.device('meta'):
        attn = MiniMaxH3Attention(WIDTH, heads, 128)
    attn.norm_q = torch.nn.RMSNorm(128, eps=1e-5, dtype=torch.bfloat16, device=on)
    attn.norm_k = torch.nn.RMSNorm(128, eps=1e-5, dtype=torch.bfloat16, device=on)
    reads = [0]
    provider = Int8ConvRotNativeLeaf()
    params = {'rotation': 'kron_regular_h4', 'rotation_size': CONVROT_GROUP}
    attn._fixture_output_refusal = ''

    def armed():
        reads[0] += 1

    projections = [(attn, name, heads * 128, WIDTH) for name in ('to_q', 'to_k', 'to_v')]
    projections.append((attn.to_out, '0', WIDTH, heads * 128))
    for parent, name, channels, columns in projections:
        refusal = provider.supports((channels, columns), params)
        if refusal:
            assert parent is attn.to_out and name == '0', refusal
            attn._fixture_output_refusal = refusal
            continue  # This geometry qualifies Heads, not an invalid encoded out projection.
        data = torch.randint(-127, 128, (channels, columns), generator=generator,
                             dtype=torch.int8).to(on)
        scales = torch.full((channels,), 0.001, dtype=torch.float32, device=on)
        leaf = provider.leaf(torch, {'weight': data, 'scale': scales},
                             getattr(parent, name), torch.bfloat16)
        hook = lora.Hook(armed)
        hook._cozy_baked = True
        leaf.register_forward_hook(hook)
        setattr(parent, name, leaf)
    processor = glue.FusedMiniMaxH3AttnProcessor()
    processor._glue = launch
    processor._parallel_config = None
    processor._projected_head_entries = (
        next(e for e in glue.GLUE.entries if e.kernel is glue._rope_norm_kernel),
        next(e for e in glue.GLUE.entries if e.kernel is glue._key_sum_kernel),
    )
    attn.set_processor(processor)
    return attn.eval(), reads


def menu(native, device, tokens, heads, chunk, seeded=True):
    """Independent real GEMM queries and the pinned INT8 ABI's concrete storage."""
    last = heads % chunk or chunk
    queries = [native.int8_plan(device, False, tokens, h * 128, WIDTH, h * 128)
               for h in (chunk, last)]
    assert all(info['status'] == 0 for info in queries)
    allocation = [2 * tokens * heads * 128, *([2 * tokens * chunk * 128] * 3)]
    if any(info['kind'] == 0 for info in queries):
        allocation += [tokens * WIDTH, tokens * 4]
    workspace = max(info['workspace_bytes'] for info in queries)
    if workspace:
        allocation.append(workspace)
    cta = 128 if tokens > 1024 else 64
    blocks = (tokens + cta - 1) // cta
    allocation += [tokens * chunk * 128, tokens * chunk * 128,
                   chunk * 128 * blocks * cta,
                   4 * chunk * ((tokens + 127) // 128) * 32,
                   4 * chunk * blocks * 4, 4 * chunk, 4 * chunk * 128]
    if seeded:
        allocation.append(4 * chunk * 128)
    return allocation, {'queries': queries, 'heads': chunk, 'tail_heads': last,
                        'cta_k': cta, 'workspace_bytes': workspace, 'allocations': allocation}


class Census(TorchDispatchMode):
    """Forward real operations, observing only the native accepted leaf interval."""

    def __init__(self, vt, device, seed, output_shape, *, refuse_after_second_seed=False):
        super().__init__()
        self.vt, self.device, self.seed = vt, device, seed
        self.output_shape = output_shape
        self.refuse = refuse_after_second_seed
        self.rows = []
        self.seed_copies = 0
        self.output_pointer = None
        self.output = None
        self.boundary = None

    def __torch_dispatch__(self, func, types, args=(), kwargs=None):
        active = self.vt.split_context(self.device)['reason'] == 'leaf_active'
        before = self.vt.stats(self.device) if active else None
        result = func(*args, **(kwargs or {}))
        if not active:
            return result
        after = self.vt.stats(self.device)
        delta = after['allocations'] - before['allocations']
        allocated = []
        if delta:
            assert delta == 1 and isinstance(result, torch.Tensor) and result.is_cuda
            capacity = after['allocated_capacity'] - before['allocated_capacity']
            assert capacity > 0
            allocated = [{'requested_bytes': result.numel() * result.element_size(),
                          'actual_class_bytes': capacity, 'shape': list(result.shape),
                          'dtype': str(result.dtype)}]
            if tuple(result.shape) == self.output_shape:
                self.output_pointer = result.untyped_storage().data_ptr()
                if self.refuse:
                    self.output = result  # escaped observer owner, explicitly retired after the failure
        role = ''
        if str(func) == 'aten.copy_.default':
            destination = args[0].untyped_storage().data_ptr()
            if destination == self.seed.untyped_storage().data_ptr():
                role = 'seed-copy'
            elif destination == self.output_pointer:
                role = 'output-transpose-copy'
        self.rows.append({'op': str(func), 'role': role, 'allocations': delta, 'storage': allocated})
        if role == 'seed-copy':
            self.seed_copies += 1
            if self.refuse and self.seed_copies == 2:
                torch.cuda.current_stream(self.device).synchronize()
                assert self.output is not None
                self.boundary = {'seed_copies_completed': 2, 'stage': 'completed-writes'}
                readback_before = self.vt.stats(self.device)
                # A whole first-head view is noncontiguous: a D2H copy may create CUDA
                # staging storage. Read every completed contiguous row directly into CPU
                # storage, retaining full first-head coverage under the unchanged cap.
                first_head = torch.empty((1, self.output.shape[1], 1, 128),
                                         dtype=self.output.dtype, device='cpu')
                for row in range(self.output.shape[1]):
                    source = self.output[0, row, 0]
                    assert source.is_contiguous()
                    first_head[0, row, 0].copy_(source)
                seed_host = self.seed.cpu()
                readback_after = self.vt.stats(self.device)
                readback_allocations = readback_after['allocations'] - readback_before['allocations']
                assert readback_allocations == 0, (readback_before, readback_after)
                self.boundary.update(stage='readback-complete',
                                     seed_after_second_copy=seed_host,
                                     first_output_head_after_write=first_head,
                                     readback_device_allocations=readback_allocations)
                # A real managed-capacity refusal after confirmed device writes. The
                # request exceeds the whole limit by itself, so no driver overcommit occurs.
                limit = after['mapped_bytes']
                self.vt.set_limit(self.device, limit)
                self.boundary['stage'] = 'real-refusal-requested'
                torch.empty(limit + (2 << 20), dtype=torch.uint8, device=self.seed.device)
                raise AssertionError('the real managed allocation unexpectedly succeeded')
        return result


def output_equal(actual, expected):
    from cozy_runtime.internal import prebuilt, verify
    error = verify.error(torch, actual, expected)
    assert torch.isfinite(actual).all() and error <= prebuilt.BUDGET, error
    return {'relative_l2': error, 'budget': prebuilt.BUDGET, 'exact': torch.equal(actual, expected)}


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, ctx: Context, tel: Telemetry) -> Report:
    from cozy_runtime import _kernels
    from cozy_runtime.author import AttentionLayout, attention_scope
    from cozy_runtime.author.kernels import quantize_once
    from cozy_runtime.internal import (
        attention,
        attention_sol,
        int8_linear,
        triton_kernels,
    )
    from cozy_runtime.internal.encoding.selection import measure_device
    from cozy_runtime.internal.execution import native_heads

    request, vt, provenance = actual_request(payload, ctx)
    device = ctx.device.index or 0
    on = torch.device('cuda', device)
    torch.set_num_threads(1)
    assert int8_linear.native(torch, device) is not None
    assert int8_linear.library(device).family == 'mma.sync'
    launch = triton_kernels.load(glue.kernels(on), device=on)
    ready, = attention.pinned('kitchen-int8', measure_device(torch, device, 'cuda'))
    selected = ready.member
    from comfy_kitchen.backends import cuda as kitchen_cuda
    assert version('comfy-kitchen') == '0.2.40'
    assert callable(kitchen_cuda._C.sage_sdpa_quantize)
    assert callable(kitchen_cuda._C.sage_sdpa_prequantized)
    provenance.update(launcher_key=launch.key, selected_backend=str(selected.value),
                      kitchen_version=version('comfy-kitchen'), kitchen_artifact=ready.artifact,
                      kitchen_extension_sha256=digest(kitchen_cuda._C.__file__))
    generator = torch.Generator().manual_seed(payload.seed)
    trim(vt, device)
    initial = vt.stats(device)
    cases, keys, declined = [], [], []
    with torch.inference_mode():
        for tokens, heads, variants in (
            (ROWS, HEADS, (HEADS, 28, 7, 1)),
            (513, HEADS, (HEADS,)),
            (513, 55, (55, 28)),
        ):
            attn, hook_reads = make_attention(on, generator, launch, heads)
            attn.processor._attention_backend = selected
            hidden = torch.randn((1, tokens, WIDTH), generator=generator, dtype=torch.bfloat16).to(on)
            angle = torch.randn((tokens, 96), generator=generator, dtype=torch.float32).to(on)
            rotary = angle.cos(), angle.sin()
            del angle
            seed = torch.full((heads, 128), 19., dtype=torch.float32, device=on)
            marker = torch.zeros(1, dtype=torch.int64, device=on)
            token = glue.KEY_MEAN.set(glue.KeyMean(seed))
            try:
                operator = glue._Heads.of(attn.processor, attn, hidden, rotary, None)
                assert type(operator) is glue._Heads
                with quantize_once(hidden):
                    reference_device = operator(operator)
                reference, seed_reference = reference_device.cpu(), seed.cpu()
                del reference_device
                assert hook_reads == [0]
                for chunk in (*variants, None, 'after-write'):
                    if chunk == 'after-write' and tokens != ROWS:
                        continue
                    seed.fill_(19.)
                    held = {}
                    chosen = 28 if chunk == 'after-write' else (1 if chunk is None else chunk)
                    allocations, query = menu(_kernels._gemm, device, tokens, heads, chosen)
                    capacity_key = f'fixture.projected-heads-capacity/{tokens}/{heads}/{chunk}'
                    census = Census(vt, device, seed, (1, tokens, heads, 128),
                                    refuse_after_second_seed=chunk == 'after-write')
                    legacy_calls = [0]
                    prior_counts = {key: row['selections'] for key, row in request.working_splits.items()}

                    def actual_original(value, legacy_calls=legacy_calls):
                        legacy_calls[0] += 1
                        return value(value)

                    def step(i, marker, seed, hidden, cos, sin, *,
                             capacity_key=capacity_key, allocations=tuple(allocations),
                             chunk=chunk, census=census, held=held, attn=attn,
                             actual_original=actual_original):
                        del i, marker
                        assert not seed.is_inference(), 'the mutable seed must be Loop-owned state'
                        census.seed = seed
                        current_operator = glue._Heads.of(attn.processor, attn, hidden, (cos, sin), None)
                        assert type(current_operator) is glue._Heads
                        trim(vt, device)
                        before = vt.stats(device)
                        room = vt.split_select(device, capacity_key, [('target', allocations)],
                                               preferred='target', stream=int(torch.cuda.current_stream(device).cuda_stream))
                        if chunk is None:
                            assert room['required_bytes'] > 0, 'minimum is fully cached; cannot force refusal'
                        vt.set_limit(device, before['mapped_bytes'] + room['required_bytes'] - (1 if chunk is None else 0))
                        mean_token = glue.KEY_MEAN.set(glue.KeyMean(seed))
                        try:
                            with census:
                                held['output'] = split(actual_original, current_operator, Cut(2), intent=current_operator.intent())
                        except torch.OutOfMemoryError as error:
                            if chunk is not None and chunk != 'after-write':
                                raise
                            assert error.varena_no_retry
                            held['refusal'] = {'no_retry': True, 'type': type(error).__name__,
                                               'allocation_delta': vt.stats(device)['allocations'] - before['allocations']}
                            if chunk is None:
                                assert held['refusal']['allocation_delta'] == 0
                        finally:
                            glue.KEY_MEAN.reset(mean_token)
                            vt.set_limit(device, None)

                    try:
                        denoise_loop(step, range(1), state=(marker,), carried=(seed,), inputs=(hidden, *rotary),
                                     key=('actual-h3-heads', tokens, heads))
                    finally:
                        vt.phase_forget(device, capacity_key)
                    phase = request.working_phases[-1]
                    tel.log('heads case boundary', tokens=tokens, heads=heads, chunk=str(chunk),
                            seed_copies=census.seed_copies, returned='output' in held,
                            refused='refusal' in held,
                            boundary='none' if census.boundary is None else census.boundary['stage'])
                    tel.log(json.dumps({'heads_case_executed': {'tokens': tokens, 'heads': heads,
                        'requested_chunk': chunk, 'legacy_calls': legacy_calls[0],
                        'after_write_boundary_reached': census.boundary is not None,
                        'allocations': [entry for row in census.rows for entry in row['storage']],
                        'operation_counts': dict(Counter(row['op'] for row in census.rows)),
                        'refusal': held.get('refusal'), 'phase': phase}}, sort_keys=True))
                    assert legacy_calls == [0], 'accepted native operation entered the original fallback'
                    if chunk is None:
                        assert 'refusal' in held and 'output' not in held
                        assert torch.equal(seed.cpu(), torch.full((heads, 128), 19., dtype=torch.float32))
                        assert phase['invalidated'] and not phase['published']
                        cases.append({'case': 'minimum-refusal', 'tokens': tokens, 'heads': heads,
                                      'seed_unchanged': True, 'refusal': held['refusal'], 'phase': phase})
                    else:
                        observed = [storage for row in census.rows for storage in row['storage']]
                        assert Counter(r['requested_bytes'] for r in observed) == Counter(allocations), (observed, allocations)
                        assert all(row['op'].startswith('aten.empty.') for row in census.rows if row['storage'])
                        seed_ops = [row for row in census.rows if row['op'].startswith('aten.div_.')
                                    or row['role'] == 'seed-copy']
                        windows = (heads + chosen - 1) // chosen
                        assert len(seed_ops) == 2 * windows
                        assert all(row['allocations'] == 0 for row in seed_ops), seed_ops
                        output_copies = [row for row in census.rows if row['role'] == 'output-transpose-copy']
                        assert len(output_copies) == (1 if chunk == 'after-write' else windows)
                        assert all(row['allocations'] == 0 for row in output_copies), output_copies
                        classes = dict(sorted(Counter(r['actual_class_bytes'] for r in observed).items()))
                        samples = phase['split_observations']
                        current = [row for key, row in request.working_splits.items()
                                   if row['selections'] > prior_counts.get(key, 0)]
                        assert len(current) == 1 and current[0]['site'] == 'projected-heads'
                        record = current[0]
                        assert record['head_chunk'] == chosen
                        assert record['implementation'] == 'kitchen-int8'
                        assert record['attention_api'] == 'kitchen-int8-split/1'
                        assert record['attention_identity'][-1] == provenance['kitchen_extension_sha256']
                        if chunk == 'after-write':
                            assert ('refusal' in held and 'output' not in held
                                    and census.boundary is not None
                                    and census.boundary['stage'] == 'real-refusal-requested'), {
                                        'head_chunk': record['head_chunk'], 'held': sorted(held),
                                        'seed_copies': census.seed_copies,
                                        'boundary': None if census.boundary is None else census.boundary['stage']}
                            assert phase['invalidated'] and not phase['published']
                            assert not torch.equal(census.boundary['seed_after_second_copy'], torch.full((heads, 128), 19., dtype=torch.float32))
                            partial = output_equal(census.boundary['first_output_head_after_write'], reference[:, :, :1])
                            cases.append({'case': 'real-refusal-after-writes', 'tokens': tokens, 'heads': heads,
                                          'boundary': 'second seed copy completed, after first output chunk',
                                          'seed_copies_completed': 2, 'first_output_head': partial,
                                          'readback_device_allocations': census.boundary['readback_device_allocations'],
                                          'refusal': held['refusal'], 'phase': phase, 'allocation_census': census.rows})
                            cases[-1]['actual_allocation_classes'] = classes
                        else:
                            assert len(samples) == 1 and samples[0]['published'] and phase['cut_choices'] == 1
                            assert record['head_chunk'] == chunk
                            keys.append(samples[0]['key'])
                            cases.append({'case': 'whole-or-head-window', 'tokens': tokens, 'heads': heads,
                                          'head_chunk': chunk, 'tail_heads': query['tail_heads'],
                                          'output': output_equal(held['output'].cpu(), reference),
                                          'seed': output_equal(seed.cpu(), seed_reference),
                                          'phase': phase, 'selection': json.loads(json.dumps(record)),
                                          'allocation_census': census.rows, 'query': query})
                            cases[-1]['actual_allocation_classes'] = classes
                    assert hook_reads == [0]
                    held.clear()
                    census.output = census.boundary = None
                    del step, actual_original, census

                if attn._fixture_output_refusal:
                    assert heads == 55 and heads * 128 == 7040
                    cases.append({'case': 'output-native-encoding-refused', 'tokens': tokens,
                                  'heads': heads, 'input_columns': heads * 128,
                                  'reason': attn._fixture_output_refusal,
                                  'heads_operation_and_tail_qualified_separately': True})
                else:
                    # The actual processor owns successful seed bookkeeping, and its ordinary
                    # output projection also exercises launch_projection's unchanged default.
                    siblings_before = getattr(attn.to_out[0], '_cozy_siblings', None)
                    # A real normal-tensor invocation learns single-use operand ownership.
                    # An inference tensor intentionally does not train that existing policy.
                    with torch.inference_mode(False), torch.no_grad():
                        attended = reference.to(on)
                        assert not attended.is_inference()
                        expected_owner = attn.to_out[1](attn.to_out[0](attended.flatten(2, 3))).cpu()
                        del attended
                    gc.collect()
                    siblings_after = getattr(attn.to_out[0], '_cozy_siblings', None)
                    assert siblings_after == 1, {'before': siblings_before, 'after': siblings_after}
                    modes = ('direct', 'exact-path', 'prefix-heavy', 'planned') if tokens == 513 else ('direct',)
                    for mode in modes:
                        layout = None
                        phase_plan = phase_region = None
                        attn.processor._attention_backend = selected
                        if mode in ('exact-path', 'prefix-heavy'):
                            sol, = attention.pinned('kitchen-sol-int4qk', measure_device(torch, device, 'cuda'))
                            attn.processor._attention_backend = sol.member
                            attention_sol.install_site(attn, 'fixture', 'attention')
                            layout = AttentionLayout(tokens, tokens // 2 + 1 if mode == 'prefix-heavy' else 0,
                                                     step=2, dense_until_step=1,
                                                     exact_paths=('attention',) if mode == 'exact-path' else ())
                        if mode == 'planned':
                            from tensorfs import varena
                            phase_region = request.memory.native.reserve(2 << 20)
                            use = phase_region.prepare()
                            tensor = use.tensor
                            use.consume_on(int(torch.cuda.current_stream(device).cuda_stream))
                            assert use.needs_fill
                            tensor.fill_(53)
                            use.commit()
                            del tensor, use
                            phase_plan = varena.Plan(request.memory.native, [('actual-heads', [phase_region])])
                        seed.fill_(19.)
                        owner_result = {}
                        before_selections = {key: row['selections'] for key, row in request.working_splits.items()}

                        def owner(i, marker, seed, hidden, cos, sin, *, attn=attn,
                                  owner_result=owner_result, layout=layout, phase_plan=phase_plan):
                            del i, marker
                            assert not seed.is_inference()
                            mean = glue.KeyMean(seed)
                            mean_token = glue.KEY_MEAN.set(mean)
                            try:
                                if phase_plan is not None:
                                    phase_plan.activate('actual-heads')
                                with attention_scope(layout) if layout is not None else nullcontext():
                                    owner_result['output'] = attn(hidden, (cos, sin), None)
                                owner_result['seeded_heads'] = mean.heads
                            finally:
                                glue.KEY_MEAN.reset(mean_token)

                        try:
                            denoise_loop(owner, range(1), state=(marker,), carried=(seed,), inputs=(hidden, *rotary),
                                         key=('actual-h3-attention-owner', tokens, heads, mode))
                        finally:
                            if phase_plan is not None:
                                phase_plan.deactivate()
                        phase = request.working_phases[-1]
                        tel.log('heads complete processor', tokens=tokens, heads=heads, route=mode,
                                seeded_heads=owner_result['seeded_heads'],
                                native_observations=len(phase['split_observations']),
                                learned_siblings=siblings_after)
                        assert owner_result['seeded_heads'] == heads and len(phase['split_observations']) == 2, {
                            'seeded_heads': owner_result['seeded_heads'], 'expected_heads': heads,
                            'native_observations': len(phase['split_observations']),
                            'siblings_before': siblings_before, 'siblings_after': siblings_after}
                        assert phase['cut_choices'] == 2 and all(s['published'] for s in phase['split_observations'])
                        owner_selection, = [row for key, row in request.working_splits.items()
                                            if row['site'] == 'projected-heads'
                                            and row['selections'] > before_selections.get(key, 0)]
                        assert owner_selection['implementation'] == 'kitchen-int8'
                        assert owner_selection['backend'] == str(attn.processor._attention_backend.value)
                        if mode == 'planned':
                            assert owner_selection['selection']['plan_generation'] > 0
                            assert owner_selection['selection']['preserves_phase_weights']
                            assert owner_selection['selection']['phase_weight_bytes'] == phase_region.allocated_bytes
                            assert phase_region.resident
                            use = phase_region.prepare()
                            tensor = use.tensor
                            use.consume_on(int(torch.cuda.current_stream(device).cuda_stream))
                            assert not use.needs_fill and int(tensor[0]) == int(tensor[-1]) == 53
                            del tensor, use
                        elif mode in ('exact-path', 'prefix-heavy'):
                            assert owner_selection['route_reason'] == ('dense_path' if mode == 'exact-path' else 'prefix_heavy')
                        cases.append({'case': 'complete-processor-and-output-projection', 'tokens': tokens,
                                      'heads': heads, 'route': mode,
                                      'seeded_heads': owner_result['seeded_heads'], 'phase': phase,
                                      'selection': json.loads(json.dumps(owner_selection)),
                                      'normal_input_learning': {'before': siblings_before, 'after': siblings_after},
                                      'output': output_equal(owner_result['output'].cpu(), expected_owner),
                                      'seed': output_equal(seed.cpu(), seed_reference)})
                        owner_result.clear()
                        del owner, owner_result, phase_plan, phase_region
                    attention_sol.remove_site(attn, 'fixture', 'attention')
                    attn.processor._attention_backend = selected
                    del expected_owner

                from dataclasses import replace
                intent = operator.intent()
                for name, declared in (
                    ('other-backend', replace(intent, backend=attention.BY_NAME['sdpa'].backend)),
                    ('unknown-backend', replace(intent, backend='not-a-selected-backend')),
                    ('seed-alias', replace(intent, seed=hidden.view(torch.float32).reshape(-1)[:heads * 128].view(heads, 128))),
                ):
                    assert native_heads.prepare_projected_heads(declared, None, (Cut(2),)) is None
                    declined.append(name)
                del intent, declared
            finally:
                glue.KEY_MEAN.reset(token)
            del attn, operator, hidden, rotary, seed, marker, reference, seed_reference
        del record, observed
    gc.collect()
    trim(vt, device)
    final = vt.stats(device)
    assert final['allocated_bytes'] == initial['allocated_bytes'], (initial, final)
    assert final['pending_bytes'] == 0 and final['pools'] == initial['pools']
    keys = sorted(request.working_phase_keys)
    return Report(True, json.dumps({'passed': True, 'scope': 'instrumented real mechanism; not a timing benchmark',
                                   'provenance': provenance, 'cases': cases, 'declined': declined,
                                   'hook_factor_reads': 0,
                                   'initial': initial, 'final': final, 'request_owned_keys': keys}, sort_keys=True))


@app.entrypoint(accelerator=True)
def cleanup(payload: Cleanup, ctx: Context) -> Report:
    request, vt, provenance = actual_request(payload, ctx)
    assert os.getpid() == payload.expected_pid and not request.working_phase_keys
    removed = {key: vt.phase_forget(ctx.device.index or 0, key) for key in payload.keys}
    assert removed and not any(removed.values()), removed
    return Report(True, json.dumps({'passed': True, 'provenance': provenance,
                                   'prior_request_keys_already_retired': removed}, sort_keys=True))
