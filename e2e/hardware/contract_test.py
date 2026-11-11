"""Exercise the real published Runtime describer, including its forbidden-import control."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

from hardware_types import object_map, sequence, text


class Options(argparse.Namespace):
    runtime_cli: Path
    out: Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime-cli', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args(namespace=Options())
    args.out.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).resolve().parent / 'probe'
    command = [str(args.runtime_cli.resolve()), '--json', 'describe', '--dir']
    accepted = subprocess.run([*command, str(source)], capture_output=True, text=True)
    (args.out / 'accepted.stdout').write_text(accepted.stdout)
    (args.out / 'accepted.stderr').write_text(accepted.stderr)
    assert accepted.returncode == 0, accepted.stdout + accepted.stderr
    value: object = json.loads(accepted.stdout)
    interface = object_map(value)
    modes: dict[str, bool] = {}
    for item in sequence(interface['entrypoints']):
        entry = object_map(item)
        accelerator = entry.get('accelerator', True)
        if not isinstance(accelerator, bool):
            raise TypeError('described accelerator must be boolean')
        modes[text(entry['name'])] = accelerator
    assert modes == {'cpu': False, 'gpu': True}, modes
    with tempfile.TemporaryDirectory() as temporary:
        invalid = Path(temporary) / 'probe'
        shutil.copytree(source, invalid)
        module = invalid / 'ci_hardware_probe/__init__.py'
        module.write_text(module.read_text() + '\nimport tensorfs\n')
        refused = subprocess.run([*command, str(invalid)], capture_output=True, text=True)
    (args.out / 'forbidden.stdout').write_text(refused.stdout)
    (args.out / 'forbidden.stderr').write_text(refused.stderr)
    assert refused.returncode != 0 and 'internal_import' in refused.stdout + refused.stderr
    print('PASS real Runtime.describe accepts public CPU/GPU probe and refuses direct TensorFS import')


if __name__ == '__main__':
    main()
