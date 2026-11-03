"""Successor diagnostic boot; original numerical client/dispatch execute unchanged."""

import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import runpy
import sys

HERE = Path(__file__).resolve().parent
manifest = json.loads((HERE / "frontend-manifest.json").read_text())
assert manifest["trace_format_version"] == 3
assert manifest["execution_base"] in ("original9d-c059", "controlled9d-guardb91-c059")
for name, expected in manifest["files"].items():
    assert hashlib.sha256((HERE / name).read_bytes()).hexdigest() == expected, name
assert manifest["files"]["original_sdxl.py"] == "e7c4eed6fc34a427355cf6fd7b7735bea16f4470b06866bc51bbfa27eed1c652"
assert manifest["files"]["capture_pressure_evidence.py"] == "7653cb76fe29e96f7a6d8d6ef81bf4030aa57623136ef41ebaf007f816964cfb"
allowed = {"--model", "--output", "--device", "--steps", "--height", "--width",
           "--graphs", "--seed", "--guidance", "--prompt"}
assert len(sys.argv[1:]) % 2 == 0
assert all(flag in allowed for flag in sys.argv[1::2]), "use exact frozen option names"
assert len(set(sys.argv[1::2])) == len(sys.argv[1::2])
assert "--seed" in sys.argv, "the frozen ordinary runner explicitly passes its qualified seed"
for flag, value in (("--steps", "20"), ("--height", "1024"), ("--width", "1024"),
                    ("--graphs", "auto"), ("--seed", "1150504160"), ("--guidance", "5.0"), ("--device", "0"),
                    ("--prompt", "A lighthouse above a calm ocean at sunset")):
    assert sys.argv.count(flag) <= 1
    assert not any(arg.startswith(flag + "=") for arg in sys.argv[1:])
    if flag in sys.argv:
        assert sys.argv[sys.argv.index(flag) + 1] == value, flag
output = Path(sys.argv[sys.argv.index("--output") + 1])
output.mkdir(parents=True, exist_ok=True)

# Whitelist observations only: no environment changes, secret-bearing dump or
# claim about Torch's latched setting based merely on environment text.
environment = {name: os.environ.get(name) for name in
               ("CUDA_LAUNCH_BLOCKING", "TORCH_CUBLAS_WORKSPACE_CACHE")}
assert environment["CUDA_LAUNCH_BLOCKING"] in (None, "0")
(output / "frontend-environment-before-torch.json").write_text(json.dumps(environment) + "\n")

import torch
import tensorfs
from tensorfs.varena import torch as vt
from tensorfs.varena import _offload_auto as auto, _offload_graph as graph, api


def load_sibling(name):
    # The ordinary runner uses python -I and runpy(file): no sibling import path
    # is available. Load only the exact files already checked by the manifest.
    assert name not in sys.modules, "diagnostic module slot is already owned"
    spec = importlib.util.spec_from_file_location(name, HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    try:
        spec.loader.exec_module(module)
    except BaseException:
        sys.modules.pop(name, None)
        raise
    return module


load_sibling("capture_pressure_evidence")
frontend = load_sibling("frontend_trace")
FrontendTrace, bind_native = frontend.FrontendTrace, frontend.bind_native

assert torch.__version__ == "2.15.0+cu132"
assert torch.version.git_version == "d0f2af8b66b20da4baa50644c0cd3f887a2eae2e"
assert not torch.version.debug and torch._C._GLIBCXX_USE_CXX11_ABI
for name, expected in manifest["tensorfs_files"].items():
    path = Path(tensorfs.__file__).parent / name
    assert hashlib.sha256(path.read_bytes()).hexdigest() == expected, name
runtime = importlib.util.find_spec("cozy_runtime")
assert runtime is not None and runtime.origin is not None
for name, expected in manifest["runtime_files"].items():
    path = Path(runtime.origin).parent / name
    assert hashlib.sha256(path.read_bytes()).hexdigest() == expected, name
assert not torch.cuda.is_initialized()
native = ctypes.CDLL(str(HERE / "allocation_trace.so"),
                     mode=os.RTLD_NOW | os.RTLD_LOCAL | os.RTLD_NODELETE)
bind_native(native)
workspace_cache = native.allocation_trace_workspace_cache_enabled()
(output / "frontend-workspace-before-cuda.json").write_text(json.dumps({
    "actual_latched_workspace_cache": workspace_cache,
    "cuda_initialized": torch.cuda.is_initialized(),
    "execution_base": manifest["execution_base"],
}) + "\n")
assert workspace_cache == 0, "default-mode diagnostic requires actual workspace cache disabled"
assert not torch.cuda.is_initialized()
installed = False
original_bind = vt._bind_allocator
original_dispatch = auto._AutomaticGraphs.dispatch


def bind():
    global installed
    original_bind()
    if not installed:
        assert not torch.cuda.is_initialized()
        if native.allocation_trace_install() != 0:
            raise RuntimeError(native.allocation_trace_error().decode(errors="replace"))
        installed = True


vt._bind_allocator = bind
trace = FrontendTrace(native, torch, vt, auto, graph, api, output, manifest)
trace.install()
try:
    assert auto._AutomaticGraphs.dispatch is original_dispatch
    runpy.run_path(str(HERE / "original_sdxl.py"), run_name="__main__")
finally:
    trace.finish()
