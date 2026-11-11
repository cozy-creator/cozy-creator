"""The Runtime's FP8 rowwise and fixed-codebook W4A8 producers (`cozy_runtime.derive.microscale`, public) against the frozen
independent references (`oracle.py`, as TensorFS's own program `suites/tensorfs/programs/offline_producers.py` holds its
native producers to them): the same payload, scales and codebook bytes, the same max error, and SSE and energy within
2e-13. The Runtime half of the former package `tensorfs/offline-producers` (xs-174), on the public API."""

import json

import msgspec
import numpy as np

from cozy_runtime.author import App, Context
from cozy_runtime.derive import microscale

from . import oracle

app = App()


class Report(msgspec.Struct):
    passed: bool
    detail: str


def cases(seed: int) -> dict[str, np.ndarray]:
    rng = np.random.default_rng(seed)
    outliers = rng.normal(size=(7, 512)).astype("<f4")
    outliers[:, 259] = 1e6
    ties = np.zeros((3, 256), dtype=np.float32)
    ties[:, :9] = [448, 1 / 1024, 3 / 1024, 1.0625, 1.1875, -1 / 1024, -3 / 1024, -1.0625, -1.1875]
    return {
        "zeros": np.zeros((3, 256), dtype=np.float32),
        "negative_zero": np.full((2, 256), -0.0, dtype=np.float32),
        "outliers": outliers,
        "ties": ties,
        "rotated_ties": (oracle.hadamard256(ties.astype(np.float64)) / 16).astype("<f4"),
        "subnormal": rng.choice([-1, 0, 1], size=(3, 256)).astype("<f4") * np.nextafter(np.float32(0), np.float32(1)),
        "floor": rng.uniform(-1e-13, 1e-13, (5, 256)).astype("<f4"),
        "wide": rng.normal(size=(9, 4096)).astype("<f4"),
        "mixed_exponents": (rng.normal(size=(17, 768)) * np.exp2(rng.integers(-90, 90, size=(17, 768)))).astype("<f4"),
    }


@app.entrypoint(accelerator=False)
def microscale_adapters(ctx: Context, *, seed: int = 103) -> Report:
    """Each case through both adapters; fails naming the first that differs from its reference."""
    pairs = {
        "fp8.rowwise": (microscale.encode_fp8_rowwise, oracle.encode_fp8_rowwise),
        "w4a8.convrot": (microscale.encode_w4a8, oracle.encode_w4a8),
    }
    checked: list[str] = []
    for scheme, (adapter, reference) in pairs.items():
        for name, values in cases(seed).items():
            actual, want = adapter(name, values), reference(name, values)
            if tuple(actual[:5]) != tuple(want[:5]) or actual.max_abs_err != want.max_abs_err:
                raise RuntimeError(f"{scheme} {name}: the Runtime's bytes or max error differ from the reference")
            np.testing.assert_allclose(actual.sse, want.sse, rtol=2e-13, atol=0)
            np.testing.assert_allclose(actual.energy, want.energy, rtol=2e-13, atol=0)
            checked.append(f"{scheme}.{name}")
    return Report(True, json.dumps({"checked": checked, "seed": seed}, sort_keys=True))
