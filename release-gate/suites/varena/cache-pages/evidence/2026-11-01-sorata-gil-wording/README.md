# Preserved typed-capacity fixture correction25555

The corrected device constructor reached the actual held-GIL capacity refusal. The C
result recorded GIL held before and after, a failed real resize and callback marker0.
The fixture then failed its overly specific English substring assertion: the actual
message was CUDA capacity exhausted allocating16777216 bytes. This failed run is preserved;
it is not counted as the completed positive GIL/later-allocation proof.

The follow-up C gate uses PyErr_GivenExceptionMatches against the actual supplied
Torch OutOfMemoryError class and returns that result plus the actual exception type.
Its ABI marker prevents using the old result layout before any Cache pages are committed.
Allocation, cap, callback and later separate-call semantics are unchanged.
