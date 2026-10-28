"""Real NVFP4 format, production loader, resident final bytes and repeated CUDA matmuls."""

from __future__ import annotations

import gc
import hashlib
import importlib.metadata
import itertools
import tempfile
from pathlib import Path
from types import SimpleNamespace

import tensorfs
import torch
from cozy_runtime import _build_provenance
from cozy_runtime.author._loader import census
from cozy_runtime.internal import accel, residency
from cozy_runtime.internal.decode_cores import Cores
from cozy_runtime.internal.decode_schemes import run as decode_reference
from cozy_runtime.internal.encoding.adapters import CoreDequant, _nvfp4_kernel
from cozy_runtime.internal.encoding.formats import params_of
from cozy_runtime.internal.encoding.selection import launch_providers, measure_device, measure_runtime
from cozy_runtime.internal.fill import Checkpoint, dtype_name
from cozy_runtime.internal.resolution import ConstructionFacts, Variant, resolve
from cozy_runtime.internal.weights import Backend, Placement


def specimen(seed, *, layout="row_major", order="low_first", divide=False, pre=False):
    shape = (256, 256)
    codes = (torch.arange(256 * 256) + seed).remainder(16).reshape(shape)
    low, high = codes[:, 0::2], codes[:, 1::2]
    packed = (low | high << 4) if order == "low_first" else (low << 4 | high)
    scales = torch.full((256, 16), 0.25).to(torch.float8_e4m3fn)
    scales[0, 0] = 0.0
    if layout == "cublas_128x4":
        raw = scales.view(torch.uint8)
        tiled = torch.empty(raw.numel(), dtype=torch.uint8)
        for row in range(256):
            for block in range(16):
                offset = ((row // 128) * 4 + block // 4) * 512
                offset += (row % 32) * 16 + (row % 128 // 32) * 4 + block % 4
                tiled[offset] = raw[row, block]
        scales = tiled.view(torch.float8_e4m3fn).reshape(256, 16)
    parts = {
        "weight": packed.to(torch.uint8),
        "scale": scales,
        "scale_2": torch.tensor(64.0 if divide else 1 / 64),
        "input_scale": torch.tensor(7.0),
    }
    if pre:
        parts["pre_quant_scale"] = (2.0 ** (torch.arange(256).remainder(3) - 1)).to(
            torch.bfloat16
        )
    params = {
        "block_size": 16,
        "global_scale": "divide" if divide else "multiply",
        "nibble_order": order,
        "scale_layout": layout,
    }
    return parts, params


def provider_parity(seed):
    cases = []
    major, minor = torch.cuda.get_device_capability()
    family = major * 10 + minor
    for layout, order, divide, pre in itertools.product(
        ("row_major", "cublas_128x4"),
        ("low_first", "high_first"),
        (False, True),
        (False, True),
    ):
        parts, params = specimen(
            seed, layout=layout, order=order, divide=divide, pre=pre
        )
        provider = CoreDequant("nvfp4", params_of(params))
        assert provider.code_revision.startswith("2/"), (
            "old Runtime decoder is installed"
        )
        assert not provider.supports((256, 256), params)
        cuda_parts = {name: value.cuda() for name, value in parts.items()}
        for dtype, name in (
            (torch.float32, "F32"),
            (torch.bfloat16, "BF16"),
            (torch.float16, "F16"),
        ):
            assert _nvfp4_kernel(family, name), (
                "this gate needs a shipped native CUDA decoder"
            )
            reference = torch.empty((256, 256), dtype=dtype)
            decode_reference(
                provider.adapter, Cores(torch, kernel=False), parts, params, reference
            )
            host = torch.empty_like(reference)
            provider.decode(torch, parts, host)
            device = torch.empty_like(reference, device="cuda")
            provider.decode(torch, cuda_parts, device)
            torch.testing.assert_close(host, reference, rtol=0, atol=0)
            torch.testing.assert_close(device.cpu(), reference, rtol=0, atol=0)
            cases.append(
                {
                    "layout": layout,
                    "order": order,
                    "divide": divide,
                    "pre": pre,
                    "dtype": name,
                }
            )
    return cases


def retained(seed, ctx):
    device = torch.device(str(ctx.device))
    memory = residency.Memory.open(str(device))
    parts, params = specimen(seed, pre=True)
    provider = CoreDequant("nvfp4", params_of(params))
    expected = torch.empty((256, 256))
    provider.decode(torch, parts, expected)
    with tempfile.TemporaryDirectory(prefix="cozy-nvfp4-final-") as directory:
        writer = tensorfs.Writer(Path(directory) / "weights.cozytensors")
        roles = {}
        for role, part in parts.items():
            dtype = (
                "F4"
                if role == "weight"
                else "F8_E4M3"
                if role == "scale"
                else "BF16"
                if role == "pre_quant_scale"
                else "F32"
            )
            shape = (256, 256) if role == "weight" else tuple(part.shape)
            roles[role] = (
                dtype,
                shape,
                part.reshape(-1).view(torch.uint8).numpy().tobytes(),
            )
        writer.component("net").put_encoded(
            "weight",
            dtype="F32",
            shape=(256, 256),
            scheme="nvfp4",
            params=params,
            parts=roles,
        )
        checkpoint = writer.commit()
        source = Checkpoint(checkpoint)
        rows = source.rows("net")
        facts = measure_device(torch, device.index or 0, "cuda")
        plan = resolve(
            variant=Variant(
                "nvfp4-final", directory, checkpoint.id, {"net": checkpoint.id}
            ),
            rows=rows,
            # Consent to the stored encoding, while explicitly qualifying the existing
            # decoded route rather than the unrelated packed GEMM provider.
            providers={
                "nvfp4": tuple(
                    candidate
                    for candidate in launch_providers()["nvfp4"]
                    if candidate.route == "decoded_float"
                )
            },
            device=facts,
            runtime=measure_runtime(torch, importlib.metadata.version("cozy-runtime")),
            construction=ConstructionFacts("nvfp4-final-proof", "Linear", ("net",)),
            encoded_leaves="accept",
            dtype_name=dtype_name,
        )
        assert plan.route_census() == {"decoded_float": 1}, plan.route_census()
        with torch.device("meta"):
            built = SimpleNamespace(net=torch.nn.Linear(256, 256, bias=False))
        walked = census(built)
        backend = Backend(
            {"net": source},
            rows,
            torch=torch,
            plan=plan,
            memory=memory,
            device=device,
            construction="Linear",
            device_facts=facts,
            release="nvfp4-final-proof",
        )
        backend.expect({"net": [row.key for row in rows]})
        fit = backend.fit(walked, encoded_leaves="accept")
        assert fit and fit["ok"], fit
        live = backend.materialize(built, walked, encoded_leaves="accept")
        for destination in walked.destinations:
            backend.fill(destination.key, destination.spec, live[destination.key])
        backend.commit()
        placement = Placement(memory, backend)
        inputs = (
            torch.arange(4 * 256, dtype=torch.float32, device=device).reshape(4, 256)
            / 1024
        )
        expected = expected.to(device)
        reference = torch.nn.functional.linear(inputs, expected)
        alias = None
        try:
            placement.open_attempt()
            with memory.request(f"nvfp4-final-{seed}") as request:
                placement.plan(("net",), request)
                with torch.no_grad():
                    for iteration in range(31):
                        with placement.using("compute", ("net",)):
                            result = built.net(inputs)
                            torch.testing.assert_close(
                                result, reference, rtol=0, atol=0
                            )
                            if iteration == 0:
                                alias = built.net.weight.detach()
                        placement.release()
                        if iteration == 0:
                            cold = (request.moved, request.final_fills)
                            assert cold[0] > 0 and cold[1] == 1, request.stats()
                        else:
                            assert (request.moved, request.final_fills) == cold, (
                                request.stats()
                            )
                warm = request.stats()
                assert request.final_hits >= 30, warm
            placement.release()
            memory.trim()
            assert alias is not None
            torch.testing.assert_close(alias, expected, rtol=0, atol=0)
            assert (
                memory.native.stats()["pinned_bytes"]
                >= alias.numel() * alias.element_size()
            )
            del alias
            alias = None
            gc.collect()
            memory.trim()
            assert memory.native.stats()["resident_bytes"] == 0, memory.native.stats()
            placement.open_attempt()
            with memory.request(f"nvfp4-refill-{seed}") as request:
                placement.plan(("net",), request)
                with torch.no_grad(), placement.using("refill", ("net",)):
                    torch.testing.assert_close(
                        built.net(inputs), reference, rtol=0, atol=0
                    )
                assert request.moved > 0 and request.final_fills == 1, request.stats()
                refill = request.stats()
            return {
                "warm": warm,
                "refill": refill,
                "alias_preserved": True,
                "provider_revision": provider.code_revision,
            }
        finally:
            del alias
            placement.release()
            backend.close()
            memory.trim()
            checkpoint.close()


def exercise(seed, ctx):
    torch.set_num_threads(1)
    accel.initialize(torch, "cuda")
    return {
        "seed": seed,
        "runtime_commit": _build_provenance.COMMIT,
        "tensorfs": tensorfs.__version__,
        "torch": torch.__version__,
        "device": torch.cuda.get_device_name(),
        "decoder_sha256": hashlib.sha256(
            Path(tensorfs._ext.__file__).read_bytes()
        ).hexdigest(),
        "provider_cases": provider_parity(seed),
        "retained": retained(seed, ctx),
    }
