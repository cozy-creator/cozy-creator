"""Cancel only this gate's exact run after its native-pinned child reports ready (Linux).

Uses the default Cozy CLI; validates the child command and process birth, observes native
cleanup, and checks that exact child is gone at terminal settlement. It never kills a PID.
"""
import argparse
import os
import json
import subprocess
import time
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--candidate-python", required=True, type=Path)
parser.add_argument("--output", required=True, type=Path)
args = parser.parse_args()
output = args.output.absolute()
output.mkdir(parents=True, exist_ok=True)
cozy = str(Path.home() / ".cozy/bin/cozy")
python = str(args.candidate_python.absolute())  # preserve the venv symlink, not its base Python
os.chdir(Path(__file__).resolve().parents[4])
run = None
watch = subprocess.Popen([cozy, '--json', 'run', './release-gate/suites/varena/stock-memory/cancel', 'seed=741', f'candidate_python={python}', '--gpus=1', '--on', 'local'], text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
assert watch.stdout is not None
receipt = {'candidate_python': python}
ready = False
with (output / 'watch.log').open('w') as log:
    for line in watch.stdout:
        log.write(line)
        log.flush()
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if run is None and isinstance(row.get('run'), dict) and row['run'].get('id'):
            run = row['run']
            receipt['run_id'] = run['id']
            print('Owned cancellation run:', run['id'], flush=True)
        if row.get('type') == 'run.log':
            try:
                detail = json.loads(row.get('text', ''))
            except ValueError:
                continue
            if isinstance(detail.get('result'), dict) and detail['result'].get('cancel_cleanup'):
                receipt['native_cleanup'] = detail['result']
            if detail.get('cancel_ready') and not ready:
                ready = True
                pid = int(detail['pid'])
                command = Path(f'/proc/{pid}/cmdline').read_bytes().replace(b'\x00', b' ').decode()
                assert python in command and 'cancel.py' in command, command
                birth = Path(f'/proc/{pid}/stat').read_text().split()[21]
                receipt.update(ready=detail, process_birth=birth, child_command=command, canceled_at=time.time())
                canceled = subprocess.run([cozy, '--json', 'run', 'cancel', run['id']], text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
                receipt['cancel_exit_code'] = canceled.returncode
                receipt['cancel_response'] = canceled.stdout
                print('Canceled exact ready child:', pid, flush=True)
        if row.get('type') in ('run.canceled', 'run.failed', 'run.completed'):
            receipt['terminal'] = row
watch.wait()
assert ready, 'no native-pinned ready receipt'
status = receipt.get('terminal', {}).get('run', {}).get('status')
assert status == 'canceled', receipt.get('terminal')
path = Path(f'/proc/{pid}/stat')
receipt['same_child_still_running'] = path.exists() and path.read_text().split()[21] == birth
receipt['settled_at'] = time.time()
(output / 'receipt.json').write_text(json.dumps(receipt, indent=2))
assert not receipt['same_child_still_running'], 'canceled native child remains alive'
assert receipt.get('native_cleanup', {}).get('passed'), 'native cleanup receipt missing'
print(json.dumps({'passed': True, 'run': run['id'], 'pid': pid, 'terminal': status, 'child_exited': True}), flush=True)
