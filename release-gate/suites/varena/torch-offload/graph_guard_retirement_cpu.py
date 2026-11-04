"""Real capture-binding bodies and C10 Storage lifetimes; CUDA is never initialized."""

from __future__ import annotations

import json
from pathlib import Path

import torch
from tensorfs.varena import _offload as adapter
from tensorfs.varena import torch as vt
from tensorfs.varena._offload_graph import (
    _CaptureBindings,
    _live_storages,
    _storage_tokens,
)

torch.set_num_threads(1)
torch.set_num_interop_threads(1)


class Model(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.weight = torch.nn.Parameter(torch.arange(12.0).reshape(3, 4))
        self.register_buffer("bias", torch.arange(3.0))

    def forward(self, value):
        return torch.nn.functional.linear(value, self.weight, self.bias)


def run(*, escape):
    model = Model().eval()
    value = torch.ones(1, 4)
    with torch.inference_mode():
        expected = model(value)
    owner = vt.offload(model, device="cpu")
    version = model.weight._version_owner
    prepared = {version: version.tensor((12,), (1,), 0, None)}
    exports = list(prepared.values())
    tokens = _storage_tokens(exports)
    binding = _CaptureBindings(owner, model, prepared, None, (value, {}))
    close = getattr(binding, "close", None)
    # Missing close is allowed only to run this same red-capable client against
    # the historical implementation; the Storage assertions below must still pass.
    with torch.inference_mode(), binding.scope():
        actual = model(value)
        if close is not None:
            try:
                close()
            except adapter.OffloadError as error:
                assert "active" in str(error)
            else:
                raise AssertionError("active read-only binding was closed")
        try:
            prepared[version].zero_()
        except adapter.OffloadError:
            pass
        else:
            raise AssertionError("read-only guard lost prepared-weight protection")
        detached = binding.tensor(model.weight).detach() if escape else None
    torch.testing.assert_close(actual, expected, rtol=0, atol=0)
    # This is the production ended-binding release seam. In actual capture it
    # is called only after the complete vt.graph capture/routing context returns.
    if close is not None:
        close()
    prepared.clear()
    exports.clear()
    retained = _live_storages(tokens)
    after_tables = len(retained)
    if detached is not None:
        assert len(retained) == 1
        assert retained[0]._cdata == detached.untyped_storage()._cdata
    detached = None
    retained_after_alias_drop = bool(_live_storages(tokens))
    retained = ()
    expired_after_retained_drop = not _live_storages(tokens)
    # Keep binding and its bound close method alive deliberately. Ended guard
    # ownership must retire even when these Python objects outlive the exports.
    if close is not None:
        assert not binding.buffers and not binding.prepared
        try:
            with binding.scope():
                raise AssertionError("closed binding scope was entered")
        except adapter.OffloadError as error:
            assert "closed" in str(error)
    owner.close()
    return {
        "escape": escape,
        "ended_binding_still_alive": True,
        "live_storages_after_guard_and_tables": after_tables,
        "retained_after_alias_drop": retained_after_alias_drop,
        "expired_after_retained_drop": expired_after_retained_drop,
    }


rows = [run(escape=False), run(escape=True)]
assert not torch.cuda.is_initialized()
assert "libcuda.so" not in Path("/proc/self/maps").read_text()
print(
    json.dumps(
        {
            "torch": torch.__version__,
            "cuda": False,
            "graphs_and_native_plan_cleanup": "not exercised",
            "actual_read_only_body_and_storage_controls": rows,
        }
    ),
    flush=True,
)
assert rows[0]["live_storages_after_guard_and_tables"] == 0
assert rows[1]["live_storages_after_guard_and_tables"] == 1
assert rows[1]["retained_after_alias_drop"]
assert all(row["expired_after_retained_drop"] for row in rows)
