"""Suite `cli` (./cells.sh): each GPU-only `run` flag did what it says, in its run's record.
    check(gate) -> (fails, notes)"""
import json
import os

GIB = 1 << 30


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
            text = json.dumps(runs[cell].record).lower()
            if 'lcm-lora-sdxl' not in text and 'adapter' not in text:
                fails.append(f'{cell}: the record names no LoRA (lcm-lora-sdxl) applied')
    vram = 'cli-cap' in runs and runs['cli-cap'].vram  # none read fails in analyze.py
    if vram:
        (fails if vram['peak'] * (1 << 20) > 12 * GIB else notes).append(
            f'cli-cap: {vram["peak"]} MiB in use at its fullest under the 12 GiB --gpu-memory-limit')
    p = os.path.join(gate['out'], 'cli-fails.txt')
    if os.path.exists(p):
        fails += [f'cli: {l.strip()}' for l in open(p) if l.strip()]
    return fails, notes
