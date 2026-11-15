# CPU caller of BiRefNet

The orchestrator imports only the generated request and `Operation` reference. BiRefNet
is a `cozy-calls` dependency; the caller performs no local model compute and does not need
its former direct Torch/Torchvision dependencies. The image grant, BiRefNet's default
model choice and the final `cut` result keep their existing meanings.

After publishing and locking the matching BiRefNet RC, regenerate from this directory:

```sh
cozy-runtime --dir . calls birefnet xs132_pipeline/birefnet_calls.py
```

The current declarations were generated using Runtime `87c0db669` from BiRefNet
`0.8.0rc1` at packages commit `fa7d5146`. The retained older lock is not qualification of
that RC; refresh it through the ordinary Cozy lock path when the matching dependency
and SDK artifacts are available. No local validation path belongs in a committed lock.
