"""One unchanged numerical execution under a sealed, host-only observer."""
import hashlib
import os
import runpy
import sys

from host_stage_ring import Ring, UNAVAILABLE, callback_proxy
from host_stage_seal import configure

E7C = 'e7c4eed6fc34a427355cf6fd7b7735bea16f4470b06866bc51bbfa27eed1c652'


def run_once(cohort, script, output, arguments, binding):
    if hashlib.sha256(script.read_bytes()).hexdigest() != E7C:
        raise RuntimeError('numerical e7c client identity changed')
    if output.exists():
        raise RuntimeError('refuse to overwrite host-stage receipt')
    output.parent.mkdir(parents=True, exist_ok=True)
    ring = Ring()
    cache_class = configure(ring, cohort)
    ring.metadata['binding'] = binding
    ring.metadata['python'] = sys.version
    ring.install()
    restore = callback_proxy(cache_class, ring)
    previous = sys.argv
    sys.argv = [str(script), *arguments]
    try:
        # Same original code, same args. No exception catch or body retry.
        runpy.run_path(str(script), run_name='__main__')
    finally:
        try:
            ring.stop()
            restore()
            sys.argv = previous
            ring.write(output)
        except BaseException:
            # Never replace the original body/cleanup failure with a sink error.
            try:
                os.write(2, UNAVAILABLE)
            except OSError:
                pass
