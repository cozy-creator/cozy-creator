"""Actual host IO/C-thread/profile controls; no CUDA or simulated native owners."""
import argparse
import contextlib
import profile
import gc
import hashlib
import importlib
import json
import pathlib
import sys
import weakref

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from host_stage_ring import Ring, STATUS, FIELDS
from host_stage_seal import bind, verify_module


@contextlib.contextmanager
def file_scope(path):
    # Real file custody exercises generator return/unwind, not GPU completion.
    handle = path.open()
    try:
        yield handle
    finally:
        handle.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--torch-cpu', action='store_true')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    checks = {}
    path = args.output / 'actual-file.txt'
    path.write_text('actual host IO\n')
    ring = Ring(proof=True, armed=True)
    for function in (pathlib.Path.read_text, pathlib.Path.touch):
        ring.bind(function.__code__, 'probe')
    ring.install()
    path.touch()
    assert path.read_text() == 'actual host IO\n'
    try:
        path.with_name('absent.txt').read_text()
    except FileNotFoundError:
        pass
    else:
        raise AssertionError('actual IO refusal absent')
    ring.stop()
    statuses = ring.data.tolist()[18:ring.size * len(FIELDS):len(FIELDS)]
    assert STATUS['normal'] in statuses and STATUS['unwind'] in statuses
    assert ring.available and not ring.calls
    assert ring.write(args.output / 'io-ring.json')
    checks['real_io_none_return_and_exception_unwind'] = True

    source = pathlib.Path(pathlib.__file__).read_bytes()
    compiled = verify_module(pathlib, hashlib.sha256(source).hexdigest())
    bind(Ring(proof=True, armed=True), compiled, pathlib.Path.read_text, 'probe')
    try:
        verify_module(pathlib, '0' * 64)
    except RuntimeError:
        pass
    else:
        raise AssertionError('wrong source seal accepted')
    checks['actual_stdlib_source_and_code_identity'] = True

    # Actual foreign C-thread body, not a proxy or simulated native owner.
    import _testcapi
    ring = Ring(proof=True, armed=True)
    ring.bind(pathlib.Path.exists.__code__, 'probe')
    ring.install()
    assert sys.getprofile() is None
    _testcapi.call_in_temporary_c_thread(path.exists)
    ring.stop()
    assert ring.size == 2 and ring.available
    assert ring.data[2] != ring.main_thread
    assert ring.write(args.output / 'foreign-thread-ring.json')
    checks['unwrapped_foreign_c_thread_original_once'] = True

    for fail in (False, True):
        ring = Ring(proof=True, armed=True)
        ring.bind(file_scope.__wrapped__.__code__, 'replay_scope')
        ring.install()
        try:
            with file_scope(path) as handle:
                assert handle.read() == 'actual host IO\n'
                assert not ring.available
                if fail:
                    path.with_name('absent.txt').read_text()
        except FileNotFoundError:
            assert fail
        ring.stop()
        assert ring.available is (not fail)
    checks['real_file_generator_normal_exit_vs_unwind_conservative'] = True

    ring = Ring(capacity=1, proof=True, armed=True)
    ring.bind(pathlib.Path.read_text.__code__, 'probe')
    ring.install()
    assert path.read_text() == 'actual host IO\n'
    ring.stop()
    assert ring.lost and not ring.available
    assert ring.write(args.output / 'overflow-must-not-exist.json') is False
    assert not (args.output / 'overflow-must-not-exist.json').exists()
    checks['overflow_reports_unavailable_without_changing_io'] = True

    ring = Ring(proof=True, armed=True)
    ring.bind(pathlib.Path.read_text.__code__, 'probe')
    ring.install()
    assert ring.lock.acquire(blocking=False)
    try:
        assert path.read_text() == 'actual host IO\n'
    finally:
        ring.lock.release()
    ring.stop()
    assert ring.lost and not ring.available
    checks['collector_lock_declines_nonblocking'] = True

    def occupied_slot():
        try:
            Ring(proof=True, armed=True).install()
        except RuntimeError:
            return
        raise AssertionError('foreign sysprofile slot was replaced')
    profile.Profile().runcall(occupied_slot)
    checks['actual_profile_slot_not_replaced'] = True
    monitor = sys.monitoring
    monitor.use_tool_id(3, 'actual-existing-tool')
    try:
        try:
            Ring().install()
        except RuntimeError:
            pass
        else:
            raise AssertionError('existing interpreter tool was replaced')
    finally:
        monitor.free_tool_id(3)
    checks['actual_monitoring_tool_not_replaced'] = True

    torch_receipt = None
    if args.torch_cpu:
        torch = importlib.import_module('torch')
        assert not torch.cuda.is_initialized()
        model = torch.nn.Linear(3, 2).eval()
        value = torch.ones(2, 3)
        ring = Ring(proof=True, armed=True)
        ring.bind(torch.nn.Linear.forward.__code__, 'probe')
        ring.install()
        with torch.no_grad():
            result = model(value)
        ring.stop()
        assert ring.available and ring.size == 2
        ring.graph_identity(model)
        from safe_bindings import tensor_ranges
        binding = tensor_ranges({'actual_cpu_input': value, 'actual_cpu_output': result})
        assert binding[0]['address'] == value.data_ptr() and binding[1]['address'] == result.data_ptr()
        assert all(type(row['address']) is int for row in binding)
        checks['actual_cpu_address_bindings_scalar_only'] = True
        refs = [weakref.ref(v) for v in (model, value, result)]
        storages = [value.untyped_storage(), result.untyped_storage()]
        tokens = [storage._weak_ref() for storage in storages]
        del storages, model, value, result
        gc.collect()
        try:
            assert all(ref() is None for ref in refs)
            assert all(torch.UntypedStorage._new_with_weak_ptr(token) is None for token in tokens)
        finally:
            for token in tokens:
                torch.UntypedStorage._free_weak_ref(token)
        assert not torch.cuda.is_initialized()
        checks['real_cpu_linear_tensor_and_c10_storage_not_retained'] = True
        assert not ring.graph_tokens
        checks['weak_graph_identity_does_not_retain_actual_module'] = True
        class TensorIO(torch.nn.Module):
            def forward(self, target, source):
                target.copy_(source)
                return target.clone()
        operations = TensorIO()
        original = TensorIO.forward.__code__
        ring = Ring(proof=True, armed=True)
        ring.bind(original, 'tensor_probe')
        ring.install()
        output = operations(torch.zeros(2, 3), torch.ones(2, 3))
        try:
            operations(torch.zeros(2, 3), torch.ones(4, 3))
        except RuntimeError:
            pass
        else:
            raise AssertionError('actual tensor shape refusal missing')
        ring.stop()
        assert TensorIO.forward.__code__ is original and torch.equal(output, torch.ones(2, 3))
        assert ring.available and ring.write(args.output / 'tensor-operations-ring.json')
        events = [row['event'] for row in [dict(zip(FIELDS, ring.data[i:i+len(FIELDS)])) for i in range(0, ring.size*len(FIELDS), len(FIELDS))]]
        from host_stage_ring import EVENTS
        assert EVENTS['input_copy_enter'] in events and EVENTS['input_copy_exit'] in events
        assert EVENTS['output_clone_enter'] in events and EVENTS['output_clone_exit'] in events
        assert STATUS['unwind'] in ring.data.tolist()[18:ring.size*len(FIELDS):len(FIELDS)]
        checks['actual_unwrapped_tensor_copy_clone_and_natural_error'] = True
        torch_receipt = {'version': torch.__version__, 'git': torch.version.git_version,
                         'cuda_initialized': torch.cuda.is_initialized(),
                         'scope': 'CPU reference lifetime only; no SDK/native/GPU ownership proof'}
    result = {'passed': True, 'python': sys.version, 'checks': checks,
              'torch': torch_receipt, 'gpu_execution': False,
              'native_capture_or_cache_owner_qualification': False}
    (args.output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
