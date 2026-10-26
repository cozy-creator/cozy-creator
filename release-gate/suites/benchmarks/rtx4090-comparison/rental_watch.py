"""Independent exact-order expiry, extending the existing default-home CLI rental watchdog."""
import argparse
import json
import re
import subprocess
import time
from pathlib import Path

CLI = Path.home() / '.cozy/bin/cozy'
parser = argparse.ArgumentParser()
parser.add_argument('--out', required=True, type=Path)
parser.add_argument('--deadline', required=True, type=float)
args = parser.parse_args()
context = json.loads((args.out / 'watchdog-context.json').read_text())


def save(name, value):
    temporary = args.out / (name + '.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(args.out / name)


def call(label, arguments, timeout=35):
    with (args.out / (label + '.json')).open('w') as out, (args.out / (label + '.err')).open('w') as err:
        timeout = min(timeout, max(1, context['hard_deadline_epoch'] - time.time()))
        result = subprocess.run([str(CLI), '--json', '--tensorhub', context['hub'], *arguments],
                                stdin=subprocess.DEVNULL, stdout=out, stderr=err, timeout=timeout)
    text = (args.out / (label + '.json')).read_text()
    return result.returncode, json.loads(text) if text.strip() else {}


def own(machine):
    return (machine.get('kind') == 'rental'
            and machine.get('rental_id') not in context['prior_ids']
            and str(machine.get('rental_id', '')).startswith('pr-')
            and machine.get('rental', {}).get('rented_at_ms', 0) >= (context['rent_start_epoch'] - 10) * 1000
            and machine['rental'].get('sku') == context['sku']
            and machine['rental'].get('gpus') == 1)


def recover_response():
    """Recover only a name returned by this rent's own stdout/stderr, never a list difference."""
    names = []
    for name, pattern in [('rent.err', r'^([a-z][a-z0-9-]*): (?:booting|ready|offline)'),
                          ('rent.txt', r'^machine\s+(\S+)')]:
        path = args.out / name
        if path.exists():
            names += re.findall(pattern, path.read_text(), re.MULTILINE)
    for index, name in enumerate(dict.fromkeys(names)):
        try:
            code, machine = call(f'watchdog-recover-{index}', ['machine', 'show', name])
            if code == 0 and own(machine):
                save('watchdog-owned.json', machine)
                return machine
        except (OSError, ValueError, subprocess.TimeoutExpired):
            pass
    return None


save('watchdog.json', {'pid': __import__('os').getpid(), 'deadline_epoch': args.deadline,
                      'armed_before_order': True, 'purpose': 'end only the exact owned benchmark rental'})
machine = None
while time.time() < args.deadline:
    if (args.out / 'order-not-started.json').exists():
        raise SystemExit(0)
    if (args.out / 'stop-now.json').exists():
        break
    if (args.out / 'ended-confirmed.json').exists():
        raise SystemExit(0)
    owner = args.out / 'owned.json'
    if owner.exists():
        try:
            candidate = json.loads(owner.read_text())
            if own(candidate): machine = candidate
        except (OSError, ValueError): pass
    time.sleep(min(1, max(0, args.deadline - time.time())))
if machine is None:
    machine = recover_response()
if machine is None:
    save('watchdog-unresolved.json', {'cleanup_verified': False, 'reason': 'No usable identity in this order response; no unrelated rental was adopted or ended.'})
    raise SystemExit(2)
rental_id = machine['rental_id']
for attempt in range(3):
    if time.time() >= context['hard_deadline_epoch']:
        break
    try:
        call(f'deadline-end-{attempt}', ['machine', 'end', rental_id, '--force', '--yes'])
        code, seen = call(f'deadline-show-{attempt}', ['machine', 'show', rental_id])
        _, listing = call(f'deadline-list-{attempt}', ['machine', 'list'])
        absent = not any(m.get('rental_id') == rental_id for m in listing.get('machines', []))
        if code == 0 and seen.get('rental_id') == rental_id and seen.get('state') == 'ended' and not seen.get('unverified') and absent and not listing.get('unanswered'):
            save('ended-confirmed.json', {'machine': seen, 'absent_from_listing': True, 'by': 'independent deadline watchdog', 'scope': 'ended/absent, not provider settlement'})
            raise SystemExit(0)
    except (OSError, ValueError, subprocess.TimeoutExpired):
        pass
    time.sleep(3)
save('watchdog-cleanup-incomplete.json', {'rental_id': rental_id, 'cleanup_verified': False})
raise SystemExit(3)
