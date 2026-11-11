"""Small physical-device proof; no models, source credentials or provider calls."""
import msgspec
from cozy_runtime.author import App

app = App()


class Input(msgspec.Struct, forbid_unknown_fields=True):
    value: int


class Output(msgspec.Struct):
    result: int
    device: str


@app.entrypoint(accelerator=False)
def cpu(payload: Input) -> Output:
    return Output(payload.value * payload.value + 7, 'cpu')


@app.entrypoint(accelerator=True)
def gpu(payload: Input) -> Output:
    import torch
    value = torch.tensor([payload.value], dtype=torch.int64, device='cuda')
    result = value.square().add(7)
    return Output(int(result.cpu().item()), str(result.device))
