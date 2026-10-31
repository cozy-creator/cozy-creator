"""An ordinary Cozy GPU call; model bytes and complete sanitizer logs stay on its rental."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

import msgspec
from cozy_runtime.author import App, Telemetry

from .manifest import MANIFEST

app = App()
ROOT = Path('/var/lib/cozy/xs138-public-diffusers')


class Probe(msgspec.Struct, forbid_unknown_fields=True):
    tensorfs_version: str
    extension_sha256: str
    shim_sha256: str
    gpu_uuid: str
    compute_sanitizer: str = '/var/lib/cozy/xs138-tools/cuda-sanitizer-13.1.1/compute-sanitizer/compute-sanitizer'
    mode: str = 'both'
    seed: int = 13801


class Report(msgspec.Struct):
    passed: bool
    detail: str


def digest(path):
    with Path(path).open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


@app.entrypoint(accelerator=True)
def gpu(payload: Probe, tel: Telemetry) -> Report:
    """Full pretrained SDXL,1024 square,two steps. Memory safety only; no native Cozy model path."""
    if payload.mode not in ('both', 'native', 'varena'):
        raise ValueError('mode must be both, native or varena')
    import torch
    if not payload.gpu_uuid.startswith('GPU-') or str(torch.cuda.get_device_properties(0).uuid) != payload.gpu_uuid:
        raise ValueError('the visible GPU must match the explicitly owned rental UUID before any weight download')
    tool = Path(payload.compute_sanitizer)
    if not tool.is_absolute() or not tool.is_file():
        raise ValueError('the reviewed Compute Sanitizer must already be installed; this fixture never installs it')
    version = subprocess.run([str(tool), '--version'], capture_output=True, text=True, check=True)
    tool_version = version.stdout + version.stderr
    if '2025.4.1.0' not in tool_version:
        raise ValueError(f'expected reviewed Compute Sanitizer2025.4.1.0, got {tool_version}')
    ROOT.mkdir(parents=True, exist_ok=True)
    run_dir = ROOT / ('report-' + str(time.time_ns()))
    run_dir.mkdir()
    # This entrypoint must be sent with --on OWNED_RENTAL. Never download weights on the controller.
    from huggingface_hub import snapshot_download
    with tel.stage('public-sdxl-download-and-hash'):
        snapshot = Path(snapshot_download(
            MANIFEST['repo_id'], revision=MANIFEST['revision'],
            allow_patterns=[item['path'] for item in MANIFEST['files']],
            cache_dir=str(ROOT / 'cache'), token=False, max_workers=2,
        ))
        for index, item in enumerate(MANIFEST['files']):
            path = snapshot / item['path']
            if path.stat().st_size != item['bytes'] or digest(path) != item['sha256']:
                raise RuntimeError(f'public model identity mismatch: {item["path"]}')
            tel.progress((index + 1) / len(MANIFEST['files']))
    download = {'repo_id': MANIFEST['repo_id'], 'revision': MANIFEST['revision'],
                'required_bytes': MANIFEST['total_bytes'], 'file_count': len(MANIFEST['files']),
                'all_file_sha256_verified': True, 'snapshot': str(snapshot),
                'scope': 'required unique model bytes, not measured network retry traffic'}
    tel.log(json.dumps({'public_model': download}, sort_keys=True))
    reports = {}
    modes = ('native', 'varena') if payload.mode == 'both' else (payload.mode,)
    for mode in modes:
        config = {**msgspec.to_builtins(payload), 'mode': mode, 'snapshot': str(snapshot)}
        child = Path(__file__).with_name('probe.py')
        command = [str(tool), '--tool', 'memcheck', '--leak-check', 'full',
                   '--error-exitcode', '99', '--target-processes', 'all',
                   sys.executable, '-I', str(child), json.dumps(config)]
        environment = {**os.environ, 'HF_HUB_OFFLINE': '1', 'TRANSFORMERS_OFFLINE': '1',
                       'HF_HUB_DISABLE_IMPLICIT_TOKEN': '1', 'PYTHONDONTWRITEBYTECODE': '1'}
        result = None
        summaries = []
        log_path = run_dir / (mode + '.log')
        with tel.stage('public-sdxl-' + mode), log_path.open('w') as log:
            process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                       text=True, env=environment)
            assert process.stdout is not None
            try:
                for line in process.stdout:
                    log.write(line)
                    log.flush()
                    tel.log(line.rstrip())  # Complete reports also persist in ordinary Cozy run logs.
                    tel.progress(0.0)
                    if match := re.search(r'ERROR SUMMARY:\s+(\d+) errors', line):
                        summaries.append(int(match.group(1)))
                    try:
                        record = json.loads(line)
                    except ValueError:
                        continue
                    if isinstance(record, dict) and 'result' in record:
                        result = record['result']
                code = process.wait()
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
        reports[mode] = {'child': result, 'process_exit_code': code, 'error_summaries': summaries,
                         'raw_log': str(log_path), 'raw_log_sha256': digest(log_path),
                         'passed': bool(code == 0 and summaries and not any(summaries)
                                        and isinstance(result, dict) and result.get('passed'))}
    report = {'passed': all(item['passed'] for item in reports.values()), 'modes': reports,
              'model': download, 'sanitizer': {'path': str(tool), 'sha256': digest(tool), 'version': tool_version},
              'scope': 'public Diffusers full pretrained fp16 SDXL1024x1024,two steps; general allocator adoption/memory safety only',
              'not_qualified': ['native Cozy int8 execution', 'CUDA graph model execution', 'quality', 'performance'],
              'retention_policy': 'all leaks remain failures; compare native versus Varena raw stacks separately, never suppress/reset/destroy borrowed handles'}
    (run_dir / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    return Report(report['passed'], json.dumps(report, sort_keys=True))
