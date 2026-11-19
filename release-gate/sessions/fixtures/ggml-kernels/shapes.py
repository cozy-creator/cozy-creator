"""ggml-kernels' cases (xs-213), shared by both sides: Qwen3.5-9B's and Qwen3.8-27B's linears (cozy_runtime's GEMM bench
`SHAPES`, models `q35-9b-m<M>` and `q38-27b-m<M>`) in each block type, at M rows.

    shapes.py tests   one test-backend-ops `--test-file` line per case (MUL_MAT, f32 dst, src0 the blocks, src1 f32)
    shapes.py models  the bench's model ids, comma-separated
"""

import sys

MODELS = {
    "q35-9b": (("qkv", 8192, 4096), ("out", 4096, 4096), ("up", 12288, 4096), ("down", 4096, 12288), ("head", 248320, 4096)),
    "q38-27b": (("qkv", 10240, 5120), ("out", 5120, 6144), ("up", 17408, 5120), ("down", 5120, 17408), ("head", 248320, 5120)),
}
ROWS = (1, 8, 32, 512)
#: ggml type: (id, elements per block, bytes per block), as ggml.h and ggml-common.h have them
TYPES = {"q8_0": (8, 32, 34), "q4_k": (12, 256, 144), "q5_k": (13, 256, 176), "q6_k": (14, 256, 210)}
MUL_MAT = 28  # GGML_OP_MUL_MAT at b12466


def cases():
    for model, sites in MODELS.items():
        for m in ROWS:
            for site, n, k in sites:
                for name, (tid, block, size) in TYPES.items():
                    yield f"{model}-m{m}-{site}-{name}", f"{model}-m{m}", site, m, n, k, name, tid, block, size


if __name__ == "__main__":
    if sys.argv[1] == "tests":
        for case, _, _, m, n, k, _, tid, block, size in cases():
            row = k // block * size
            src0 = f"{tid} {k} {n} 1 1 {size} {row} {row * n} {row * n}"
            src1 = f"0 {k} {m} 1 1 4 {4 * k} {4 * k * m} {4 * k * m}"
            print(f"{MUL_MAT} 0 {n} {m} 1 1 0 2 {src0} {src1} {case}")
    else:
        print(",".join(f"{model}-m{m}" for model in MODELS for m in ROWS))
