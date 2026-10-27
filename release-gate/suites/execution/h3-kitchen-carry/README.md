# H3 Kitchen producer summaries under native segments

Component gate for packages #655: a production-width H3 DiT (hidden 5376, FFN 14336, 56×128 heads, rotary 96;
4 layers: blocks 0–1 dense paths, 2–3 Kitchen sites) on the real FL2VA 15 s geometry (362 frames, 1344×768,
111,334 live tokens, 30 steps, stereo audio). Weights are random but written, uploaded and loaded as a model, so
Runtime's fill, H3's own execution plan (glue, rotary hook) and attention selection (the Kitchen FIXED_MEANS
producer on SM120 at level 4) are the production ones. Not a video, quality or speed result.

`prepare.py <packages>/minimax-h3 <new dir>` copies `adaln_pruned/turbo/glue/producer/_prepared.py` unchanged
and records their SHA256, the commit, and official.py's dense/exact paths and `sol_dense_steps`.

`gpu mode=gate` runs eager twice (run-to-run control, per-step digests and summaries), then native from step 0
(S1), after one eager step (S2) and seeded at step 15 (S4); R15 makes a preparation refuse after its carry
attached (15 eager steps held) and checks the request state, Kitchen's table cache and the eager continuation. It passes when native's final video/audio and every
site's final F32 [H,128] summaries equal eager's (or, if eager itself is not reproducible, stay within 2× its own
spread), S1/S2 plan a bootstrap stage and S4 none, and the producer serves every sparse block.

Run it with `sessions/h3-kitchen-carry.sh` (rental, cohort update, checkpoint, warm-up, gate, model delete, end):

```sh
gh workflow run pod-session.yaml -R cozy-creator/cozy-creator --ref opus/h3-kitchen-carry-gate-20261027 \
  -f script=h3-kitchen-carry -f packages_refs=h3=<packages commit> -f sku=rtx-5090 -f disk=100 -f cap=2 \
  -f cozy=v0.13.0 -f rent=true
```

Without `rent=true` it stops after the $0 pre-flight (fixture build, sign-in, quote, guard).
