"""Validate scalar host evidence. A normal return is never GPU completion."""
import argparse
import hashlib
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from host_stage_ring import EVENTS, FIELDS, STATUS


def saved_gap(directory):
    facts = json.loads((directory / 'artifacts/27056-artifacts-1.json').read_text())
    trace = json.loads((directory / 'artifacts/27056-artifacts-6.json').read_text())
    late = facts['facts']['boundaries']['unet.up_blocks.0']['replays']
    traced = trace['extra']['replays']
    missing = trace['attempt'] == 2 and traced == 1 and late == 2
    return {'evidence': 'saved original27056', 'late_interval_present': not missing,
            'status': 'MISSING_LATE_INTERVAL' if missing else 'not_the_bound_case',
            'trace_boundary': trace['boundary'], 'trace_attempt': trace['attempt'],
            'trace_rows': len(trace['rows']), 'later_up0_replays': late,
            'original_gpu_result_remains_failed': True}, 1 if missing else 2


def analyze(path):
    raw = path.read_bytes()
    data = json.loads(raw)
    if data['format'] != 'varena-host-stage-v1' or data['fields'] != list(FIELDS):
        raise ValueError('host-stage format identity mismatch')
    if data['events'] != EVENTS or data['statuses'] != STATUS:
        raise ValueError('host-stage event/status dictionary mismatch')
    if not data['available'] or data['lost'] or data['normal_return_is_gpu_completion'] is not False:
        raise ValueError('unavailable or misclassified host evidence')
    names = {int(k): v for k, v in data['names'].items()}
    native_ids = {int(k): v for k, v in data['native_cache_ids'].items()}
    events = {v: k for k, v in EVENTS.items()}
    statuses = {v: k for k, v in STATUS.items()}
    result = []
    for seq, row in enumerate(data['rows'], 1):
        if len(row) != len(FIELDS) or row[0] != seq:
            raise ValueError('uncommitted/noncontiguous host row')
        if any(type(value) is not int or not 0 <= value < 2**64 for value in row):
            raise ValueError('nonprimitive host row')
        entry = dict(zip(FIELDS, row))
        entry['event_name'] = events[entry['event']]
        entry['status_name'] = statuses[entry['status']]
        entry['boundary_name'] = names.get(entry['boundary_id'])
        entry['derived_native_cache_owner_id'] = native_ids.get(entry['cache_entry_id'], 0)
        result.append(entry)
    target = [row for row in result if row['boundary_name'] == 'unet.up_blocks.0' and row['dispatch_attempt'] == 3]
    following = [row for row in result if row['boundary_name'] == 'unet.up_blocks.1' and row['dispatch_attempt'] == 3]
    target_entered = any(row['event_name'] == 'dispatch_enter' for row in target)
    target_failed = any(row['event_name'] == 'dispatch_exit' and row['status_name'] == 'unwind' for row in target)
    following_finished = any(row['event_name'] == 'dispatch_exit' and row['status_name'] in ('normal', 'unwind') for row in following)
    complete = target_entered and (target_failed or following_finished)
    return {'available': True, 'host_control_only': data['host_control_only'],
            'metadata': data['metadata'], 'sha256': hashlib.sha256(raw).hexdigest(),
            'row_count': len(result), 'late_interval_present': complete,
            'normal_return_is_gpu_completion': False,
            'first_observed_unwind': next((row for row in result if row['status_name'] == 'unwind'), None),
            'last_normal_dispatch_return': next((row for row in reversed(result) if row['event_name'] == 'dispatch_exit' and row['status_name'] == 'normal'), None),
            'rows': result}, 0 if complete else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument('--ring', type=pathlib.Path)
    group.add_argument('--saved-27056', type=pathlib.Path)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    result, code = saved_gap(args.saved_27056) if args.saved_27056 else analyze(args.ring)
    with args.output.open('x') as output:
        json.dump(result, output, indent=2)
        output.write('\n')
    print(json.dumps({k: v for k, v in result.items() if k != 'rows'}))
    return code


if __name__ == '__main__':
    sys.exit(main())
