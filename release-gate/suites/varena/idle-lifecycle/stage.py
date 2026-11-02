"""Freeze two distinct ordinary applications; does not install, rent, or run a GPU."""
import argparse
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--out', type=Path, required=True)
parser.add_argument('--runtime-version', required=True)
parser.add_argument('--tensorfs-version', required=True)
parser.add_argument('--extension-sha256', required=True)
parser.add_argument('--gpu-uuid', default='UNBOUND')
args = parser.parse_args()
args.out.mkdir(parents=True, exist_ok=False)
source = Path(__file__).resolve().parent
owner = source.parent / 'owner-runtime'
for tag in ('a', 'b'):
    project = args.out / tag
    module = project / 'ci_varena_idle'
    module.mkdir(parents=True)
    (module / '__init__.py').write_text((source / 'app.py').read_text().replace('__APPLICATION__', tag))
    text = (owner / 'pyproject.toml').read_text().replace('ci-varena-owner', f'ci-varena-idle-{tag}').replace('ci_varena_owner', 'ci_varena_idle')
    (project / 'pyproject.toml').write_text(text)
    (project / 'uv.lock').write_text((owner / 'uv.lock').read_text().replace('ci-varena-owner', f'ci-varena-idle-{tag}'))
    (project / 'package.toml').write_text('[application]\nobject = "ci_varena_idle:app"\n')
common = {key: getattr(args, key) for key in ('runtime_version', 'tensorfs_version', 'extension_sha256', 'gpu_uuid')}
(args.out / 'common.json').write_text(json.dumps(common, indent=2) + '\n')
manifest = {str(p.relative_to(args.out)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in args.out.rglob('*') if p.is_file()}
(args.out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps({'bundle': str(args.out.resolve()), 'gpu_bound': args.gpu_uuid != 'UNBOUND', 'files': len(manifest)}))
