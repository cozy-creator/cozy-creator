"""Bytes per second on stdin, for up to 10 s from the first byte; 0 for too little to judge."""
import io
import sys
import time
from typing import cast

# Standard input is a BufferedReader: typeshed types sys.stdin.buffer as BinaryIO, which has no read1.
stdin = cast(io.BufferedReader, sys.stdin.buffer)
begun: float | None = None
n = 0
while chunk := stdin.read1(1 << 20):
    begun = begun or time.monotonic()
    n += len(chunk)
    if time.monotonic() - begun > 10:
        break
print(round(n / (time.monotonic() - begun)) if begun and n > 1 << 16 else 0)
