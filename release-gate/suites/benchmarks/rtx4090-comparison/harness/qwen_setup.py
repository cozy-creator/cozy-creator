# /// script
# requires-python = ">=3.12"
# [tool.cozy]
# accelerator = false
# ///
"""Untimed format-only common Qwen files and actual Comfy-loader parity; no quantization."""
import hashlib
import json
import os
import signal
import subprocess
import time
from pathlib import Path
from cozy_runtime.author import Context, FileAsset, Outputs, Telemetry

B = Path('/var/tmp/bench4090-20261026')
SOURCES = None  # @QWEN_SOURCES@


def main(ctx: Context, tel: Telemetry, *, out: Outputs) -> list[FileAsset]:
    assert isinstance(SOURCES, dict) and SOURCES
    python = B / 'comfy-env/bin/python'
    comfy = B / 'ComfyUI'
    for name, source in SOURCES.items():
        assert name in ('qwen_common_files.py', 'qwen_comfy_identity.py', 'qwen_preflight.py')
        (B / name).write_text(source)
    actions = [
        ['qwen_common_files.py', '--export', B / 'qwen-cozy-export', '--models', comfy / 'models',
         '--cozy-receipt', B / 'qwen-cozy-identity.json', '--out', B / 'qwen-common-files.json'],
        ['qwen_comfy_identity.py', '--comfy', comfy,
         '--dit', comfy / 'models/diffusion_models/qwen21-cozy-common-dit.safetensors',
         '--text', comfy / 'models/text_encoders/qwen21-cozy-common-text.safetensors',
         '--vae', comfy / 'models/vae/qwen21-cozy-common-vae.safetensors', '--out', B / 'qwen-comfy-identity.json'],
        ['qwen_preflight.py', B / 'qwen-cozy-identity.json', B / 'qwen-comfy-identity.json', B / 'matched-qwen.json'],
    ]
    record = {'steps': [], 'sources': {name: hashlib.sha256(source.encode()).hexdigest() for name, source in SOURCES.items()}}
    for index, args in enumerate(actions):
        command = [str(python), str(B / args[0]), *map(str, args[1:])]
        began = time.monotonic()
        with (B / f'qwen-setup-{index}.log').open('w') as log:
            process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            beat = began
            try:
                while process.poll() is None:
                    ctx.raise_if_cancelled()
                    if time.monotonic() - beat >= 10:
                        tel.log(f'{args[0]} still running after {time.monotonic()-began:.1f}s')
                        beat = time.monotonic()
                    time.sleep(.5)
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
        record['steps'].append({'command': command, 'seconds': time.monotonic()-began, 'returncode': process.returncode})
        if process.returncode:
            break
    record['ok'] = len(record['steps']) == 3 and all(step['returncode'] == 0 for step in record['steps'])
    if record['ok']:
        match_path = B / 'matched-qwen.json'
        match = json.loads(match_path.read_text())
        assert match['matched']
        match['gpu_identity'] = subprocess.check_output(['nvidia-smi', '--query-gpu=uuid,pci.bus_id,name,driver_version', '--format=csv,noheader'], text=True).strip()
        match['steps'] = 30
        match_path.write_text(json.dumps(match, indent=2))
    path = B / 'qwen-setup.json'; path.write_text(json.dumps(record, indent=2))
    paths = [path, *(B / f'qwen-setup-{i}.log' for i in range(len(record['steps']))), B / 'qwen-common-files.json', B / 'qwen-comfy-identity.json', B / 'matched-qwen.json']
    return [out.save_file(p, media_type='application/json' if p.suffix == '.json' else 'text/plain') for p in paths if p.exists()]
