# Current cohort artifact and device plan

Checked product source: Creator `118d668f0d7ec05348ff011bb8502488f210f27e`,
Runtime `78e58f7e51bade1bea92c75f238ba058d59421a2`. Evidence-only successors
retain these product trees. Native source equals `ebe91ad3447c668a93776cf4628b8f585e5aedd6`.
These commands are proposed only: no artifact build, repack, dispatch or install
was run by this owner. Coordinate cache custody before any compilation.

## Reuse the matching Creator build cache

`evidence/current-cohort-118d/source-equivalence-002.json` records the 18 different
Cargo/platform/protocol inputs by Git blob. Varena/TensorFS/binding/client code
and manifests equal ebe91. Existing compiler cache reuse is preferred: preserve
unchanged source mtimes, use the verified toolchain and feature graph, and copy
immutable outputs before another build changes the cache. A fresh target is
necessary only for an evidenced incompatibility that cannot safely reuse the
matching target; no such incompatibility was found here.

The portable cache is owned by the GPU operator. This owner never accessed or
borrowed it. Its next owner must secure exclusive custody and audit processes.
The known cached driver is
`outputs/xs138-native-api-integrated/build-native-successor-final-001.py` and its
qualified helpers-first/final-Maturin-last recipe is
`outputs/xs138-native-api-integrated/linux-a247d24a3-final-001/build-wheel.sh`.
The driver's ancestry check assumes native 6e22 is an ancestor; this composed
branch imports that tree instead. A task-local adaptation must use the recorded
composition/source-equivalence proof, not run the unmodified ancestry check.

Archive the committed current tree into a new durable **source/output** directory,
stamp dev versions only in that copy, preserve its exact patch and source SHA,
and reuse unchanged mtimes with the existing helper:

```sh
python3 /home/fidika/cozy_v2/outputs/varena-v2-p0-20261031/reuse-source-mtimes.py \
  "$PREVIOUS_SOURCE_STAGE" "$CURRENT_SOURCE_STAGE" \
  "$CURRENT_ARTIFACT_OUT/source-mtime-reuse.json"
```

Inside the same pinned portable environment, with its Python/Torch/AOT inputs
verified, use the exclusively borrowed matching cache. The worker runs in a
separate process from Python. Build and seal its private bundle before later
Python feature resolution changes the cache:

```sh
cd "$CURRENT_SOURCE_STAGE"
export CARGO_TARGET_DIR="$MATCHING_PORTABLE_TARGET"
export CARGO_BUILD_JOBS=1
nice -n 19 cargo build --release --offline --locked -j 1 -p cozy --bin cozy
uv run --no-project --with patchelf python scripts/bundle-native.py \
  --binary "$CARGO_TARGET_DIR/release/cozy" \
  --target "$CARGO_TARGET_DIR/release" \
  --out "$CURRENT_ARTIFACT_OUT/worker" \
  --archive "$CURRENT_ARTIFACT_OUT/cozy-linux-x86_64.tar.gz" --max-glibc 2.28
```

Next use the qualified helper/wheel recipe, which builds tfs/extension/helpers
with explicit identical features, copies helpers out, records their/core hashes,
runs the canonical full-wheel frontend **last**, and checks hashes afterward.
The Torch shim must match the target CUDA Torch package; CPU Torch used for host
checks is not its build input. Use that recipe's existing container mounts and
version-stamping steps, changing only the current source/output locations:

```sh
nice -n 19 bash "$CURRENT_HELPERS_AND_FINAL_WHEEL_RECIPE" \
  --out "$CURRENT_ARTIFACT_OUT/wheels" --compatibility linux \
  -i "$TORCH_BUILD_PYTHON" -j 1
uv run --no-project --with patchelf python crates/varena/bundle.py \
  --target "$CARGO_TARGET_DIR/release" --sdk "$CURRENT_ARTIFACT_OUT/sdk"
```

Copy SDK and native helper artifacts only from the final matching dependency
cohort. Verify all wheel RECORD hashes, Python source closure, both distributed
core/std copies, extension equality with final Cargo output, immutable helper
hashes, native ELF dependency closure and mixed C/Rust/Python load orders with
one mapped core. Do not run a primary `-p varena` build afterward. This is
same-process private binary compatibility; separate worker/Python processes
require paired wire compatibility, not exact source or library SHA lockstep.
Do not relabel a host-built wheel as manylinux.

## Repack current Runtime with verified native bytes

`runtime-native-reuse-002.json` in the paired Runtime evidence directory proves:
65 actual native/build input files match both a09 and frozen 2f7 source; all
external uv.lock dependencies/resolution match (only Runtime version changes);
real current/frozen Triton imports produce identical 49 set/architecture specs
and 285 unique keys, exactly the shipped manifest keys. Launcher source matches
all shipped launchers. CUDA remained uninitialized. All 581 native member hashes
were independently rechecked in both original and controlled 40fde wheels and in
the preserved a09 native prefix. No native rebuild is indicated.

The faster path is the existing source-stamped repack driver after committing the
source/evidence tree. It creates new output, preserves the previous artifacts,
and verifies all current Python sources and every native member:

```sh
nice -n 19 python3 \
  /home/fidika/cozy_v2/outputs/varena-v2-p0-20261031/build-runtime-candidate-v4.py \
  /home/fidika/cozy/.worktrees/cozy-runtime/xs138-v2-current-core \
  /home/fidika/cozy_v2/outputs/xs138-p2-vae-block/native-a09e68dba/native-build-receipt.json
```

Then verify the new wheel's complete RECORD, not just native member count:

```sh
nice -n 19 python3 - "$RUNTIME_WHEEL" <<'RECORD'
import base64,csv,hashlib,io,sys,zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    record, = [n for n in z.namelist() if n.endswith('.dist-info/RECORD')]
    rows = list(csv.reader(io.StringIO(z.read(record).decode())))
    assert {r[0] for r in rows} == {n for n in z.namelist() if not n.endswith('/')}
    for name, expected, size in rows:
        if name == record:
            assert expected == size == ''
            continue
        data = z.read(name)
        digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).decode().rstrip('=')
        assert expected == 'sha256=' + digest and int(size) == len(data), name
print('Complete RECORD passed')
RECORD
```

No full GitHub native rebuild is needed for these matched inputs. If subsequent
native source, prebuild keys, toolchain or dependencies change, list the concrete
differences and rebuild only their affected modules using the existing recipes.
This packaging/reuse proof does not transfer GPU qualification to current Python.

## Required device qualification

1. Hash worker archive, TensorFS wheel and Runtime wheel; inspect private-library
   receipts. Exercise updated install/update custody and live readback together,
   including Calcifer journal and mode. The old static worker does not gain
   updater support from these source commits.
2. On an operator-authorized two-GPU machine, execute concurrent independent
   `--gpus=1` jobs. Record distinct UUIDs, executor PIDs, descriptor selections,
   owner mappings, overlap and completed outputs. Apply pressure/retirement on
   one while the other progresses; retain both GPUs' accounting and cleanup.
3. Exercise actual rank 1 native startup and follower acknowledgement before
   collective formation. Verify each local ordinal's real UUID. Example UUIDs in
   the CPU frames prove no GPU identity or constructor execution.
4. Run H3 Ulysses `--gpus=2`, then `--gpus=4`: 15 seconds, 362 frames, 1344x768,
   30 steps and audio; matched model/seed/precision/cache conditions, output
   quality, timing boundary, asymmetric pressure and cleanup. Existing H3 gate
   assertions of height>=700 and frames>=300 alone do not prove those settings.

After coherent installation, the existing two-GPU entry point is:

```sh
bash release-gate/gate.sh "$GPU_SKU" --on "$AUTHORIZED_MACHINE" \
  --suite execution --only qwen-r1,qwen-r2,h3-u2-std \
  --cli "$CURRENT_ARTIFACT_OUT/worker/cozy" \
  --cells "$PWD/release-gate/suites/execution/multi-gpu/qwen-replicas.sh" \
  --cells "$PWD/release-gate/suites/execution/multi-gpu/h3-ulysses.sh" \
  --out "$APPROVED_RECEIPT_DIRECTORY"
```

It requires a named existing machine, baseline and pinned package set; it grants
no rental authority. The Qwen concurrency proof alone does not cover asymmetric
pressure/owner isolation. Four-GPU H3 and pressure proofs need additional operator
cells. Preserve the strict numerical failure and 4.73-4.98x warm offload slowdown
reported by the paid lane. No padding-only explanation or P0-P4 completion is
inferred from this composition.
