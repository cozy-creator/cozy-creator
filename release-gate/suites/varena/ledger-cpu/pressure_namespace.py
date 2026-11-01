"""Real host/nested namespace probes. No NVIDIA library or GPU is initialized."""
import argparse
import json
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('probe', type=Path)
parser.add_argument('out', type=Path)
args = parser.parse_args()
args.out.mkdir(parents=True, exist_ok=False)
host = subprocess.run([str(args.probe.resolve())], check=True, capture_output=True, text=True)
report = {'host': json.loads(host.stdout), 'gpu_execution': False}
command = ['unshare', '--user', '--map-root-user', '--pid', '--fork', str(args.probe.resolve())]
nested = subprocess.run(command, capture_output=True, text=True, timeout=10)
report['nested_command'] = command
report['nested_exit'] = nested.returncode
report['nested_stderr'] = nested.stderr
if nested.returncode == 0:
    report['nested'] = json.loads(nested.stdout)
    assert report['nested']['pid'] == 1
    assert not report['nested']['host_pid_attribution']
    assert report['nested']['pid_namespace_inode'] != report['host']['pid_namespace_inode']
else:
    report['nested'] = None
    report['nested_qualified'] = False
    assert 'Operation not permitted' in nested.stderr, nested.stderr
report['passed_available_probes'] = True
(args.out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
