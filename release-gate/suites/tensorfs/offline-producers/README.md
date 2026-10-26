# Native FP8 rowwise and fixed-codebook W4A8 producers

Run these only in the primary Varena agent's coordinated validation window, using the
installed CLI and default `~/.cozy`. The current `cozy run --help` accepts `--on local`
and `--gpus N`; `--gpus=1` caps the run at one GPU. These entrypoints declare
`accelerator=False`, so their numerical work is CPU-only and gives no GPU-consumer proof:

```sh
cozy run /absolute/path/to/offline-producers/run mode=basic seed=103 --on local --gpus=1 --json
cozy run /absolute/path/to/offline-producers/run mode=cancel-fp8 seed=103 --on local --gpus=1 --json
cozy run /absolute/path/to/offline-producers/run mode=cancel-w4a8 seed=103 --on local --gpus=1 --json
```

After the Runtime adapter patch is installed, repeat `basic` with `runtime=true`.
The native candidate must support `fp8-rowwise-rne/1` and `w4a8-convrot256-fixed/1`
in both `raw.quantize_measured` and `Writer.quantize`.

The basic cell compares all output bytes with frozen independent Python producers for
zero, negative-zero, subnormal, floor, outlier, tie, wide and mixed-exponent matrices.
It checks all three logical dtypes, row chunks 1/2/7, exact max-error/saturation and
SSE/energy within relative tolerance 2e-13. It writes and reads real checkpoints,
checks native decode against checkpoint decode, rejects invalid source geometry,
nonfinite data, unsupported strategies/parameters and insufficient workspace/output
budgets, and refuses publication after a real quality failure. Writer conversions
exercise actual F16/BF16/F32 source data and shared W4A8 codebook staging.

The two cancellation cells interrupt real native Writer work after one complete entry,
join it, reopen the existing journal, reject a changed recipe and finish the remaining
entry. A missed cancellation window is an explicit failure, not a passing no-op.

No native or installed-CLI qualification receipt exists at introduction. Rust/Python
source checks are not numerical proof. These cells establish CPU producer and Writer
behavior only, not model quality, GPU serving performance or a public release.
