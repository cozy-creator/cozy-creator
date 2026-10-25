"""Public two-output quantization followed by a real independent CUDA plan."""

import hashlib
import json
import tempfile
from pathlib import Path

import msgspec
from cozy_runtime.author import App, Context, ModelArtifact, Telemetry, WeightsOutput

app = App()


class Request(msgspec.Struct, forbid_unknown_fields=True):
    seed: int = 103


class Report(msgspec.Struct):
    passed: bool
    int8: ModelArtifact
    nvfp4: ModelArtifact
    detail: str


@app.entrypoint(
    accelerator=True,
    weights=(WeightsOutput("int8", 1 << 20), WeightsOutput("nvfp4", 1 << 20)),
)
def gpu(payload: Request, ctx: Context, tel: Telemetry) -> Report:
    import numpy as np
    import tensorfs
    import torch
    from cozy_runtime.compute import PreparedRepeat
    from cozy_runtime.derive import facade

    from .oracle import encode_int8_convrot256, encode_nvfp4

    rng = np.random.default_rng(payload.seed)
    values = rng.normal(size=(8, 256)).astype("<f4")
    with tempfile.TemporaryDirectory(prefix="cozy-native-caller-") as directory:
        writer = tensorfs.Writer(Path(directory) / "source.cozytensors")
        component = writer.component("net", config={"seed": payload.seed})
        component.put("proj.weight", values)
        component.put("proj.bias", np.zeros(8, dtype="<f4"))
        source = writer.commit()
        first = facade.quantize_artifact(
            source,
            facade.plan(("net",), "int8.convrot"),
            ctx=ctx,
            tel=tel,
            output="int8",
        )
        second = facade.quantize_artifact(
            source, facade.plan(("net",), "nvfp4"), ctx=ctx, tel=tel, output="nvfp4"
        )
        checked = []
        for artifact, producer in (
            (first, encode_int8_convrot256),
            (second, encode_nvfp4),
        ):
            checkpoint = tensorfs.open(artifact.checkpoint, local_files_only=True)
            encoded = checkpoint.components["net"].tensors.read("proj.weight")
            reference = producer("proj.weight", values)
            assert encoded.parts["weight"].tobytes() == reference.payload
            for role, part in zip(("scale", "scale_2"), reference.companions):
                assert encoded.parts[role].tobytes() == part.raw
            assert checkpoint.verify().ok
            checked.append(
                {
                    "checkpoint": checkpoint.id,
                    "scheme": encoded.scheme,
                    "weight_sha256": hashlib.sha256(reference.payload).hexdigest(),
                }
            )
            checkpoint.close()

        # Both synchronous conversions have joined and left Context.plan's one slot unused.
        with torch.inference_mode(False), torch.no_grad():
            value = torch.zeros(1, device="cuda")
            calls = [0]

            def step(index):
                calls[0] += 1
                value.add_(1)

            plan = PreparedRepeat.capture(step, state=(value,), steps=31)
            try:
                execution = ctx.plan(plan)
                (result,) = execution.wait()
                assert result.item() == 31 and calls[0] == 2
                facts = execution.facts()
                assert facts["host_graph_launches"] == 1
            finally:
                plan.close()
        detail = {
            "seed": payload.seed,
            "outputs": checked,
            "after_conversion_plan": facts,
            "tensorfs": tensorfs.__version__,
            "gpu": torch.cuda.get_device_name(),
        }
        return Report(True, first, second, json.dumps(detail, sort_keys=True))
