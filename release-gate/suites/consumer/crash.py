# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.45", "msgspec>=0.19"]
# ///
"""A deliberate native fault on a call thread (cr-275): ctypes reads address 1, libc's strlen faults and the executor
dies of SIGSEGV. Suite consumer holds the run's record to the Runtime's crash record."""
import ctypes

import msgspec

from cozy_runtime.author import App

app = App()


class Ask(msgspec.Struct):
    note: str = "crash"


class Never(msgspec.Struct):
    pass


@app.entrypoint(accelerator=False)
def crash(payload: Ask) -> Never:
    ctypes.string_at(1)
    return Never()
