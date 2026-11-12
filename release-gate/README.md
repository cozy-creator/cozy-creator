# Release gate

A PR's own proof is not here: what a CPU runner can show (projects, Calcifer's HTTP surface, version skew, restarts,
local code) is a cell of the e2e cli suite, which the merge queue runs on every PR it selects (`e2e/README.md`, "A PR's
proof is a cell"). Cells that need a GPU or a signed-in account are this gate's.

Release testing is manual: the release agent picks the suites a release needs (`--suite`, no default), runs them, and
the promotion names the passing run (`verify.sh`). Every cell is a real run held against the last passed gate on the
same SKU and suites; any FAIL blocks the promotion. Design: tracker `design/ci-v2.md` §3.

## Suites

| Suite | Proves | Rentals (`skus`) | Cost · time | Pick it for |
|---|---|---|---|---|
| `runtime` | every first-party package's default call (H3 15 s with a described reference, SeedVR2, SDXL, Klein, Qwen, Anima, BiRefNet, the Qwen3.5 describer, Fibo, HiDream, MOSS, Qwen-Image and its one-release swap), then SDXL in a second executor, SDXL cold then two warm calls after each of three restarts and a loop step no capture records (cr-227), six SDXL child calls each with its own LoRA stack (cl-687) and an SDXL int8 quantize (a weights output), and Regenerate-2K over the whole 15 s shot at 1440p, the largest one forward takes (cr-259): kernels, times, VRAM, quality, LM coverage | rtx-4090, rtx-5090 | ~$6.5 · ~95 min, in parallel | a Runtime, TensorFS, varena or TensorD change; any rewrite |
| `packages` | the packages `--pins` (or `--only`) names, plus the SDXL smoke | rtx-4090, rtx-5090 | ~$1–3 · 15–30 min | a package bump |
| `varena` | the memory manager: cold load, co-residency, reload, 8/6/3 GiB caps, a real 8 GB card, host RAM, no backing left without a page (below), a warm call served by graphs, real models repeating byte for byte warm, capped, with memory unmapped and perturbed, and with a kept graph's weights gone or no room for them, and with graphs off (`differential/`), the memory authority's rules (`core`), `vt.offload` beside stock SDXL (`offload-*`) and deliberate faults over SSH (`fault-*`, `faults`); with it, run the `vram-yield` session: a game started next to cozy has its memory within 2 s, idle and mid-call, and the call completes (Sessions) | its `skus`; the session one rtx-3070 | session ~$0.5 · ~45 min | a varena, memory, TensorD or Runtime memory change |
| `tensord` | the machine agent through `cozy`: a package generation installed, updated and rolled back, the Runtime back and forward; a run queued, canceled queued and running, its outputs landed whole; models stacked until the describer must fit (idle executors end); a new executor after a restart; a restart mid-run never re-executes; an API-warmed `gpu` member kept and its executor reused; two runs at once on two GPUs; output landing against this computer's download speed; distinct LoRA'd calls add nothing to the disk (cl-683) | rtx-5060-ti ×2 | ~$0.60 · ~60 min | a TensorD or cozy change |
| `tensorfs` | downloads as fast as the last pass (Hub and datacenter cache), nothing from the Hub again, resume after a cut (a private model no cache holds: from the Hub); uploads of new weights, dedup of a re-upload, resume after a cut; a recipe that encodes each component in its own call writes every component; a straggler re-asked, not waited out; a LoRA's scale never taken from training metadata; a source that is no form of its family refused before it downloads (below) | cpu | ~$0.3 · ~40 min | a TensorFS, download, upload or quantize change |
| `hub` | the Hub's contract, the account and one `cpu` pod per provider on production (`hub=prod`); publication in the isolated fixture | cpu, plus one `cpu` pod per provider | ~$0.4 · ~40 min | every Hub deploy |
| `skew` | candidate Hub in a disposable Postgres/MinIO fixture with current and supported released clients; package/model publication, install and cleanup | none | $0 · ~15 min | every Hub release |
| `h3` | minimax-h3's generate, edit and upscale at their edges: Regenerate-2K at 1080p, 1440p and 2160p against the 768p shot it starts from (resolution, A/V sync, PSNR/SSIM, `frames.py`), SeedVR2 at 2160p, a synthetic plate edited (`edit`) as long as the shot, shorter, longer, at 30 fps and at 9:16 (frame-by-frame follow, coherence, length, canvas), first and last keyframes (`frames.py still`), and three reference-then-text-only shot switches whose executor the host rule ends as the next call starts, each placed within 30 s (cl-697), an upscale from a shot's own latents (#762: strength 0.7, no video encode) and `quality: draft` against a `standard` run of the same request made just before it (#751: one dense step, denoise ≤ 0.90×: with #764's cheaper int4-QK dense steps the floor is near 0.85× even with no disk reads, and se-091's A/B with the residency fix measured 0.875×, runs 25377/25406), a take picked up from its latents for 5 s more (`pickup`: its head kept, the join smooth), the full model at 30 steps, two community fine-tunes republished complete, one named by its old release that resolves forward (`h3-finetune-*`, xs-183), and h3-shot-moves-on. Prompts in MiniMax's own form used as written, each cut where the text says (`h3-own-shots`, `h3-own-sections`). `regen-2160p-15s` (three windows on 24 GB, ~1 h) only by `--only` | rtx-4090 | ~$2.5 · ~85 min | a minimax-h3 release |
| `disk` | a 100 GB machine past a full disk (tfs-140, cl-709, tatara): three packages installed and updated, `machine show`'s disk by kind; a CPU SDXL call never evicted while H3 int8 and Qwen land; H3, Qwen, SDXL, more of H3's lanes until the store evicts and H3 again all land, each eviction logged | cpu (`--disk 100`) | ~$0.3 · ~45 min | a TensorFS store, reclaim or install change |
| `project` | `cozy project render` on fixture projects (the walk in this computer's Calcifer): a shot of two beats in one call, its upscale from latents and the 1080p edit, a new take keeping a take's start (`--take`, `--in`), a plate, a voice, stills at 16:9 and 9:16, a node rendered with local code (`with: ./still.py/draw`), and refusals before any run | rtx-4090 | ~$0.7 · ~30 min | a project, Calcifer or shot-function change |
| `escaped` | every first-party package's cells at a 6 GiB cap: no model keeps more than 256 MiB across its steps as plain tensors outside the memory manager, or grows more than 16 MiB a step (`check.py`: varena's escaped record; cross-step state goes through `author.retained`); a cell that failed or was refused memory runs again uncapped on the same pod (`<cell>-uncapped`): one that fails capped and completes uncapped sizes its work from the card, and the capped peak and the kernels must match | rtx-4090 | ~$3 · ~90 min, plus each twin | a new package or model, a package bump, a Runtime memory change |
| `consumer` | real consumer cards under their own memory pressure: on the 12 GB card SDXL 1024 int8 cold, then four warm calls, all complete (cozy-runtime #1956); on the 8 GB card H3 15 s as its first call, no weights step, so the 52 GB land during it (cr-275: kushina's executor died of SIGSEGV there); on both a deliberate SIGSEGV whose run fails `executor_crashed` naming its frame, with the native frames in its detail and the core kept | rtx-3060, rtx-3060-ti | ~$1 · ~60 min | a Runtime, varena or TensorD change |
| `inputs` | each input that grows working memory at the most its field takes, per package with a text path (cr-279): P-H3-Image as run 46479 (an edit whose upsampler's decode capture ran short), then text-to-image and an 8-picture edit at 16 MP with a 65,536-character prompt, upsampler on and off; Klein, Qwen-Image 2, FLUX.2 and Qwen-Image with 10 pictures at their largest size (Qwen-Image's prompt shorter: se-119); Fibo 2 with 4; Ideogram 4; the describer with 16 pictures and a 16,384-token answer; H3 15 s with the longest prompt its inline segment calls carry (cr-281). On a card over 20 GB the LM-heavy cells run again at an 8 GiB cap, on seeds of their own (a memo would answer them). Every cell completes | rtx-5060-ti, rtx-5090 | ~$4 · ~2.5 h, in parallel | a Runtime, varena or memory change; a package's new input or limit |
| `families` | the packages that send attention masks (qwen-image-2 at its prefill, ideogram-4 on every call) on the families whose kernels serve none: each completes, a refused call runs on SDPA and the record counts it (runs 26242-26244) | h100-sxm5-80gb, b200 | ~$5–8 · 30–40 min | a Runtime change to attention, kernels or selection |
| `multi-gpu` | H3 over two GPUs, 5 s Turbo, new prompt and seed, cold then warm, and a chosen fine-tune fetched from the Hub during the call (its token reaches GPU 1, cl-935): two ranks on two GPUs, a 768p video with audio, frames judged numerically (`frames.py coherent`), Sol's sparse steps sparse (none dense on the warm call) and a peer-capable pair's exchange on the copy engine (cr-268, xs-182). And a 26 s shot in two segments, the second carrying the first's frames, each segment's steps over its rows squared within 1.12× segment 1's (`h3-carried.sh`, cr-288: 1.18 on Runtime 0.45.1). A group opens more kernels than one GPU does, so each family that takes its own group path gets a pod | one card per family, each its cheapest with two GPUs: sm120, sm90, older mma.sync (`skus`; today's cheapest: `suites/multi-gpu/pick.sh`, `-f skus=<card> -f gpus=2`). No sm100 card lists two | ~$2 · 3 pods, 2 to 25 min of render a family, the longest when its card downloads H3's weights cold (0.39.6: 2× H100 2 min, 2× RTX 3090 10 min, 2× RTX 5090 25 min), plus up to 20 min to get a pod. A dispatch's `cap_minutes` under 60 can cancel it before the verdict, which runs the fault and probe checks | every release |
| `kernels` | every kernel the Runtime ships, once, on each family it claims, through its real launch path, against a slow reference (`suites/kernels/README.md`); a new kernel with no exerciser fails | one card per family (`suites/kernels/pick.sh`) | ~$1.2 + ~$2 for B200 · ~10 min a card | **every promotion** (Paul 11-04) |
| `attention` | the Runtime's attention table (xs-171): every attention kernel the card runs, timed at ~35 shapes of our models' real lengths and judged against FP32 attention, for `collect.py` and the Runtime's `scripts/attention_table.py` (`suites/attention/README.md`) | one reference card per hardware class (`skus`) | ~$0.8–9 a card · ~40 min | a kernel, recipe pin, worker image or attention-module change that `scripts/attention_table.py check` flags |
| `attention-corpus` | real Q/K/V for the attention suite (xs-171): Klein, Qwen-Image 2.1, MOSS and H3 at their bench requests, each with a census of every attention call and captures of chosen sites and calls (`suites/attention-corpus/README.md`) | rtx-4090 | ~$1.5 · ~1 h | a model's attention inputs change |
| `gemm` | each lane's GEMM rule against every candidate at our packages' shapes (xs-177): bf16 cuBLAS/cuBLASLt, int8/fp8 prebuilt tiles, cuBLASLt, comfy-kitchen, nvfp4 W4A4/W4A16, w4a8 tiles; a rule over 10% slower than the best (time-weighted per model) or a tile with other bits fails (`check.py`); the host is calibrated first and refused when far below its card (`suites/gemm/README.md`) | rtx-4060-ti, rtx-4090, rtx-5090, h100-sxm5-80gb (`skus`) | ~$0.1 a consumer card, ~$0.3 an H100 · ~10 min a card | **every Runtime promotion**, beside `kernels`; a kernel, torch, CUDA or GEMM-rule change |
| `progress` | every first-party package's default call, cold on a fresh machine (no API warming before it) and then warm, each held to its event stream (cl-921, `suites/progress/check.py`): every phase in order (placed, rented, queued, installed, weights downloaded, started, loads, compiles, stages and steps, decode, encode, save, the result coming here), the machine from the moment it runs and the GPU by its end, bytes for every download and load, what each count counts in words the page knows (`web/app/src/panels/runs/stages.json`; never steps for tokens or a cap), a time left while bytes move and from a stage's second step (a warm run from its start), and nothing still for over a minute. HunyuanImage 3.0 only by `--only`, on 48 GB or more | rtx-5090 (500 GB) | ~$5 · ~2 h | a change to what a run reports or how the page says it: Runtime progress, TensorD phases, Calcifer's events or time left, `words.ts`/`stages.json` |
| `execution` | every proof that reports `passed` and belongs to no other suite (`suites/execution/cells.sh`): the native loop, kernel readiness, capture, prompt LMs decoding after another model with no capture refused (`prompt-lm`: P-H3-Image's upsampled text-to-image then edit, SDXL, then Klein and P-H3-Image upsampled; xs-182), conditional bodies, LoRA overlays, child calls, TensorFS's producers and decoders, varena's stock-memory mechanisms, SDXL's qualification; a false `passed` fails its cell | rtx-5090, rtx-4090 | ~$2 each · 1–2 h | every promotion, alongside `kernels` |
| `slow` | a run is never silently slow (cl-913): a GPU machine refuses the pure Runtime wheel and a source checkout, naming the dev-build; a Runtime whose kernels are gone says so on every run and in `machine show`; a hot host's recorded GPU readings (rabou's, e2e/cli/slow) warn `gpu_throttled` and `slow_run` as the run goes and mark the machine | rtx-3070 | ~$0.2 · ~30 min | a TensorD, Calcifer or Runtime change to warnings, kernels or GPU readings |
| `cli` | (building) | | | a CLI change |

Every gate also holds each cell to the package contract (`suites/contract/check.py`, no runs of its own): a cell fails on a `ContractViolation`, an out-of-memory failure in a package's `split` (its `holds` is short), an `oom_swallowed` or `contract_thread` warning, or plain-tensor state left in a run that declares `author.retained`.

A rewrite picks every suite. Suites stack on one rental per SKU (`--suite runtime,packages` runs each cell once,
against `gate-<SKU>-runtime-packages.json`). A suite is `suites/<name>/`: `cells.sh` (sourced in gate.sh: `cell`,
`pkg`, `product`, `expect`, `log`, `$CLI`, `$M`, `$R`, `$REF`, the scenario, and e2e/run.sh's `run_*`, the one
way a suite runs `cozy` and reads a run), optional `check.py` (`check(gate) -> (fails, notes)`, listed in analyze.py;
`gate['runs']` holds each completed cell as `runs.py`'s summary of its record), and `skus` (`<sku> [disk GB] [gpus]` per line, the workflow's rentals). The verdict's `SUITES` line names what ran.

## Before renting

`release-gate/interfaces.sh --runtime <wheel file|version> [--tensorfs <wheel file|version>] <dir>...` (each a package or a
checkout of them) reads each package's interface back through the reader an executor starts on, as TensorD writes it, with the TensorFS the Runtime imports
(its own requirement from PyPI unless named): no GPU, no pod, seconds. Describing alone misses what only that reader
refuses, as with minimax-h3 on Runtime 0.40.0. A cut runs it before anything else.

`release-gate/suites/attention/table.sh <wheel file> <version>` checks the attention table a candidate Runtime ships against
the source it was stamped with (xs-171: regenerated byte for byte, every listed kernel the build that was measured): CPU,
seconds. A cut runs it beside `interfaces.sh`; the attention suite runs it as its first cell.

## What a gate does

Local-package (`--packages`) gates build a small source-staging helper from this checkout before renting. It uses the CLI's source selection and sends that content through the API for download-only preparation.

1. Rents one fresh machine of the SKU.
2. Puts the candidate on it: `--runtime`, `--tensorfs`, `--tensord` (a cozy binary; each a file or `URL#sha256=`), `--pins`
   (exact package releases, `sdxl@0.1.1,…`: a package candidate is published and yanked, and installs only by its pin),
   or `--packages` (a packages checkout whose directories run). Unpinned cells run the root package users get.
   This computer's cozy puts it on (a cozy of another version would restart a shared Calcifer); `machine update` fails unless the machine then runs it.
   The chosen suites' own packages are described on the Runtime the machine runs (`interfaces.sh`): one the package contract
   refuses fails `gate-packages` by name, whatever its cells do. Gate packages use the public API only; what needs the
   Runtime's internals is a built-in of the Runtime (`cozy_runtime.bench`, xs-174) that a gate package calls.
3. Runs the chosen suites' cells, each once, with new prompts and seeds. Before a function's first cell, API warming to `downloaded`
   puts all its weights on the disk, its on-demand models' too (an upsampler), and `off` takes it out of the warm
   set: a cell times what a user who has used the function sees, never a download (cl-790). A slow run goes on; a hung one
   stops: when no run on the machine moves for `--stall` minutes by the runs' own progress clock (15; the longest quiet in
   786 real cells was 7.2), the gate cancels them, fails and ends the rental. It renews the rental's idle clock only while
   they move. A download that can't land before the job's cap (`--cap`, the workflow's `cap_minutes`) at the rate it has
   kept 4 to 8 minutes in fails the host, `host too slow: X MB/s, N GiB needs M min`: the pod ends, its files go to
   `slow-<machine>/`, and the gate starts over on another host, at most 5, one already found slow ended at once (th-364).
4. Ends the rental and checks that it is gone.
5. Runs `analyze.py`. Any GPU fault in the machine's log or kernel probe crash in its record fails the gate, whatever the
   cells did. A cell passes when it completed, did not report `passed: false`, served the last PASS's kernels (with no fallback), produced
   a sane output, and kept execution / step 1 / steady s/step / VRAM within the last PASS's spread. VRAM is the
   driver's: TensorD reads each GPU's bytes in use (NVML) every 100 ms over a call into the record's `memory.driver`,
   and `runs.py` reads what the cell added at its fullest (caps judge everything in use). A GPU cell without it fails.
   Each executor's package import and CUDA init are held against the same cells' known-good: twice those or more is a
   slow host, which the verdict names, and its time fails read `(slow host)`, not a regression.

## Baselines

Assets of the `release-gate-baselines` release: `gate-<SKU>-<suites>.json`, plus the reference image and clip the next gate's H3
cell starts from. Every gate reads them. A PASS (with `--publish`, or the workflow's `baseline` job) replaces them.
A SKU and suites with no baseline need `--establish` once.

## Running it

- **By hand:** `release-gate/gate.sh rtx-4090 --suite runtime --runtime <url#sha256=…> --publish`, through the
  default `~/.cozy`, signed in as you. `--on local` runs the cells on this computer's machine as it is (a dev proof:
  no rental, no `--before`).
- **From Actions:** `gh workflow run release-gate.yml -R cozy-creator/cozy-creator --ref rc/<x> -f hub=prod -f suites=runtime,packages -f pins=…`.
  The suites' `skus` pick the rentals; `skus` overrides them, `only` narrows the cells.
  Testing is manual (Paul 10-24): only the release agent dispatches it, with the suites it chose; no workflow calls it.
  Dispatch from `master`, `rc/*` or `release/*`: the `release-gate` environment refuses any other branch before a runner
  starts, so the run has no log (`gh run view --log-failed` prints nothing) and the reason is the job's annotation
  (`gh api repos/cozy-creator/cozy-creator/check-runs/<job id>/annotations`). A SKU `cozy machine rent` does not list fails
  at once with nothing rented. A SKU with no baseline for those suites needs `establish=true`; `publish=false` keeps that
  run from becoming its baseline.
  `cells` forwards the existing `--cells` hook from a `.sh` or `.tsv` file under this checkout's
  `release-gate/`; it is checked before a rental starts. The hook shares the same candidate, rental,
  `only` filter, cleanup and verdict as the selected suites. Optional `gpus` sets an exact positive
  count on every selected rental; empty keeps the existing suite selection. Optional `disk` sets every rental's
  container disk in GB (empty: each suite's `skus` disk, else 400); size it to the cells' real need, since disk
  is billed by the hour.
  `cells=release-gate/loras.sh` runs community LoRAs (xs-184): each uploaded from its provider as a throwaway, then
  its function at one seed without and with it; `only` picks rows (`moss-axiom,anima-sagging,…`). `only=moss-edit`
  runs MOSS Image's built-in edits of the gate's reference picture (Moss-Image-Edit) and its image_to_image.

## Promotion

Nothing reaches users before a recorded PASS. Candidates wait where users don't look: cozy as a GitHub prerelease
(cozy-publish.yaml; `cozy update`, the installer and Homebrew follow latest), TensorFS and the Runtime as GitHub
prerelease assets (tensorfs-publish.yaml `candidate=true`, cozy-runtime runtime-publish.yaml `mode=source`), and packages
as yanked releases (`task publish`). The release agent picks the suites, runs the gate by hand or from Actions, and
records the result. `candidate.json` in each gate's output names the exact set: the versions the machine ran and the
candidate files' digests. `verify.sh <run|dir>...` prints it only when every supplied SKU passed (or its FAIL was accepted by the lead). `verify.sh --release` also requires every row of `escaped.tsv` (a bug that reached users: the expect row of its cell and the SKUs it must pass on) to have passed in those gates: a Runtime, TensorFS or cozy release's record names its output (xs-182: 0.24.0's hand gate ran the bench, prompt LMs off, and no Hopper pair). Numeric IDs must be completed, repository-owned dispatches of `release-gate.yml`; PR and unrelated-workflow artifacts are rejected before download. Local gate directories remain explicit hand receipts. `promote.sh <run id>` (or
`--record <URL> <dirs>...` for a hand gate) hands that set to users in one step: the same files to PyPI, the cozy
prerelease latest plus Homebrew (cozy-promote.yaml), the packages un-yanked (`cozy package yank --undo`) on the Hub. Packages' `task promote` calls it. The worker image and the Hub pin follow, by the release owner.

## Clients and the Hub

The Hub is one deployment, while clients update independently. Every Hub release runs:

1. **Before deployment:** a disposable Hub, Postgres and MinIO fixture checks package/model
   publication with the candidate CLI. Run `e2e/skew/run.sh --hub-src DIR --cozy-bin FILE --out DIR`.
   The fixture validates its local resource/issuer before signing in, and destroys its state on exit.
2. **Previous clients:** the same isolated fixture runs with `--client VERSION` through the `skew`
   CI job. Unsupported clients must receive the typed `client_too_old` response; any 5xx fails.
3. **Hardware qualification:** real pods rent on production, the one Hub with providers (Paul, 11-11:
   two Hubs, production and the laptop's). Every `release-gate.yml` and `pod-session.yaml` dispatch names its
   Hub, `hub=prod` or `hub=isolated` (`e2e/test-hub.sh` `require_ci_rental_hub`); no default.

The former `hubgate --swap` path is retired: tests must not mutate a persistent developer Hub.
See [isolated publication tests](../e2e/test-hub.md) for candidate/cohort options and the target guard.

The supported range is the Hub's `clients.cozy_min`: raise it only after the newer cozy is promoted.

## CI sign-in

The CLI e2e shards and synthetic publication use the development account their disposable Hub creates.
A `hub=prod` gate or session signs in with the CI device key (`COZY_CI_DEVICE_KEY` in the `release-gate`
environment, which only master, rc/*, tensorfs-v* and release/* may use): dispatch proofs from rc/* after merge.
It rents and runs there, and writes no repository: every synthetic write still requires the fixture.

## Sessions

`pod-session.yaml` runs one rental from a runner, signed in as above, so no laptop has to stay up for it (the pod
renews its own run capability while its runs live, cl-675). `sessions/<script>.sh` sources `sessions/lib.sh` (`quote`, `rent`,
`step`, the guard at `cap` USD and the trap that ends the rental by its own id); its header lists its `params`.
Without `rent=true` it stops after the $0 pre-flight. Dispatched from `rc/<x>`, or with `runtime_ref`, it has that Runtime's wheel in `RUNTIME_WHEEL`. `model-ingest` puts a first-party model on the root (xs-104), or with `mirrors` authors' models and LoRAs under their own orgs as
mirrors (xs-185: converted on the pod, composed on a base, run, tagged, published or held). The `CIVITAI_TOKEN` secret, when set, signs
the session's Calcifer in to Civitai;
`ab` runs cells from `packages_refs` checkouts on one pod and compares their outputs (a refactor's same-bytes proof).
`tool-functions` (cl-774) takes `qwen-image-2/image`, `birefnet/remove_background`, `minimax-h3/generate`, `upscale` and `edit` from dropping the tool to accepting its output on one 4090, as the page does (`e2e/page.py`), a shape like `lab`'s beside them, from a `packages_refs` checkout `pk`, keeping frames to judge.
`render-placement` (cl-774) is a render with the page's Run on at Automatically (`e2e/page.py render --on auto`) on a runner with no GPU and one rental: a tie and a model this computer has no GPU for go to the rental, nothing warns, and with the rental busy a weightless still goes here.
`run-no-on` (tools.md) is `cozy run` with no `--on` on a runner with no GPU and one rental: a weightless still and a function that orchestrates a GPU model go to the rental, as a render does, and nothing warns; `e2e/cli/project.sh`'s `run_no_on` is the case with no rental.
`disk` runs the `disk` suite's cells on one pod under the session's TensorD (its installs only with `installs: yes`);
`disk-valentino` (cl-774) is a 100 GB 4090 past a full disk: Qwen-Image 2.1 and turbo, then H3, which makes its room by
ending turbo's idle executor and evicting both, H3 untouched while it renders, H3 bf16 refused before it downloads.
`probes` (cl-775, cl-633) measures on one cpu pod whether an executor can read TensorD's memory (a benign check of a mapping's first four bytes), and WebRTC from the published HTTPS player in Chromium, Firefox and WebKit, which isn't Safari.
`multi-machine` (xs-126) rents two pods with SSH and works in a shell on each (`cozy machine ssh`): a pod's `cozy` uses
the image's TensorD as `local` beside the runner's runs, one pod controls the other by `authorize`/`add`, and the
previous release and the session's cozy control each other (`-f params='{"old": "0.13.3"}'`). A session that rents
more than one pod calls `order` per pod, then `ready` per pod; the guard and the trap cover every one.
`group-formation` (cr-263) faults GPU 1's executor of a 2-GPU H3 group over SSH (`fixtures/group-stall.py`): killed
before it joins, the run fails at once; stopped once joined, it fails `group_unformed` within one attempt and its unwind
and no GPU stays held; stopped and continued inside the unwind, the group forms on attempt 2 and the run completes. Then
H3 untouched renders 362 frames. `-f params='{"machine": "<name>"}'` uses a rental already up.
`h3-8gb` is the 8 GB tier: H3 Turbo 15 s on one RTX 3070 must complete, 362 frames, under its time bar.
`upload-proofs` uploads twice in a row on one pod, then once while a download of a private commit runs there and a second
download waits on it; each must complete under the pod's own DPoP proofs, signed in the Hub's time, and its record has the
pod's clock against the Hub's (cli-release 42224725083: a host 665 s fast had every redemption refused).
`client-gone` is a run on a pod finishing with its client gone: a second user on one pod asks over a line the session throttles and cuts, and what comes next doesn't wait on that line. `-f params='{"before": "<cozy>"}'` shows what its parts did on an earlier cozy.
`vram-yield` (xs-088) starts a stand-in game (`fixtures/game.py`) on an 8 GB card in shared mode, with models idle and
mid-call: it must have its memory within 2 s, and the call must complete, never ended for it. The game must start
outside cozy's process tree (the session rents with SSH; a root shell starts it over `cozy machine ssh`): started by
a cozy run, it is counted as ours wherever the driver names the container's pids.
`retained-pressure` (run 41362) is Qwen-Image 2.1 Turbo's two-reference edit with its K/V kept on a capped 24 GB card,
and the same game taking most of the card after step 2: the call completes, decode included, with no failed working
allocation, and the game has its memory.
`dedicated-room` (xs-088 review) holds dedicated mode to master's on one 16 GB card (rtx-a4000), in three phases: the
pod's stack, the session's cozy as TensorD alone, then `params.runtime` and `params.tensorfs` too. Each phase has
Qwen beside an idle Klein bf16, and a deviceless root (`fixtures/pause-twice`) calling SDXL twice with a 30 s pause.
The bar: every call completes; nothing moves in the pause; no `room` is sent; the second call replays when production's did.
`hunyuan-48gb` (cr-225) pages Hunyuan-Image-3's 82 GB int8 lane through one 48 GB card (a40 or l40s): a text-only
warm-up, then `runs` more, each completed with a 3:4 image that is not flat. Runtime 0.35.6 failed the warm-up.
`bench` runs the Cozy-vs-ComfyUI benchmark, which lives in cozy-creator/benchmarks (`params.bench_ref`, read with
the `BENCHMARKS_READ_KEY` deploy key): it rents with SSH, starts the runner on the pod and brings its results back.
`byo-model` is a user's own model as the docs show it, on one pod: a LoRA downloaded under a `local/` name runs by
that name, a `local/` name the pod lacks is refused with its fix, and a whole SDXL checkpoint replaces the package's
model by its `local/` name and by its provider source.
`cold-start` is a package's first request right after its install, once per TensorD arm on one pod (each a
release, a dev-build run or the image's): it reinstalls SDXL and Anima (and Qwen-Image), counts their environments'
bytecode, evicts their weights and runs one cold request each, now or once the import-only parent (`imported`) and
its kernel compiles are up; it reads the executors' memory after each machine start.
`warming` (xs-132) covers several measurements on one pod.
- An environment's import with its files evicted from the page cache vs cached, and with its import pages read back first (SDXL, H3).
- API warming to `gpu` held and reused.
- A request queued behind another package's run. With a dev TensorD (`tensord`), item `4b` runs this in shared mode (via the pod's config.toml) and then dedicated; its pipeline is the `fixtures/xs132-pipeline` orchestrator.
`one-executor` (xs-113) runs `suites/tensord/idle-entrypoints.sh` once per TensorD arm on one pod (1 GPU or a
`gpus` group): two constructions of one package on the same processes, then a cancel and a clean successor, with the
pod's executor processes and GPU bytes after each cell, set side by side in `summary.tsv`.
`hub-outage` (cl-708, one `cpu` pod with SSH) breaks the pod's resolver before a deviceless root's first child call
resolves its model (`fixtures/hub-outage`): the call waits and its run says it retries until the resolver is mended, a
later call of the same model asks the Hub nothing, and a model the Hub has not is refused at once. The run completes.
`idle-hub-silent` (cl-778, one `cpu` pod with SSH) names the Hub's host at an address nothing answers part way through
the pod's idle window: its release goes unaccepted, and it ends itself at its provider within one window of its deadline.
`runtime-floor` (run 28075, one cheap GPU pod) puts a machine on an older numbered prerelease pair: a package whose floor
that pair misses takes its own Runtime in its own environment, and the machine's pair stays.

## More cells and checks

- **Cells:** `--cells FILE` adds rows of `name<TAB>target<TAB>args`. The args are expanded in gate.sh, which defines
  `$SCENE`, `$CHAR`, `$R`, `$K`, `$M` and `$CLI`. A FILE ending in `.sh` is sourced after the last product cell.
  `released.sh` runs every cell of a `--packages` checkout again from the release the Hub serves, on the same pod,
  twice in turn: a candidate's same-pod A/B (each pair's execution, steady step and output, in the notes).
  `tensorfs-ab.sh` (with `-f tensorfs_wheel=…`) is the varena matrix on one pod: each model alone after a restart, cold and
  warm, on the TensorFS the pod came with and on the candidate: SDXL, Klein, Qwen-Image 4 MP and H3 Turbo 15 s uncapped and at
  8 and 6 GiB, Ideogram 4 uncapped; then Ideogram 4 beside idle SDXL and Qwen executors (`ab-peers-*`).
  `runtime-ab.sh` (with `-f runtime_wheel=…`) is the same matrix for a Runtime candidate against the Runtime the pod came
  with: SDXL, Klein and Qwen-Image 4 MP with their prompt LMs on, H3 Turbo 15 s, uncapped and at 8 and 6 GiB; Ideogram 4
  and P-H3-Image's edit with its upsampler uncapped.
- **One release, two checkpoints** (`swap.py`): Qwen-Image text → edit → text and its two Lightning LoRAs in turn, each group on one executor with no restart (run 15744: TFS 0.15.0's `{release}/{component}` set names collided). No first-party SDXL LoRA exists to swap.
- **LM coverage** (`lm.py`, every gate): Fibo's and Klein's prompt upsamplers and H3's reference describer ran their
  greedy token loops (H3's cell gives a character reference, which BiRefNet cleans and Qwen3.5 describes).
- **varena** (`suites/varena`): the memory manager through its paces on SDXL, Klein and 5 s H3 on Ampere, Ada,
  Blackwell and a CPU rental (Hopper/B200 on request): cold load, co-residency, unload and reload, 8/6/3 GiB caps with
  SDXL and Klein alternating at 6, a real 8 GB card, and host RAM under the rental's cgroup with page/spill to disk.
  `check.py` holds each load and refill to a share of the machine's measured ceiling (`ceiling.py`), checks caps
  (the driver's bytes in use under each), alternation and host RAM; `analyze.py`, time and VRAM. Selected by `varena` (varena, TensorFS, TensorD, the
  Runtime); a SKU new to it needs `-f suites=varena -f skus=<sku> -f establish=true` once.
- **tensorfs** (`suites/tensorfs`, one `cpu` rental, ~$0.3, ~40 min, `-f suites=tensorfs`): pick it for a TensorFS or
  download/upload path change. It shows TensorFS moves weights as fast as before and dedups:
  - `probe` (on the rental): the machine's disk, written and read cold (the varena method); an executor never dials
    (cr-012), so there is no network ceiling, and downloads are held against the last pass.
  - `model install` of a large many-object model cold (Hub and datacenter-cache times are separate baselines), again
    (nothing from the Hub), and cut part-way then resumed (only the rest comes): once by a cancel, once by killing
    TensorD on the rental, as a crash ends it (`kill`, rented with SSH; an `only` without it rents none). Unfinished
    chunks count: at least 80% of the bytes that had arrived must be held. A boot that ends `boot_idle` is ordered again.
  - `model upload` of new random weights from the gate's host to a throwaway `<account>/ci-tfs-<R>` (the time
    includes staging from the host: run it from a runner), the same folder as a second commit in another lane (dedup:
    the same objects, a tenth of the time), and new weights cut part-way then resumed. The model is deleted after.
  - `straggler` (on the gate's host, where a client may dial): `tfs download` of BiRefNet clean and through
    `straggler.py`, which drips its first objects as a slow storage GET does. A straggler must be re-asked, not
    waited out, and a crawling object, each ask of which moves 64 KiB and drips, read as parallel ranges (each at
    most 1.5x clean + 20 s, the crawling ones 2x clean + 60 s).
  - `lora-alpha` (on the gate's host): `tfs import --converter h3.lora` of an H3 LoRA in ComfyUI's spelling with no
    `.alpha` tensors whose kohya metadata says `ss_network_alpha` 420 at rank 4. Its committed `lora_alpha` must be `r`, a scale of 1, as kohya's and ComfyUI's
    loaders apply it; metadata alpha scaled the Mystic XXX LoRAs 26x into colour noise (10-29).
  - `multi-conversion` (`suites/tensorfs/multi-conversion`): a two-component model's int8 recipe opens one output and
    encodes each component in its own call with its own plan, as H3's recipes do. Source, `model upload` to a
    throwaway `<account>/ci-tfs-mc-<R>`, `model quantize --package`, then `inspect` reads the lane's commit back: both
    components' weights encoded and near the source, biases the source's bytes. The model is deleted after. Escaped
    from TensorFS 0.17.0 to #2238 (a writer bound one conversion and refused the second component).
  - `family` (tfs-137): two `model upload`s refused before their bulk downloads. An SDXL inpainting single file
    (sdxl.single_file's keys, `conv_in` at 9 channels) is no SDXL form (`FAMILY_MISMATCH`, at most 64 MiB moved).
    Comfy-Org's ComfyUI H3 Ref2VA DiT forced as `h3.native` fails the layout probe (`LAYOUT_MISMATCH`, at most
    512 MiB): its contiguous Q/K/V is not where the official rows are. Nothing reaches the Hub.
  - `--only` picks cases (`probe`, `download`, `upload`, `family`, `multi-conversion`, `straggler`, `lora-alpha`); `check.py` judges only those.
- **hub** (`suites/hub`): on production (`hub=prod`), with publication in the isolated fixture (Clients and the Hub, above). It shows the
  API contract (tensorhub `api/openapi.yaml`, as the Hub serves it at `/openapi.yaml`) holds as cozy uses it:
  - `contract.py`: each path no verb reaches, called anonymously through `e2e/hubcheck`, answers as its contract says
    (200, or 401 when bearer-only), and hubcheck finds every request and response as the document has it. This is
    the one place in release-gate that speaks HTTP to the Hub by hand: it is the contract test.
  - In the isolated fixture only, through `cozy`, on a throwaway `<account>/ci-hub-<R>`: whoami (balance, buy page), the rental catalog; a package
    locked, published twice, run, yanked (the simple index marks it `data-yanked`), resolved past, run by exact pin,
    un-yanked, and a yanked candidate never named newest; a model commit uploaded in a lane by the machine, published
    (listed), downloaded, downloaded by pin and lane, deleted (gone).
  - Real pods, one fresh `cpu` rental on each provider (`machine rent --provider runpod|vast`: dev-only, so the suite
    writes `dev = true` into the home's config; never a `--help` probe): a local weightless package sent by content,
    no process holding a Hub secret in its environment and
    the machine authenticated by its own key (xs-157), ended and gone. A cozy that refuses `--provider` fails both
    cells, naming why, and the pod cells run once on the Hub's choice. Run it with a published cozy (a release, or the
    candidate's prerelease): a cozy moves each pod to its own minor first, and an unreleased dev build has none to move to.
  - Any 5xx from the Hub fails its cell, whatever the cell expected.
  - Before a Hub deploy, the release agent also rehearses its migrations: dump prod (in-pod `pg_dump -Fc`, copied
    in sha256-checked chunks, a full `pg_restore -f /dev/null`), restore it into a scratch Postgres, start the
    candidate image against it (it migrates on start) and wait for `/healthz` 204. Record the dump and the result
    with the gate.
- **Rent:** an order that a moved price holds gets a new order at the new price, at most 5; 20 min with no pod bought fails the gate, a not-run and not a verdict.
- **Quality** (`quality.py`, every gate): CLIP prompt adherence of every render against its prompt (floor 0.25; 0.20 for varena's 4-step CPU `host-sdxl`), and
  the gate's median CLIP-IQA sharpness against the last passed gates. Never a same-seed pixel diff.
- **Checks:** `analyze.py` runs every `check(gate) -> (fails, notes)` in `CHECKS` (quality, lm, swap, and a suite's).
- **Known lists:** `known-blocking.json` and `known-failing.json` hold request-path blocking and lead-ruled skips;
  both may only shrink. A known-failing entry's `cells` names the suite cells it parks (`sku` "*": every SKU).
