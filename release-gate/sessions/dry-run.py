"""var-036 prototype (proof only, never merged): H3 15 s Turbo's DiT forward on fake tensors.

The package's own block code (minimax_h3.glue, adaln_pruned) and the Runtime's own int8 leaves,
row projections, `split` and Kitchen attention adapter run under torch's FakeTensorMode on a fake
`cuda:0`. Only launchers are replaced: the Triton launcher and the prebuilt 8-bit library's C calls
do nothing (both allocate their outputs in Python first), and comfy_kitchen's int8 attention is its
Python body's allocations with the C call left out. `split` is a dry-run memory (`Dry`) that takes
the levels a recorded run took. A dispatch mode counts every storage's bytes from birth to death.

usage: python dry-run.py <minimax-h3 source dir> <out dir>
"""

from __future__ import annotations

import collections
import contextlib
import json
import math
import os
import platform
import sys
import time
import traceback
import types
import weakref

import torch
from torch._subclasses.fake_tensor import FakeTensor, FakeTensorMode
from torch.utils._python_dispatch import TorchDispatchMode
from torch.utils._pytree import tree_leaves

GiB = float(1 << 30)
PAGE = 2 << 20
DEV = torch.device("cuda", 0)
T_VIDEO, T_AUDIO, T_TEXT = 107856, 1206, 58
T = T_VIDEO + T_AUDIO + T_TEXT  # 109120: the record's _Heads(1, 109120, -1, 128)
SOURCE, OUT = sys.argv[1], sys.argv[2]
os.makedirs(OUT, exist_ok=True)
sys.path.insert(0, SOURCE)
RESULT: dict[str, object] = {"torch": torch.__version__, "python": platform.python_version(), "failures": []}


def fail(stage: str, exc: BaseException) -> None:
    RESULT["failures"].append({"stage": stage, "error": f"{type(exc).__name__}: {exc}"[:2000],
                               "trace": traceback.format_exc()[-6000:]})
    print(f"FAILED {stage}: {type(exc).__name__}: {exc}", flush=True)


# ------------------------------------------------- what a CPU-only torch lacks (a GPU machine has it)
torch.cuda.is_current_stream_capturing = lambda: False  # type: ignore[assignment]
torch.cuda.is_initialized = lambda: False  # type: ignore[assignment]
torch.cuda.get_device_capability = lambda device=None: (8, 9)  # type: ignore[assignment]
torch.cuda.current_device = lambda: 0  # type: ignore[assignment]
if not hasattr(torch._C, "_cuda_getCurrentRawStream"):
    torch._C._cuda_getCurrentRawStream = lambda index: 0  # type: ignore[attr-defined]

# A fake tensor has no address; launchers check alignment and compare outputs by pointer.
_BASES: dict[int, int] = {}
_NEXT = [1 << 40]


def _fake_ptr(self: torch.Tensor) -> int:
    st = self.untyped_storage()
    base = _BASES.get(st._cdata)
    if base is None:
        base = _BASES[st._cdata] = _NEXT[0]
        _NEXT[0] += -(-max(st.nbytes(), 1) // 4096) * 4096 + 4096
    return base + self.storage_offset() * self.element_size()


FakeTensor.data_ptr = _fake_ptr  # type: ignore[method-assign]


# ------------------------------------------------------------------------------- the tracker
class Site:
    __slots__ = ("name", "level", "entry", "peak", "est", "host")

    def __init__(self, name: str, level: int, entry: int, est: int, host: bool) -> None:
        self.name, self.level, self.entry, self.peak, self.est, self.host = name, level, entry, entry, est, host


class Tracker(TorchDispatchMode):
    """Device bytes live by storage, from its first appearance as an op's output to its death."""

    def __init__(self) -> None:
        super().__init__()
        self.live = self.peak = self.ops = self.allocs = 0
        self.sizes: dict[int, tuple[int, bool]] = {}
        self.stack: list[Site] = []
        self.done: list[dict[str, object]] = []
        self.events: list[tuple[object, ...]] | None = None
        self.hosting = False

    def __torch_dispatch__(self, func, types_, args=(), kwargs=None):  # type: ignore[no-untyped-def]
        out = func(*args, **(kwargs or {}))
        self.ops += 1
        for t in tree_leaves(out):
            if isinstance(t, torch.Tensor) and t.device.type == "cuda":
                self._see(t, func)
        return out

    def exclude(self, module: torch.nn.Module) -> int:
        """A module's weights: varena's regions, not working memory; their views count nothing."""
        n = 0
        for t in (*module.parameters(), *module.buffers()):
            st = t.untyped_storage()
            if st._cdata not in self.sizes:
                self.sizes[st._cdata] = (int(st.nbytes()), True)
                weakref.finalize(st, self._free, st._cdata)
                n += int(st.nbytes())
        return n

    def _see(self, t: torch.Tensor, func: object) -> None:
        st = t.untyped_storage()
        key = st._cdata
        if key in self.sizes:
            return
        n = int(st.nbytes())
        host = self.hosting
        self.sizes[key] = (n, host)
        weakref.finalize(st, self._free, key)
        if "inference" not in RESULT:
            RESULT["inference"] = bool(t.is_inference())
        self.allocs += 1
        if not host:
            self.live += n
            if self.live > self.peak:
                self.peak = self.live
            for s in self.stack:
                if self.live > s.peak:
                    s.peak = self.live
        if self.events is not None and n >= (1 << 20):
            self.events.append(("+", n, self.live, str(func).replace("aten.", ""), tuple(t.shape), str(t.dtype).replace("torch.", ""),
                                host, self.stack[-1].name if self.stack else "", _where()))

    def _free(self, key: int) -> None:
        n, host = self.sizes.pop(key)
        if not host:
            self.live -= n
        if self.events is not None and n >= (1 << 20):
            self.events.append(("-", n, self.live, "", (), "", host, self.stack[-1].name if self.stack else "", ""))

    @contextlib.contextmanager
    def site(self, name: str, level: int = -1, est: int = 0, host: bool = False):  # type: ignore[no-untyped-def]
        s = Site(name, level, self.live, est, host)
        self.stack.append(s)
        try:
            yield s
        finally:
            self.stack.pop()
            self.done.append({"site": name, "level": level, "entry": s.entry, "peak_above": s.peak - s.entry, "est": est, "host": host})

    @contextlib.contextmanager
    def host(self):  # type: ignore[no-untyped-def]
        before, self.hosting = self.hosting, True
        try:
            yield
        finally:
            self.hosting = before


def _where() -> str:
    f = sys._getframe(3)
    while f is not None:
        name = f.f_code.co_filename
        if "/torch/" not in name and not name.endswith("dry-run.py"):
            parts = name.split("/")
            short = "/".join(parts[-2:])
            return f"{short}:{f.f_lineno} {f.f_code.co_name}"
        f = f.f_back
    return ""


TRACK = Tracker()


# -------------------------------------------------------------- the dry-run memory (`split`)
def pieces(units: int, level: int) -> int:
    return -(-units // (1 << level))


def levels(units: int) -> int:
    return max(units - 1, 0).bit_length()


def paged(n: int) -> int:
    return -(-n // PAGE) * PAGE


class Dry:
    """`author._split.Pieces` as `internal.execution.cuts` is, its level from `policy` (site name ->
    level) instead of varena's selection; a whole goes to host where `hosted` names its site."""

    def __init__(self) -> None:
        self.policy: dict[str, int] = {}
        self.hosted: set[str] = set()
        self.wholes: dict[str, tuple[tuple[int, ...], torch.dtype]] = {}
        self.calls: collections.Counter[str] = collections.Counter()

    def _level(self, name: str, deepest: int) -> int:
        for part, level in self.policy.items():
            if part in name:
                return min(level, deepest)
        return 0

    def _host(self, name: str) -> bool:
        return any(part in name for part in self.hosted)

    def split(self, name, key, total, finest, out_bytes, item_bytes, holds, run, pure=False, hostable=False, exact=""):  # type: ignore[no-untyped-def]
        deepest = levels(-(-total // max(finest, 1)))
        level = self._level(name + key, deepest)
        window = max(pieces(total, level), finest)
        host = hostable and self._host(name + key)
        whole = 0 if host else out_bytes
        exact = exact or key
        if exact in self.wholes and not host:
            shape, dtype = self.wholes[exact]
            whole = math.prod(shape) * dtype.itemsize
        est = paged(whole) + holds * paged(int(window * item_bytes)) if whole else holds * paged(int(window * item_bytes))
        self.calls[f"{name} L{level}{' host' if host else ''}"] += 1
        with TRACK.site(f"split:{name}", level, est, host):
            return run(window, host)

    def piece(self) -> None:
        pass

    def whole(self, part, shape, key, host, zeros=False):  # type: ignore[no-untyped-def]
        self.wholes[key] = (tuple(int(n) for n in shape), part.dtype)
        with TRACK.host() if host else contextlib.nullcontext():
            return part.new_zeros(shape) if zeros else part.new_empty(shape)

    def ahead(self, key, host, zeros):  # type: ignore[no-untyped-def]
        known = self.wholes.get(key)
        if known is None:
            return None
        shape, dtype = known
        with TRACK.host() if host else contextlib.nullcontext():
            make = torch.zeros if zeros else torch.empty
            return make(shape, dtype=dtype, device=DEV)

    def moved(self, out):  # type: ignore[no-untyped-def]
        return out

    def narrowed(self, whole, dtype):  # type: ignore[no-untyped-def]
        return whole.to(dtype)

    def write(self, dst, src):  # type: ignore[no-untyped-def]
        dst.copy_(src)

    def read(self, window):  # type: ignore[no-untyped-def]
        # `pinned.read`: a window of a host whole is copied to the device.
        if isinstance(window, torch.Tensor) and TRACK.sizes.get(window.untyped_storage()._cdata, (0, False))[1]:
            out = torch.empty(window.shape, dtype=window.dtype, device=window.device)
            out.copy_(window)
            return out
        return window

    def windows(self, name, key, needs):  # type: ignore[no-untyped-def]
        return 0, contextlib.nullcontext


DRY = Dry()

# ------------------------------------------------------------------------ the launchers
LAUNCHES: collections.Counter[str] = collections.Counter()


class StubGemm:
    """cozy_runtime._kernels._gemm with every launch a no-op returning success."""

    NOT_SUPPORTED, OUT_OF_MEMORY, GEMV_ROWS, MAX_K, TMA = -1, -2, 4, 1 << 16, False

    @staticmethod
    def error_name(code: int) -> str:
        return str(code)

    def __getattr__(self, name: str):  # type: ignore[no-untyped-def]
        def launch(*_: object) -> int:
            LAUNCHES[f"gemm.{name}"] += 1
            return 0
        return launch


def noop_launch(kernel: object, grid: tuple[int, ...], *args: object) -> None:
    LAUNCHES[f"triton.{getattr(kernel, '__name__', kernel)}"] += 1


KITCHEN_MODE = ["mirror"]


def kitchen_int8_attention(q, k, v, *, scale=None, attn_mask=None):  # type: ignore[no-untyped-def]
    """comfy_kitchen.sage_attention._int8_attention_cuda's allocations, its C call left out
    (`mirror`); with `fake`, its register_fake: the output alone."""
    LAUNCHES["kitchen.int8_attention"] += 1
    if KITCHEN_MODE[0] == "fake":
        return q.new_empty(q.shape)
    batch, heads, length, dim = q.shape
    _, kv_heads, kv_length, _ = k.shape
    kd = 64 if dim <= 64 else 128 if dim <= 128 else 256
    cta = 128 if attn_mask is None and kd >= 128 and kv_length > 1024 else 64
    padded = -(-kv_length // cta) * cta
    keep = [
        torch.empty(q.shape, dtype=torch.int8, device=q.device),
        torch.empty(k.shape, dtype=torch.int8, device=k.device),
        torch.empty(batch, heads, ((length + 127) // 128) * (64 if kd == 256 else 32), dtype=torch.float32, device=q.device),
        torch.empty(batch, kv_heads, ((kv_length + cta - 1) // cta) * 4, dtype=torch.float32, device=q.device),
        torch.empty(batch * kv_heads * kd, padded, dtype=torch.int8, device=q.device),
        torch.empty(batch * kv_heads * kd, dtype=torch.float32, device=q.device),
    ]
    output = torch.empty(batch, heads, length, kd, dtype=q.dtype, device=q.device)
    keep.append(torch.empty(batch, kv_heads, dtype=torch.int32, device=q.device))
    del keep
    return output[..., :dim]


# ---------------------------------------------------------------------------- the build
def build():  # type: ignore[no-untyped-def]
    import cozy_runtime._kernels as kernels_pkg
    kernels_pkg._gemm = StubGemm()  # type: ignore[attr-defined]
    from cozy_runtime.accel.kernels import INT8_CONVROT
    from cozy_runtime.author import _split
    from cozy_runtime.internal import int8_linear, prebuilt
    from cozy_runtime.internal.encoding.leaves import _native_leaf_class

    lib = prebuilt.Prebuilt(torch, 0, "mma.sync", False)
    int8_linear._LIBS[0] = lib
    int8_linear._OPS[0] = lib.int8_linear
    int8_linear._HALF[:] = (torch.bfloat16, torch.float16)
    _split.install(DRY)

    kitchen = types.ModuleType("comfy_kitchen")
    kitchen.int8_attention = kitchen_int8_attention  # type: ignore[attr-defined]
    sys.modules.setdefault("comfy_kitchen", kitchen)
    from cozy_runtime.internal import attention_upstream

    def dispatch(query, key, value, attn_mask=None, dropout_p=0.0, is_causal=False, backend=None, parallel_config=None, **_):  # type: ignore[no-untyped-def]
        return attention_upstream.kitchen_int8(query, key, value, attn_mask=attn_mask, dropout_p=dropout_p, is_causal=is_causal)

    import diffusers.models.transformers.transformer_minimax_h3 as upstream
    from minimax_h3 import adaln_pruned, glue

    upstream.dispatch_attention_fn = dispatch
    glue.dispatch_attention_fn = dispatch
    for name in ("_norm_mod_kernel", "_gate_add_kernel", "_swiglu_kernel", "_rope_norm_kernel", "_key_sum_kernel"):
        if not hasattr(glue, name):  # no Triton on this runner: the launcher only needs a name
            setattr(glue, name, types.SimpleNamespace(__name__=name))

    leaf = _native_leaf_class(torch)

    def int8(linear: torch.nn.Linear) -> torch.nn.Module:
        data = torch.empty(linear.out_features, linear.in_features, dtype=torch.int8, device=DEV)
        scale = torch.empty(linear.out_features, dtype=torch.float32, device=DEV)
        return leaf(None, {"weight": data, "scale": scale}, linear.bias, torch.bfloat16, INT8_CONVROT, "int8.convrot",
                    linear.in_features, linear.out_features)

    config = json.load(open(os.path.join(SOURCE, "minimax_h3/preparation/assets/model-config.json")))["transformer"]
    config = {k: (tuple(v) if isinstance(v, list) else v) for k, v in config.items() if not k.startswith("_")}
    started = time.perf_counter()
    # A served model is built with its weights and never initialized; here nothing draws random numbers (a fake CUDA
    # generator draws through a real one, which a CPU-only torch lacks).
    init = torch.nn.init
    for name in ("uniform_", "normal_", "kaiming_uniform_", "kaiming_normal_", "xavier_uniform_", "xavier_normal_", "trunc_normal_"):
        setattr(init, name, lambda tensor, *a, **k: tensor)
    torch.Tensor.uniform_ = lambda self, *a, **k: self  # type: ignore[method-assign]
    torch.Tensor.normal_ = lambda self, *a, **k: self  # type: ignore[method-assign]
    with DEV:
        model = adaln_pruned.H3Transformer(**config).to(torch.bfloat16).eval()
        for block in model.transformer_blocks:
            a, ff = block.attn, block.ff
            a.to_q, a.to_k, a.to_v = int8(a.to_q), int8(a.to_k), int8(a.to_v)
            a.to_out[0] = int8(a.to_out[0])
            ff.net[0].proj = int8(ff.net[0].proj)
            ff.net[2] = int8(ff.net[2])
            glue._release_qkv(a)
    glue._install_transformer(model, "transformer", noop_launch)
    RESULT["build_s"] = round(time.perf_counter() - started, 2)
    RESULT["blocks"] = len(model.transformer_blocks)
    RESULT["block_classes"] = sorted({type(b).__name__ for b in model.transformer_blocks})
    return model, adaln_pruned


def inputs():  # type: ignore[no-untyped-def]
    return dict(
        hidden_states=torch.empty(1, T_VIDEO, 96, dtype=torch.float32, device=DEV),
        audio_hidden_states=torch.empty(1, T_AUDIO, 32, dtype=torch.float32, device=DEV),
        encoder_hidden_states=torch.empty(1, T_TEXT, 5120, dtype=torch.bfloat16, device=DEV),
        timestep=torch.empty(2, dtype=torch.float32, device=DEV),
        timestep_indices=torch.zeros(T, dtype=torch.int64, device=DEV),
        token_tags=torch.zeros(T, dtype=torch.int64, device=DEV),
        position_ids=torch.zeros(T, 3, dtype=torch.int64, device=DEV),
        video_indices=torch.arange(T_VIDEO, device=DEV),
        audio_indices=torch.arange(T_AUDIO, device=DEV),
        text_indices=torch.arange(T_TEXT, device=DEV),
        return_dict=False,
    )


# ---------------------------------------------------------------------------- the passes
BLOCK_ARGS: list[object] = []


def hooks(model) -> None:  # type: ignore[no-untyped-def]
    for i, block in enumerate(model.transformer_blocks):
        def pre(module, args, i=i):  # type: ignore[no-untyped-def]
            if i == 0 and not BLOCK_ARGS:
                BLOCK_ARGS.extend(args)
            stack = contextlib.ExitStack()
            stack.enter_context(TRACK.site(f"block{i}"))
            module._dry_stack = stack
        def post(module, args, out):  # type: ignore[no-untyped-def]
            module._dry_stack.close()
        block.register_forward_pre_hook(pre)
        block.register_forward_hook(post)


def blocks_of(done: list[dict[str, object]]) -> list[dict[str, object]]:
    return [d for d in done if str(d["site"]).startswith("block")]


def sites_of(done: list[dict[str, object]]) -> dict[str, dict[str, object]]:
    by: dict[str, dict[str, object]] = {}
    for d in done:
        if str(d["site"]).startswith("split:"):
            k = f"{d['site']} L{d['level']}{' host' if d['host'] else ''}"
            row = by.setdefault(k, {"calls": 0, "peak_above_gib": 0.0, "est_gib": 0.0})
            row["calls"] += 1
            row["peak_above_gib"] = max(row["peak_above_gib"], round(d["peak_above"] / GiB, 4))
            row["est_gib"] = max(row["est_gib"], round(d["est"] / GiB, 4))
    return by


def forward(model, adaln_pruned, label: str):  # type: ignore[no-untyped-def]
    TRACK.done.clear()
    args = inputs()
    entry = TRACK.live
    TRACK.peak = entry
    ops = TRACK.ops
    started = time.perf_counter()
    with adaln_pruned.checked_loop():
        out = model(**args)
    seconds = time.perf_counter() - started
    blocks = blocks_of(TRACK.done)
    row = {
        "label": label,
        "seconds": round(seconds, 2),
        "ops": TRACK.ops - ops,
        "forward_peak_above_entry_gib": round((TRACK.peak - entry) / GiB, 4),
        "block0_live_at_entry_gib": round(blocks[0]["entry"] / GiB, 4) if blocks else None,
        "block0_peak_above_entry_gib": round(blocks[0]["peak_above"] / GiB, 4) if blocks else None,
        "blocks_peak_above_entry_gib": round(max(b["peak_above"] for b in blocks) / GiB, 4) if blocks else None,
        "blocks_live_at_entry_gib": sorted({round(b["entry"] / GiB, 3) for b in blocks}),
        "sites": sites_of(TRACK.done),
        "out_shapes": [list(t.shape) for t in out],
    }
    del out, args
    print(json.dumps({k: v for k, v in row.items() if k != "sites"}), flush=True)
    return row


def block_once(block, policy, hosted, label, prime=True, record=False):  # type: ignore[no-untyped-def]
    """Block 0 alone on the forward's own arguments: its peak above entry at `policy`."""
    DRY.policy, DRY.hosted = dict(policy), set(hosted)
    if prime:  # a whole is allocated ahead once an earlier call measured it (cuts.ahead)
        with adaln_ctx():
            block(*BLOCK_ARGS)
    elif not prime:
        DRY.wholes.clear()
    TRACK.done.clear()
    TRACK.events = [] if record else None
    started = time.perf_counter()
    out = block(*BLOCK_ARGS)
    seconds = time.perf_counter() - started
    events, TRACK.events = TRACK.events, None
    del out
    blocks = blocks_of(TRACK.done)
    row = {"label": label, "policy": policy, "hosted": sorted(hosted), "seconds": round(seconds, 3),
           "peak_above_entry_gib": round(blocks[-1]["peak_above"] / GiB, 4), "live_at_entry_gib": round(blocks[-1]["entry"] / GiB, 4),
           "sites": sites_of(TRACK.done)}
    return row, events


def adaln_ctx():  # type: ignore[no-untyped-def]
    return contextlib.nullcontext()


def main() -> None:
    fake = FakeTensorMode(allow_non_fake_inputs=True)
    measured = {
        "c8_step2_block0": {"policy": "heads L3, out-proj L2, tail L3, wholes on the card", "live_at_entry": 1.404, "peak_above_entry": 4.281},
        "c8_step1_block0": {"policy": "heads L4, out-proj L3, tail L3, first call", "live_at_entry": 1.404, "peak_above_entry": 3.604},
        "c6_step2_block0": {"policy": "heads L4 with A on host, out-proj L2, tail L4", "live_at_entry": 1.404, "peak_above_entry": 2.217},
        "c8_step_peak": 5.685, "c8_live_at_step_start": 0.320, "c6_steps3_4_peak": 3.712,
        "source": "pod-session 44393475788 records c8-a.json / c6-a.json (first_blocks, var030.steps, working_splits)",
    }
    RESULT["measured"] = measured
    with fake:
        try:
            with torch.no_grad():
                model, adaln_pruned = build()
        except Exception as exc:
            fail("build", exc)
            return
        with torch.inference_mode(), TRACK:
            RESULT["weights_gib"] = round(TRACK.exclude(model) / GiB, 3)
            hooks(model)
            run_passes(model, adaln_pruned)


def run_passes(model, adaln_pruned) -> None:  # type: ignore[no-untyped-def]
    if True:
        passes = []
        steady = {"glue._Heads": 3, "NativeLinear": 2, "_Tail.run": 3}
        try:
            DRY.policy, DRY.hosted = {"glue._Heads": 4, "NativeLinear": 3, "_Tail.run": 3}, set()
            DRY.wholes.clear()
            passes.append(forward(model, adaln_pruned, "8GiB step 1 levels (4,3,3), no whole measured yet"))
            DRY.policy = steady
            passes.append(forward(model, adaln_pruned, "8GiB steady levels (3,2,3), wholes ahead"))
            passes.append(forward(model, adaln_pruned, "8GiB steady levels (3,2,3), wholes ahead, again"))
        except Exception as exc:
            fail("forward", exc)
        RESULT["forwards"] = passes
        RESULT["launches_per_forward"] = {k: v // max(len(passes), 1) for k, v in LAUNCHES.items()}
        if not BLOCK_ARGS:
            return
        block = model.transformer_blocks[0]
        rows = []
        try:
            r, events = block_once(block, steady, (), "c8 step 2 block 0: (3,2,3) ahead", record=True)
            rows.append(r)
            with open(os.path.join(OUT, "timeline-block0-c8-steady.json"), "w") as f:
                json.dump(events, f)
            RESULT["timeline_top"] = [e for e in events if e[0] == "+" and e[1] >= (64 << 20)][:80]
            DRY.wholes.clear()
            r, _ = block_once(block, {"glue._Heads": 4, "NativeLinear": 3, "_Tail.run": 3}, (), "c8 step 1 block 0: (4,3,3) first call", prime=False)
            rows.append(r)
            r, _ = block_once(block, {"glue._Heads": 4, "NativeLinear": 2, "_Tail.run": 4}, ("glue._Heads",), "c6 step 2 block 0: heads L4 host, (2,4)")
            rows.append(r)
            r, _ = block_once(block, {"glue._Heads": 5, "NativeLinear": 2, "_Tail.run": 3}, ("glue._Heads",), "c6 steps 3-4: heads L5 host, (2,3)")
            rows.append(r)
            KITCHEN_MODE[0] = "fake"
            r, _ = block_once(block, steady, (), "c8 steady (3,2,3), Kitchen as its register_fake (output only)")
            rows.append(r)
            KITCHEN_MODE[0] = "mirror"
        except Exception as exc:
            fail("block", exc)
        RESULT["block_runs"] = rows
        sweep = []
        try:
            for site, top in (("glue._Heads", 6), ("NativeLinear", 4), ("_Tail.run", 6)):
                for level in range(top + 1):
                    policy = dict(steady)
                    policy[site] = level
                    r, _ = block_once(block, policy, (), f"sweep {site} L{level}")
                    sweep.append(r)
                    print(f"sweep {site} L{level}: peak above entry {r['peak_above_entry_gib']} GiB in {r['seconds']} s", flush=True)
        except Exception as exc:
            fail("sweep", exc)
        RESULT["sweep"] = sweep
    RESULT["tracker"] = {"ops": TRACK.ops, "allocations": TRACK.allocs}


if __name__ == "__main__":
    t0 = time.perf_counter()
    try:
        main()
    except Exception as exc:
        fail("main", exc)
    RESULT["total_s"] = round(time.perf_counter() - t0, 2)
    with open(os.path.join(OUT, "dry-run.json"), "w") as f:
        json.dump(RESULT, f, indent=1, default=str)
    print(json.dumps({k: RESULT.get(k) for k in ("torch", "build_s", "total_s", "failures")}, default=str)[:4000])
