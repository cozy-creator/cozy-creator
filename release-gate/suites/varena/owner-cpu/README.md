# Automatic-owner CPU startup proof

`hello.py` launches the actual Runtime executor entrypoint and exchanges Hello
and Shutdown over the real length-framed local stream socket. The supplied
TensorFS extension/Python overlay is private task evidence, not an installed SDK.
It verifies the cooperative capability, exact child PID, no CUDA/NVML driver
mapping, no CUDA initialization, and no extra live thread after the handshake.

The broader CPU metadata proof remains in `../ledger-cpu`: `nonblocking.py`
uses an actual stopped OFD publication-lock holder and proves deferral plus
best-effort owner Drop while that owner's PID remains alive. Its row is reaped
only after the independent row lock is observed free, and no GPU memory credit
is invented. `prove.py` covers the full row/fork/publication/death matrix.

The native example's concurrent fork/open test explicitly reaps after exact
child waits before asserting the one-live-row invariant. A read-only snapshot
can retain departed metadata after best-effort Drop; it is not a cleanup call.
No GPU execution, automatic donation or full Start behavior is claimed by these
CPU checks.
