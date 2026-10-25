# Real image-model qualification

[Full SDXL run 17882](evidence/sdxl-17882-native29.md) passed strict one-eager-plus-29-native
step evidence on an RTX 3090 with the recorded diagnostic cohort. This single image is
separate from the four-call comparison protocol below and does not resolve the earlier
8 GiB graph refusal.

`run.py` makes one first/cold call and three sequential warm calls through the default Cozy CLI.
It never installs a machine SDK, rents hardware or uses a private execution path. Run it only
after receiving the GPU lane. Four distinct fixed seeds prevent request memoization from
substituting cached outputs for warm computation. Every result must report the requested
denoising-step count and a real executor PID; warm calls must reuse that executor.

```sh
python3 run.py \
  --target /absolute/path/to/reviewed/package/generate \
  --input /absolute/path/to/request.json --machine MACHINE --lane int8 \
  --seeds 730200 730201 730202 730203 --steps 30 --expect-size 1024x1024 \
  --output /absolute/path/to/a-new-report-directory
```

The report preserves monotonic CLI subprocess wall time through output delivery/exit, recorded
request creation-to-settlement time, reported execution time, first/steady device-step timings,
executor initialization and construction fields, stage events, actual PIDs, model manifests,
kernel routes and graph counters. Each downloaded image must match its recorded file SHA256
and decode with Pillow at the requested size; RGB pixel hashes are recorded too.
The measured step stage defaults to `denoise`; Qwen uses `--step-stage 'generating image'`.

These timing scopes can overlap and are not added together or called GPU-only latency.
Download duration is left unknown when the backend does not report it separately. The first
call is not asserted to have an empty disk/CAS cache: before-machine inventory and raw source
read measurements remain available. Memory peaks are tracked process/native figures, not a
hard driver VRAM or whole-machine cap. Missing device timings, peaks or graph measurements
remain explicit gaps. A failed call stops the sequence; it is never rerun automatically.

Use different seeds across before/after arms as well: a completed-job cache hit has no new
invocation measurements and is rejected. Freshness uses worker invocation/GPU/step clocks and
disjoint sequential GPU intervals, not a controller-clock tolerance. `--resume` preserves the
old report, revalidates saved completed receipts, and submits only missing calls. It never
resubmits failed or unsettled work. Unsaved original CLI wall time remains unknown.

Logical completed reads exclude canceled speculation and are not physical disk/PCIe traffic.
Prefetch wait measures host settlement, not GPU starvation. Exposed weight-ready stalls and
isolated cold-weight latency remain explicitly unmeasured.

## Measured phase-policy comparison

[RTX3070 8GB: SDXL and Anima](evidence/phase-8gb.md) and
[RTX3090 24GB: Qwen Image2.1](evidence/phase-24gb.md) compare real 30-step runs on each same GPU,
with exact source/model/cohort identities and per-request evidence in the adjacent JSON files.
These are phase-residency comparisons, not CUDA-graph speedups or release qualification.

## Anima preparation

Reviewed source is the `anima` subtree at packages commit
`da31a14a05e85707d5a3499dfca117dbfa35e1e7`, tree `c992749097f25d7ecb6a72138010b9965c6825fa`.
That source is the before arm. The after arm uses packages `f7261695`, adding the explicit
text-encoder → text-conditioner → transformer → VAE plan without changing model math. Published
package 0.11.3 instead requires Runtime<0.32 and TensorFS<0.15; 0.12.0/0.12.1 are yanked.
Use the reviewed source directory rather than those incompatible published dependencies.

`anima-request.json` requests 1024², 30 steps, guidance 4.5, CFG interval 0.15–0.70 and first-block
cache disabled. Its model default is the published `anima@1.0.0`; live CLI metadata and exact
manifests are saved in `anima-preparation.json`:

| Lane | Stored bytes | GiB | RTX 3070/SM86 route evidence |
|---|---:|---:|---|
| bf16 | 5,642,859,713 | 5.2553 | Floating weights; activation/graph headroom still required |
| int8 | 3,445,246,394 | 3.2086 | `int8.convrot` native leaf supports SM80+; verify the actual selected route |
| fp8 | 3,691,835,152 | 3.4383 | Native rowwise FP8 requires SM89+; SM86 may decode to floating weights |

Int8 is the first 8 GiB qualification candidate. Stored bytes alone do not establish a fit;
BF16/all-resident graphs may need more headroom. If measured capacity prevents the requested
1024²/30-step run, preserve the failure and let the rental owner choose a larger card.

The package's optional fused plan currently admits Torch 2.13.0/2.14.0; the selected 2.14.1
cohort reports an explicit eager fallback. Its allowlist has not been widened. Anima still uses
the Diffusers Python denoising loop; component CUDA-graph counters are measured, and a successful
model render does not establish a whole-model autonomous GPU loop.

The linked receipts now contain completed baseline and phase-policy Anima runs.
