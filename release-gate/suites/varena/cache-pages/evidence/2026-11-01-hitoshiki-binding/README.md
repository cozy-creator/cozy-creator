# Python Cache binding on integrated a96/7ad

Run 25373 executes tensorfs.varena.Cache through the parent a96 extension (SHA
070d70182056a6d202b28a92db49b078ec9f0239ee7337f829d894e7cafff5e6), under Runtime
0.36.22+dev.g7adc1e82 and TensorFS 0.17.16+dev.ga96c7c3e. It does not execute the
standalone helper. The e545 helper manifest is verified by shared controller setup
only; its c3 base must not be mistaken for the executing extension in this cell.

Three 2MiB pages are committed and fully read back. Releasing the middle leaves 4MiB;
poll observes completion, recommit retains the exact base, and both neighbors plus
the changed middle pass full direct CUDA readback. Explicit release/poll/close returns
all Cache counts and live/mapped/pending/ready/reserved bytes to their zero baseline.
Working live bytes 9,046,016 and mapped bytes 18,874,368 also match baseline. Working
pending bytes change 0→2,048 in this request, so this is exact Cache cleanup, not an
all-working-pending-zero assertion. The raw statistics are retained.

This is primitive Python-wrapper qualification on the coherent SDK. It does not cover
asynchronous owner callbacks, GIL/owner-busy progress, Runtime graph retirement or
d3 split-choice behavior. Those remain separate proofs.
