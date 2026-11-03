"""Eager component stream scopes; no native Pin, graph or byte-policy ownership."""
from __future__ import annotations

import contextvars
import threading
from contextlib import contextmanager

import torch
from torch import nn
from torch.utils._pytree import tree_flatten

_ACTIVE = contextvars.ContextVar("varena_component_streams", default=())
_CAPTURE = contextvars.ContextVar("varena_component_capture", default=False)


def _error(message):
    from ._offload import OffloadError
    return OffloadError(message)


def _key(stream):
    return None if stream is None else (stream.device, stream.cuda_stream)


def _frame(owner):
    thread = threading.get_ident()
    return next((frame for frame in reversed(_ACTIVE.get())
                 if frame.owner is owner and frame.thread == thread and not frame.closed), None)


def _tensors(values, device):
    from ._offload import _Weight
    found = []
    for value in values:
        if not isinstance(value, torch.Tensor) or isinstance(value, _Weight):
            continue
        if type(value) not in (torch.Tensor, nn.Parameter):
            raise _error("component tensors need an ordinary Tensor adapter")
        if value.device.type != "cuda":
            continue
        if value.device != device:
            raise _error("cross-device component tensors need an explicit copy")
        found.append(value)
    return tuple(found)


def _outputs(result, device, values):
    if values is None:
        values = tree_flatten(result)[0]
        scalars = (type(None), bool, int, float, complex, str, bytes, torch.dtype, torch.device)
        if any(not isinstance(value, torch.Tensor) and type(value) not in scalars for value in values):
            raise _error("opaque component outputs require explicit tensor leaves")
    return _tensors(values, device)


class Completion:
    """Private checked stream-tail proof; `can_train` alone never certifies completion."""
    def __init__(self, device, event=None, *, changed=False):
        self.device = device
        self.stream_changed = changed
        self.can_train = event is not None and not changed
        self.checked = False
        self._event = event
        self._failure = None

    def _check(self):
        if self._failure is not None:
            raise _error("failed component has no successful completion witness") from self._failure

    def query(self):
        self._check()
        self.checked = self._event is None or self._event.query()
        return self.checked

    def wait(self):
        self._check()
        if self._event is not None:
            self._event.synchronize()
        if not self.query():
            raise _error("component completion event remains pending after wait")

    def handoff(self, stream, outputs, *, values=None):
        """Order a later explicit consumer and record its actual output/alias lifetime."""
        self._check()
        if self._event is None:
            if stream is not None:
                raise _error("host completion cannot order a CUDA consumer")
            return outputs
        if stream.device != self.device:
            raise _error("component completion cannot cross devices")
        tensors = _outputs(outputs, self.device, values)
        stream.wait_event(self._event)
        for tensor in tensors:
            tensor.record_stream(stream)
        return outputs


class _Frame:
    def __init__(self, owner, stream, *, changed=False):
        self.owner, self.stream = owner, stream
        self.thread = threading.get_ident()
        self.streams = {_key(stream): stream}
        self.changed, self.closed = changed, False

    def observe(self, stream):
        self.streams[_key(stream)] = stream
        self.changed |= _key(stream) != _key(self.stream)

    def join(self):
        self.observe(torch.cuda.current_stream(self.owner.device))
        # Actual changes are never redirected. Join only the observed tails at a
        # checkpoint/return boundary; unknown side work still requires caller joins.
        for stream in self.streams.values():
            if _key(stream) != _key(self.stream):
                self.stream.wait_stream(stream)

    def witness(self):
        if self.stream is None:
            return Completion(self.owner.device)
        self.join()
        event = torch.cuda.Event(enable_timing=False)
        event.record(self.stream)
        return Completion(self.owner.device, event, changed=self.changed)


def reuse(owner, current_stream):
    """Reuse only this owner/thread's actual current selected stream.

    Call after existing validation, including capture refusal. Keep all existing
    ordinary-input record_stream and materialized-weight Pin/fence handling.
    A changed stream returns False for the old safe per-operation path and marks
    the outer scope plus nested descendants ineligible for automatic training.
    """
    if _CAPTURE.get():
        return False
    frame = _frame(owner)
    if frame is None:
        return False
    frame.observe(current_stream)
    return _key(current_stream) == _key(frame.stream)


class Scope:
    def __init__(self, owner, stream, frame, inputs, *, reused=False, capture=False):
        self.stream, self.reused, self.capture = stream, reused, capture
        self.completion = None
        self._owner, self._frame = owner, frame
        self._inputs, self._outputs = inputs, ()
        self._checkpoints = []
        self._closed = False

    def output(self, result, *, values=None):
        """Return the identical tree; opaque method outputs supply exact `values`."""
        if self._closed:
            raise _error("component outputs must be supplied before scope exit")
        if not self.capture and self.stream is not None:
            self._outputs = _outputs(result, self._owner.device, values)
        return result

    def checkpoint(self):
        """Cold measurement only: a checked-tail witness while this scope stays open.

        No output handoff or scope close occurs. Check `can_train`, then wait or
        obtain a positive query before ending a native cold observation. Capture
        bindings return None. Warm paths need not call this or allocate an event.
        """
        if self.capture:
            return None
        if self._closed:
            raise _error("component checkpoint requires an open scope")
        proof = self._frame.witness()
        self._checkpoints.append(proof)
        return proof

    def _failed(self, error):
        for proof in self._checkpoints:
            proof._failure = error
            proof.can_train = False
        if self.completion is not None:
            self.completion._failure = error
            self.completion.can_train = False

    def _finish(self, caller, error, *, outer):
        try:
            if self.stream is None:
                self.completion = self._frame.witness()
                return
            for tensor in self._inputs:
                tensor.record_stream(self.stream)
            if outer and _key(caller) != _key(self.stream):
                self.completion = self._frame.witness()
                caller.wait_event(self.completion._event)
            else:
                # Same-stream/nested steady execution needs no completion event.
                self._frame.join()
            for tensor in self._outputs:
                tensor.record_stream(caller)
        except BaseException as failure:
            self._failed(failure)
            raise
        finally:
            if error is not None:
                self._failed(error)
            self._closed = True
            self._inputs = self._outputs = ()
            self._owner = self._frame = None


@contextmanager
def component_scope(owner, values=(), *, capture_bindings=None):
    """One outer eager stream context/handoff; nested same-stream contexts reuse it."""
    if capture_bindings is not None:
        token = _CAPTURE.set(True)
        scope = Scope(owner, capture_bindings.stream, None, (), capture=True)
        try:
            yield scope
        finally:
            scope._closed = True
            _CAPTURE.reset(token)
        return
    with owner.lock:
        if torch.compiler.is_compiling() or torch.jit.is_tracing():
            raise _error("component stream scope supports eager inference")
        caller = torch.cuda.current_stream(owner.device) if owner.stream is not None else None
        frame = _frame(owner)
        reused = frame is not None and _key(caller) == _key(frame.stream)
        inputs = _tensors(values, owner.device) if caller is not None else ()
        if frame is not None:
            frame.observe(caller)
        stream = caller if reused else (
            owner.stream if caller is not None and caller.cuda_stream in (0, 1) else caller
        )
        if reused:
            scope = Scope(owner, stream, frame, inputs, reused=True)
            error = None
            try:
                yield scope
            except BaseException as caught:
                error = caught
                raise
            finally:
                scope._finish(caller, error, outer=False)
            return
        frame = _Frame(owner, stream, changed=frame.changed if frame is not None else False)
        scope = Scope(owner, stream, frame, inputs)
        token = _ACTIVE.set((*_ACTIVE.get(), frame))
        error = None
        try:
            if stream is None:
                yield scope
            else:
                if torch.cuda.is_current_stream_capturing():
                    raise _error("eager component scope requires capture bindings during capture")
                with torch.cuda.stream(stream):
                    if torch.cuda.is_current_stream_capturing():
                        raise _error("eager component scope cannot enter a capturing stream")
                    if _key(stream) != _key(caller):
                        stream.wait_stream(caller)
                    try:
                        yield scope
                    except BaseException as caught:
                        error = caught
                        raise
                    finally:
                        scope._finish(caller, error, outer=True)
        except BaseException as caught:
            error = caught
            scope._failed(caught)
            raise
        finally:
            if stream is None:
                scope._finish(caller, error, outer=True)
            elif not scope._closed:
                scope._closed = True
                scope._inputs = scope._outputs = ()
            frame.closed = True
            _ACTIVE.reset(token)
