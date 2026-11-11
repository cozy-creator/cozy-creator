"""Trusted SDK qualification outside any published Runtime author package."""
from __future__ import annotations
import argparse
from array import array
import csv
import gc
import hashlib
from importlib.metadata import version
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from typing import TYPE_CHECKING
from zipfile import ZipFile

if TYPE_CHECKING:
    from hardware_types import PayloadFingerprint, ProductResult, SDKReceipt


def wheel_payload(wheel: ZipFile, loaded: str | Path) -> PayloadFingerprint:
    """Compare actual loaded bytes against their unique member in the exact wheel ZIP."""
    path = Path(loaded)
    suffix = 'tensorfs/' + path.name
    members = [name for name in wheel.namelist() if name == suffix or name.endswith('/' + suffix)]
    if len(members) != 1:
        raise ValueError(f'expected one wheel member for {path.name}, found {members}')
    expected = hashlib.sha256(wheel.read(members[0])).hexdigest()
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != expected:
        raise ValueError(f'loaded {path.name} does not match the candidate wheel: {actual} != {expected}')
    return {'path': str(path), 'sha256': actual, 'wheel_entry': members[0], 'wheel_sha256': expected}


class Options(argparse.Namespace):
    wheel: Path
    expected_sha: str
    out: Path
    gpu_uuid: str


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--wheel', type=Path, required=True)
    parser.add_argument('--expected-sha', required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--gpu-uuid', required=True)
    args = parser.parse_args(namespace=Options())
    if args.out.exists() and any(args.out.iterdir()):
        raise ValueError('operator SDK proof needs an empty owned output directory')
    args.out.mkdir(parents=True, exist_ok=True)
    receipt: SDKReceipt = {'status': 'running', 'python': sys.executable, 'wheel': str(args.wheel),
               'wheel_sha256': hashlib.sha256(args.wheel.read_bytes()).hexdigest()}
    try:
        if receipt['wheel_sha256'] != args.expected_sha:
            raise ValueError('operator SDK wheel differs from the CLI update journal')
        import tensorfs
        from tensorfs import _ext
        from tensorfs.varena import torch as vt
        import torch
        from torch.utils.dlpack import from_dlpack

        receipt.update({'sdk_version': tensorfs.__version__, 'runtime_version': version('cozy-runtime'),
                        'torch_version': str(torch.__version__)})
        shim = Path(tensorfs.__file__).parent / f'_varena_torch-{torch.__version__}.so'
        with ZipFile(args.wheel) as wheel:
            receipt['native_payloads'] = {'extension': wheel_payload(wheel, _ext.__file__),
                                          'torch_shim': wheel_payload(wheel, shim)}
        source = args.out / 'source-store'
        artifact = args.out / 'model.cozytensors'
        with tensorfs.Writer(artifact, cache_dir=source) as writer:
            component = writer.component('model', cls=('fixture', '1', 'Linear'),
                                         config={'in_features': 2, 'out_features': 2})
            component.put('weight', array('f', [1, 2, 3, 4]), dtype='F32', shape=[2, 2])
        saved = writer.commit()
        receipt['revision'] = saved.id
        assert artifact.stat().st_size > 8192
        saved.close()
        del saved, component, writer
        gc.collect()
        shutil.rmtree(source)

        def products(device: str, cache: Path) -> ProductResult:
            checkpoint = tensorfs.open(artifact, cache_dir=cache, local_files_only=True)
            try:
                assert checkpoint.id == receipt['revision'] and checkpoint.missing() == []
                native = checkpoint.components['model'].tensors.read('weight', device=device, decode=True)
                weight = from_dlpack(native)
                assert str(weight.device) == device and tuple(weight.shape) == (2, 2)
                values: list[float] = (weight @ torch.tensor([2., 3.], dtype=torch.float32, device=device)).cpu().tolist()
                assert values == [8., 18.], values
                return {'device': str(weight.device), 'products': values}
            finally:
                checkpoint.close()

        receipt['cpu'] = products('cpu', args.out / 'fresh-cpu-store')
        # This is the documented standalone SDK activation, outside a Runtime executor.
        vt.install(device=0)
        receipt['gpu'] = products('cuda:0', args.out / 'fresh-gpu-store')
        rows = subprocess.check_output(['nvidia-smi', '--query-compute-apps=pid,gpu_uuid', '--format=csv,noheader'], text=True)
        active = [row[1].strip() for row in csv.reader(rows.splitlines()) if row and row[0].strip() == str(os.getpid())]
        assert active == [args.gpu_uuid], active
        receipt['gpu_uuid'] = active[0]
        receipt['status'] = 'passed'
        (args.out / 'verified.json').write_text(json.dumps(receipt, indent=2) + '\n')
        print('PASS separate operator SDK CPU/CUDA/DLPack and exact wheel native payloads')
    except Exception as error:
        receipt['error'] = str(error)
        raise
    finally:
        (args.out / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')


if __name__ == '__main__':
    main()
