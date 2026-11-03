"""Observe actual diagnostic-client returns; never replace preparation or inference."""

import sys
import threading


class ReturnEvidence:
    def __init__(self, prepare_pool, phase_close, phase_stats, context):
        self._prepare = prepare_pool.__code__
        self._close = phase_close.__code__
        self._stats, self._context = phase_stats, context
        self._thread = threading.get_ident()
        self._callback = self._returned
        self._events, self._errors, self._keys = [], [], set()

    def install(self):
        if sys.getprofile() is not None:
            raise RuntimeError("diagnostic profile slot is already owned")
        sys.setprofile(self._callback)

    def stop(self):
        if sys.getprofile() is self._callback:
            sys.setprofile(None)
        else:
            self._errors.append("diagnostic profile ownership changed")

    @staticmethod
    def _device(value):
        if type(value) in (int, str, type(None)):
            return value
        if type(value).__module__ == "torch" and type(value).__name__ == "device":
            return {"type": value.type, "index": value.index}
        raise TypeError("unrecordable preparation device")

    def _returned(self, frame, event, result):
        if event != "return":
            return
        if frame.f_code is not self._prepare and frame.f_code is not self._close:
            return
        try:
            context = self._context()
            if context is None:
                return
            if context["stage"] == 3:
                self._errors.append("return metadata omitted during active capture")
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
            self._errors.append(type(error).__name__)

    def read(self, native_key=None, device=None):
        if threading.get_ident() != self._thread:
            raise RuntimeError("return evidence reader changed thread")
        context = self._context()
        if context is not None and context["stage"] == 3:
            raise RuntimeError("return evidence must be read outside capture")
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
            "format_version": 1,
            "scope": "actual Python returns on the selected inference host thread; metadata queried/serialized outside capture",
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
            },
        }
