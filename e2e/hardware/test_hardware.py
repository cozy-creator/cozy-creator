"""Isolation regressions for the Linux hardware harness; no Hub or provider calls."""
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
from zipfile import ZipFile

import run as runner
import operator_sdk as sdk_operator
from hardware_types import (
    ArithmeticResult, InstalledCohort, Ownership, ProcessRecord, ProcessSnapshot,
    RunRecord, SDKVerified, Software, read_run,
)

HERE = Path(__file__).resolve().parent


def probe_result(device: str) -> ArithmeticResult:
    return {'result': 368, 'device': device}


def write_sdk_result(proof: Path, software: Software, digest: str) -> SDKVerified:
    directory = proof / 'operator-sdk'
    directory.mkdir(exist_ok=True)
    record: SDKVerified = {'status': 'passed', 'gpu_uuid': 'GPU-owned', 'sdk_version': software['tensorfs'],
              'runtime_version': software['runtime'], 'wheel_sha256': digest,
              'cpu': {'device': 'cpu', 'products': [8., 18.]},
              'gpu': {'device': 'cuda:0', 'products': [8., 18.]},
              'native_payloads': {'extension': {'path': '/owned/tensorfs/_ext.abi3.so', 'wheel_entry': 'tensorfs/_ext.abi3.so',
                                                'sha256': 'd' * 64, 'wheel_sha256': 'd' * 64},
                                  'torch_shim': {'path': '/owned/tensorfs/shim.so', 'wheel_entry': 'tensorfs/shim.so',
                                                'sha256': 'e' * 64, 'wheel_sha256': 'e' * 64}}}
    (directory / 'verified.json').write_text(json.dumps(record))
    return record


def process_record(home: str = '/person', args: list[str] | None = None) -> ProcessRecord:
    return {'birth': '1', 'state': 'S', 'exe': home + '/.cozy/bin/cozy', 'home': home,
            'args': ['cozy', 'calcifer'] if args is None else args}


class Isolation(unittest.TestCase):
    def test_operator_compares_loaded_native_bytes_with_actual_wheel_member(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            payload, archive = root / '_ext.abi3.so', root / 'candidate.whl'
            payload.write_bytes(b'the exact native payload')
            with ZipFile(archive, 'w') as wheel:
                wheel.writestr('tensorfs/_ext.abi3.so', payload.read_bytes())
            with ZipFile(archive) as wheel:
                matched = sdk_operator.wheel_payload(wheel, payload)
                self.assertEqual(matched['sha256'], matched['wheel_sha256'])
                payload.write_bytes(b'other native payload with the same package version')
                with self.assertRaisesRegex(ValueError, 'does not match'):
                    sdk_operator.wheel_payload(wheel, payload)
    def test_candidate_url_becomes_verified_owned_local_wheel_for_cli(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            payload = b'the exact candidate wheel payload'
            digest = hashlib.sha256(payload).hexdigest()
            url = 'https://example.invalid/tensorfs-0.24.2-cp311-abi3-linux_x86_64.whl#sha256=' + digest
            with patch.object(runner, 'build_opener') as opener:
                opener.return_value.open.return_value = io.BytesIO(payload)
                local, recorded = runner.materialize_component(url, Path(temporary))
            self.assertTrue(Path(local).is_file(), 'cozy update accepts a local wheel, not this HTTPS URL')
            self.assertEqual(Path(local).read_bytes(), payload)
            self.assertTrue(Path(local).is_relative_to(Path(temporary)))
            self.assertEqual(recorded['input'], url)
            self.assertEqual(recorded['sha256'], digest)

    def test_candidate_download_rejects_another_payload_before_cli(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            url = 'https://example.invalid/tensorfs-0.24.2-cp311-abi3-linux_x86_64.whl#sha256=' + 'a' * 64
            with patch.object(runner, 'build_opener') as opener:
                opener.return_value.open.return_value = io.BytesIO(b'wrong candidate')
                with self.assertRaisesRegex(ValueError, 'sha256 mismatch'):
                    runner.materialize_component(url, Path(temporary))
            self.assertEqual(list(Path(temporary).glob('*.whl')), [])

    def test_main_exit_does_not_wait_for_orphan_stdout_eof(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            pidfile = root / 'sleeper.pid'
            program = ('import pathlib,subprocess,sys; '
                       'child=subprocess.Popen([sys.executable,"-c","import time; time.sleep(3)"]); '
                       f'pathlib.Path({str(pidfile)!r}).write_text(str(child.pid)); '
                       'print("fixture verdict FAIL",flush=True); raise SystemExit(3)')
            sleeper = None
            try:
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    status = runner.run_logged([sys.executable, '-c', program],
                        os.environ | {'HOME': temporary}, root / 'console.log')
                self.assertEqual(status, 3)
                self.assertIn('fixture verdict FAIL', output.getvalue())
                sleeper = int(pidfile.read_text())
                record = runner.processes().get(sleeper)
                self.assertTrue(record is not None and runner.owned(record, root),
                                'main completion incorrectly waited for its background sleeper to close stdout')
            finally:
                if pidfile.exists():
                    sleeper = int(pidfile.read_text())
                    record = runner.processes().get(sleeper)
                    if record and runner.owned(record, root):
                        os.kill(sleeper, 15)

    def test_failing_fixture_verdict_is_live_and_retained(self) -> None:
        with tempfile.TemporaryDirectory() as temporary, contextlib.redirect_stdout(io.StringIO()) as console:
            path = Path(temporary) / 'console.log'
            status = runner.run_logged([sys.executable, '-c', 'print("fixture verdict FAIL"); raise SystemExit(3)'],
                                       os.environ.copy(), path)
            self.assertEqual(status, 3)
            self.assertIn('fixture verdict FAIL', path.read_text())
            self.assertIn('fixture verdict FAIL', console.getvalue())

    def test_readonly_owned_tree_removes_without_touching_linked_public_cache(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            owned, cache = root / 'owned', root / 'public-cache'
            locked = owned / 'sandbox'
            locked.mkdir(parents=True)
            cache.mkdir()
            (cache / 'wheel').write_text('public immutable dependency')
            (locked / 'README.md').write_text('sandbox package')
            (locked / 'README.md').chmod(0o444)
            (owned / 'cache').symlink_to(cache, target_is_directory=True)
            locked.chmod(0o555)
            cache.chmod(0o555)
            try:
                runner.remove_owned_tree(owned)
                self.assertFalse(owned.exists())
                self.assertEqual((cache / 'wheel').read_text(), 'public immutable dependency')
                self.assertEqual(cache.stat().st_mode & 0o777, 0o555)
            finally:
                if locked.exists():
                    locked.chmod(0o700)
                cache.chmod(0o700)

    def test_environment_does_not_inherit_production_or_service_credentials(self) -> None:
        incoming = {'PATH': os.defpath, 'RUNPOD_API_KEY': 'production', 'COZY_HUB_URL': 'https://tensorhub.com',
                    'HTTPS_PROXY': 'https://tensorhub.com', 'AWS_SECRET_ACCESS_KEY': 'production',
                    'DBUS_SESSION_BUS_ADDRESS': 'unix:path=/run/user/1000/bus', 'HF_TOKEN': 'private'}
        with patch.dict(os.environ, incoming, clear=True):
            env = runner.private_environment(Path('/owned/home'), Path('/owned/bin'), 'GPU-owned')
        self.assertFalse(set(incoming).difference({'PATH'}).intersection(env))
        self.assertEqual(env['HOME'], '/owned/home')
        self.assertEqual(env['COZY_MACHINE_ROOT'], '/owned/home/.cozy/machine')
        self.assertEqual(env['COZY_TENSORFS_ROOT'], '/owned/home/tensorfs')

    def test_cleanup_ends_only_the_owned_home_process(self) -> None:
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            child = subprocess.Popen(['sleep', '60'], env={'HOME': first, 'PATH': os.defpath})
            sibling = subprocess.Popen(['sleep', '60'], env={'HOME': second, 'PATH': os.defpath})
            try:
                time.sleep(.05)
                with patch.object(runner.subprocess, 'check_output', return_value=''):
                    receipt = runner.cleanup(Path(first), '/unused/docker', 'test-owned')
                child.wait(timeout=2)
                self.assertIsNone(sibling.poll())
                self.assertIn(child.pid, receipt['processes'])
                self.assertNotIn(sibling.pid, receipt['processes'])
                self.assertEqual(receipt['remaining_pids'], [])
            finally:
                for process in (child, sibling):
                    if process.poll() is None:
                        process.terminate()
                    process.wait(timeout=2)

    def test_birth_change_is_never_reported_as_the_same_process(self) -> None:
        before = process_record('/owned')
        replacement = before.copy()
        replacement['birth'] = '2'
        self.assertFalse(runner.same_process(before, replacement))
        running = before.copy()
        running['state'] = 'R'
        self.assertTrue(runner.same_process(before, running))

    def test_contained_executor_home_is_owned_but_sibling_prefix_is_not(self) -> None:
        record: Ownership = {'home': '/owned/job/.cozy/machine/executor-home', 'state': 'S'}
        self.assertTrue(runner.owned(record, Path('/owned/job')))
        sibling = record.copy()
        sibling['home'] = '/owned/job-other'
        self.assertFalse(runner.owned(sibling, Path('/owned/job')))
        sibling['home'] = ''
        self.assertFalse(runner.owned(sibling, Path('/owned/job')))

    def test_completed_cli_calls_are_not_mistaken_for_restarted_user_services(self) -> None:
        snapshot: ProcessSnapshot = {1: process_record(args=['cozy', 'calcifer']),
                    2: process_record(args=['cozy', 'machine', 'serve']),
                    3: process_record(args=['cozy', 'machine', 'rent', 'cpu']),
                    4: process_record(args=['cozy', 'run', 'package/function'])}
        self.assertEqual(set(runner.existing_services(snapshot, '/person')), {1, 2})

    def test_provider_selection_fails_before_fixture_startup(self) -> None:
        result = subprocess.run([sys.executable, str(HERE / 'run.py'), '--hub-src', '/absent',
            '--cozy-bin', '/absent', '--runtime', '0.45.0', '--tensorfs', '0.24.0',
            '--device', 'GPU-absent', '--out', '/absent', '--mode', 'provider-sandbox'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn('provider lifecycle is unqualified', result.stderr)

    def test_numeric_mismatch_and_cpu_fallback_fail(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            proof = Path(tmp)
            software: Software = {'runtime': '0.45.0', 'tensorfs': '0.24.1'}
            cpu: RunRecord = {'status': 'completed', 'result': probe_result('cpu'), 'measurements': {'software': software}}
            gpu: RunRecord = {'status': 'completed', 'result': probe_result('cuda:0'),
                   'measurements': {'software': software, 'execution': {'gpus': [{'uuid': 'GPU-owned'}]}}}
            (proof / 'cpu.json').write_text(json.dumps(cpu))
            installed: InstalledCohort = {'runtime': software['runtime'], 'tensorfs': software['tensorfs'], 'tensorfs_sha256': 'a' * 64}
            (proof / 'candidate-update.ndjson').write_text(json.dumps({'type': 'run.completed', 'run': {'result': {'to': installed}}}) + '\n')
            sdk = write_sdk_result(proof, software, 'a' * 64)
            def verify() -> int:
                (proof / 'gpu.json').write_text(json.dumps(gpu))
                return subprocess.run([sys.executable, str(HERE / 'verify.py'), tmp, '0.45.0', '0.24.1'],
                    env=os.environ | {'CUDA_VISIBLE_DEVICES': 'GPU-owned'}, capture_output=True).returncode
            self.assertEqual(verify(), 0)
            gpu['result']['result'] = 367
            self.assertNotEqual(verify(), 0)
            gpu['result'] = probe_result('cpu')
            self.assertNotEqual(verify(), 0)
            gpu['result'] = probe_result('cuda:0')
            gpu['measurements']['execution']['gpus'] = [{'uuid': 'GPU-other'}]
            self.assertNotEqual(verify(), 0)
            gpu['measurements']['execution']['gpus'] = [{'uuid': 'GPU-owned'}]
            sdk['gpu']['products'] = [8., 19.]
            (proof / 'operator-sdk/verified.json').write_text(json.dumps(sdk))
            self.assertNotEqual(verify(), 0)

    def test_gpu_proof_rejects_old_sdk_even_when_numeric_result_matches(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            proof = Path(temporary)
            software: Software = {'runtime': '0.45.0', 'tensorfs': '0.24.0'}
            for name in ('cpu', 'gpu'):
                record: RunRecord = {'status': 'completed', 'result': probe_result('cuda:0' if name == 'gpu' else 'cpu'),
                          'measurements': {'software': software, 'execution': {'gpus': [{'uuid': 'GPU-owned'}]}}}
                (proof / f'{name}.json').write_text(json.dumps(record))
            installed: InstalledCohort = {'runtime': '0.45.0', 'tensorfs': '0.24.0', 'tensorfs_sha256': 'b' * 64}
            (proof / 'candidate-update.ndjson').write_text(json.dumps({'type': 'run.completed', 'run': {'result': {'to': installed}}}) + '\n')
            candidate = 'https://example.invalid/tensorfs-0.24.1-cp311-abi3-linux_x86_64.whl#sha256=' + 'a' * 64
            result = subprocess.run([sys.executable, str(HERE / 'verify.py'), temporary, '0.45.0', candidate],
                env=os.environ | {'CUDA_VISIBLE_DEVICES': 'GPU-owned'}, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            # Matching labels cannot hide another wheel payload with the same version.
            installed['tensorfs'] = '0.24.1'
            (proof / 'candidate-update.ndjson').write_text(json.dumps({'type': 'run.completed', 'run': {'result': {'to': installed}}}) + '\n')
            for name in ('cpu', 'gpu'):
                path = proof / f'{name}.json'
                record = read_run(path)
                record['measurements']['software']['tensorfs'] = '0.24.1'
                path.write_text(json.dumps(record))
            result = subprocess.run([sys.executable, str(HERE / 'verify.py'), temporary, '0.45.0', candidate],
                env=os.environ | {'CUDA_VISIBLE_DEVICES': 'GPU-owned'}, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            installed['tensorfs_sha256'] = 'a' * 64
            (proof / 'candidate-update.ndjson').write_text(json.dumps({'type': 'run.completed', 'run': {'result': {'to': installed}}}) + '\n')
            write_sdk_result(proof, {'runtime': '0.45.0', 'tensorfs': '0.24.1'}, 'a' * 64)
            result = subprocess.run([sys.executable, str(HERE / 'verify.py'), temporary, '0.45.0', candidate],
                env=os.environ | {'CUDA_VISIBLE_DEVICES': 'GPU-owned'}, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == '__main__':
    unittest.main()
