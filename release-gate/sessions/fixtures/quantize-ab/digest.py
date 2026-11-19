"""digest.py <store> <source ref> <lane ref>: a lane commit as compare.py judges it, one JSON document on stdout.

Read from the runner's TensorFS store (local_files_only): every tensor of every component, plain or encoded, with the SHA-256 of
each stored part's bytes; each component's config and asset digests; the commit's header id and metadata. Beside each tensor the
source commit's dtype for that key (compare.py's staging arithmetic)."""

from __future__ import annotations

import hashlib
import json
import sys

import tensorfs


def plain(value: object) -> object:
    return json.loads(json.dumps(value, default=str, sort_keys=True))


def main() -> int:
    store, source_ref, ref = sys.argv[1:]
    source = tensorfs.open(source_ref, local_files_only=True, cache_dir=store)
    lane = tensorfs.open(ref, local_files_only=True, cache_dir=store)
    out: dict = {"ref": ref, "id": lane.id, "tree": getattr(lane, "tree", None), "metadata": plain(lane.metadata),
                 "components": {}, "tensors": {}}  # fmt: skip
    for name, component in lane.components.items():
        out["components"][name] = {"config": plain(component.config),
                                   "assets": {k: list(v) for k, v in component.assets.items()}}  # fmt: skip
        origin = source.components.get(name)
        origin_tensors = origin.tensors if origin is not None else {}
        tensors = component.tensors
        for key, meta in tensors.items():
            read = tensors.read(key)
            if meta.encoding is None:
                scheme, params, parts = "plain", {}, {"": hashlib.sha256(read.tobytes()).hexdigest()}
            else:
                scheme, params = meta.encoding.scheme, plain(meta.encoding.params)
                parts = {role: hashlib.sha256(array.tobytes()).hexdigest() for role, array in read.parts.items()}
            row = {"dtype": meta.dtype, "shape": list(meta.shape), "scheme": scheme, "params": params, "parts": parts}
            if key in origin_tensors:
                was = origin_tensors[key]
                row["source"] = {"dtype": was.dtype, "encoded": was.encoding is not None}
            out["tensors"][f"{name}/{key}"] = row
    json.dump(out, sys.stdout)
    return 0


if __name__ == "__main__":
    sys.exit(main())
