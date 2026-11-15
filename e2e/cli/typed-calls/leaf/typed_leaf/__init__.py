"""Legacy child with no cozy-calls extra: ordinary compute dependencies and a private self call."""

import sys
import msgspec
from sympy import isprime  # type: ignore[import-untyped]  # SymPy ships no typing metadata.
from xxhash import xxh64_hexdigest
from cozy_runtime.author import App, InvalidRequest, call

app = App()


class PrimeRequest(msgspec.Struct, kw_only=True):
    number: int = 17
    fail: bool = False


class PrimeResult(msgspec.Struct, kw_only=True):
    prime: bool
    python: str
    fingerprint: str


@app.entrypoint(accelerator=False, private=True)
def private(number: int = 17, fail: bool = False) -> PrimeResult:
    if fail:
        raise InvalidRequest(
            "the requested transitive child failure", code="typed_fixture_failure"
        )
    return PrimeResult(
        prime=bool(isprime(number)),
        python=sys.executable,
        fingerprint=xxh64_hexdigest(str(number).encode()),
    )


@app.entrypoint(accelerator=False)
async def judge(payload: PrimeRequest) -> PrimeResult:
    return await call(private, number=payload.number, fail=payload.fail)
