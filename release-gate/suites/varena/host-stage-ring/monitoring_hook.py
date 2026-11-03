"""Code-local interpreter events cover fresh C-created Python threads."""
import sys

from host_stage_ring import STATUS

TOOL = 3
NAME = 'xs138-shared-host-markers'


class InterpreterHook:
    def __init__(self, ring):
        self.ring = ring
        self.monitor = sys.monitoring
        e = self.monitor.events
        self.handlers = {
            e.PY_START: self.start, e.PY_RESUME: self.start, e.PY_THROW: self.throw,
            e.PY_RETURN: self.returned, e.PY_UNWIND: self.unwind, e.PY_YIELD: self.yielded,
            e.CALL: self.call, e.C_RETURN: self.c_return, e.C_RAISE: self.c_raise,
        }
        self.masks = {}
        self.active = False
        self.global_mask = e.PY_UNWIND | e.PY_THROW

    def install(self):
        import threading
        if sys.getprofile() is not None or threading.getprofile() is not None or self.monitor.get_tool(TOOL) is not None:
            raise RuntimeError('diagnostic interpreter/profile slot already owned')
        self.monitor.use_tool_id(TOOL, NAME)
        e = self.monitor.events
        base = e.PY_START | e.PY_RESUME | e.PY_RETURN | e.PY_YIELD
        c_events = e.CALL | e.C_RETURN | e.C_RAISE
        for event, callback in self.handlers.items():
            self.monitor.register_callback(TOOL, event, callback)
        for code, kind in self.ring.codes.items():
            mask = c_events if kind == 'clone_lambda' else base
            if kind in ('graph', 'static_input', 'tensor_probe'):
                mask |= c_events
            self.monitor.set_local_events(TOOL, code, mask)
            self.masks[code] = self.monitor.get_local_events(TOOL, code)
        self.monitor.set_events(TOOL, self.global_mask)
        self.active = True

    def owned(self):
        return self.active and self.monitor.get_tool(TOOL) == NAME and self.monitor.get_events(TOOL) == self.global_mask and all(
            self.monitor.get_local_events(TOOL, code) == mask for code, mask in self.masks.items()
        )

    def stop(self):
        if not self.owned():
            self.ring.uncertain = True
            return
        self.monitor.set_events(TOOL, 0)
        for code in self.masks:
            self.monitor.set_local_events(TOOL, code, 0)
        for event in self.handlers:
            self.monitor.register_callback(TOOL, event, None)
        self.monitor.free_tool_id(TOOL)
        self.active = False

    def visit(self, code, event, value, status):
        if code not in self.ring.codes:
            return
        try:
            frame = sys._getframe(2)
            if frame.f_code is not code:
                self.ring.uncertain = True
                return
            self.ring.observe(frame, event, value, status)
        except BaseException:
            self.ring.uncertain = True

    def start(self, code, offset):
        self.visit(code, 'call', None, STATUS['call'])

    def throw(self, code, offset, error):
        self.visit(code, 'call', None, STATUS['call'])

    def returned(self, code, offset, value):
        self.visit(code, 'return', value, STATUS['normal'])

    def unwind(self, code, offset, error):
        self.visit(code, 'return', None, STATUS['unwind'])

    def yielded(self, code, offset, value):
        self.visit(code, 'return', value, STATUS['yield'])

    def c_event(self, code, offset, callable, status):
        try:
            frame = sys._getframe(2)
            if frame.f_code is not code:
                self.ring.uncertain = True
                return
            kind = self.ring.codes[code]
            name = getattr(callable, '__name__', None)
            event = 'input_copy' if kind in ('graph', 'static_input', 'tensor_probe') and name == 'copy_' else 'output_clone' if kind in ('clone_lambda', 'tensor_probe') and name == 'clone' else None
            if event:
                event += '_enter' if status == STATUS['call'] else '_exit'
                self.ring._emit(event, self.ring._current(), status, offset)
        except BaseException:
            self.ring.uncertain = True

    def call(self, code, offset, callable, arg0):
        self.c_event(code, offset, callable, STATUS['call'])

    def c_return(self, code, offset, callable, arg0):
        self.c_event(code, offset, callable, STATUS['normal'])

    def c_raise(self, code, offset, callable, arg0):
        self.c_event(code, offset, callable, STATUS['unwind'])
