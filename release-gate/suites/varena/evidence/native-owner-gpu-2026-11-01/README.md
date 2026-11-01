# Native owner GPU checkpoint

RTX 4090, native TensorFS `1fec2950b`, Runtime `4d5844f0`, qualified TensorD
`afa6f922`: ordinary run **25190** and same-PID observation **25191** pass. PID
439849 retains row slot 0/incarnation 3. The native loop donates 48 MiB with an
actual 48 MiB driver-used decrease; the independent requester allocates/touches
32 MiB with an actual 32 MiB increase. Live working and pinned weight values
survive; the exact parent baseline returns, and the requester settles to zero.
Capture/replay automatic deferral each adds zero native driver calls.

The bounded collector passes exact versions, stable worker boot, a positive
Cooperative binding and no logged Legacy byte action in its anchored interval.
This is bounded log coverage, not proof of every unlogged path. The fixture
temporarily configures its owner Shared/priority −10 and restores Start's
dedicated/priority 0; machine shared-mode propagation is not claimed.

The initial cleanup upload failed before application execution (`invalid_range`,
install run 25192). An unchanged retry executes **25197** in a separate executor,
PID 440031. It intentionally closes the actual native owner, requires the checked
Memory boundary to refuse, then unwinds through unchecked cleanup. The run retains
the original leading `RuntimeError: xs138_cleanup_original_error` and latches the
coordination fault. Its secondary poisoned/non-quiescent error is expected and
preserved: **this is not clean quiescence or successful handler execution**.
Checked refusal is established by reaching the sentinel in the immutable fixture;
the printed marker is not present in the saved NDJSON. The closing cell occurs
after the normal collector interval.

`manifest.json` records 54 unaltered raw files and hashes, including the upload
failure. `gpu-independent-review.json` records exact boundaries. This qualifies
native automatic donor parity and error preservation. Physical-shortage asks,
GIL-held allocation progress, initialized-GPU fork/retirement stress, cache-owner
callbacks, foreign-pressure/game coexistence and full P3 remain open.
