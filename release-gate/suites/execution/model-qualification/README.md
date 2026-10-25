# Real image-model qualification

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

## Anima preparation

Reviewed source is the `anima` subtree at packages commit
`da31a14a05e85707d5a3499dfca117dbfa35e1e7`, tree `c992749097f25d7ecb6a72138010b9965c6825fa`.
No package edits are needed for the selected Runtime 0.32.1/TensorFS 0.15.2 cohort. Published
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

Prepared only: no Anima GPU execution or machine SDK change was performed while writing these files.
