# Run 27056 frontend diagnostic

Prepared diagnostic; GPU execution is unrun. The existing real red is run 27056
(CUDA719), Creator9d959e31 / Runtimec059, original client e7c4eed6,
6 GiB, 1024x1024, 20 steps, two calls.

This directory owns only a successor wrapper, scalar host profiler and binding
instructions. Native helper/header are owned by packing_review_resume20 in
`up0-replay-diagnostic`. Product offload code remains unchanged.

The frontend window starts at up_blocks.0 invocation2, retains native events
with ATen disabled between selections, traces invocation3, and ends after the
following automatic boundary returns or errors. Every return is a host return,
not GPU completion. No synchronization, body retry, launch blocking or numerical
change is introduced.

See ownership.json for separately labeled development and execution identities.
