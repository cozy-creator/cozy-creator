"""Real MPS checkpoint loading, escaped storage and refill through the installed SDK."""

import gc
import importlib.metadata
import json
import tempfile
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context

app = App()


class Request(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 103


class Report(msgspec.Struct):
    passed: bool
    detail: str


@app.entrypoint(accelerator=True)
def gpu(payload: Request, ctx: Context) -> Report:
    import tensorfs
    import torch
    from cozy_runtime.internal import accel, residency

    assert ctx.device.type == "mps" and torch.backends.mps.is_available()
    memory = residency.Memory.open("mps")
    assert memory.native is None, "MPS must use Torch allocation, not Varena"
    rows, cols = 1024, 16385  # F32 bytes cross the64MiB staging boundary by4096bytes.
    specs = {
        "wide": (rows, cols, torch.float32),
        "half": (128, 128, torch.float16),
        "empty": (2, 0, torch.float32),
    }
    models, modules = {}, {}
    alias = None
    with tempfile.TemporaryDirectory(prefix="cozy-mps-residency-") as directory:
        writer = tensorfs.Writer(Path(directory) / "weights.cozytensors")
        for name, (out_width, in_width, dtype) in specs.items():
            weight = torch.zeros((out_width, in_width), dtype=dtype)
            if in_width:
                weight[torch.arange(out_width), torch.arange(out_width)] = 1
            if name == "wide":
                weight[-1, -1] = 2
            writer.component(name).put("weight", weight)
            del weight
        checkpoint = writer.commit()
        for name, (out_width, in_width, dtype) in specs.items():
            with torch.device("meta"):
                module = torch.nn.Linear(in_width, out_width, bias=False, dtype=dtype)
            vars(module)["_cozy_weights"] = residency.Declared(
                residency.Weights(checkpoint, name)
            )
            modules[name] = module
            models[name] = memory.load(module, "ram")
        wide_input = torch.ones((1, cols), device="mps")
        expected = torch.ones((1, rows))
        expected[0, -1] = 3
        try:
            with memory.request(f"mps-load-{payload.seed}") as request, torch.no_grad():
                with models["wide"].passing("load"):
                    actual = modules["wide"](wide_input)
                    alias = modules["wide"].weight.detach()
                torch.testing.assert_close(actual.cpu(), expected, rtol=0, atol=0)
                cold_bytes = request.moved
                assert cold_bytes == rows * cols * 4
                with models["wide"].passing("warm"):
                    torch.testing.assert_close(
                        modules["wide"](wide_input).cpu(), expected, rtol=0, atol=0
                    )
                assert request.moved == cold_bytes, "warm MPS weights were reread"
                with models["half"].passing("typed"):
                    half = torch.ones((1, 128), dtype=torch.float16, device="mps")
                    torch.testing.assert_close(
                        modules["half"](half).cpu(),
                        torch.ones((1, 128), dtype=torch.float16),
                        rtol=0,
                        atol=0,
                    )
                    del half
                empty = models["empty"].retain()
                try:
                    with empty.prepare():
                        assert tuple(modules["empty"].weight.shape) == (2, 0)
                finally:
                    empty.close()
                stats = request.stats()
                assert stats["measurement"] == "observed_current"
                assert memory.room().free <= accel.unified_ceiling(torch)
            del actual
            before = torch.mps.current_allocated_memory()
            memory.trim()
            assert alias is not None and alias.device.type == "mps"
            assert alias[-1, -1].item() == 2 and alias[0, 0].item() == 1
            held = torch.mps.current_allocated_memory()
            assert held >= rows * cols * 4, "escaped MPS storage was released"
            del alias
            alias = None
            gc.collect()
            memory.trim()
            after = torch.mps.current_allocated_memory()
            assert held - after >= rows * cols * 4, (held, after)
            with (
                memory.request(f"mps-refill-{payload.seed}") as request,
                torch.no_grad(),
            ):
                with models["wide"].passing("refill"):
                    torch.testing.assert_close(
                        modules["wide"](wide_input).cpu(), expected, rtol=0, atol=0
                    )
                assert request.moved == rows * cols * 4, request.stats()
            result = {
                "seed": payload.seed,
                "device": str(ctx.device),
                "runtime": importlib.metadata.version("cozy-runtime"),
                "tensorfs": tensorfs.__version__,
                "torch": torch.__version__,
                "cross_window_bytes": cold_bytes,
                "warm_read_bytes": 0,
                "typed_and_zero_parts": True,
                "alias_retained": True,
                "allocated_before_trim": before,
                "allocated_with_alias": held,
                "allocated_after_alias_drop": after,
                "refill_bytes": request.moved,
                "mps_stats": stats,
            }
            return Report(True, json.dumps(result, sort_keys=True))
        finally:
            del alias
            for model in models.values():
                model.close()
            memory.trim()
            checkpoint.close()
