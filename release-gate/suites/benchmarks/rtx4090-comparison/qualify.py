"""Ordinary default-home CLI samples; matching, setup and diagnostic timings stay separate."""
from __future__ import annotations

import hashlib
import contextlib
import datetime
import fcntl
import json
import statistics
import subprocess
import time
from pathlib import Path

import cozy_bench

CLI = Path.home() / '.cozy/bin/cozy'
HUB = 'https://tensorhub.com'
DEADLINE = None


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def save(out, name, value):
    (out / name).write_text(json.dumps(value, indent=2) + '\n')


def documents(text):
    decoder = json.JSONDecoder()
    while text.strip():
        value, end = decoder.raw_decode(text.lstrip())
        yield value
        text = text.lstrip()[end:]


def source_check(cohort):
    import PIL.Image  # noqa: F401
    for path, expected in cohort['source_sha256'].items():
        assert digest(path) == expected, path
    for artifact in cohort['artifacts'].values():
        assert digest(artifact['path']) == artifact['sha256'], artifact
    assert cohort['model_order'] and all(key in ('sdxl', 'qwen21') for key in cohort['model_order'])
    assert all(cohort['models'][key]['steps'] == 30 for key in cohort['model_order'])
    assert len({s['seed'] for s in cohort['samples'] if s['engine'] == 'cozy'}) == len([s for s in cohort['samples'] if s['engine'] == 'cozy'])


def _call(out, label, arguments, *, cohort=None, timed=False, check=True):
    argv = [str(CLI), '--json', '--tensorhub', HUB, *map(str, arguments)]
    before = digest(CLI)
    if timed:
        assert before == cohort['caller_sha256'], 'primary changed the caller CLI; stop this comparison without mutating local'
        assert time.time() < cohort['deadline_epoch'] - 180, 'leave time for output/profiler collection and teardown'
    save(out, label + '-command.json', argv)
    started = time.monotonic_ns()
    with (out / (label + '.json')).open('w') as stdout, (out / (label + '.err')).open('w') as stderr:
        try:
            result = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                    timeout=max(1, DEADLINE - time.time()) if DEADLINE is not None else 90)
        except subprocess.TimeoutExpired:
            elapsed = (time.monotonic_ns() - started) / 1e6
            save(out, label + '-timing.json', {'cli_wall_ms': elapsed, 'deadline_timeout': True,
                 'caller_sha256_before': before, 'scope': 'CLI follower timed out; owned-rental finally/watchdog handles remote work'})
            raise
    elapsed = (time.monotonic_ns() - started) / 1e6
    values = list(documents((out / (label + '.json')).read_text()))
    runs = [row['run'] for row in values if isinstance(row.get('run'), dict)]
    run = runs[-1] if runs else values[-1] if values else {}
    images, usable_ms, image_error = [], None, None
    if timed and run.get('status') == 'completed':
        try:
            images = cozy_bench.image_evidence(run, (1024, 1024))
            usable_ms = (time.monotonic_ns() - started) / 1e6
        except Exception as error:
            image_error = repr(error)
    after = digest(CLI)
    timing = {'cli_wall_ms': elapsed, 'caller_to_reopened_image_ms': usable_ms,
              'returncode': result.returncode, 'caller_sha256_before': before,
              'caller_sha256_after': after, 'images': images, 'image_error': image_error,
              'run_number': run.get('number'), 'run_status': run.get('status'),
              'caller_loadavg_after': Path('/proc/loadavg').read_text().strip()}
    save(out, label + '-timing.json', timing)
    if check:
        assert result.returncode == 0, (label, run)
    if timed:
        assert after == before, 'caller binary changed during the sample; retain but do not compare'
        assert image_error is None, (label, image_error)
    return run, values, timing


@contextlib.contextmanager
def measured_caller(cohort, out, label):
    """Brief read-only caller window; primary setup/inference is never interrupted."""
    path = Path.home() / 'cozy_v2/agent-tools/.local-stack.lock'
    began = time.monotonic()
    with path.open('a') as lock:
        while True:
            assert time.time() < cohort['deadline_epoch'] - 180, 'rental deadline reached while waiting for primary caller window'
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                time.sleep(.5)
                continue
            if digest(CLI) == cohort['caller_sha256']:
                break
            fcntl.flock(lock, fcntl.LOCK_UN)
            time.sleep(.5)
        waited = time.monotonic() - began
        def note(state):
            with (path.parent / 'local-stack.log').open('a') as log:
                log.write(f'{datetime.datetime.now(datetime.UTC).isoformat()} Codex RTX4090 read-only caller {label}: {state}; no SDK mutation\n')
        note('acquired')
        save(out, label + '-caller-window.json', {'wait_seconds_excluded_from_timing': waited, 'caller_sha256': cohort['caller_sha256'], 'local_sdk_mutated': False})
        try:
            yield
        finally:
            note('released')
            fcntl.flock(lock, fcntl.LOCK_UN)


def call(out, label, arguments, *, cohort=None, timed=False, check=True):
    if timed:
        with measured_caller(cohort, out, label):
            return _call(out, label, arguments, cohort=cohort, timed=True, check=check)
    return _call(out, label, arguments, cohort=cohort, timed=False, check=check)


def idle(out, label, machine, commit):
    row, _, _ = call(out, label, ['machine', 'show', machine])
    assert row['state'] == 'ready' and not row.get('running') and not row.get('queued'), row
    assert not row['status'].get('runs')
    assert row['status']['commit'] == commit
    return row


def image_request(sample):
    request = {'prompt': sample['prompt'], 'negative_prompt': sample['negative_prompt'],
            'aspect_ratio': '1:1', 'megapixels': 1.0, 'steps': sample['steps'],
            'guidance': sample['guidance'], 'seed': sample['seed']}
    if sample['cell'] == 'sdxl':
        request['hidiffusion'] = False
    return request


def full_run(out, label, run):
    full, _, _ = call(out, label + '-show', ['run', 'show', str(run['number']), '--once'])
    assert full['id'] == run['id'] and full['status'] == 'completed'
    return full


def result_json(run, field):
    """A JSON receipt carried as a string field on the entrypoint's result struct (run['result']).
    Receipts ride the result, not an output asset, because 0.33.2 output rows carry no source
    filename (name=field, path=results/<id>/<index>-<digest>); binaries are matched by media type."""
    return json.loads(run['result'][field])


def terminal(out, label, arguments, **options):
    run, events, timing = call(out, label, arguments, **options)
    assert run.get('status') == 'completed', (label, run)
    return run, events, timing


def reset_owned(cohort, out, label, machine):
    args = ['machine', 'update', machine, '--gpu-memory-limit', 'off']
    for kind, artifact in cohort['artifacts'].items():
        args.extend([f'--{kind}', artifact['path']])
    run, _, _ = terminal(out, label, args)
    for kind in ('runtime', 'tensorfs'):
        assert run['result']['to'][kind + '_sha256'] == cohort['artifacts'][kind]['sha256']
    idle(out, label + '-idle', machine, cohort['creator_source'])
    return run['number']


def sampler(cohort, out, label, machine):
    return terminal(out, label, ['run', cohort['scripts']['sampler_start'],
        f'session={cohort["session"]}', f'deadline_unix_s={cohort["deadline_epoch"]}', '--on', machine])[0]


def eager_reference(cohort, out, machine, key, samples):
    """Real same-SDK eager output, excluded from the benchmark sample population."""
    reference = cohort.get('eager_reference', {}).get(key)
    if reference is None:
        return None
    model = cohort['models'][key]
    target = Path(reference['packages']) / model['package'] / 'generate'
    candidate = Path(cohort['packages']) / model['package'] / 'generate'
    assert target != candidate, 'reference and candidate must have distinct source installations'
    sample = next(s for s in samples if s['engine'] == 'cozy' and s['mode'] == 'cold')
    request = image_request(sample)
    save(out, 'eager-reference-request.json', request)
    run, events, timing = terminal(out, 'eager-reference', [
        'run', target, '--input', out / 'eager-reference-request.json',
        f'model.model={model["reference"]}', '--lane', model['lane'],
        '--on', machine, '--gpus=1', '--out', out / 'eager-reference-image'])
    full = full_run(out, 'eager-reference', run)
    detail = cozy_bench.summarize(full, request, (1024, 1024), events,
                                  timing['cli_wall_ms'], 'denoise' if key == 'sdxl' else 'generating image')
    assert detail['observed_step_callback_count'] == model['steps']
    assert detail['native_completed_steps'] == 0 and detail['mode'] == 'eager_steps'
    assert detail['software']['runtime'].endswith(cohort['runtime_source'][:8])
    assert detail['software']['tensord_commit'] == cohort['creator_source']
    assert any(m['manifest'] == model['checkpoint'] for m in detail['models'])
    result = {'request': request, 'source': reference, 'detail': detail,
              'scope': 'Untimed same-device eager numerical control. Distinct package source excludes output memo reuse. Subsequent reset retires its executor before candidate timing.'}
    save(out, 'eager-reference-proof.json', result)
    return result


def require_eager_parity(out, label, reference, record):
    expected = reference['detail']
    assert record['seed'] == expected['seed']
    assert record['software']['runtime'] == expected['software']['runtime']
    assert record['software']['tensord_commit'] == expected['software']['tensord_commit']
    left, right = expected['images'][0], record['images'][0]
    checks = {key: left[key] == right[key] for key in ('size', 'rgb_sha256', 'rgba_sha256')}
    proof = {'passed': all(checks.values()), 'checks': checks,
             'reference_run': expected['run_number'], 'candidate_run': record['run_number'],
             'reference_image': left, 'candidate_image': right,
             'reference_routes': expected['routes'], 'candidate_routes': record['routes'],
             'policy': 'Exact decoded RGB and alpha parity; no tolerance relaxation. PNG metadata/file bytes may differ.'}
    save(out, label + '-eager-parity.json', proof)
    assert proof['passed'], 'native candidate differs from the same-SDK eager reference; diagnose before comparing speed'


def prepare_model(cohort, out, machine, key, samples):
    model, scripts = cohort['models'][key], cohort['scripts']
    args = ['--on', machine, '--gpus=1']
    warm = ['--on', machine]  # a --warm/download keeps the function ready, not a run; --gpus is rejected there (warm_gpus)
    target = Path(cohort['packages']) / model['package'] / 'generate'
    script = scripts['comfy_' + key]
    # cozy_args drives only the --warm/download/off invocations (here and in measure_group). --warm
    # keeps a function ready and takes NO request inputs (warm_inputs) — only model.<slot>= and --lane
    # (which picks the quantization to warm) and --lora. No --input/payload, no --gpus (warm_gpus).
    cozy_args = ['run', target, f'model.model={model["reference"]}', '--lane', model['lane'], *warm]
    setup = {}
    run, _, timing = terminal(out, 'cozy-download', [*cozy_args, '--warm', 'downloaded'])
    setup['cozy_download'] = {'run': run['number'], **timing}
    run, _, timing = terminal(out, 'comfy-wrapper-download', ['run', script, *warm, '--warm', 'downloaded'])
    setup['comfy_wrapper'] = {'run': run['number'], **timing}
    identity_args = [f'model.weights={model["reference"]}', '--lane', model['lane'],
                     f'expected_runtime_source={cohort["runtime_source"]}']
    if key == 'qwen21':
        identity_args += ['folder=/var/tmp/bench4090-20261026', 'export=true']
    run, _, timing = terminal(out, 'cozy-identity', ['run', scripts['identity_' + key], *identity_args, *args])
    setup['cozy_identity'] = {'run': run['number'], **timing}
    match_script = scripts['match_preflight'] if key == 'sdxl' else scripts['qwen_setup']
    run, _, timing = terminal(out, 'matching', ['run', match_script, *args])
    setup['matching'] = {'run': run['number'], **timing}
    if key == 'qwen21':
        prepared = result_json(run, 'setup')
        assert prepared['ok'], prepared
        setup['common_file_setup'] = prepared
        match = result_json(run, 'match')
    else:
        match = result_json(run, 'match')
    assert match['matched'] and match['tensorfs_checkpoint'] == model['checkpoint'], match
    reference = eager_reference(cohort, out, machine, key, samples)
    if reference is not None:
        setup['eager_reference'] = reference
    save(out, 'matched.json', match)
    save(out, 'setup.json', setup)
    return setup, cozy_args


def measure_group(cohort, out, machine, key, engine, samples, setup, cozy_args):
    model, scripts = cohort['models'][key], cohort['scripts']
    args = ['--on', machine, '--gpus=1']
    on = ['--on', machine]  # --warm invocations: --gpus is rejected there (warm_gpus)
    target = Path(cohort['packages']) / model['package'] / 'generate'
    script = scripts['comfy_' + key]
    records, pixels = [], set()
    server_birth = None
    try:
        for sample in samples:
            label = sample['label']
            if engine == 'comfy':
                run, _, timing = terminal(out, label, ['run', script, f'label={label}',
                    f'seed={sample["seed"]}', f'prompt={sample["prompt"]}',
                    f'require_warm={str(sample["mode"] != "cold").lower()}',
                    *args, '--out', out / 'images' / label], cohort=cohort, timed=True)
                receipt = result_json(run, 'session')
                assert receipt['qualified'] and receipt['request']['observed_steps'] == model['steps'], receipt
                assert receipt['request']['job']['seed'] == sample['seed']
                birth = receipt['server_pid'], receipt['server_start_ticks'], receipt['wrapper_pid']
                if sample['mode'] != 'cold':
                    assert receipt['server_reused'] and birth == server_birth, (birth, server_birth)
                server_birth = birth
                record = {'engine': engine, 'sample': sample, 'run_number': run['number'], 'run_id': run['id'],
                          'execution_ms_as_reported': run.get('execution_ms'),
                          'request_created_to_settled_ms': run['settled_at_ms'] - run['created_at_ms'],
                          'receipt': receipt, **timing}
                full_run(out, label, run)
            else:
                request = image_request(sample); save(out, label + '-request.json', request)
                run, events, timing = terminal(out, label, ['run', target, '--input', out / (label + '-request.json'),
                    f'model.model={model["reference"]}', '--lane', model['lane'], *args,
                    '--out', out / 'images' / label], cohort=cohort, timed=True)
                full = full_run(out, label, run)
                detail = cozy_bench.summarize(full, request, (1024, 1024), events, timing['cli_wall_ms'], 'denoise' if key == 'sdxl' else 'generating image')
                assert full['measurements']['software']['runtime'].endswith(cohort['runtime_source'][:8])
                assert any(m['manifest'] == model['checkpoint'] for m in detail['models'])
                record = {'engine': engine, 'sample': sample, **detail, **timing}
                save(out, label + '-execution-proof.json', record)
                if model.get('require_native_interval'):
                    assert detail['native_completed_steps'] == model['steps'] - 1, 'candidate did not execute the required native interval'
                    assert detail['observed_step_callback_count'] == 1
                if sample['mode'] == 'cold' and setup.get('eager_reference') is not None:
                    require_eager_parity(out, label, setup['eager_reference'], record)
            image_hash = record['images'][0]['rgb_sha256']
            assert image_hash not in pixels, 'fresh seeds produced an identical image; preserve and refuse this comparison'
            pixels.add(image_hash)
            records.append(record)
            save(out, engine + '-samples.json', records)
            if sample['mode'] == 'cold':
                warm = [*cozy_args, '--warm', 'gpu'] if engine == 'cozy' else ['run', script, *on, '--warm', 'gpu']
                run, _, timing = terminal(out, engine + '-explicit-warm', warm)
                setup[engine + '_explicit_warm'] = {'run': run['number'], **timing}
                save(out, 'setup.json', setup)
    finally:
        if engine == 'comfy':
            call(out, 'comfy-stop', ['run', script, 'operation=stop', 'label=timed-stop', *args], check=False)
        call(out, engine + '-unwarm', [*cozy_args, '--warm', 'off'] if engine == 'cozy' else ['run', script, *on, '--warm', 'off'], check=False)
    return records


def profile_model(cohort, out, machine, key):
    model, scripts = cohort['models'][key], cohort['scripts']
    args = ['--on', machine, '--gpus=1']
    warm = ['--on', machine]  # --warm invocations: --gpus is rejected there (warm_gpus)
    sample = next(s for s in reversed(cohort['samples']) if s['cell'] == key)
    base_seed = cohort['profile_seed'] + (100 if key == 'qwen21' else 0)
    assert not set(range(base_seed, base_seed + 4)) & {s['seed'] for s in cohort['samples']}
    bootstrap_request = image_request(sample); bootstrap_request['seed'] = base_seed
    request = {**bootstrap_request, 'seed': base_seed + 1}
    save(out, 'profile-bootstrap-request.json', bootstrap_request)
    save(out, 'profile-request.json', request)
    target = Path(cohort['packages']) / model['package'] / 'profile_generate'
    diagnostics = []

    def cozy_observed(label, run, events, timing, payload, profiled):
        full = full_run(out, label, run)
        result = full['result']
        detail = json.loads(result['detail'])
        assert detail['profile'] is profiled and result['image']['seed'] == payload['seed']
        assert result['image']['prompt'] == payload['prompt']
        assert (result['trace'] is not None) is profiled
        # The same entrypoint runs its unchanged image body for bootstrap and trace.
        # Retain full current-step and native-completion proof even though neither is
        # a latency benchmark sample; a returned old image cannot qualify as warmup.
        normalized = {**full, 'result': result['image']}
        observed = cozy_bench.summarize(normalized, payload, (1024, 1024), events,
            timing['cli_wall_ms'], 'denoise' if key == 'sdxl' else 'generating image')
        assert observed['software']['runtime'].endswith(cohort['runtime_source'][:8])
        assert observed['software']['tensord_commit'] == cohort['creator_source']
        assert any(row['manifest'] == model['checkpoint'] for row in observed['models'])
        if profiled:
            traces = [row for row in full['output'] if row.get('media_type') == 'application/json']
            assert len(traces) == 1, traces
            trace = traces[0]
            assert Path(trace['path']).stat().st_size > 0
            assert digest(trace['path']) == trace['sha256']
        return full, observed

    reset_owned(cohort, out, 'cozy-profile-reset', machine)
    sampler(cohort, out, 'cozy-profile-telemetry', machine)
    common = ['run', target, f'model.model={model["reference"]}', '--lane', model['lane'], *args]
    common_warm = ['run', target, f'model.model={model["reference"]}', '--lane', model['lane'], *warm]
    # --warm takes only model/lane choices, never request inputs: no --input, no profile=false here.
    run, _, warm_timing = terminal(out, 'cozy-profile-warm', [*common_warm, '--warm', 'gpu'])
    bootstrap, bootstrap_events, bootstrap_timing = terminal(out, 'cozy-profile-bootstrap',
        [*common, '--input', out / 'profile-bootstrap-request.json', 'profile=false', '--out', out / 'profiles/cozy-bootstrap'])
    bootstrap_full, bootstrap_proof = cozy_observed('cozy-profile-bootstrap', bootstrap,
        bootstrap_events, bootstrap_timing, bootstrap_request, False)
    save(out, 'cozy-profile-bootstrap-proof.json', bootstrap_proof)
    run, events, timing = terminal(out, 'cozy-profile',
        [*common, '--input', out / 'profile-request.json', 'profile=true', '--out', out / 'profiles/cozy'])
    full, profile_proof = cozy_observed('cozy-profile', run, events, timing, request, True)
    assert profile_proof['executor_pids'] == bootstrap_proof['executor_pids'], 'diagnostic executor was replaced after real bootstrap'
    assert profile_proof['models'] == bootstrap_proof['models']
    assert profile_proof['software'] == bootstrap_proof['software']
    before = bootstrap_full['measurements']['execution'].get('construction', {}).get('prepared_unix_ms')
    after = full['measurements']['execution'].get('construction', {}).get('prepared_unix_ms')
    construction_reused = None
    if before is not None and after is not None:
        assert before == after, 'diagnostic construction changed after real bootstrap'
        construction_reused = True
    assert min(row['start_us'] for row in profile_proof['gpu_intervals_as_reported']) >= max(row['end_us'] for row in bootstrap_proof['gpu_intervals_as_reported'])
    save(out, 'cozy-profile-proof.json', profile_proof)
    diagnostics.append({'engine': 'cozy', 'run': run['number'], 'timing_not_benchmark': timing,
                        'result': run['result'], 'outputs': run['output'], 'warm_preparation': warm_timing,
                        'bootstrap': {'run': bootstrap['number'], 'timing_not_benchmark': bootstrap_timing,
                                      'profile': False, 'seed': bootstrap_request['seed']},
                        'warm_proof': {'same_entrypoint': str(target), 'same_executor_pids': profile_proof['executor_pids'],
                                       'same_sources': profile_proof['models'], 'construction_reused': construction_reused,
                                       'bootstrap_mode': bootstrap_proof['mode'], 'profile_mode': profile_proof['mode']},
                        'scope': 'Same prompt, fresh seed, actual unprofiled bootstrap then CPU/CUDA diagnostic; all bootstrap/preparation/profiler costs excluded from benchmark medians.'})
    save(out, 'diagnostics.json', diagnostics)
    reset_owned(cohort, out, 'comfy-profile-reset', machine)
    sampler(cohort, out, 'comfy-profile-telemetry', machine)
    terminal(out, 'comfy-profile-install', ['run', scripts['profile_install'], '--on', machine])
    script = scripts['comfy_' + key]
    try:
        bootstrap, _, bootstrap_timing = terminal(out, 'comfy-profile-bootstrap', ['run', script,
            'label=profile-bootstrap', f'seed={base_seed+2}', f'prompt={request["prompt"]}',
            *args, '--out', out / 'profiles/comfy-bootstrap'])
        full_run(out, 'comfy-profile-bootstrap', bootstrap)
        bootstrap_receipt = result_json(bootstrap, 'session')
        assert bootstrap_receipt['qualified'] and bootstrap_receipt['request']['observed_steps'] == model['steps']
        assert bootstrap_receipt['request']['job']['seed'] == base_seed + 2
        assert bootstrap_receipt['request']['job']['prompt'] == request['prompt']
        assert bootstrap_receipt['request'].get('profile') is None
        cozy_bench.image_evidence(bootstrap, (1024, 1024))
        terminal(out, 'comfy-profile-hold', ['run', script, *warm, '--warm', 'gpu'])
        run, _, timing = terminal(out, 'comfy-profile', ['run', script, 'label=profile-diagnostic',
            f'seed={base_seed+3}', f'prompt={request["prompt"]}', 'require_warm=true', 'profile=true',
            *args, '--out', out / 'profiles/comfy'])
        full_run(out, 'comfy-profile', run)
        receipt = result_json(run, 'session')
        assert receipt['qualified'] and receipt['request']['profile']
        assert receipt['request']['observed_steps'] == model['steps']
        assert receipt['request']['job']['seed'] == base_seed + 3
        assert receipt['request']['job']['prompt'] == request['prompt']
        assert receipt['server_reused']
        for field in ('server_pid', 'server_start_ticks', 'wrapper_pid'):
            assert receipt[field] == bootstrap_receipt[field], ('diagnostic Comfy birth changed', field)
        cozy_bench.image_evidence(run, (1024, 1024))
        # The trace is the run's only application/json output now that the session receipt rides run['result'].
        traces = [row for row in run['output'] if row.get('media_type') == 'application/json']
        assert len(traces) == 1, traces
        assert digest(traces[0]['path']) == receipt['request']['profile']['sha256']
        diagnostics.append({'engine': 'comfy', 'run': run['number'], 'timing_not_benchmark': timing,
                            'receipt': receipt, 'outputs': run['output'],
                            'bootstrap': {'run': bootstrap['number'], 'timing_not_benchmark': bootstrap_timing,
                                          'profile': False, 'seed': base_seed + 2},
                            'warm_proof': {field: receipt[field] for field in ('server_pid', 'server_start_ticks', 'wrapper_pid')},
                            'scope': 'Same prompt, fresh seed, actual unprofiled bootstrap then CPU/CUDA diagnostic; all bootstrap/profiler costs excluded from benchmark medians.'})
        save(out, 'diagnostics.json', diagnostics)
    finally:
        call(out, 'comfy-profile-stop', ['run', script, 'operation=stop', 'label=profile-stop', *args], check=False)
    return diagnostics


def qualify(cohort, out, machine):
    global DEADLINE, HUB
    DEADLINE, HUB = cohort["deadline_epoch"], cohort["hub"]
    source_check(cohort)
    records, setup, diagnostics, failures, skipped = [], {}, [], [], []
    scripts = cohort['scripts']
    telemetry_started = False
    completed_models = []
    try:
        run, _, timing = terminal(out, 'comfy-setup', ['run', scripts['comfy_setup'], '--on', machine, '--gpus=1'])
        receipt = result_json(run, 'setup')
        setup['comfy'] = {'run': run['number'], **timing, 'receipt': receipt}
        assert receipt['ok'], receipt
        telemetry_started = True
        sampler(cohort, out, 'telemetry-start', machine)
        for model_index, key in enumerate(cohort['model_order']):
            order = ('cozy', 'comfy') if model_index % 2 == 0 else ('comfy', 'cozy')
            if time.time() > cohort['deadline_epoch'] - 900:
                skipped.append({'model': key, 'reason': 'less than 15 minutes remain for a full matching/model/sample group and cleanup'})
                break
            try:
                destination = out / key; destination.mkdir()
                samples = [s for s in cohort['samples'] if s['cell'] == key]
                reset_owned(cohort, destination, 'before-model', machine)
                model_setup, cozy_args = prepare_model(cohort, destination, machine, key, samples)
                setup[key] = model_setup
                sampler(cohort, destination, 'telemetry-model-start', machine)
                for engine in order:
                    reset_owned(cohort, destination, 'before-' + engine, machine)
                    sampler(cohort, destination, 'telemetry-' + engine, machine)
                    state, _, _ = terminal(destination, 'gpu-before-' + engine, ['run', scripts['gpu_state'], '--on', machine])
                    assert json.loads(state['result']['state'])['free'], state
                    own = [s for s in samples if s['engine'] == engine]
                    records.extend(measure_group(cohort, destination, machine, key, engine, own, model_setup, cozy_args))
                    save(out, 'samples.json', records)
                completed_models.append(key)
            except Exception as error:
                failures.append({'model': key, 'error': repr(error), 'when_unix_ns': time.time_ns()})
                save(out, 'failures.json', failures)
                break
        # The first window prioritizes a complete matched model and its profiles.
        # Diagnostic source/server hooks are installed only after all timed samples.
        for key in completed_models:
            if time.time() > cohort['deadline_epoch'] - 360:
                skipped.append({'profile': key, 'reason': 'less than six minutes remain for diagnostic preparation, traces and cleanup'})
                continue
            destination = out / (key + '-profiles'); destination.mkdir()
            diagnostics.extend(profile_model(cohort, destination, machine, key))
            save(out, 'diagnostics.json', diagnostics)
    except BaseException as error:
        failures.append({'error': repr(error), 'when_unix_ns': time.time_ns()})
        save(out, 'failures.json', failures)
        raise
    finally:
        if telemetry_started:
            try:
                run, _, timing = terminal(out, 'telemetry-stop', ['run', scripts['sampler_stop'], f'session={cohort["session"]}', '--on', machine, '--out', out / 'telemetry'])
                stop = result_json(run, 'stop')
                assert stop.get('gpu_csv') and stop.get('processes_csv'), stop
                assert stop.get('verified_birth_gone') is True, 'sampler stop did not verify process exit'
                setup['telemetry_stop'] = {'run': run['number'], 'complete': True, **timing}
            except Exception as error:
                failures.append({'telemetry_cleanup_error': repr(error)})
                setup['telemetry_stop'] = {'complete': False, 'error': repr(error)}
        for which in ('tensord', 'memory', 'tensorfs'):
            try: call(out, 'final-' + which, ['machine', 'logs', machine, which, '-n', '2500'], check=False)
            except Exception as error: failures.append({'log_error': repr(error)})
        save(out, 'setup.json', setup); save(out, 'diagnostics.json', diagnostics)
        save(out, 'failures.json', failures); save(out, 'skipped.json', skipped)
    summary = {}
    for key in completed_models:
        for engine in ('cozy', 'comfy'):
            for mode in ('cold', 'same-prompt-warm', 'changed-prompt-warm'):
                values = [r['caller_to_reopened_image_ms'] for r in records if r['sample']['cell'] == key and r['engine'] == engine and r['sample']['mode'] == mode]
                assert values
                summary[f'{key}:{engine}:{mode}'] = {'count': len(values), 'median_ms': statistics.median(values), 'min_ms': min(values), 'max_ms': max(values)}
    proof = {'valid_comparisons': bool(completed_models), 'models_completed': completed_models,
             'samples': records, 'summary': summary, 'setup': setup, 'diagnostics': diagnostics,
             'failures': failures, 'skipped': skipped,
             'scope': 'Matched common-source runs on one device; profile timings excluded; WebP/PNG encoding boundaries stay explicit.'}
    save(out, 'comparison.json', proof)
    return proof
