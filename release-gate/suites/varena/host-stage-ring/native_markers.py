"""Exact agreed diagnostic ABI. This module never queries or synchronizes CUDA."""
import ctypes as C
import hashlib
import json
import os

HEADER_SHA = '2551e703227f6bda7f9ecc10231217bac552097712d2fee6840becf2a16940ad'


class NativeTrace:
    def __init__(self, helper, header, expected_helper):
        if hashlib.sha256(header.read_bytes()).hexdigest() != HEADER_SHA:
            raise RuntimeError('shared marker header identity mismatch')
        if hashlib.sha256(helper.read_bytes()).hexdigest() != expected_helper:
            raise RuntimeError('shared marker helper identity mismatch')
        self.library = C.CDLL(str(helper), mode=os.RTLD_NOW | os.RTLD_LOCAL | os.RTLD_NODELETE)
        self._marker = self.library.allocation_trace_marker
        self._marker.argtypes = [C.c_uint32, C.c_uint32, C.c_int32, C.c_uint64,
                                 C.c_uint64, C.c_uint64, C.c_uint64, C.c_uint64]
        self._marker.restype = C.c_uint64
        self._stage = self.library.allocation_trace_stage
        self._stage.argtypes, self._stage.restype = [C.c_int], None
        self._window = self.library.allocation_trace_window
        self._window.argtypes, self._window.restype = [C.c_int], C.c_int
        self._install = self.library.allocation_trace_install
        self._install.argtypes, self._install.restype = [], C.c_int
        self._dump = self.library.allocation_trace_dump
        self._dump.argtypes, self._dump.restype = [], C.c_char_p
        self._error = self.library.allocation_trace_error
        self._error.argtypes, self._error.restype = [], C.c_char_p
        self._checkpoint = self.library.allocation_trace_checkpoint
        self._checkpoint.argtypes = [C.c_int, C.c_int, C.c_uint64, C.c_uint64]
        self._checkpoint.restype = C.c_int
        self.errors = []
        self._workspace = self.library.allocation_trace_workspace_cache_enabled
        self._workspace.argtypes, self._workspace.restype = [], C.c_int
        self.installed = False

    def workspace_cache(self):
        value = self._workspace()
        if value not in (0, 1):
            raise RuntimeError('latched workspace getter unavailable')
        return bool(value)

    def install_after_allocator(self):
        # Observed normal _bind_allocator return, before original CUDA init.
        if not self.installed:
            self.installed = self._install() == 0
        return self.installed

    def open_window(self, context):
        if self._window(1) != 0:
            raise RuntimeError('native trace window unavailable')
        self.stage(4)
        if self._checkpoint(1, context.get('device', -1), 0, 0) != 0:
            self.errors.append('initial native snapshot unavailable')

    def close_window(self):
        self.stage(0)
        if self._window(0) != 0:
            raise RuntimeError('native trace window close unavailable')

    def stage(self, value):
        self._stage(value)

    def marker(self, marker, context):
        return int(self._marker(
            marker, context.get('dispatch_attempt', 0), context.get('device', -1),
            context.get('stream', 0), context.get('segment_id', 0),
            context.get('plan_handle', 0), context.get('pool_a', 0), context.get('pool_b', 0)))

    def write_dump(self, path):
        # Caller must establish confirmed exclusion exit. Preserve v3 bytes/schema.
        self.close_window()
        raw = self._dump()
        if not raw:
            return False
        payload = json.loads(raw)
        if payload['format_version'] != 3:
            raise RuntimeError('native trace schema mismatch')
        with path.open('xb') as output:
            output.write(raw)
            output.write(b'\n')
        return True
