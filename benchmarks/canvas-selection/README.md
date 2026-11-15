# Creator canvas selection benchmark

Research prototypes, not product code. The companion decision is in
[`../../docs/research/canvas-library-selection.md`](../../docs/research/canvas-library-selection.md).

All browser runners are headless. Do not change them to open windows on the user's desktop.

## Reproduce

Requires Node 24, pnpm, uv, ffmpeg, a compatible Chrome-for-Testing executable, and working GPU drivers.
Versions of React, React Flow, PixiJS, Leafer, GPUCanvas, Three, Vite, and Playwright are pinned in the lockfile.

```sh
pnpm install --frozen-lockfile
uv run --with pillow python fixtures.py --out .cache/assets --count 5000
export BENCH_CHROME=/absolute/path/to/chrome-for-testing/chrome
export CANVAS_BENCH_ASSETS="$PWD/.cache/assets"
pnpm exec tsc --noEmit
pnpm build
pnpm serve
```

In another terminal with the same two environment variables:

```sh
node run.mjs --modes=reactflow,react-dom,pixi,leafer,pixi-webgpu,gpucanvas \
  --counts=1000,5000 --gpu=intel-coherent --tour-ms=10000 \
  '--query=floor=256&view=working&retention=budget' --out=results/working
```

`hardware.mjs` contains the qualified local Intel configuration. It is a test configuration,
not a claim that these flags are needed or appropriate on every machine. First qualify solid,
photo, and text Canvas2D pixels, WebGL pixels, and real non-fallback WebGPU pixels using
`probe-intel.mjs`. `probe-gpu.mjs` and `probe-hardware.mjs` retain diagnostic experiments.
Do not accept a software adapter as a hardware GPU result.

The original local runs used isolated Chrome-for-Testing 157.0.8103.78 downloaded from:
`https://storage.googleapis.com/chrome-for-testing-public/157.0.8103.78/linux64/chrome-linux64.zip`.
The installed Chrome and the user's browser profiles are not modified.

## Workloads and controls

- `view=working&floor=256`: production-like overview, 256px source floor, adaptive 512px sources when needed,
  48 cards initially visible including overscan. Total scene size varies independently.
- `view=fit&floor=256`: all-visible stress test; keep separate from ordinary working-view results.
- `tier=256`: diagnostic fixed-source-resolution control, not evidence for lowering visual quality.
- `pool=8`: diagnostic shared-texture control; default has distinct photographic resources for all cards.
- `cacheMiB=16`: Pixi/Leafer source-cache tradeoff; default 64, visible sources stay pinned and overflow is reported.
- `retention=budget`: GPUCanvas keeps offscreen source images under a 64MiB nominal budget; `retention=strict`
  detaches sources as soon as they leave overscan. `atlasSize=2048` tests 80MiB nominal atlas capacity versus 320MiB default.
- `rfLayer=1`: explicit React Flow viewport compositor hint; default 0.
- `rfHandles=unknown`: diagnostic unknown-port initialization; default optimized known-empty ports.
- `rfMode=controlled`: retained historical adapter for reproducing the invalid measurement-reset baseline.
  Do not use it as the production React Flow comparator.

`diagnose.mjs` records first and repeated camera tours under one-variable controls.
`features.mjs` checks projected plane picking/dragging, real glTF rendering/orbit, paused/animated rendering,
resource removal, and optional paired 3D overhead. `video-probe.mjs` checks actual video pixels inside GPUCanvas
before invoking fresh snapshots that could otherwise mask stale cached output.

## Measurement boundaries

Each case launches a fresh isolated browser and records source-file hashes, browser/GPU information,
source admission, rendered-photo pixel gates, cold-browser readiness, frame intervals, CPU time,
process PSS/RSS,500ms sampled process-memory peaks, native editor/video checks, and teardown.
Source fixtures are pre-generated and served locally: server thumbnail generation, network latency,
project auth/database/events, and the full production card UI are outside this renderer comparison.

CPU 100% means one fully used logical core. PSS is apportioned process RAM, not total resident GPU memory.
Do not add nominal texture bytes to PSS: unified-memory accounting may overlap.
Frame intervals are paced by headless Chrome at roughly 60Hz; this is not maximum uncapped throughput or
hardware input-to-photon latency. Input-to-second-RAF is only a browser callback proxy.
Cold-browser readiness does not imply cold OS files or a cleared global GPU shader cache.

React Flow's optimized adapter uses public measured bounds, known-empty port handles, an uncontrolled
node store, and display context only for mounted cards. Known-empty handles are an additional optimization
beyond the inspected production source. The benchmark owns common input across all adapters; built-in graph
editing, multi-selection, undo, collaboration, IME, accessibility, and complete product integration are not
proved by these prototypes.

Failed and excluded receipts are retained. The initial headed NVIDIA/Vulkan cohort had broken Canvas2D;
Leafer's blank output and GPUCanvas's black color parsing invalidate that cohort. Early controlled React Flow
runs reset measurement metadata and generated offscreen request floods; those do not establish production costs.

GPU snapshots also record [Linux DRM fdinfo](https://www.kernel.org/doc/html/latest/gpu/drm-usage-stats.html) for owned browser processes, deduplicating device/client IDs. Resident/shared client aggregates and engine duty are separate from PSS; shared buffers can still appear in multiple clients. These are driver counters, not a portable VRAM-total query.
