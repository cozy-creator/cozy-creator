from pathlib import Path
import hashlib, json, shutil, sys
root = Path(sys.argv[1]); out = root / 'perf-publish'; out.mkdir(exist_ok=True)
# Never publish the shared source harness, dependencies, browser profiles, holder, or raw headers.
for source in [root/'perf-matrix'/'matrix.json', root/'perf-matrix'/'execution.json', root/'native-version.txt']:
    if source.is_file(): shutil.copy2(source, out/source.name)
for label in ['current-plain', 'baseline-plain', 'baseline-profiled', 'current-profiled']:
    source = root/'perf-matrix'/label
    if not source.is_dir(): continue
    target = out/label; target.mkdir(exist_ok=True)
    for file in source.rglob('*'):
        if not file.is_file(): continue
        if file.suffix not in ['.json', '.err', '.png', '.cpuprofile', '.map']: continue
        relative = file.relative_to(source)
        if any(part in ['node_modules', 'profiles', 'shared-harness'] for part in relative.parts): continue
        dest = target/relative; dest.parent.mkdir(parents=True, exist_ok=True); shutil.copy2(file, dest)
for label in ['current', 'baseline']:
    source = root/f'frontend-{label}'/'manifest.json'
    if not source.is_file(): continue
    shutil.copy2(source, out/f'{label}-frontend-manifest.json')
    manifest = json.loads(source.read_text())
    dist = root/f'frontend-{label}'/'profiled'
    identities = []
    for name, expected in manifest['variants']['profiled']['files'].items():
        if not name.endswith('.map'): continue
        raw = (dist/name).read_bytes()
        if hashlib.sha256(raw).hexdigest() != expected: raise RuntimeError('profile map identity changed')
        mapping = json.loads(raw)
        # Keep exact coordinates/names/mappings. Source bodies are available at the pinned refs; never publish their text as runtime data.
        mapping.pop('sourcesContent', None)
        target = out/'source-maps'/label/name
        target.parent.mkdir(parents=True, exist_ok=True)
        rendered = (json.dumps(mapping, separators=(',', ':'))+'\n').encode()
        target.write_bytes(rendered)
        bundle = name[:-4]
        identities.append({'map': name, 'original_map_sha256': expected,
            'mapping_metadata_sha256': hashlib.sha256(rendered).hexdigest(),
            'bundle': bundle, 'bundle_sha256': manifest['variants']['profiled']['files'].get(bundle),
            'source_ref': manifest['ref'], 'sources_content_omitted': True})
    (out/f'{label}-source-map-identities.json').write_text(json.dumps(identities, indent=2)+'\n')
    # The small observer/entry are diagnostic code, not runtime object contents; preserve their exact source identities too.
    for name in ['profile-observer.ts', 'profile-entry.ts']:
        code = root/f'frontend-{label}'/name
        if code.is_file(): shutil.copy2(code, out/f'{label}-{name}.txt')
(out/'publication-scope.json').write_text(json.dumps({'scope':'allowlisted diagnostic result formats; no holder, shared harness, dependencies, profiles or raw headers', 'limits':'profiling and targeted warmup cannot qualify product performance'}, indent=2)+'\n')
