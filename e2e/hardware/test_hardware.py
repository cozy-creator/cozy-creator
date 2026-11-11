"""Isolation regressions for the Linux hardware harness; no Hub or provider calls."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('hardware_run', HERE / 'run.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class Isolation(unittest.TestCase):
    def test_environment_does_not_inherit_production_or_service_credentials(self):
        incoming = {'PATH': os.defpath, 'RUNPOD_API_KEY': 'production', 'COZY_HUB_URL': 'https://tensorhub.com',
                    'HTTPS_PROXY': 'https://tensorhub.com', 'AWS_SECRET_ACCESS_KEY': 'production',
                    'DBUS_SESSION_BUS_ADDRESS': 'unix:path=/run/user/1000/bus', 'HF_TOKEN': 'private'}
        with patch.dict(os.environ, incoming, clear=True):
            env = runner.private_environment(Path('/owned/home'), Path('/owned/bin'), 'GPU-owned')
        self.assertFalse(set(incoming).difference({'PATH'}).intersection(env))
        self.assertEqual(env['HOME'], '/owned/home')
        self.assertEqual(env['COZY_MACHINE_ROOT'], '/owned/home/.cozy/machine')
        self.assertEqual(env['COZY_TENSORFS_ROOT'], '/owned/home/tensorfs')

    def test_cleanup_ends_only_the_owned_home_process(self):
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

    def test_birth_change_is_never_reported_as_the_same_process(self):
        before = {'birth': '1', 'exe': '/owned/cozy', 'home': '/owned', 'state': 'S'}
        self.assertFalse(runner.same_process(before, before | {'birth': '2'}))
        self.assertTrue(runner.same_process(before, before | {'state': 'R'}))

    def test_contained_executor_home_is_owned_but_sibling_prefix_is_not(self):
        record = {'home': '/owned/job/.cozy/machine/executor-home', 'state': 'S'}
        self.assertTrue(runner.owned(record, Path('/owned/job')))
        self.assertFalse(runner.owned(record | {'home': '/owned/job-other'}, Path('/owned/job')))
        self.assertFalse(runner.owned(record | {'home': ''}, Path('/owned/job')))

    def test_completed_cli_calls_are_not_mistaken_for_restarted_user_services(self):
        record = {'home': '/person', 'exe': '/person/.cozy/bin/cozy'}
        snapshot = {1: record | {'args': ['cozy', 'calcifer']},
                    2: record | {'args': ['cozy', 'machine', 'serve']},
                    3: record | {'args': ['cozy', 'machine', 'rent', 'cpu']},
                    4: record | {'args': ['cozy', 'run', 'package/function']}}
        self.assertEqual(set(runner.existing_services(snapshot, '/person')), {1, 2})

    def test_provider_selection_fails_before_fixture_startup(self):
        result = subprocess.run([sys.executable, str(HERE / 'run.py'), '--hub-src', '/absent',
            '--cozy-bin', '/absent', '--runtime', '0.45.0', '--tensorfs', '0.24.0',
            '--device', 'GPU-absent', '--out', '/absent', '--mode', 'provider-sandbox'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn('provider lifecycle is unqualified', result.stderr)

    def test_numeric_mismatch_and_cpu_fallback_fail(self):
        with tempfile.TemporaryDirectory() as tmp:
            proof = Path(tmp)
            cpu = {'status': 'completed', 'result': {'result': 368, 'device': 'cpu'}}
            gpu = {'status': 'completed', 'result': {'result': 368, 'device': 'cuda:0'},
                   'measurements': {'execution': {'gpus': [{'uuid': 'GPU-owned'}]}}}
            (proof / 'cpu.json').write_text(json.dumps(cpu))
            def verify():
                (proof / 'gpu.json').write_text(json.dumps(gpu))
                return subprocess.run([sys.executable, str(HERE / 'verify.py'), tmp],
                    env=os.environ | {'CUDA_VISIBLE_DEVICES': 'GPU-owned'}, capture_output=True).returncode
            self.assertEqual(verify(), 0)
            gpu['result']['result'] = 367
            self.assertNotEqual(verify(), 0)
            gpu['result'] = {'result': 368, 'device': 'cpu'}
            self.assertNotEqual(verify(), 0)
            gpu['result']['device'] = 'cuda:0'
            gpu['measurements']['execution']['gpus'] = [{'uuid': 'GPU-other'}]
            self.assertNotEqual(verify(), 0)


if __name__ == '__main__':
    unittest.main()
