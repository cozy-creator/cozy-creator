# /// script
# requires-python = ">=3.12"
# [tool.cozy]
# accelerator = false
# ///
"""Install only the frozen diagnostic hook after unprofiled Comfy measurements."""
import hashlib
import json
from pathlib import Path
from cozy_runtime.author import Context, FileAsset, Outputs

B = Path('/var/tmp/bench4090-20261026')
PROFILE_SOURCE = '"""Benchmark-only Torch/Kineto capture around the unchanged Comfy prompt execution.\n\nInstall only for diagnostic samples, after the unprofiled measurements. This hook\nforwards to Comfy\'s own executor and nodes; it contains no copied engine/sampler code.\nThe HTTP extra_data field benchmark_profile opts a request into the existing profiler.\n"""\nfrom __future__ import annotations\n\nimport contextvars\nimport functools\nimport hashlib\nimport inspect\nimport json\nimport os\nimport time\nfrom pathlib import Path\n\nimport execution\nimport torch\n\nTRACE_ROOT = Path(\'/var/tmp/bench4090-20261026/profiles\')\nEXPECTED_EXECUTION_SHA256 = \'40c5306bc8d032aea90b93a7e1ef4ca137e0cf372187132d32b99ae070ba3d87\'\nassert hashlib.sha256(Path(inspect.getfile(execution)).read_bytes()).hexdigest() == EXPECTED_EXECUTION_SHA256\nACTIVE = contextvars.ContextVar(\'benchmark_profile_active\', default=False)\n_original_prompt = execution.PromptExecutor.execute_async\n_original_node = execution.execute\n\n\n@functools.wraps(_original_node)\nasync def observed_node(*args, **kwargs):\n    if not ACTIVE.get():\n        return await _original_node(*args, **kwargs)\n    dynprompt = kwargs.get(\'dynprompt\') if \'dynprompt\' in kwargs else args[1]\n    node_id = kwargs.get(\'current_item\') if \'current_item\' in kwargs else args[3]\n    kind = dynprompt.get_node(node_id)[\'class_type\']\n    with torch.profiler.record_function(f\'comfy.node/{node_id}/{kind}\'):\n        return await _original_node(*args, **kwargs)\n\n\n@functools.wraps(_original_prompt)\nasync def observed_prompt(self, prompt, prompt_id, extra_data=None, execute_outputs=None):\n    extra_data = {} if extra_data is None else extra_data\n    execute_outputs = [] if execute_outputs is None else execute_outputs\n    if extra_data.get(\'benchmark_profile\') is not True:\n        return await _original_prompt(self, prompt, prompt_id, extra_data, execute_outputs)\n    assert prompt_id and all(c.isalnum() or c in \'-_\' for c in prompt_id)\n    TRACE_ROOT.mkdir(parents=True, exist_ok=True)\n    path = TRACE_ROOT / f\'{prompt_id}.trace.json\'\n    assert not path.exists()\n    started = time.time_ns()\n    token = ACTIVE.set(True)\n    try:\n        with torch.profiler.profile(\n            activities=[torch.profiler.ProfilerActivity.CPU, torch.profiler.ProfilerActivity.CUDA],\n            profile_memory=False, record_shapes=False, with_stack=False,\n        ) as profile:\n            with torch.profiler.record_function(f\'comfy.prompt/{prompt_id}/epoch_ns={started}\'):\n                result = await _original_prompt(self, prompt, prompt_id, extra_data, execute_outputs)\n                torch.cuda.synchronize()\n        finished = time.time_ns()\n        profile.export_chrome_trace(str(path))\n        ready = {\'prompt_id\': prompt_id, \'path\': str(path),\n                 \'sha256\': hashlib.sha256(path.read_bytes()).hexdigest(),\n                 \'started_unix_ns\': started, \'finished_unix_ns\': finished,\n                 \'success\': bool(self.success), \'torch\': str(torch.__version__),\n                 \'scope\': \'Diagnostic CPU/CUDA timeline; profiler overhead excluded from benchmark summaries\'}\n        (TRACE_ROOT / f\'{prompt_id}.ready.json\').write_text(json.dumps(ready, indent=2))\n        return result\n    except BaseException as error:\n        (TRACE_ROOT / f\'{prompt_id}.failed.json\').write_text(json.dumps({"error": repr(error), "prompt_id": prompt_id}))\n        raise\n    finally:\n        ACTIVE.reset(token)\n\n\nexecution.execute = observed_node\nexecution.PromptExecutor.execute_async = observed_prompt\nNODE_CLASS_MAPPINGS = {}\n\nTRACE_ROOT.mkdir(parents=True, exist_ok=True)\n(TRACE_ROOT / "module-ready.json").write_text(json.dumps({"pid": os.getpid(), "module_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), "execution_sha256": EXPECTED_EXECUTION_SHA256, "torch": str(torch.__version__)}))\n'
EXECUTION_SHA256 = '40c5306bc8d032aea90b93a7e1ef4ca137e0cf372187132d32b99ae070ba3d87'


def main(ctx: Context, *, out: Outputs) -> FileAsset:
    assert isinstance(PROFILE_SOURCE, str) and PROFILE_SOURCE
    assert hashlib.sha256((B / 'ComfyUI/execution.py').read_bytes()).hexdigest() == EXECUTION_SHA256
    target = B / 'ComfyUI/custom_nodes/cozy_benchmark_profile/__init__.py'
    if target.exists():
        assert target.read_text() == PROFILE_SOURCE, 'different existing diagnostic hook'
    else:
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(PROFILE_SOURCE)
    receipt = B / 'profile-install.json'
    receipt.write_text(json.dumps({'path': str(target), 'sha256': hashlib.sha256(target.read_bytes()).hexdigest(),
                                  'execution_sha256': EXECUTION_SHA256, 'scope': 'diagnostic only; server restart required'}, indent=2))
    return out.save_file(receipt, media_type='application/json')
