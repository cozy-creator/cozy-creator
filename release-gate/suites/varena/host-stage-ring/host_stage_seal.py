"""Exact source/code identity gates for the two explicitly selected cohorts."""
import hashlib
import json
import pathlib
import types


def code_tree(code):
    yield code
    for constant in code.co_consts:
        if isinstance(constant, types.CodeType):
            yield from code_tree(constant)


def verify_module(module, expected):
    path = pathlib.Path(module.__file__)
    raw = path.read_bytes()
    actual = hashlib.sha256(raw).hexdigest()
    if actual != expected:
        raise RuntimeError(f'host-stage source mismatch: {module.__name__}: {actual}')
    # Compile only. Never execute a substitute product or inject an owner.
    compiled = compile(raw, str(path), 'exec')
    return {code.co_qualname: code for code in code_tree(compiled)}


def bind(ring, compiled, function, kind):
    code = function.__code__
    expected = compiled.get(code.co_qualname)
    if expected is None:
        raise RuntimeError(f'host-stage function absent: {code.co_qualname}')
    # The active import can use a symlink spelling; compare with its own filename.
    if code != expected.replace(co_filename=code.co_filename):
        raise RuntimeError(f'host-stage original code changed: {code.co_qualname}')
    ring.bind(code, kind)


def configure(ring, cohort):
    from tensorfs.varena import _offload, _offload_auto, _offload_graph, _offload_stream, api
    from tensorfs.varena import torch as vt
    seals = json.loads(pathlib.Path(__file__).with_name('source_seals.json').read_text())
    selected = seals[cohort]
    import torch
    import torch.cuda.graphs as torch_graphs
    if torch.version.git_version != selected['torch_git']:
        raise RuntimeError('host-stage Torch source cohort mismatch')
    modules = (_offload, _offload_auto, _offload_graph, _offload_stream, api, vt)
    checked = {}
    for module in modules:
        suffix = module.__name__.removeprefix('tensorfs.').replace('.', '/') + '.py'
        checked[module.__name__] = verify_module(module, selected['files'][suffix])
    torch_compiled = verify_module(torch_graphs, selected['torch_graphs_sha256'])
    bind(ring, torch_compiled, torch.cuda.CUDAGraph.capture_begin, 'capture_begin')
    targets = (
        (_offload_auto, _offload_auto._AutomaticGraphs.__init__, 'controller_init'),
        (_offload_auto, _offload_auto._AutomaticGraphs.dispatch, 'dispatch'),
        (_offload_auto, _offload_auto._GraphCacheEntry.__init__, 'cache_init'),
        (_offload_auto, _offload_auto._GraphCacheEntry.facts, 'cache_facts'),
        (_offload_auto, _offload_auto._GraphCacheEntry._shrink, 'callback'),
        (_offload_graph, _offload_graph.GraphSegment.__init__, 'capture'),
        (_offload_graph, _offload_graph.GraphSegment.__call__, 'graph'),
        (_offload_graph, _offload_graph.GraphSegment._finish_epoch, 'tails'),
        (_offload_graph, _offload_graph.GraphSegment._close_definition, 'definition_close'),
        (_offload_graph, _offload_graph._Epoch.acquire, 'acquire'),
        (_offload_graph, _offload_graph._Epoch.release, 'release'),
        (_offload_graph, _offload_graph._Epoch.restore_intent, 'restore'),
        (api, api.Gpu.prepare_use, 'prepare_use'),
        (api, api.Use.wait_ready, 'wait_ready'),
        (vt, vt.replay, 'replay'),
        (vt, vt.phase_enter, 'phase'),
        (vt, vt.capture_complete, 'capture_complete'),
        (vt, vt.graph.__wrapped__, 'graph_scope'),
        (vt, vt.replay_scope.__wrapped__, 'replay_scope'),
        (_offload_stream, _offload_stream.Scope._finish, 'component_exit'),
    )
    for module, function, kind in targets:
        bind(ring, checked[module.__name__], function, kind)
    ring.metadata = dict(cohort=cohort, product_revision=selected['revision'],
                         source_files=selected['files'],
                         scope='one inference thread plus the fixture-attached native callback bodies',
                         callback_proxy=True, product_bodies_replaced=False,
                         native_reads_added=False, synchronization_added=False,
                         torch_version=torch.__version__, torch_git=torch.version.git_version)
    return _offload_auto._GraphCacheEntry
