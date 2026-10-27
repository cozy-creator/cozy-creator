"""The existing release-gate's real checkpoint census and Backend/Placement route."""

from __future__ import annotations

import hashlib
import importlib.metadata
import shutil
from pathlib import Path
from types import SimpleNamespace

import tensorfs
import torch
from cozy_runtime.author._loader import census
from cozy_runtime.author._model import PLACED
from cozy_runtime.internal import residency
from cozy_runtime.internal.derive import Observations, serving_substrate
from cozy_runtime.internal.encoding import launch_providers
from cozy_runtime.internal.encoding.selection import measure_device, measure_runtime
from cozy_runtime.internal.fill import Checkpoint, dtype_name
from cozy_runtime.internal.resolution import ConstructionFacts, Variant, resolve
from cozy_runtime.internal.weights import Backend, Placement


def place(memory, checkpoint, component, factory, *, encoded=False):
    opened = Checkpoint(checkpoint)
    rows = opened.rows(component)
    device = memory.device
    facts = measure_device(torch, device.index or 0, "cuda")
    plan = resolve(
        variant=Variant(
            "component-load", "fixture", checkpoint.id, {component: checkpoint.id}
        ),
        rows=rows,
        providers=launch_providers(),
        device=facts,
        runtime=measure_runtime(torch, importlib.metadata.version("cozy-runtime")),
        construction=ConstructionFacts(
            "component-load-proof", "PlacedFixture", (component,)
        ),
        encoded_leaves="accept" if encoded else "refuse",
        dtype_name=dtype_name,
    )
    # The real serving construction path preserves derived/nonpersistent buffer values.
    # Full-meta derivation erases CLIP position_ids, which no checkpoint can refill.
    with serving_substrate(f"sm{facts.sm}", Observations()):
        root = factory().eval()
    assert all(value.device.type == "meta" for value in root.parameters())
    for module in root.modules():
        for name in module._non_persistent_buffers_set:
            value = module._buffers.get(name)
            assert value is None or value.device.type != "meta", name
            if name == "position_ids" and value is not None:
                assert torch.equal(
                    value.cpu(), torch.arange(value.numel()).reshape(value.shape)
                )
    built = SimpleNamespace(**{component: root})
    walked = census(built)
    backend = Backend(
        {component: opened},
        rows,
        torch=torch,
        plan=plan,
        memory=memory,
        device=device,
        construction="PlacedFixture",
        device_facts=facts,
        release="component-load-proof",
    )
    backend.expect({component: [row.key for row in rows]})
    fit = backend.fit(walked, encoded_leaves="accept" if encoded else "refuse")
    assert fit and fit["ok"], fit
    live = backend.materialize(
        built, walked, encoded_leaves="accept" if encoded else "refuse"
    )
    for destination in walked.destinations:
        backend.fill(destination.key, destination.spec, live[destination.key])
    backend.commit()
    placement = Placement(memory, backend)
    assert PLACED.get(root) == (placement, component)
    return root, backend, placement


def current_request(memory):
    request = residency._REQUEST.get()
    assert request is not None and request.memory is memory and not request.idle
    assert not request.overlap, "fixture requires one admitted request"
    return request


def slots(model):
    return [
        (site, site.table[site.name])
        for group in model.sites.values()
        for site in group
    ]


def assert_slots(saved):
    assert all(site.table[site.name] is value for site, value in saved), (
        "module slots changed"
    )


def private_input(checkpoint, directory):
    exported = Path(tensorfs.materialize(checkpoint, local_dir=directory / "export"))
    private = directory / "private-input"
    shutil.copytree(exported, private, copy_function=shutil.copyfile, symlinks=False)
    copied = tensorfs.open(str(private), local_files_only=True)
    chunks = [
        private / "chunks" / digest.removeprefix("sha256:")
        for digest, _ in copied.chunks
    ]
    assert chunks and all(p.is_file() and p.stat().st_nlink == 1 for p in chunks)
    return copied, chunks


def versions():
    from cozy_runtime import compute

    return {
        "runtime": importlib.metadata.version("cozy-runtime"),
        "tensorfs": tensorfs.__version__,
        "torch": torch.__version__,
        "device": torch.cuda.get_device_name(0),
        "native_extension_sha256": hashlib.sha256(
            Path(tensorfs._ext.__file__).read_bytes()
        ).hexdigest(),
        "compute_sha256": hashlib.sha256(
            Path(compute.__file__).read_bytes()
        ).hexdigest(),
        "residency_sha256": hashlib.sha256(
            Path(residency.__file__).read_bytes()
        ).hexdigest(),
    }
