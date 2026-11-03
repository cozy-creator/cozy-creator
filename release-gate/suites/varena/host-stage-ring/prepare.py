"""Seal NEW artifact copies against an existing qualified SDK; no import/install/build."""
import argparse
import hashlib
import json
import pathlib
import sys
import shutil

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from host_stage_observe import E7C

FILES = ('entry.py', 'host_stage_observe.py', 'host_stage_ring.py', 'host_stage_seal.py', 'source_seals.json', 'analyze.py')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cohort', choices=('9d', '627'), required=True)
    parser.add_argument('--sdk-root', type=pathlib.Path, required=True, help='existing tensorfs package directory')
    parser.add_argument('--numerical-client', type=pathlib.Path, required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    here = pathlib.Path(__file__).resolve().parent
    selected = json.loads((here / 'source_seals.json').read_text())[args.cohort]
    for name, expected in selected['files'].items():
        if hashlib.sha256((args.sdk_root / name).read_bytes()).hexdigest() != expected:
            raise RuntimeError(f'SDK Python source mismatch: {name}')
    if hashlib.sha256(args.numerical_client.read_bytes()).hexdigest() != E7C:
        raise RuntimeError('original e7c numerical body mismatch')
    native_names = ('_ext.abi3.so', '_varena_torch.so', '.native/libvarena.so')
    native = {name: hashlib.sha256((args.sdk_root / name).read_bytes()).hexdigest() for name in native_names}
    args.output.mkdir(parents=True, exist_ok=False)
    for name in FILES:
        shutil.copyfile(here / name, args.output / name)
    shutil.copyfile(args.numerical_client, args.output / 'original_sdxl.py')
    hashes = {name: hashlib.sha256((args.output / name).read_bytes()).hexdigest() for name in (*FILES, 'original_sdxl.py')}
    binding = dict(format='varena-host-stage-binding-v1', cohort=args.cohort,
                   product_revision=selected['revision'], torch_git=selected['torch_git'],
                   native_files=native, diagnostic_files=hashes,
                   numerical_client_sha256=E7C, cap_bytes=6 * 1024**3,
                   numerical_script_invocations=1, pipeline_calls_in_original_body=2,
                   synchronization_added=False,
                   allocator_tracer_added=False, gpu_execution='UNRUN')
    (args.output / 'binding.json').write_text(json.dumps(binding, indent=2) + '\n')
    print(json.dumps(binding, indent=2))


if __name__ == '__main__':
    main()
