# Attention

The measurements behind the Runtime's attention table (xs-171; tracker `cozy-runtime/xs-171`, designs in
`outputs/kernel-tables/`). Paul, 11-11: "we, the developers, do testing on our end, and then hardcode that into the
runtime releases". Per hardware class, on its reference card (`skus`), every attention kernel the card runs is timed at
about 35 shapes from our packages' real lengths and judged against FP32 attention. Only the ranking within one pod is
used, never times compared across pods.

- `attention-inventory`: the card's class, its kernels, and the sets that can share one process.
- `attention-speed-<n>`: one set over the grid (`GRID` in the Runtime's `internal/bench/attention.py`), interleaved, three rounds; SDPA
  runs in every set as the anchor. A kernel that faults the process is named and the set runs again without it.
- `attention-quality-<kernel>`: worst-head relative L2 against FP32 on every site, and the best answer a kernel could
  give without computing; a quantized sparse kernel also against the class's 16-bit sparse kernel on the same route.
- `check.py` fails a kernel the inventory names that no cell timed, explained or judged.

The measuring is the Runtime's own: `cozy_runtime.bench`, a built-in of the Runtime's App, which reaches its kernels as
a model's call does (`attention.pinned`, then the kernel's backend call) and times each call with CUDA events. A
package can't import the Runtime's internals (the package contract), so `drive/` is public API only: each cell calls the
built-in with `call()`. The pod's clocks and throttling come from TensorD's NVML reads in each run's record. Q/K/V are
synthetic (seeded random, outlier rows, block-routable for sparse kernels) until the corpus of captured activations
exists.

## One round

```sh
release-gate/gate.sh <card> --suite attention --runtime <wheel> --establish   # per class whose kernels changed
release-gate/suites/attention/collect.py --runtime <cozy-runtime checkout> <gate dir>...
cd <cozy-runtime checkout> && uv run python scripts/attention_table.py generate   # then a Runtime PR with ## Proof
```

`collect.py` replaces each measured class's record in the Runtime's `measurements/attention.json` and computes every
kernel's build identity from the source the pod ran (the wheel's stamped commit). The Runtime's
`scripts/attention_table.py check` runs in its merge queue: the table must regenerate byte for byte, every listed kernel
must still be the build that was measured, and every kernel a class runs must have been measured there. `table.sh` runs
that check on a candidate wheel against the source it was stamped with, and fails a wheel whose table is not that source's:
before a cut rents anything (release-gate/README.md, Before renting), and as this suite's first cell (`attention-table`).

Cost: about 40 min a card; L40S $0.8/h, RTX 4090 $1.0, RTX 5090 $1.4, RTX PRO 6000 $4.2, H100 $6.7, B200 $12.9.

## Calibration (xs-171 F)

Once, before the first measured table, and again only if Paul asks: two dispatches from the open RC. First the corpus,
captured once on an RTX 4090 (the Q/K/V are the model's, not the card's):

```sh
gh workflow run release-gate.yml -R cozy-creator/cozy-creator --ref <open rc> -f hub=prod -f suites=attention-corpus \
  -f packages_ref=<open rc> -f cozy=<RC dev-build run> -f tensorfs_wheel=artifact:<RC dev-build run>/tensorfs-wheel-<sha> \
  -f runtime_wheel=<the RC's cozy-runtime dev-build run> -f establish=true -f publish=false
```

Then every class's round on that corpus plus the end-to-end arms (six pods; about $30). Each gate pushes the first run's
`corpus/` to its pod over SSH before its bench runs; the quality cells and the sparse speed points read it, and dense speed
stays synthetic:

```sh
gh workflow run release-gate.yml -R cozy-creator/cozy-creator --ref <open rc> -f hub=prod -f suites=attention \
  -f corpus=<the first run's id> -f cells=release-gate/suites/attention/calibrate.sh -f packages_ref=<open rc> \
  -f runtime_wheel=<the RC's cozy-runtime dev-build run> -f establish=true -f publish=false
```

`calibrate.sh` renders the bench arms' requests (`suites/bench/cells.tsv`, `a-*`) at `--attention-kernel exact` (the 16-bit
reference), on the table's own kernels, and pinned to each 8-bit kernel the class runs, on each class with 8-bit models to
judge (H3 on the 4090, 5090, H100 and B200; Klein, Qwen-Image 2.1 and MOSS on the 4090 and 5090). Each output goes to
`signoff/` (PNG, or H.264 at CRF 18) with its distance from the reference in `signoff/signoff.jsonl`. Then, on the six gate
directories:

```sh
release-gate/suites/attention/signoff.py ~/cozy_v2/outputs/kernel-tables/signoff <gate dir>...
```

writes the outputs per class and `manifest.json`: the proposed bars (micro: worst-head error on the corpus at most 0.01 at
16 bits and 0.04 at 8, the synthetic sites' beside it; end to end: S_8/E_8, today's choice's worst distance from 16 bits), every kernel's verdict per class, and one
row per output {class, model, kernel, level, seed, metrics, pass}. Paul signs the bars, which go in their own commit to the
Runtime's `measurements/rules.toml`.
