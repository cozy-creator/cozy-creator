"""Actual dense and sparse H3 processors, with a fresh first-use output projection."""
from __future__ import annotations

from contextvars import ContextVar
from dataclasses import replace

import torch
from cozy_runtime.author import (
    AttentionLayout,
    Context,
    ProducerSite,
    attention_scope,
    denoise_loop,
    register_attention,
)
from minimax_h3 import glue, producer, producer_kernels

from . import Probe, Report, actual, app, report, settle

WIDTH = 5376
_CASE = ContextVar('norm_output_fixture', default=None)
_ORIGINAL = producer.PRODUCERS[producer.INT4QK]


def observed(call, hidden, rotary=None, attention_mask=None):
    case = _CASE.get()
    if case is None or not case['reference']:
        return _ORIGINAL.produce(call, hidden, rotary, attention_mask)
    # The independent ordinary recipe runs on a different real module. It cannot teach
    # the candidate module's sibling count or consume its private attended output.
    assert attention_mask is None
    if case['mode'] == 'dense':
        processor = call.module.processor.original
        operation = glue._Heads.of(processor, call.module, hidden, rotary, None)
    else:
        tokens, heads = call.layout.live_tokens, call.module.heads
        freqs = producer.rope_presentation(rotary, tokens, call.state)
        means = producer._Means(call, (call.component, call.path), producer.INT4QK,
                                (tokens, heads, call.layout.protected_prefix))
        operation = producer._Heads.of(call, means, hidden, freqs)
    assert operation is not None
    attended = operation(operation)
    return call.module.to_out[1](call.module.to_out[0](attended.flatten(2, 3)))


_OBSERVED = register_attention(replace(_ORIGINAL, produce=observed))


def make_attention(on, generator, launch, stats, selected, heads):
    from cozy_runtime import lora
    from cozy_runtime.internal import producers
    from cozy_runtime.internal.encoding.formats import CONVROT_GROUP
    from cozy_runtime.internal.encoding.leaves import Int8ConvRotNativeLeaf
    from diffusers.models.transformers.transformer_minimax_h3 import MiniMaxH3Attention

    with torch.device('meta'):
        attn = MiniMaxH3Attention(WIDTH, heads, 128)
    attn.norm_q = torch.nn.RMSNorm(128, eps=1e-5, dtype=torch.bfloat16, device=on)
    attn.norm_k = torch.nn.RMSNorm(128, eps=1e-5, dtype=torch.bfloat16, device=on)
    provider = Int8ConvRotNativeLeaf()
    params = {'rotation': 'kron_regular_h4', 'rotation_size': CONVROT_GROUP}
    reads = [0]

    def armed():
        reads[0] += 1

    for parent, name, channels, columns in [
        *((attn, n, heads * 128, WIDTH) for n in ('to_q', 'to_k', 'to_v')),
        (attn.to_out, '0', WIDTH, heads * 128),
    ]:
        refusal = provider.supports((channels, columns), params)
        if refusal:
            assert parent is attn.to_out and heads == 55
            continue
        leaf = provider.leaf(torch, {
            'weight': torch.randint(-127, 128, (channels, columns), generator=generator,
                                    dtype=torch.int8).to(on),
            'scale': torch.full((channels,), 0.001, dtype=torch.float32, device=on),
        }, getattr(parent, name), torch.bfloat16)
        hook = lora.Hook(armed)
        hook._cozy_baked = True
        leaf.register_forward_hook(hook)
        setattr(parent, name, leaf)
    processor = glue.FusedMiniMaxH3AttnProcessor()
    processor._glue, processor._producer_stats = launch, stats
    processor._projected_head_entries = (
        next(entry for entry in glue.GLUE.entries if entry.kernel is glue._rope_norm_kernel),
        next(entry for entry in glue.GLUE.entries if entry.kernel is glue._key_sum_kernel),
    )
    processor._attention_backend, processor._parallel_config = selected, None
    attn.set_processor(processor)
    producers.install(attn, _OBSERVED, str(selected.value), 'comfy_kitchen', 'fixture', 'attention')
    return attn.eval(), reads



@app.entrypoint(accelerator=True)
def projection(payload: Probe, ctx: Context) -> Report:
    from cozy_runtime.author.kernels import _RECENT, quantize_once
    from cozy_runtime.internal import (
        attention,
        int8_linear,
        prebuilt,
        triton_kernels,
        verify,
    )
    from cozy_runtime.internal.encoding.selection import measure_device

    request, vt, provenance = actual(payload, ctx)
    device, on = ctx.device.index or 0, request.memory.device
    torch.set_num_threads(1)
    assert int8_linear.native(torch, device) is not None
    ready, = attention.pinned(_OBSERVED.name, measure_device(torch, device, 'cuda'))
    launch = triton_kernels.load(glue.kernels(on), device=on)
    stats = triton_kernels.load(producer_kernels.STATS, device=on)
    provenance.update(backend=str(ready.member.value), kitchen_artifact=ready.artifact,
                      launcher=launch.key, stats_launcher=stats.key)
    settle(vt, device)
    initial = vt.stats(device)
    result = {'provenance': provenance, 'initial': initial, 'cases': [], 'keys': []}
    state = _OBSERVED.request_state()

    def run(result):
        attn = reference_attn = hidden = rotary = marker = slot = output = None
        step = None
        try:
            with torch.no_grad():
                tokens, heads = 513, 56
                for mode in ('dense', 'sparse-bootstrap'):
                    hidden = torch.randn((1, tokens, WIDTH), dtype=torch.bfloat16,
                                         generator=torch.Generator().manual_seed(payload.seed + 1)).to(on)
                    angles = torch.randn((tokens, 96), generator=torch.Generator().manual_seed(payload.seed + 2)).to(on)
                    rotary = angles.cos(), angles.sin()
                    del angles
                    layout = AttentionLayout(tokens, 64, step=0,
                                             dense_until_step=99 if mode == 'dense' else 0)
                    reference_attn, reference_reads = make_attention(
                        on, torch.Generator().manual_seed(payload.seed), launch, stats, ready.member, heads)
                    token = _CASE.set({'mode': mode, 'reference': True})
                    try:
                        with attention_scope(layout):
                            expected = reference_attn(hidden, rotary, None).cpu()
                    finally:
                        _CASE.reset(token)
                        state.close()
                    assert reference_reads == [0]
                    reference_attn = None
                    settle(vt, device)
                    attn, reads = make_attention(on, torch.Generator().manual_seed(payload.seed),
                                                launch, stats, ready.member, heads)
                    assert getattr(attn.to_out[0], '_cozy_siblings', None) is None
                    marker = torch.zeros(1, dtype=torch.int64, device=on)
                    slot = torch.zeros((heads, 128), dtype=torch.float32, device=on)
                    producer.present((ProducerSite(attn, 'fixture', 'attention', _OBSERVED),),
                                     rotary, tokens)
                    case = {'mode': mode, 'reference': False}
                    before = {key: row['selections'] for key, row in request.working_splits.items()}
                    held = {}

                    def step(i, marker, slot, values, cos, sin, *, attn=attn, case=case,
                             layout=layout, held=held):
                        del i, marker
                        assert not values.is_inference()
                        assert getattr(attn.to_out[0], '_cozy_siblings', None) is None
                        token = _CASE.set(case)
                        try:
                            with producer.carrying(producer.Carry({id(attn): slot}, rolling=False)), attention_scope(layout):
                                output = attn(values, (cos, sin), None)
                                held['output'] = output.cpu()
                                del output
                        finally:
                            _CASE.reset(token)

                    denoise_loop(step, range(1), state=(marker,), carried=(slot,),
                                 inputs=(hidden, *rotary),
                                 key=('first-output', mode))
                    changed = [dict(row) for key, row in request.working_splits.items()
                               if row['selections'] > before.get(key, 0)]
                    linear = [row for row in changed if row['site'] == 'int8-linear-rows']
                    phase = request.working_phases[-1]
                    error = verify.error(torch, held['output'], expected)
                    case_result = {'case': f'first-normal/{mode}', 'records': changed, 'phase': phase,
                                   'relative_l2': error, 'bit_exact': torch.equal(held['output'], expected),
                                   'sibling_count_after': getattr(attn.to_out[0], '_cozy_siblings', None),
                                   'factor_reads': reads[0]}
                    result['cases'].append(case_result)
                    result['keys'] += [row['key'] for row in changed]
                    assert len(linear) == 1 and linear[0]['input_ownership'] == 'declared_single_use'
                    assert len(changed) == 2, changed  # actual complete Heads plus its one projection
                    assert len(phase['split_observations']) == 2
                    assert all(row['published'] for row in phase['split_observations'])
                    assert error <= prebuilt.BUDGET and reads == [0]
                    assert case_result['sibling_count_after'] is None
                    held.clear()
                    step = None
                    state.close()
                    # Real explicit sharing still owns its quantized operand. The new
                    # declaration must decline it and leave the ordinary consumer path.
                    values = torch.randn((1, 513, heads * 128), dtype=torch.bfloat16,
                                         generator=torch.Generator().manual_seed(payload.seed + 3)).to(on)
                    prior = {key: row['selections'] for key, row in request.working_splits.items()}
                    with quantize_once(values):
                        shared_result = glue.output_projection(attn.to_out[0], values)
                        expected_shared = attn.to_out[0](values)
                        shared_error = verify.error(torch, shared_result.cpu(), expected_shared.cpu())
                    assert all(row['selections'] == prior.get(key, 0)
                               for key, row in request.working_splits.items())
                    result['cases'].append({'case': f'explicit-sharing/{mode}',
                                            'relative_l2': shared_error, 'native_selections': 0})
                    assert shared_error <= prebuilt.BUDGET
                    shared_result = expected_shared = values = None
                    # Two real ordinary reads of the same normal input teach a multi-use
                    # group; no test assigns _cozy_siblings to manufacture this exclusion.
                    values = torch.randn((1, 513, heads * 128), dtype=torch.bfloat16,
                                         generator=torch.Generator().manual_seed(payload.seed + 4)).to(on)
                    first = attn.to_out[0](values)
                    second = attn.to_out[0](values)
                    assert first.shape == second.shape == (1, 513, WIDTH)
                    assert _RECENT.held is not None and _RECENT.held.uses == 2
                    values = first = second = None
                    settle(vt, device)
                    assert getattr(attn.to_out[0], '_cozy_siblings', None) == 2
                    values = torch.randn((1, 513, heads * 128), dtype=torch.bfloat16,
                                         generator=torch.Generator().manual_seed(payload.seed + 5)).to(on)
                    prior = {key: row['selections'] for key, row in request.working_splits.items()}
                    output = glue.output_projection(attn.to_out[0], values)
                    assert all(row['selections'] == prior.get(key, 0)
                               for key, row in request.working_splits.items())
                    expected_shared = attn.to_out[0](values)
                    learned_error = verify.error(torch, output.cpu(), expected_shared.cpu())
                    assert bool(torch.isfinite(output.cpu()).all()) and learned_error <= prebuilt.BUDGET
                    result['cases'].append({'case': f'actual-learned-sharing/{mode}',
                                            'sibling_count': attn.to_out[0]._cozy_siblings,
                                            'relative_l2': learned_error, 'native_selections': 0})
                    # A real new input retires the previous learned group's last-reader
                    # marker, then its own weak input lifetime releases the new operand.
                    next_values = torch.randn((1, 513, heads * 128), dtype=torch.bfloat16,
                                              generator=torch.Generator().manual_seed(payload.seed + 6)).to(on)
                    next_output = attn.to_out[0](next_values)
                    assert next_output.shape == (1, 513, WIDTH)
                    next_values = next_output = expected_shared = None
                    attn = hidden = rotary = marker = slot = output = values = None
                    settle(vt, device)
        finally:
            vt.set_limit(device, None)
            step = None
            attn = reference_attn = hidden = rotary = marker = slot = output = None
            values = shared_result = expected_shared = first = second = None
            next_values = next_output = None
            state.close()
            settle(vt, device)
            result['final'] = vt.stats(device)
            result['keys'] = sorted(set(result['keys']) | request.working_phase_keys)
            for field in ('allocated_bytes', 'allocated_capacity', 'mapped_bytes',
                          'pending_bytes', 'pools', 'pools_bytes', 'event_handles'):
                assert result['final'][field] == initial[field], (field, initial, result['final'])
    return report(result, run)
