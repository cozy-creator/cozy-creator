"""vt.offload's promises on a real GPU, with real tiny Diffusers and Torch models."""

import copy
import gc
import json
import os

import tensorfs.varena.torch as vt
from tensorfs.varena import api

vt.install(0)

import torch
from diffusers import AutoencoderKL
from tensorfs.varena import _offload as offload_module
from torch import nn

offload, OffloadError = vt.offload, offload_module.OffloadError
device = torch.device("cuda", 0)
passed = []


def check(name):
    def wrap(function):
        function()
        passed.append(name)
        print(name, flush=True)

    return wrap


def refuses(action):
    try:
        action()
    except OffloadError:
        return True
    return False


class Tied(nn.Module):
    _no_split_modules = ["Block"]

    def __init__(self):
        super().__init__()
        self.a, self.b = nn.Linear(4, 4), nn.Linear(4, 4)
        self.b.weight = self.a.weight
        self.own = nn.Parameter(torch.ones(4))
        self.blocks = nn.ModuleList([Block(), Block()])

    def forward(self, x):
        x = self.a(x) + self.b(x) + self.own
        for block in self.blocks:
            x = block(x)
        return x


class Block(nn.Module):
    def __init__(self):
        super().__init__()
        self.layer = nn.Linear(4, 4)
        self.register_buffer("scale", torch.full((4,), 2.0))

    def forward(self, x):
        self.__dict__["escaped"] = self.layer.weight
        return self.layer(x) * self.scale


class Writer(nn.Module):
    def __init__(self):
        super().__init__()
        self.layer = nn.Linear(2, 2)

    def forward(self, x):
        self.layer.weight.add_(1.0)
        return self.layer(x)


x = torch.randn(1, 4)


@check("a block too big to pin descends to its children")
def split():
    template = nn.Sequential(*(nn.Sequential(nn.Linear(7168, 7168), nn.Linear(7168, 7168)) for _ in range(2))).eval()
    y = torch.randn(1, 7168)
    with torch.inference_mode():
        want = template(y).to(device)
        # The GEMM's first launch loads its kernels into the context, outside our bytes: before any room is measured
        # (a 5090 lost 68 MiB of a 256 MiB room to it, and a 2 MiB activation found none).
        warm = torch.zeros(7168, 7168, device=device)
        torch.nn.functional.linear(y.to(device), warm, torch.zeros(7168, device=device))
    del warm
    torch.cuda.synchronize()
    torch.cuda.empty_cache()
    gpu, wide = api.open(0), torch.cuda.mem_get_info(0)[1]
    # What's outside us moves by host, so no one cap is right. A layer is 196 MiB and a half 392: climbing in smaller
    # steps, the first room that runs is one a layer fits and a half doesn't. The room is what we may hold beyond what
    # can't go (live working bytes and pins): a refused room's regions and the warm-up's freed pages can, and would add
    # their bytes to it (a 64 MiB room bound the 392 MiB block whole while a warm-up's freed pages were still pending).
    # Another process giving memory back mid-run (the gate's last executor unloading) widens the room under us: a run
    # whose capacity grew says nothing, and that room goes again.
    room, moved = 64, 0
    while room < 1024:
        model = copy.deepcopy(template)
        handle = offload(model, device=0)
        torch.cuda.empty_cache()
        vt.set_limit(0, wide)
        held = gpu.room()
        vt.set_limit(0, wide - held["capacity"] + held["working_live"] + held["pinned"] + room * 2**20)
        before = gpu.room()["capacity"]
        try:
            with torch.inference_mode():
                got = model(y.to(device))
        except torch.OutOfMemoryError:
            got = None  # its traceback holds the modules until this block ends
        finally:
            after = gpu.room()["capacity"]
            vt.set_limit(0, None)
        grew = after - before > 32 << 20 and moved < 3
        if got is None or grew:
            moved += grew
            room += 0 if grew else 64
            del model, handle
            gc.collect()
            continue
        assert torch.allclose(got, want, rtol=1e-3, atol=1e-3)
        assert any(unit.split for unit in handle.units), (
            f"{room} MiB of room ran and nothing descended (capacity {before >> 20} MiB, {after >> 20} after)")
        return
    raise AssertionError("no room ran the model")


@check("a dropped handle keeps working, identities and ties preserved")
def dropped():
    model = Tied().eval()
    with torch.inference_mode():
        want = model(x).to(device)
    ids = [id(p) for p in model.parameters()]
    offload(model, device=0)
    gc.collect()
    assert [id(p) for p in model.parameters()] == ids and model.a.weight is model.b.weight
    with torch.inference_mode():
        got = model(x.to(device))
    assert torch.allclose(got, want, atol=1e-5)
    assert isinstance(model.a.weight, nn.Parameter) and model.a.weight.device == device


@check("direct reads, aliases and copies outside a block")
def direct():
    model = nn.Linear(4, 4).eval()
    want = model.weight.detach().clone().to(device)
    offload(model, device=0)
    weight = model.weight
    assert torch.equal(weight.clone(), want) and torch.equal(weight + 0, want)
    view = weight.view(-1)
    assert type(view) is type(weight) and torch.equal(view.clone(), want.view(-1))
    assert torch.equal(weight.t().clone(), want.t())
    copy = weight.clone()
    copy.add_(1)
    assert torch.equal(weight.clone(), want)
    assert len(model.state_dict()) == 2
    assert torch.equal(model.state_dict()["weight"].cpu(), want.cpu())
    for action in (lambda: weight.add_(1), lambda: weight.numpy(), lambda: weight.data_ptr(), lambda: weight.requires_grad_(True)):
        assert refuses(action)


@check("an alias that escaped its block stays readable through trim")
def escaped():
    model = Tied().eval()
    want = model.blocks[0].layer.weight.detach().clone().to(device)
    offload(model, device=0)
    with torch.inference_mode():
        model(x.to(device))
    alias = model.blocks[0].__dict__.pop("escaped")
    assert type(alias) is torch.Tensor
    torch.cuda.empty_cache()
    assert torch.equal(alias, want)
    assert isinstance(model.blocks[0].layer.weight, offload_module._Weight)


@check("the VAE's dtype conversions, identities kept, weights never written")
def conversion():
    torch.manual_seed(1)
    vae = AutoencoderKL(block_out_channels=(4, 8), down_block_types=("DownEncoderBlock2D",) * 2, up_block_types=("UpDecoderBlock2D",) * 2,
                        latent_channels=4, norm_num_groups=2, sample_size=16, force_upcast=True).to(torch.float16).eval()
    latents = torch.randn(1, 4, 8, 8)
    upcast = AutoencoderKL.from_config(vae.config).to(torch.float32).eval()
    upcast.load_state_dict(vae.state_dict())
    with torch.inference_mode():
        want = upcast.to(device).decode(latents.to(device)).sample
    upcast.to("cpu")
    ids = [id(p) for p in vae.parameters()]
    offload(vae, device=0)
    first = vae.dtype
    vae.to(dtype=torch.float32)
    assert vae.dtype == torch.float32
    with torch.inference_mode():
        got = vae.decode(latents.to(device)).sample
    assert torch.allclose(got, want, atol=1e-4)
    vae.to(dtype=first)
    assert vae.dtype == first and [id(p) for p in vae.parameters()] == ids
    vae.to(dtype=torch.float32)
    with torch.inference_mode():
        assert torch.allclose(vae.decode(latents.to(device)).sample, want, atol=1e-4)
    vae.half()
    vae.float()
    vae.to("cuda")
    assert refuses(lambda: vae.to("cpu")) and refuses(lambda: vae._apply(lambda t: t * 2))


@check("a write to a weight inside a block is caught")
def written():
    model = Writer().eval()
    offload(model, device=0)
    assert refuses(lambda: model(torch.ones(1, 2, device=device)))


@check("an explicit caller stream is honored; a default-stream thread gets one explicit stream")
def streams():
    model = nn.Linear(4, 4).eval()
    want = model(x).detach().to(device)
    offload(model, device=0)
    mine = torch.cuda.Stream(device=device)
    with torch.cuda.stream(mine), torch.inference_mode():
        assert torch.allclose(model(x.to(device)), want, atol=1e-5)
        assert torch.cuda.current_stream(device) == mine
    assert torch.cuda.current_stream(device).cuda_stream > 2


class Noisy(nn.Module):
    def __init__(self):
        super().__init__()
        self.blocks = nn.Sequential(nn.Linear(4, 4), nn.Linear(4, 4))

    def forward(self, x):
        return self.blocks(x) + torch.randn_like(x)


class Counting(Noisy):
    def forward(self, x):
        self.calls = getattr(self, "calls", 0) + 1
        return self.blocks(x)


@check("a loop replays and matches eager; effects a replay would lose stay eager")
def graphs():
    y = torch.randn(1, 4)
    plain = nn.Sequential(nn.Linear(4, 4), nn.Linear(4, 4)).eval()
    with torch.inference_mode():
        want = plain(y).to(device)
    handle = offload(plain, device=0)
    y = y.to(device)
    with torch.inference_mode():
        for _ in range(6):
            assert torch.allclose(plain(y), want, atol=1e-5)
    assert (handle.graphs.counts["captures"], handle.graphs.counts["replays"]) == (1, 4), handle.graphs.counts
    noisy = Noisy().eval()
    handle = offload(noisy, device=0)
    with torch.inference_mode():
        outputs = [noisy(y) for _ in range(6)]
    assert not torch.equal(outputs[-1], outputs[-2]) and handle.graphs.counts["captures"] == 0, handle.graphs.counts
    counting = Counting().eval()
    handle = offload(counting, device=0)
    with torch.inference_mode():
        for _ in range(6):
            counting(y)
    assert counting.calls == 6 and handle.graphs.counts["captures"] == 0, handle.graphs.counts


@check("a forked child gets a refusal, not a hang")
def forked():
    model = nn.Linear(4, 4).eval()
    offload(model, device=0)
    y = x.to(device)
    pid = os.fork()
    if pid == 0:
        code = 1
        try:
            model(y)
        except RuntimeError:
            code = 0
        os._exit(code)
    assert os.waitpid(pid, 0)[1] == 0


print(json.dumps({"report": {"passed": passed}}), flush=True)
