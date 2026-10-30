"""Suite `cli` (./cells.sh): each GPU-only `run` flag did what it says, in its run's record.
    check(gate) -> (fails, notes)"""
import os

GIB = 1 << 30


def reserved_mib(run):
    """Torch's reservations plus native backing at their fullest (`memory.peak_reserved_bytes`), in MiB."""
    return round(max((m.get('memory') or {}).get('peak_reserved_bytes') or 0 for m in run.measurements) / 2**20)


def check(gate):
    fails, notes, runs, summary = [], [], gate['runs'], gate['summary']
    if not any(n.startswith('cli-') for n in summary) and not os.path.exists(os.path.join(gate['out'], 'cli-fails.txt')):
        return fails, notes
    if 'cli-gpus' in runs:
        gpus = runs['cli-gpus'].gpus
        (fails if len(gpus) > 1 else notes).append(f'cli-gpus: --gpus 1 ran on GPU(s) {gpus or [0]}')
    if 'cli-lane16' in runs:
        run = runs['cli-lane16']
        lanes = {m.get('lane') for m in run.models}
        int8 = [g for g in run.gemm if g[0].startswith(('int8', 'fp8', 'nvfp4'))]
        (fails if 'bf16' not in lanes or int8 else notes).append(
            f'cli-lane16: --lane bf16 served {sorted(p for p in lanes if p)} and 8/4-bit GEMM {int8}')
    if 'cli-attn' in runs:
        attn = runs['cli-attn'].attention
        (notes if attn == ['sdpa'] else fails).append(f'cli-attn: --attention-kernel sdpa served {attn}')
    for cell in ('cli-lora', 'cli-lora-input'):
        if cell in runs:
            rows = [(m.get('repository'), a) for m in runs[cell].models for a in m.get('adapters') or []]
            named = [f"{base} + {a.get('model')},{a.get('strength')} on {a.get('components')}" for base, a in rows]
            # The base by its model name under whichever org holds it (`_/sdxl`, since xs-133 `stabilityai/sdxl`).
            ok = any((base or '').split('@')[0].rsplit('/', 1)[-1] == 'sdxl' and 'lcm-lora-sdxl' in (a.get('model') or '')
                     and a.get('components') for base, a in rows)
            (notes if ok else fails).append(f'{cell}: the record names {named or "no LoRA"}')
    vram = 'cli-cap' in runs and runs['cli-cap'].vram  # none read fails in analyze.py
    if vram:
        (fails if vram['peak'] * (1 << 20) > 12 * GIB else notes).append(
            f'cli-cap: {vram["peak"]} MiB in use at its fullest under the 12 GiB --gpu-memory-limit')
    # 6 GiB binds SDXL (about 9 GiB uncapped): its driver peak stays within the cap plus what the driver charges beyond
    # Torch's reservations and native backing (the CUDA context), measured on this pod's uncapped cli-gpus.
    capped, open_ = runs.get('cli-cap6'), runs.get('cli-gpus')
    if capped and capped.vram and open_ and open_.vram:
        reserve = max(open_.vram['peak'] - reserved_mib(open_), 0)
        bar = 6 * 1024 + reserve
        (fails if capped.vram['peak'] > bar else notes).append(
            f'cli-cap6: {capped.vram["peak"]} MiB in use at its fullest under the 6 GiB --gpu-memory-limit '
            f'(bar {bar} MiB: 6144 + {reserve} the driver charges beyond Torch and native backing uncapped; '
            f'uncapped {open_.vram["peak"]} MiB)')
    p = os.path.join(gate['out'], 'cli-fails.txt')
    if os.path.exists(p):
        fails += [f'cli: {l.strip()}' for l in open(p) if l.strip()]
    return fails, notes
