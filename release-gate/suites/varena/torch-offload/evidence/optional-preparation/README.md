# Optional native pool preparation: pre-body strategy boundary

_Preparation performs exactly one native vt.prepare_pool call with the supplied native
measured peak unchanged. It catches only typed torch.OutOfMemoryError at that call and
cancels the unconsumed preparation. Only then can can_choose_eager authorize one ordinary
eager body. No authored warm/capture/compute callback is inside the catch. Body-start
and one-shot guards precede admission; after body start or cleanup failure that strategy
capability is false. Other errors clean up/rethrow; cancellation failure retains exact
pool/device/failure state and cannot authorize fallback. No byte arithmetic/resizing/
retry loop or physical-byte release credit is introduced.

Private native error mapping was inspected at actual77178571c and eb315: raise() hashes
match9a907780. OutOfMemory/Exhausted map to torch.OutOfMemoryError; Unsupported falls into
ordinary untagged RuntimeError. It is therefore not safe to classify native Unsupported
for fallback yet. No broad RuntimeError handler/message parser or native API edit was
added. Known Python eligibility/capability refusals remain pre-body decisions.

Automatic eligibility is initial reviewed immutable RNG-free eval behavior only. The
real CPU unknown-random module is declined before extra execution, runs its original
cold body once, and has exactly the independent control Generator's once-advanced state.
Known reviewed eval Linear/Dropout/ReLU repeated calls preserve global CPU RNG state and
exact output. No RNG snapshot/reset/reseed conceals product warmup. Deterministic/warn
flags invalidate and restore existing keys; earlier alias/key controls stay green.

Actual checks: both CPU clients exit0 on an exact source-matching owned SDK overlay;
Ruff/format/syntax/AST/diff pass, and the new GPU fixture --help imports/parses exit0.
No fake native owners, manufactured capacity exception or mocked pool proves a GPU path.
GPU optional_pool_admission_gpu.py is PREPARED/UNRUN: a live input Working page plus its
native measured target makes a private pool fail under a native-facts cap while a tiny
eager output can reuse the same page. It must show no body before refusal, one eager
execution under the SAME cap, no post-body admission, then later exact target admission
with a separate owner after cap restoration. Test cap use is isolated fixture control,
not product byte policy. Native cleanup/refusal and full strategy switch require that
real run; no claim follows from source inspection or --help.

Dispatcher/private admitted-pool GraphSegment integration remains unwired. Auto must
consume the SAME privately admitted pool before warmup, marking body-start before each
authored warm/capture/compute entry; no fallback is allowed afterward. Explicit
GraphSegment's independent output/preparation contract is unchanged. No GPU/rental/
machine/native-policy/merge/release action occurred in this lane.
