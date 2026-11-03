"""Path-independent fingerprints for explicitly reviewed Python implementations."""

from __future__ import annotations

import hashlib
import json
import types
from functools import lru_cache


def _constant(value):
    if isinstance(value, types.CodeType):
        return ("code", _code(value))
    if type(value) is bytes:
        return ("bytes", value.hex())
    if type(value) in (type(None), bool, int, float, str):
        return (type(value).__name__, value)
    if value is Ellipsis:
        return ("ellipsis",)
    if isinstance(value, type):
        # __class__ cells in super() wrappers. The registry separately requires
        # the actual imported class identity, not a class with the same name.
        return ("class", value.__module__, value.__qualname__)
    if type(value) is tuple:
        return ("tuple", tuple(_constant(item) for item in value))
    if type(value) is frozenset:
        return (
            "frozenset",
            tuple(sorted((_constant(item) for item in value), key=repr)),
        )
    raise TypeError("reviewed code contains an unsupported constant")


@lru_cache(maxsize=128)
def _code(code):
    # Interpreter patch/path/line metadata is excluded. Actual instructions,
    # constants, globals, arguments, flags and exception handling are included.
    return (
        code.co_code.hex(),
        code.co_exceptiontable.hex(),
        code.co_argcount,
        code.co_posonlyargcount,
        code.co_kwonlyargcount,
        code.co_flags,
        code.co_names,
        code.co_varnames,
        code.co_freevars,
        code.co_cellvars,
        tuple(_constant(value) for value in code.co_consts),
    )


def callable_fingerprint(function, _seen=None):
    """Inspect actual code/closures; never trust __wrapped__ as provenance."""
    function = getattr(function, "__func__", function)
    if not isinstance(function, types.FunctionType):
        raise TypeError("reviewed Python callable required")
    seen = set() if _seen is None else _seen
    if id(function) in seen:
        raise TypeError("recursive closure is not a reviewed implementation")
    seen.add(id(function))
    closure = []
    for cell in function.__closure__ or ():
        value = cell.cell_contents
        closure.append(
            ("function", callable_fingerprint(value, seen.copy()))
            if isinstance(value, types.FunctionType)
            else _constant(value)
        )
    payload = (
        function.__globals__.get("__name__"),
        _code(function.__code__),
        _constant(function.__defaults__),
        tuple(
            (key, _constant(value))
            for key, value in sorted((function.__kwdefaults__ or {}).items())
        ),
        tuple(closure),
    )
    return hashlib.sha256(
        json.dumps(payload, separators=(",", ":")).encode()
    ).hexdigest()
