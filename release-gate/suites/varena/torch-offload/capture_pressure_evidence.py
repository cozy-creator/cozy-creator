"""Observe actual diagnostic-client returns; never replace preparation or inference."""

import dis
import os
import sys
import threading


class ReturnEvidence:
    def __init__(
        self,
        prepare_pool,
        phase_close,
        phase_stats,
        context,
        *,
        capture_begin,
        capture_complete,
        graph_scope,
        replay_scope,
    ):
        self._prepare = prepare_pool.__code__
        self._close = phase_close.__code__
        self._stats, self._context = phase_stats, context
        self._thread = threading.get_ident()
        self._callback = self._returned
        self._events, self._errors, self._keys = [], [], set()
        self._begin, self._complete = capture_begin.__code__, capture_complete.__code__
        self._graph = graph_scope.__wrapped__.__code__
        self._replay = replay_scope.__wrapped__.__code__
        self._returns = {
            code: {
                i.offset
                for i in dis.get_instructions(code)
                if i.opname in ("RETURN_VALUE", "RETURN_CONST")
            }
            for code in (self._complete, self._graph, self._replay)
        }
        self._yields = {
            code: {
                i.offset
                for i in dis.get_instructions(code)
                if i.opname == "YIELD_VALUE"
            }
            for code in (self._graph, self._replay)
        }
        self._captures, self._replays = set(), set()
        self._uncertain = self._running = self._stopped = False
        self._unavailable = {
            "format_version": 2,
            "available": False,
            "unavailable_reason": "capture_or_replay_completion_unconfirmed",
            "events": None,
            "stored_native_samples": None,
        }

    def install(self):
        if sys.getprofile() is not None:
            raise RuntimeError("diagnostic profile slot is already owned")
        sys.setprofile(self._callback)
        self._running = True

    def stop(self):
        if sys.getprofile() is self._callback:
            self._stopped = not (self._uncertain or self._captures or self._replays)
            sys.setprofile(None)
        else:
            self._uncertain = True
            self._errors.append("diagnostic profile ownership changed")
        self._running = False

    @property
    def collection_available(self):
        if self._running and sys.getprofile() is not self._callback:
            self._uncertain = True
        return (self._running or self._stopped) and not (
            self._uncertain or self._captures or self._replays
        )

    def report_unavailable(self):
        # Pre-serialized at import; do not dump buffered objects under uncertain capture.
        try:
            os.write(
                2,
                b'VARENA_RETURN_EVIDENCE_UNAVAILABLE {"available":false,"reason":"capture_or_replay_completion_unconfirmed"}\n',
            )
        except OSError:
            pass  # Diagnostic sink refusal must not replace execution failure.

    def _control(self, frame, event):
        code = frame.f_code
        if code is self._begin and event == "call":
            caller = frame.f_back
            if caller.f_code is not self._graph:
                self._uncertain = True
            else:
                self._captures.add(
                    (caller.f_locals["device"], tuple(caller.f_locals["pool"]))
                )
        elif code is self._complete and event == "return":
            # None also accompanies exception unwind: only RETURN_* is completion.
            if frame.f_lasti in self._returns[code]:
                self._captures.discard(
                    (frame.f_locals["index"], tuple(frame.f_locals["pool"]))
                )
        elif code is self._graph and event == "return":
            if frame.f_lasti not in self._yields[code] and self._captures:
                self._uncertain = True
        elif code is self._replay:
            if event == "call":
                self._replays.add(id(frame))
            elif event == "return" and frame.f_lasti not in self._yields[code]:
                self._replays.discard(id(frame))
                if frame.f_lasti not in self._returns[code]:
                    self._uncertain = True

    @staticmethod
    def _device(value):
        if type(value) in (int, str, type(None)):
            return value
        if type(value).__module__ == "torch" and type(value).__name__ == "device":
            return {"type": value.type, "index": value.index}
        raise TypeError("unrecordable preparation device")

    def _returned(self, frame, event, result):
        try:
            self._control(frame, event)
            if event != "return":
                return
            if frame.f_code is not self._prepare and frame.f_code is not self._close:
                return
            context = self._context()
            if context is None:
                return
            if not self.collection_available:
                return
            if context["stage"] == 3:
                self._uncertain = True
                return
            if len(self._events) >= 64:
                self._errors.append("return evidence overflow")
                return
            row = dict(context, thread=threading.get_ident())
            if frame.f_code is self._prepare:
                row.update(
                    kind="prepare_pool_return",
                    pool=tuple(frame.f_locals["pool"]),
                    requested_device=self._device(frame.f_locals["device"]),
                    requested_bytes=frame.f_locals["bytes"],
                    returned_capacity_bytes=result if type(result) is int else None,
                    returned_capacity_available=type(result) is int,
                    caller=frame.f_back.f_code.co_name,
                )
                # A profile return with None can be exception unwind. It is
                # never a successful zero-capacity native result.
            else:
                observation = dict(result) if type(result) is dict else None
                row.update(kind="phase_close_return", observation=observation)
                if observation is not None and observation["key"] is not None:
                    self._keys.add((context["device"], observation["key"]))
            self._events.append(row)
        except BaseException as error:  # noqa: BLE001 - preserve the original execution.
            # The diagnostic must not replace an authored/native exception.
            self._uncertain = True
            self._errors.append(type(error).__name__)

    def read(self, native_key=None, device=None):
        if threading.get_ident() != self._thread:
            raise RuntimeError("return evidence reader changed thread")
        if not self.collection_available:
            return self._unavailable
        context = self._context()
        if context is not None and context["stage"] == 3:
            self._uncertain = True
            return self._unavailable
        if native_key is not None:
            self._keys.add((device, native_key))
        samples = []
        for selected_device, key in sorted(self._keys):
            row = {"device": selected_device, "native_key": key}
            try:
                row["sample"] = self._stats(selected_device, key)
            except BaseException as error:  # noqa: BLE001 - preserve the original execution.
                row["sample_error"] = type(error).__name__
            samples.append(row)
        return {
            "format_version": 2,
            "available": True,
            "scope": "actual returns and confirmed SDK capture/replay exits on this profiled inference thread; not a cross-thread admission capability",
            "events": list(self._events),
            "stored_native_samples": samples,
            "errors": list(self._errors),
            "same_lock_fit_eligibility": {
                "available": False,
                "missing": [
                    "stream_usable_free_total",
                    "largest_usable_range",
                    "live_and_blocked_ranges_with_reasons",
                ],
                "snapshot_scope": "existing native allocator-exception snapshot is post-lock, not an atomic usable-fit report",
                "separate_native_report": "implemented in diagnostic307c9380 as VARENA_POOL_FIT_FAILURE_V1; production SDK257 remains separate",
            },
        }
