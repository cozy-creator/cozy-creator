# LoRA overlay refill

One executor serves Qwen-Image-Edit-2511's DiT (its int8 lane; 2512's root release is bf16 only)
and its Lightning LoRA, with no reference image, the LoRA armed by the DiT's
own forward pre-hook from that forward's `attention_kwargs`, as H3 arms its Turbo overlay. References
first, served per step (nothing baked, so no page can carry a stale overlay): the LoRA (the oracle)
and a base call; then, served baked (a new fill identity, so every page goes), the baked LoRA. Then two
rounds of base and LoRA warm with the card free (most of the other overlay's pages are resident when
each call begins), and two more from an emptied cache under a ballast that leaves the DiT `room_gib`
(6 GiB), so blocks are displaced and refilled within every call. An out-of-memory error reports the
allocation boundary (driver, Torch and weight-memory bytes, holds).

It fails when:

- the fresh baked LoRA reference baked fewer than every block's LoRA'd linears, or is not nearer the
  per-step LoRA than base is (a fill baked nothing);
- a warm call differs from its reference by more than 1e-3 (rel. L2 on the latents): a page filled
  under one overlay served the other;
- (Runtime 0.34+, from the request's fill trace) a warm call with the card free began with fewer than
  half the blocks resident (it could not show a stale page), one under the ballast did not refill
  within the call, or a call baked a different number of weights than the blocks it refilled from
  the store hold LoRA'd linears (a refill from kept baked bytes, Runtime 0.34.3+, bakes nothing).

It needs a card where the int8 DiT and the f32 LoRA stay resident (about 23 GB: RTX 5090 or larger).
On an RTX 4090 every baked block is refilled every forward, so no stale page can be served (runs
40449344821: 0.34.0 and the fix alike).

```sh
cozy run "$PWD/release-gate/suites/execution/lora-overlay-refill/gpu" seed:=321 \
  --gpus=1 --lane int8 --attention-kernel sdpa --on MACHINE --json
```

`cells.sh` adds it to the runtime suite. Expected on Runtime 0.33.2 and 0.34.0: FAIL (0.33.2: the
scope hold fills blocks before the forward arms, so the baked reference misses the LoRA; 0.34.0: warm
calls are served pages of the other overlay). With pages keyed by their fill overlay: PASS.

Why Qwen-Image covers H3: the same Runtime path end to end (`lora.Hook` per LoRA'd linear, `_Bakes`
baking the armed factors into the int8 lane's pages on fill, residency reusing or refilling those pages,
the overlay a separately loaded model whose scope holds its factors) and H3's arming contract (inside
the DiT forward, after the component scope opened). It is the smallest first-party DiT with a hook LoRA
(20B, int8 about 20 GB; FLUX.2 [dev] is 32B, H3 larger) and pages on one consumer card, as H3 does on 4090/5090. The
residency bug is per GPU, so H3's sequence-parallel ranks add nothing. The conditioning is seeded text
rows: the cell is about weights, not prompts.
