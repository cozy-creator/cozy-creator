"""Benchmark-only Torch/Kineto capture around the unchanged Comfy prompt execution.

Install only for diagnostic samples, after the unprofiled measurements. This hook
forwards to Comfy's own executor and nodes; it contains no copied engine/sampler code.
The HTTP extra_data field benchmark_profile opts a request into the existing profiler.
"""
from __future__ import annotations

import contextvars
import functools
import hashlib
import inspect
import json
import os
import time
from pathlib import Path

import execution
import torch

TRACE_ROOT = Path('/var/tmp/bench4090-20261026/profiles')
EXPECTED_EXECUTION_SHA256 = '40c5306bc8d032aea90b93a7e1ef4ca137e0cf372187132d32b99ae070ba3d87'
assert hashlib.sha256(Path(inspect.getfile(execution)).read_bytes()).hexdigest() == EXPECTED_EXECUTION_SHA256
ACTIVE = contextvars.ContextVar('benchmark_profile_active', default=False)
_original_prompt = execution.PromptExecutor.execute_async
_original_node = execution.execute


@functools.wraps(_original_node)
async def observed_node(*args, **kwargs):
    if not ACTIVE.get():
        return await _original_node(*args, **kwargs)
    dynprompt = kwargs.get('dynprompt') if 'dynprompt' in kwargs else args[1]
    node_id = kwargs.get('current_item') if 'current_item' in kwargs else args[3]
    kind = dynprompt.get_node(node_id)['class_type']
    with torch.profiler.record_function(f'comfy.node/{node_id}/{kind}'):
        return await _original_node(*args, **kwargs)


@functools.wraps(_original_prompt)
async def observed_prompt(self, prompt, prompt_id, extra_data=None, execute_outputs=None):
    extra_data = {} if extra_data is None else extra_data
    execute_outputs = [] if execute_outputs is None else execute_outputs
    if extra_data.get('benchmark_profile') is not True:
        return await _original_prompt(self, prompt, prompt_id, extra_data, execute_outputs)
    assert prompt_id and all(c.isalnum() or c in '-_' for c in prompt_id)
    TRACE_ROOT.mkdir(parents=True, exist_ok=True)
    path = TRACE_ROOT / f'{prompt_id}.trace.json'
    assert not path.exists()
    started = time.time_ns()
    token = ACTIVE.set(True)
    try:
        with torch.profiler.profile(
            activities=[torch.profiler.ProfilerActivity.CPU, torch.profiler.ProfilerActivity.CUDA],
            profile_memory=False, record_shapes=False, with_stack=False,
        ) as profile:
            with torch.profiler.record_function(f'comfy.prompt/{prompt_id}/epoch_ns={started}'):
                result = await _original_prompt(self, prompt, prompt_id, extra_data, execute_outputs)
                torch.cuda.synchronize()
        finished = time.time_ns()
        profile.export_chrome_trace(str(path))
        ready = {'prompt_id': prompt_id, 'path': str(path),
                 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                 'started_unix_ns': started, 'finished_unix_ns': finished,
                 'success': bool(self.success), 'torch': str(torch.__version__),
                 'scope': 'Diagnostic CPU/CUDA timeline; profiler overhead excluded from benchmark summaries'}
        (TRACE_ROOT / f'{prompt_id}.ready.json').write_text(json.dumps(ready, indent=2))
        return result
    except BaseException as error:
        (TRACE_ROOT / f'{prompt_id}.failed.json').write_text(json.dumps({"error": repr(error), "prompt_id": prompt_id}))
        raise
    finally:
        ACTIVE.reset(token)


execution.execute = observed_node
execution.PromptExecutor.execute_async = observed_prompt
NODE_CLASS_MAPPINGS = {}

TRACE_ROOT.mkdir(parents=True, exist_ok=True)
(TRACE_ROOT / "module-ready.json").write_text(json.dumps({"pid": os.getpid(), "module_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), "execution_sha256": EXPECTED_EXECUTION_SHA256, "torch": str(torch.__version__)}))
