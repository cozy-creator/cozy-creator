"""Audit a completed 17-case INT8 Heads run and its same-process cleanup from raw receipts."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--run', type=Path, required=True)
parser.add_argument('--cleanup', type=Path, required=True)
parser.add_argument('--input', type=Path, required=True)
parser.add_argument('--out', type=Path, required=True)
args = parser.parse_args()
sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
raw, clean_raw, payload = [json.loads(p.read_text()) for p in (args.run, args.cleanup, args.input)]
assert raw['status'] == clean_raw['status'] == 'completed'
assert raw['result']['passed'] and clean_raw['result']['passed']
result, cleanup = [json.loads(r['result']['detail']) for r in (raw, clean_raw)]
assert result['passed'] and cleanup['passed']
for key, value in payload.items():
    if key == 'gpu_uuid':
        assert result['provenance'][key].removeprefix('GPU-').lower() == value.removeprefix('GPU-').lower()
    else:
        assert result['provenance'][key] == value, key
assert payload['runtime_version'] == '0.36.22+dev.g3daaaeda'
assert payload['tensorfs_version'] == '0.17.16+dev.g4dcf2618'
assert result['provenance']['kitchen_version'] == '0.2.40'
assert result['provenance']['kitchen_source_sha256'] == '23447a19aa113722f9b0e9a3b1fcc30cec59c438fe652535b870922f50d0543a'
kernel_sha = result['provenance']['kitchen_extension_sha256']
assert len(kernel_sha) == 64 and int(kernel_sha, 16) > 0
assert result['provenance']['kitchen_artifact']
assert sorted(tuple(row['shape']) for row in result['fused_references']) == [(1,55,513,128),(1,56,513,128),(1,56,8195,128)]
assert all(row['op'] == 'comfy_kitchen.int8_attention.default' and row['dtype'] == 'torch.bfloat16'
           and row['device'] == 'cuda:0' for row in result['fused_references'])
assert cleanup['provenance']['pid'] == cleanup['provenance']['expected_pid'] == result['provenance']['pid']
assert set(cleanup['prior_request_keys_already_retired']) == set(result['request_owned_keys'])
assert not any(cleanup['prior_request_keys_already_retired'].values())
cases = result['cases']
assert len(cases) == 17
assert Counter(c['case'] for c in cases) == {
    'whole-or-head-window': 7, 'minimum-refusal': 3, 'real-refusal-after-writes': 1,
    'complete-processor-and-output-projection': 5, 'output-native-encoding-refused': 1,
}
seen, numeric, allocations, processors = set(), [], [], []
for case in cases:
    kind = case['case']
    if kind == 'output-native-encoding-refused':
        assert case['heads'] == 55 and case['tokens'] == 513 and case['input_columns'] == 7040
        assert '256-column groups' in case['reason'] and case['heads_operation_and_tail_qualified_separately']
        continue
    phase = case['phase']
    if kind == 'minimum-refusal':
        assert case['seed_unchanged'] and case['refusal']['allocation_delta'] == 0
        assert case['refusal']['no_retry'] and phase['invalidated'] and not phase['published']
        assert not phase['split_observations'] and phase['cut_choices'] == 0
        continue
    if kind == 'real-refusal-after-writes':
        assert case['seed_copies_completed'] == 2 and case['readback_device_allocations'] == 0
        assert case['refusal']['no_retry'] and phase['invalidated'] and not phase['published']
        assert len(phase['split_observations']) == 1 and not phase['split_observations'][0]['published']
        metrics = [case['first_output_head']]
    else:
        metrics = [case['output'], case['seed']]
        assert not phase['invalidated']
        expected = 2 if kind == 'complete-processor-and-output-projection' else 1
        assert phase['cut_choices'] == expected and len(phase['split_observations']) == expected
        assert all(s['published'] and s['observed'] and not s['pending'] for s in phase['split_observations'])
        if kind == 'complete-processor-and-output-projection':
            assert case['heads'] == case['seeded_heads'] == 56
            assert case['normal_input_learning']['after'] == 1
            processors.append((case['tokens'],case['route']))
            selected = case['selection']
            if case['route'] == 'planned':
                choice = selected['selection']
                assert isinstance(choice['plan_generation'],int) and choice['plan_generation'] > 0
                assert choice['preserves_phase_weights'] and choice['phase_weight_bytes'] >= (2 << 20)
                cleanup_weights = case['region_cleanup']
                for field in ('resident_bytes','reserved_bytes','pinned_bytes'):
                    assert cleanup_weights['after'][field] == cleanup_weights['before'][field]
                assert 0 <= cleanup_weights['explicitly_reclaimed_bytes'] <= choice['phase_weight_bytes']
            if case['route'] in ('exact-path','prefix-heavy'):
                assert selected['backend'] == '_cozy_kitchen_sol_int4qk'
                assert selected['route_reason'] == ('dense_path' if case['route']=='exact-path' else 'prefix_heavy')
            else:
                assert selected['backend'] == '_cozy_kitchen_int8' and not selected['route_reason']
        selected = case['selection']
        assert selected['implementation'] == 'kitchen-int8'
        assert selected['attention_api'] == 'kitchen-int8-split/1'
        assert selected['attention_identity'][-1] == kernel_sha
    for metric in metrics:
        assert metric['budget'] == 0.01 and 0 <= metric['relative_l2'] <= metric['budget']
        numeric.append(metric)
    if 'allocation_census' not in case:
        continue
    census = case['allocation_census']
    observed = [entry for row in census for entry in row['storage']]
    assert sum(row['allocations'] for row in census) == len(observed)
    assert all(row['op'].startswith('aten.empty.') for row in census if row['storage'])
    assert all(row['allocations'] == 0 for row in census if row['op'].startswith(('aten.copy_.', 'aten.div_.')))
    copies = [row for row in census if row['role'] == 'output-transpose-copy']
    seed_copies = [row for row in census if row['role'] == 'seed-copy']
    windows = 2 if kind == 'real-refusal-after-writes' else (case['heads'] + case['head_chunk'] - 1)//case['head_chunk']
    assert len(seed_copies) == windows
    assert len(copies) == (1 if kind == 'real-refusal-after-writes' else windows)
    assert all(row['allocations'] == 0 for row in copies)
    actual_classes = {int(k): v for k, v in case['actual_allocation_classes'].items()}
    assert actual_classes == dict(Counter(entry['actual_class_bytes'] for entry in observed))
    assert all(entry['actual_class_bytes'] >= entry['requested_bytes'] for entry in observed)
    if kind == 'real-refusal-after-writes':
        control, = [c for c in cases if c['case'] == 'whole-or-head-window'
                    and (c['tokens'],c['heads'],c['head_chunk']) == (8195,56,28)]
        assert Counter(entry['requested_bytes'] for entry in observed) == Counter(control['query']['allocations'])
    if kind == 'whole-or-head-window':
        tokens, heads, chunk = case['tokens'], case['heads'], case['head_chunk']
        seen.add((tokens, heads, chunk))
        query = case['query']
        assert query['tail_heads'] == case['tail_heads'] == (heads % chunk or chunk)
        cta = 128 if tokens > 1024 else 64
        blocks = (tokens + cta - 1) // cta
        expected = [2*tokens*heads*128, *([2*tokens*chunk*128]*3)]
        if any(q['kind'] == 0 for q in query['queries']):
            expected += [tokens*5376, 4*tokens]
        workspace = max(q['workspace_bytes'] for q in query['queries'])
        if workspace:
            expected.append(workspace)
        expected += [tokens*chunk*128, tokens*chunk*128, chunk*128*blocks*cta,
                     4*chunk*((tokens+127)//128)*32, 4*chunk*blocks*4,
                     4*chunk, 4*chunk*128, 4*chunk*128]
        assert Counter(query['allocations']) == Counter(expected)
        assert Counter(entry['requested_bytes'] for entry in observed) == Counter(expected)
        assert case['selection']['head_chunk'] == chunk
        allocations.append({'tokens': tokens, 'heads': heads, 'chunk': chunk,
                            'allocations': len(observed), 'classes': actual_classes})
assert seen == {(8195,56,56),(8195,56,28),(8195,56,7),(8195,56,1),(513,56,56),(513,55,55),(513,55,28)}
assert set(processors) == {(8195,'direct'),(513,'direct'),(513,'exact-path'),(513,'prefix-heavy'),(513,'planned')}
assert result['hook_factor_reads'] == 0
for field in ('allocated_bytes','allocated_capacity','mapped_bytes','pending_bytes','pools','pools_bytes','event_handles'):
    assert result['final'][field] == result['initial'][field], field
assert result['final']['pending_bytes'] == 0
for field in ('resident_bytes','reserved_bytes','pinned_bytes'):
    assert result['final_weights'][field] == result['initial_weights'][field]
receipt = {'passed': True, 'run': raw['number'], 'cleanup': clean_raw['number'],
           'source_scope': 'Runtime3daaaeda9, packagef1e8d2bb, TensorFS4dc, fixture31a53afb5',
           'gpu_uuid': result['provenance']['gpu_uuid'], 'pid': result['provenance']['pid'],
           'cases': len(cases), 'numeric_comparisons': len(numeric),
           'max_relative_l2': max(n['relative_l2'] for n in numeric),
           'exact_comparisons': sum(bool(n['exact']) for n in numeric),
           'allocation_census': allocations, 'initial': result['initial'], 'final': result['final'],
           'initial_weights': result['initial_weights'], 'final_weights': result['final_weights'],
           'fused_references': result['fused_references'], 'processor_routes': processors,
           'kitchen_extension_sha256': kernel_sha,
           'request_owned_keys_retired': result['request_owned_keys'],
           'files': {str(p): sha(p) for p in (args.run,args.cleanup,args.input)},
           'scope': 'instrumented complete operation and lifetime/refusal proof; no full-model speed or quality verdict'}
assert not args.out.exists(), 'preserve previous audits'
args.out.write_text(json.dumps(receipt, indent=2)+'\n')
print(json.dumps({k:v for k,v in receipt.items() if k not in ('allocation_census','initial','final','files','request_owned_keys_retired')}, indent=2))
