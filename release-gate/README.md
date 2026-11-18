# Release gate

A PR's own proof is not here: what a CPU runner can show (projects, Calcifer's HTTP surface, version skew, restarts,
local code) is a cell of the e2e cli suite (`e2e/README.md`). Cells that need a GPU or a signed-in account are this gate's.

Release testing is manual: the release agent picks the suites a release needs (`--suite`, no default), runs them, and
the promotion names the passing run (`verify.sh`). Every cell is a real run held against the last passed gate on the
same SKU and suites; any FAIL blocks the promotion. Design: tracker `design/ci-v2.md` §3.

## What may live here (xs-219, Paul 11-18)

A GPU test exists only if all three hold; otherwise it is deleted, not parked:
- it is a hardest case (a memory cap, the smallest card, the largest request, a fault), never the average call;
- it shows something no cheaper test shows: not a CPU runner, not another cell, not the same cell on a second card;
- its time is a row in the benchmark ledger, and the next run is compared against it.

A session written for one issue's proof is deleted by the PR that closes the issue.

## Suites

| Suite | Proves | Rentals (`skus`) |
|---|---|---|
| `escaped` | every first-party package's cells (`products.sh`) at a 6 GiB cap: nothing kept across steps outside the memory manager; a cell that failed or was refused memory runs again uncapped (`<cell>-uncapped`) | rtx-4090 |
| `varena` | H3 cold, then SDXL and Klein beside it and H3 again; H3 Turbo 15 s on a real 8 GB card (`h3-8gb`); SDXL repeating byte for byte warm, capped at 6 GiB, perturbed, refilled and with no room (`differential/`) | rtx-3070, rtx-4090 |
| `bench` | SDXL, FLUX.2, Qwen-Image 4 MP, Klein and H3 at 8 and 6 GiB (`rtx-4090x1.capped`); H3 Turbo 15 s uncapped against the ledger; each box tool at its defaults (`t-*`: the `benchmarks.json` a release carries); the ComfyUI matrix by hand (`matrix-skus`, `suites/bench/README.md`) | per group (`suites/bench/skus`) |
| `graphs` | CUDA graphs replayed bit for bit on a dev machine: P-H3-Image, SDXL, Anima uncapped and at 6 GiB, Klein on cr-283's request, the describer's second call, H3's latent upscaler | rtx-4090 |
| `execution` | prompt LMs decoding after another model with no capture refused (`prompt-lm`, xs-182) | rtx-4090 |
| `multi-gpu` | H3 over two GPUs: two beat one, and a fine-tune fetched during the call; a run on GPU 1 alone records its own card (`h3-second`, cr-326). A GPU the machine says was slowed (heat or its host's power limit: `ledger.py throttled`) voids `h3-gpus-faster`, which fails to run again on another host, and its ledger rows carry `host_throttled`, never a base | one card per family, ×2 |
| `consumer` | SDXL's fp8 lane on its float route, on a card below sm89 (`sdxl-fp8-float`, cr-338) | rtx-3060 |
| `edit`, `upscale` | chained Qwen Image 2 edits keep the photo (se-153); each picture upscaler beats Lanczos on small JPEGs (se-147) | rtx-4090 |
| `packages` | what a release changes, from the `--packages` checkout's source, plus what `--only` names | rtx-4090, rtx-5090 |
| `rentals` | a rental idles out through updates and closes when its provider stops it (cl-1203, th-404) | cpu |
| `kernels` | every kernel the Runtime ships, on each family it claims, against a slow reference (`suites/kernels/README.md`) | one card per family |
| `inputs` | each package's largest inputs, the LM-heavy ones again at an 8 GiB cap (cr-279); the suite the `*-ab.sh` hooks ride on | rtx-5060-ti, rtx-5090 |
| `h3`, `runtime` | minimax-h3's generate, edit and upscale at their edges; H3's video VAE and latent upscaler at their stored times (`h3-vae`, `h3-upscaler`) | rtx-4090, rtx-5090 |
| `hub` | TensorHub's deploy check: its contract, the account and one `cpu` pod per provider | cpu |
| `publish`, `checkpoints` | $0, on a runner: packages through the Hub-publish path on a throwaway Hub; published checkpoints carry what their tool reads | none |

`attention` and `attention-corpus` are not tests: they measure the attention table the Runtime ships (xs-171,
`suites/attention/README.md`), as do bench's `*.arms` rows. Every gate also holds each cell to the package contract
(`suites/contract/check.py`). `suites/progress/check.py` judges a run's event stream and its time left (cl-921,
cl-1247); its own cells are gone, and it runs once the gate names the runs it reads (cl-1264). `escaped.tsv` lists the
bugs that reached users and the cell each must pass (`verify.sh --release`).

A suite is `suites/<name>/`: `cells.sh` (sourced in gate.sh: `cell`, `pkg`, `product`, `expect`, `log`, `$CLI`, `$M`, `$R`,
`$REF`, and e2e/run.sh's `run_*`), an optional `check.py` (`check(gate) -> (fails, notes)`, listed in analyze.py) and
`skus` (`<sku> [disk GB] [gpus]` per line). Suites stack on one rental per SKU. `--cells FILE` adds a hook:
`tensorfs-ab.sh`, `runtime-ab.sh` and `varena-ab.sh` run SDXL, Klein, Qwen-Image 4 MP and H3 Turbo 15 s uncapped and at
8 and 6 GiB on one pod, base against candidate, A B B A.

## A gate

1. Resolves its candidate set at $0 (`resolve.py`; `-f rent=false` or `gate.sh --preflight` stops there) and describes the
   suites' packages on the candidate Runtime (`interfaces.sh`).
2. Rents one fresh machine of the SKU and puts the candidate on it: `--runtime`, `--tensorfs`, `--tensord`, `--packages`
   (a checkout run from source), `--models` (held private releases).
3. Runs each cell once, with new prompts and seeds, its weights downloaded first. A slow run goes on; a hung one stops
   (`--stall`, 15 min with nothing moving); a host too slow to finish before `--cap` is replaced, at most 5.
4. Ends the rental and checks that it is gone.
5. `analyze.py`: a cell passes when it completed, served the last PASS's kernels, produced a sane output and kept its
   time and VRAM within the last PASS's spread. Any GPU fault in the machine's log fails the gate.
6. Appends a row per package run to the benchmark ledger (`ledger.py`: the tracker's `benchmarks/<sku>/<YYYY-MM>.jsonl`;
   `ledger.py compare <rows>` calls each gap noise (5%), host, regression or improvement). `--skip-unchanged` skips a
   cell whose last PASS asked the same of the same software.

Baselines are assets of the `release-gate-baselines` release (`gate-<SKU>-<suites>.json`); a PASS with `--publish`
replaces them, and a SKU with none needs `--establish` once.

- **By hand:** `release-gate/gate.sh rtx-4090 --suite escaped --runtime <url#sha256=…> --publish`, in the default `~/.cozy`.
- **From Actions:** `gh workflow run release-gate.yml -R cozy-creator/cozy-creator --ref rc/<x> -f hub=prod -f suites=<one>
  -f skus=<sku> -f disk=<GB> -f packages_ref=<packages commit>`, from `master`, `rc/*` or `release/*`. One suite a
  dispatch. `hub=prod` signs in with the CI device key (environment `release-gate`). A PASS becomes the baseline only
  with `publish=true` (off by default), and only a whole suite's: with `only` or `cells` the dispatch is refused before
  anything rents.
- **Promotion:** `verify.sh <run>...` prints the candidate set only when every gate passed on the same set;
  `verify.sh --release` also needs each `escaped.tsv` row passed, or accepted by name for that release in
  `escaped-accepted.tsv` (printed `ACCEPTED`; never one that ran and failed). `promote.sh <run id>` then hands it to
  users: PyPI, cozy Latest and Homebrew, the packages, the held models, the worker images last.

## Sessions

`pod-session.yaml` runs `sessions/<script>.sh` on one rental from a runner. A script sources `sessions/lib.sh` (`quote`,
`rent`, `step`, the guard at `cap` USD and the trap that ends the rental by its own id); its header lists its `params`.
Without `rent=true` it stops after the $0 pre-flight.

- **Gate:** `h3-group` (cr-326: H3 over each group size in `degrees` on one rental; two GPUs must beat one) and
`rental-boot-first` (cl-1069, LAB2-47826; one cheap GPU pod) sends local code whose Runtime floor only the open RC meets to a
rental as it boots, in a dev home on the session's RC cozy (`cozy=<dev-build run>`): the rental moves to the RC before it takes the code,
which runs on the RC's Runtime; held by hand on the release pair, the code is refused `runtime_held` before any install. Both sends
name the rental (the account's rentals are one pool, cl-1224).
- **Memory work:** `ab` (cells from `packages_refs` checkouts or wheels on one pod, cold and warm, capped or with the card
  held), `idle-held` (a second model beside an idle one, cl-1159), `vram-yield` (a game takes the card mid-call, xs-088).
- **Models:** `model-ingest` puts a first-party model or an author's mirror on the Hub from a pod.
- **One open issue each**, deleted when it closes: `at-rest`, `carry-ahead`, `edit-ahead`, `first-guess`, `load-ahead`,
  `needs-ahead`, `nofit-local`, `submit-at-once` (cl-1123, cl-1202), `control-plane`, `derived-disk`, `egress`, `far-reads`,
  `group-formation`, `h3-upscaler`, `h3-vae`, `install-share`, `io-fault`, `lane-fit`, `lane-floor`, `llm-stream`, `llm-tools`,
  `llm-two-gpus`, `lm-construct`, `ltx-images`, `one-pool`, `orbit`, `upscale`, `user-flow` (xs-203: Paul's flow through
  the page on real rentals). `bundle` runs several on one rental; `gpu-reader` is `ab`'s helper.
