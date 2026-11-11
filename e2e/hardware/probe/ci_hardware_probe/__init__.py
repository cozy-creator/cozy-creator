"""Small native TensorFS CPU/CUDA proof; no downloaded model or provider calls."""
from array import array
import hashlib
from pathlib import Path
import shutil
import tempfile

import msgspec
from cozy_runtime.author import App

app = App()


class Input(msgspec.Struct, forbid_unknown_fields=True):
    value: int


class Output(msgspec.Struct):
    result: int
    device: str
    tensorfs_products: list[float]
    tensorfs_revision: str
    tensorfs_native_sha256: str
    torch_shim_sha256: str


def probe(payload: Input, device: str) -> Output:
    import tensorfs
    from tensorfs import _ext
    import torch

    with tempfile.TemporaryDirectory(prefix='ci-hardware-sdk-') as directory:
        root = Path(directory)
        source, artifact = root / 'source', root / 'model.cozytensors'
        with tensorfs.Writer(artifact, cache_dir=source) as writer:
            component = writer.component('model', cls=('fixture', '1', 'Linear'),
                                         config={'in_features': 2, 'out_features': 2})
            component.put('weight', array('f', [1, 2, 3, 4]), dtype='F32', shape=[2, 2])
        saved = writer.commit()
        revision = saved.id
        saved.close()
        del saved, component, writer
        shutil.rmtree(source)
        checkpoint = tensorfs.open(artifact, cache_dir=root / 'fresh', local_files_only=True)
        try:
            assert checkpoint.id == revision and checkpoint.missing() == []
            native = checkpoint.components['model'].tensors.read('weight', device=device, decode=True)
            weight = torch.from_dlpack(native)
            assert str(weight.device) == device and tuple(weight.shape) == (2, 2)
            products = (weight @ torch.tensor([2., 3.], dtype=torch.float32, device=device)).cpu().tolist()
            assert products == [8., 18.], products
            value = torch.tensor([payload.value], dtype=torch.int64, device=device)
            result = int(value.square().add(7).cpu().item())
        finally:
            checkpoint.close()
    shim = Path(tensorfs.__file__).parent / f'_varena_torch-{torch.__version__}.so'
    return Output(result, device, products, revision,
                  hashlib.sha256(Path(_ext.__file__).read_bytes()).hexdigest(),
                  hashlib.sha256(shim.read_bytes()).hexdigest() if device.startswith('cuda:') else '')


@app.entrypoint(accelerator=False)
def cpu(payload: Input) -> Output:
    return probe(payload, 'cpu')


@app.entrypoint(accelerator=True)
def gpu(payload: Input) -> Output:
    return probe(payload, 'cuda:0')
