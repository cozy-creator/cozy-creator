# Torch and Diffusers offload

```python
import tensorfs.varena.torch as vt
vt.install()                 # first: Torch now allocates through Varena
import torch, diffusers
pipe = diffusers.DiffusionPipeline.from_pretrained("stabilityai/stable-diffusion-xl-base-1.0", torch_dtype=torch.float16)
vt.offload(pipe, device=0)   # the modules keep the handle alive
image = pipe("a red fox in snow").images[0]
print(vt.stats(0))           # weights_resident, weights_filled, allocated_peak, event_waits
```

Works on any `nn.Module` or pipeline exposing `components`, with materialized CPU weights in eval mode.

## How it works

- **Weights are placeholders.** Adoption swaps each Parameter's contents for a metadata tensor, in place,
  so Parameter identity and ties survive. Regions fill from the original storage, which stays: there is no second host copy.
- **Blocks bind real tensors.** A unit is an outermost block (`_no_split_modules`, or a member of a repeated
  layer list), a block-free subtree between them, or a composite parent's own weights. Its forward takes a
  native Use, puts real views of the Region in the module slots and runs stock ATen with no Python per op.
- **Too big to pin:** a unit with children and over 512 MiB comes apart at adoption, since a unit is what eviction
  moves at once. When native refuses a unit's Regions, its children bind instead, recursively. A single tensor still must fit.
- **Reads outside a block** take the placeholder's slow path. Aliases (`view`, `detach`, `.t()`) stay
  managed; other ops return ordinary tensors. An alias that escapes a block owns its Pin until it dies.
- **One explicit stream.** An explicit caller stream is honored. A thread on the default stream moves to the
  handle's blocking stream on first use, after waiting for its work, and stays there, so
  `torch.cuda.current_stream()` changes. A thread that reads this thread's tensors without entering the
  model must wait on that stream.
- **`to`, `half`, `float`, `double`, `bfloat16`, `cuda`** all arrive at one `_apply`. A dtype change is a new
  immutable version, derived once from the original bytes and kept, so SDXL's VAE flips without copying.
  The device is fixed, and any other `_apply` is refused.

## Refused

- Writes to a weight: outside a block by dispatch, inside one by its version counter after the fact. A write
  through `.data` inside a block is not caught.
- Raw storage, data pointers, NumPy or DLPack on a placeholder: copy first.
- Gradients on weights and models with existing Accelerate or Diffusers hooks.
- Replacing a module's weights after adoption.

## Graphs

`graphs="eager"` turns them off. A module called with the same arguments three times in a row is a loop. Its next pass runs eager, as the request's
answer, under a check for what a replay would not repeat: randomness, host reads, writes to inputs or buffers, hooks, a
change to the modules' Python state. Then it is captured, and one replay is compared with that answer, bit for bit. Any doubt or failure
declines that key, and eager serves it.

All of a handle's graphs share one pool, replayed one at a time, and their tensors are scratch: inputs are copied in and
outputs cloned out. The pool is sized at 125% of native's measure of the eager pass, and native decides when its pages go.
