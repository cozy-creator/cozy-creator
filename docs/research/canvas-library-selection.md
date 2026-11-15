# Creator WebUI canvas library selection

Status: source review complete, 2026-11-14; benchmarking pending; no application renderer change.

## Scope and priorities

Select Creator's WebUI canvas architecture from a fresh comparison, prioritizing interaction speed, memory, CPU/GPU work,
and idle cost. Existing React Flow adoption is not a reason to retain it. Compare actual media cards and editor behavior;
large counts of plain rectangles or sprites do not establish performance for Creator.

## Research workspace

- Owner: Codex root, with assigned research agents contributing evidence.
- Purpose: source-backed library selection; no migration authorized by this document.
- Branch: `research/canvas-library-selection`.
- Base: `b7fb810bdfdfb5a9f8a0d55d3d0c13c42fea4cc9`.
- Worktree: `/home/fidika/cozy/.worktrees/cozy-creator/canvas-library-selection`.
- Sources inspected: current `origin/master`, official project documentation, source repositories, browser implementation documentation, and the supplied Dessn post.

## Evidence boundary

The findings below distinguish inspected source, vendor measurements, arithmetic estimates, and architectural inference. This source-review stage produced no measured results; matched prototype benchmarking is now authorized and in progress. Creator references are pinned to the base above; external documentation was checked on 2026-11-14. Adapter readiness requires the requested thumbnail tier to be decoded and included in a draw, even if an older tier remains visible during loading. Pixi WebGPU's draw callback measures CPU render submission without a GPU completion fence or presentation guarantee.

## Decision

**Provisional choice: PixiJS v8 core for the passive overview, with an imperative renderer, a spatial index, screen-size LOD,
and byte-budgeted thumbnails. Keep the React shell and focused DOM editor.** This is the best architectural fit found
for thousands of media cards, not proof of universally fastest rendering or lowest memory. A minimal virtualized DOM
viewport may win at realistic small visible counts; Leafer is a credible challenger. Both deserve matched measurements.

Pixi makes batching and resource ownership explicit without requiring us to build a graphics backend. Its scene graph and CPU image sources can still cost memory; duplicate overview/focused assets, per-string text textures, masks, and many unique thumbnail textures can erase the expected advantage. Selection depends on the complete architecture.
Use [PixiJS v8.23.0](https://github.com/pixijs/pixijs/releases/tag/v8.23.0), the current release inspected, as a candidate pin.

## What Creator actually needs

- [CanvasHost at the base][creator-host] supplies no edges, disables node connections, and already enables
  `onlyRenderVisibleElements`. Dependency relationships in `graph.ts` serve placement and warnings; this is not currently
  a user-facing port-and-wire graph editor. Do not pay for an execution/graph framework to solve a media viewport.
- Cards contain native form controls, media, version strips, progress, menus, drag/drop, and text interaction.
  [FocusedViewport][creator-focus] already supplies a separate focused DOM surface, a useful seam for the hybrid design.
- Existing work includes memoized cards, selective signals, zoom-dependent detail/sharpness, image load concurrency,
  thumbnail sizing, hover-delayed video previews, and explicit preview source teardown. Preserve these behaviors.
- [BufferedImage][creator-buffer] keeps a selected decoded image and up to four neighboring versions per mounted image
  card, shares only in-flight work, limits global loads to two, and removes image sources on discard.
  [Media][creator-media] supplies these neighbors for ordinary image cards too. A count bound per card is not a global byte bound.
- Both thumbnail paths bottom out at 256 pixels; `Thumb` also remembers each asset's widest requested size after zoom-out.
  These policies deserve measurement independently of the renderer. Do not upload every retained version into GPU memory.
- CanvasHost's WebKit layering comment records historical 214–590 ms versus 16 ms frames for 300 cards, and a separate
  Chrome 5,000-card fit problem. These are source comments, not measurements reproduced here or a head-to-head comparison.
  Safari is a real target. No current Tauri/wry embedding was established from this Creator source tree.

## Options and tradeoffs

| Category | Option | Assessment for this workload |
| --- | --- | --- |
| Provisional choice | PixiJS core + DOM editor | Batched GPU media/chrome with explicit lifetimes and manual redraw. Own camera, selection, hit testing policy, LOD, and accessibility. [MIT][pixi-license]. |
| Required baseline | Minimal virtualized DOM viewport | React shell, indexed visible cards, CSS transforms, native media/text/forms. Serious option for smaller visible sets; avoids React Flow's graph machinery. Implemented as a benchmark baseline; valid measurements pending. |
| Required baseline | Optimized React Flow | Native HTML nodes, SVG edges/ports, keyboard/ARIA facilities; current visibility rendering is already enabled. Complex visible DOM remains a potential cost. [Performance][flow-perf], [API][flow-api], [a11y][flow-a11y], [MIT][flow-license]. |
| Credible challenger | LeaferJS | Canvas2D scene graph with partial/layered rendering and lazy images. Suitable static-board challenger; future WebGL/WebGPU plans are not current backends. [Source][leafer-source], [docs/license][leafer-core]. |
| Credible challenger | Konva / react-konva | Canvas2D scene graph, hit testing, caching, dirty layer redraw. Extra scene/hit canvases and caches affect memory; DOM overlays still needed for editing. [Performance][konva-perf], [MIT][konva-license]. |
| Conditional | CanvasKit / Skia WASM | GPU drawing, paths, and advanced paragraph shaping. More relevant if typography/vector fidelity becomes central; WASM/native ownership and payload have no demonstrated benefit for short labels/media. [Docs][canvaskit], [BSD-style license][skia-license]. |
| Conditional | GPUCanvas | Snapshots browser-rendered HTML into GPU textures while retaining DOM/React trees. Useful HTML fidelity experiment on supported Chromium; not free DOM virtualization or a cross-browser GPU replacement. [Pinned source][gpu-react], [MIT][gpu-license]. |
| Different priority | tldraw | Complete DOM-based editor with tools, bindings, history, culling, and asset LOD. Strong editor platform, no established rendering-efficiency win here; production requires an active SDK license key. [Architecture][tldraw-source], [license][tldraw-license]. |
| Poor current fit | Rete.js / LiteGraph | Graph/port editing and execution add little to the current card surface. Rete's GPU LOD example itself uses Pixi; LiteGraph's editor is Canvas2D. [Rete][rete-lod], [LiteGraph/MIT][litegraph]. |
| Inspiration | Dessn | Published system demonstrates a direction, not an inspected embeddable renderer. Public agent plugin is not its rendering implementation. [Product pipeline][dessn-pipeline], [plugin][dessn-plugin]. |
| Profile first | Custom WebGL/WebGPU | Could remove scene-graph overhead, but no profile establishes that overhead as material. Own batching, text, geometry, resources, context loss, and backend correctness only if evidence warrants it. |

DOM/SVG and Canvas2D can use browser GPU acceleration; the distinction is retained browser layout/paint versus an explicit GPU scene graph, not a claim that every alternative is CPU-only. Vendor FPS, element counts, and memory figures are not comparable across different content, versions, hardware, zooms, visual fidelity, or cache states.

## Recommended rendering and media ownership

1. Keep the document/model independent of rendering. React owns application UI, focused forms, and native editing.
   Update Pixi objects imperatively during pan/zoom/drag; avoid React reconciliation for every display object each frame.
   Maintain a world-space spatial index and cached screen transforms for visibility, picking, selection, and overlays.
2. Render visible cards plus measured overscan. At small screen sizes, simplify chrome, labels, and decorations.
   Pixi's built-in culling is disabled by default and can hurt CPU-bound workloads; compare it with indexed app culling.
   Batch compatible sprites and reusable geometry; masks, shadows, changing paths, draw order, and unique textures matter.
   [Pixi performance guidance][pixi-perf] is a starting point, not a workload capacity guarantee.
3. Use a dirty scheduler: one requested frame for camera/input changes, relevant state updates, and ready media.
   Stop when nothing changes. Pixi's default application ticker renders every animation frame; disable automatic start.
   Avoid passive cards' `dynamic` event mode, which performs ticker-driven overlap checks. [Ticker][pixi-ticker], [events][pixi-events].
4. Budget encoded bytes/cache, decoded CPU images, GPU textures, render targets, and active video decoders separately.
   Share asset/resolution ownership between overview and focus where possible. Cancel obsolete work and explicitly evict
   unneeded CPU and GPU resources. Pixi's automatic GC is not a byte cap; GPU unload alone does not release decoded sources.
   [Resource lifecycle][pixi-gc]; WebGL exposes no portable total-VRAM query. [Browser guidance][webgl-practices].
5. Keep the requested production thumbnail floor at 256 pixels for the primary comparison. Smaller 32/64/128-pixel
   overview tiers are a separate policy experiment, using actual screen size, DPR, aspect ratio, and quality.
   Prefer matching source thumbnails; if locally downsampling, release larger decoded sources when no longer needed.
   Upgrade on zoom, use hysteresis to avoid reload thrash, and evict even small thumbnails under a global byte budget.
   A cache containing every asset ever visited is not bounded merely because each thumbnail is small.
6. Keep selected-card neighbors under a small global budget rather than preloading four GPU versions for every card.
   Use static posters for inactive video. Start active playback in a native DOM overlay and measure texture-video separately.
   DOM/video-to-texture uploads can introduce processing and pipeline flushes. [Browser upload behavior][webgl-practices].
   If using Pixi video textures, connect frame callbacks to invalidation and tear down their independent update lifecycle.
   Stopping the application ticker alone is insufficient. [Pinned VideoSource][pixi-video].
7. Plan labels explicitly: Pixi `Text` rasterizes strings to textures; `BitmapText` shares glyph atlases but missing glyphs
   and large character sets need care. Test multilingual names, prompts, and emoji; sparse DOM labels or fallback text may
   be preferable to a huge atlas. Preserve full native text/IME interaction in focus. [Text][pixi-text], [bitmap caveats][pixi-bitmap].
8. Supply keyboard navigation, semantic DOM controls, announcements, and visible focus with the visual scene.
   Pixi's accessibility support uses DOM overlays; pixels alone do not provide a usable accessible editor.
   Do not carry forward `disableKeyboardA11y` without an app-level replacement. [Pixi accessibility][pixi-a11y].
   Future edges/ports can use indexed geometry and Pixi curves; they do not currently justify a graph engine.

### Pixel-memory arithmetic, not measured resident memory

| Potential RGBA8 allocation | Nominal bytes |
| --- | --- |
| One 4096 × 4096 image/texture | 64 MiB |
| 5,000 square 256-pixel textures | 1.22 GiB |
| 5,000 square 64-pixel textures | 78.1 MiB |
| 5,000 square 32-pixel textures | 19.5 MiB |

These are `width × height × 4` calculations, excluding mipmaps, alignment, decoded copies, caches, video surfaces, and framebuffers. At fit-to-all, a tile may occupy only 10–30 screen pixels; the current 256 minimum can dominate memory. Smaller tiers must preserve output at the actual display density. An efficient draw loop cannot fix oversized media residency.

## GPUCanvas: attractive technique, additional constraints

Inspected package 0.4.4 at `338a88d390dfa68ad10e43625d5fbf179f37c38e`; the README calls the API early.
Every `CanvasNode` creates a DOM container and portals its React children into it. Parked pages use
`content-visibility: hidden`; the React trees remain mounted. This reduces rendering work but does not remove the
underlying application objects or decoded media. [React adapter][gpu-react], [page parking][gpu-page].

The default five 4096² RGBA8 atlas layers imply **320 MiB nominal texture storage**, configurable to e.g. 80 MiB at 2048².
This is not measured resident VRAM and excludes DOM, decoded images/video, snapshots, and other GPU allocations.
Ordinary atlas generations recycle; pinned thumbnail slots do not individually free/recycle. This is a bounded-cache
churn concern, not an established unbounded leak. [Options][gpu-types], [atlas allocator][gpu-atlas].

Small-node and thumbnail refresh intervals trade freshness for raster work. Pixel-only video/canvas changes are not DOM
mutations; unattended media freshness and decoder lifecycle need direct testing rather than assuming snapshot invalidation
handles them. This is an inference from scheduling, not a reproduced bug. [Raster scheduling][gpu-backend].

Its published benchmark compares its own DOM fallback with its GPU path on an M-series Mac/Chrome, not optimized React Flow, virtualized DOM, or Pixi. No comparable memory/power results were established. The harness's programmatic clicks/value events do not validate physical pointer, keyboard, or IME behavior; parked-page Tab navigation is explicitly unfinished. [Benchmark source][gpu-bench], [known gaps][gpu-readme].

HTML-in-Canvas ships in Chrome 157. WebKit's current implementation preference is testable and defaults false;
Safari WebGPU support does not imply HTML-in-Canvas support. WebKit's implementation also describes image-buffer
readback and texture upload, not a universal zero-copy path. [Chrome][html-canvas], [WebKit preference][webkit-prefs],
[WebKit implementation][webkit-upload]. GPUCanvas's default GPU path is therefore not the Safari solution today.

## Dessn and other challengers

Dessn's [author post][dessn-post] claims roughly 7,000 HTML/CSS frames, over two million elements, and 120 FPS interaction. Treat this as an author claim: the inspected public material supplies no matched Creator workload, hardware/memory/CPU receipt, or reproducible renderer harness. A 60 FPS encoded video cannot itself verify a 120 FPS renderer claim.
The official product pipeline copies/builds frontend code in a hosted environment and mocks backend/auth/network state; this is a larger system than a canvas dependency. No public embeddable renderer API/source was located in the reviewed repositories. The MIT agent plugin exposes MCP/skills, not that renderer. [Pipeline][dessn-pipeline], [plugin][dessn-plugin].

Leafer is worth a matched static-board trial: its current backend calls `getContext('2d')`; WebGL/WebGPU/Skia are future pluggable backends. Its vendor comparisons show workload-dependent outcomes and do not establish a Pixi win or loss. Native editing still needs attention; its text editor currently advertises web PC support. [Backend][leafer-source], [roadmap][leafer-core], [vendor tests][leafer-home], [text editor][leafer-editor].
Konva redraws changed layers on demand, but scene/hit canvases, DPR, caches, and layer count affect memory. Its official text-editing guidance uses native inputs/textarea; DOM portals are not included in canvas image export. [Performance][konva-perf], [dirty redraw][konva-dirty], [editing][konva-edit], [DOM portals][konva-dom].

CanvasKit batches WebGL work on surface flush and supplies paragraph shaping, but adds WASM and explicit `.delete()` ownership of native objects. No short-label/media advantage was demonstrated. [CanvasKit][canvaskit], [quickstart][canvaskit-start].
tldraw's culling hides DOM shapes rather than unmounting them; asset LOD is useful, but media release remains a separate policy. Its workflow kit has ports/bindings if those become necessary. [Culling][tldraw-culling], [LOD][tldraw-perf], [workflow][tldraw-workflow].
Rete core is mostly MIT with noncommercial advanced-package exceptions; its GPU LOD example is marked Pro. Do not infer commercial module rights from the site footer. [Licensing][rete-license], [example][rete-lod].

## Migration boundary

[CanvasStore's `Flow` type][creator-store] is a `ReactFlowInstance`; node mutation, world bounds, selection, and
`screenToFlowPosition` are integration points across CanvasHost, Card, and Sheet. Replacing the renderer also means
replacing camera/geometry/hit testing, box and multi-selection, drag/drop, touch/pinch, keyboard, and clipboard/history
integration. Preserve existing project/transport/store business behavior and focused editing. This is implementation
ownership to plan and qualify separately; selecting a library does not authorize that migration.

## Browser and operational boundary

Use WebGL as the initial production Pixi baseline: current docs call it stable/recommended and WebGPU experimental. Enable WebGPU only after matched correctness/resource tests show a benefit on the actual target. [Renderer status][pixi-renderers].
Safari 26 ships WebGPU, but older Safari and OS combinations still matter. Do not gate the design on experimental HTML capture APIs. Test actual supported browsers and target OSs.
If a native webview embedding is introduced later, qualify its own engine/version; no current Tauri dependency was established here. [Safari][webkit-safari], [webview differences][tauri-webview].
The app owns context loss/recovery, resource cleanup, stale decode cancellation, and background/idle behavior regardless of library. A custom backend becomes justified only when profiling shows a material limitation in the chosen renderer.

## Matched qualification plan

First compare optimized React Flow, minimal indexed/virtualized DOM, and the proposed Pixi hybrid; add Leafer or Konva
as challengers when a real scene warrants them. Keep rendering backend and media-policy effects independently attributable.
Run the current media policy and a shared bounded/LOD policy so a thumbnail improvement is not mislabeled a renderer win.

| Dimension | Required evidence |
| --- | --- |
| Scene | 100 / 1,000 / 5,000 cards from the same actual files; identical positions, aspect ratios, labels, controls, progress, and selected state. Record total and visible counts separately. |
| Display | Same hardware, OS, browser build, viewport, DPR, refresh rate, zoom path, and visual quality. Chrome, Firefox, and Safari on their supported target OSs. |
| Interaction | Physical pointer/trackpad pan, pinch/wheel zoom, fit-to-all, drag/multiselect, context menu, version churn, focused forms, native text selection, clipboard, IME, and keyboard/a11y. |
| Timing | Input latency and frame-time p50/p95/p99, long tasks, missed frames; cold/warm scene readiness and focus/version-switch latency. Define each timing boundary and sample count. |
| Resources | Process CPU/RAM and available GPU counters alongside nominal resource bytes; peak/steady decode/texture bytes, upload/decode counts, active decoders, network bytes, and ownership. JS heap alone is insufficient. |
| Lifecycle | Repeated zoom/pan across the whole scene, add/remove/version churn, project switching, idle foreground/background, tab hide/show, media end/teardown, and context loss/recovery. |
| Correctness | Same image/alpha/color/text output, clear current-version identity, native editor behavior, no stale/blank snapshots, no missing glyphs; screenshots/video plus state assertions. |

Separate zero-video, hover preview, active playback, and live-output cases with equal concurrent media and formats.
Do not lower DPR, resolution, fidelity, or active previews to manufacture a win. Report cold/warm cache states, failures,
and cleanup behavior, including memory after returning to the starting view; account for intended cache retention explicitly.
Publish the exact renderer version/commit, scene manifest, settings, raw observations, and measured tradeoffs before claiming fastest/lowest-memory operation. This report authorizes no migration and records no passing benchmark.

## Feature and ownership matrix

These are capability/ownership distinctions, not measured performance rankings. Creator currently has no edge UI and
already separates focused DOM editing; the benchmark's common selection/input harness does not test native editor feature completeness.

| Option | Engine versus editor | Native editing / accessibility | Clipboard, undo, collaboration | Browser and maintenance boundary |
| --- | --- | --- | --- | --- |
| Minimal DOM / React Flow | DOM baseline owns camera/selection; React Flow supplies viewport, node selection and graph ports/edges. | Native inputs, selection and IME remain browser features; React Flow supplies configurable keyboard/ARIA support. App semantics still matter. | Preserve Creator's model/history/clipboard. React Flow's advanced examples are integrations, not all core features; its copy/paste example has separate Pro terms. | Standard DOM/SVG browser paths; inspect CSS/media in browser tools. React Flow core is MIT. [API][flow-api], [a11y][flow-a11y], [copy/paste][flow-copy]. |
| PixiJS core hybrid | Scene graph and GPU renderer; app owns marquee, selection, ports, dragging and document tools. | Focused native DOM forms/IME; visual labels are textures/glyphs. Accessibility needs semantic DOM and app navigation. | App owns all document operations; renderer objects must not become the source of truth. | Stable WebGL path; optional WebGPU needs separate qualification. JS scene plus GPU resources require renderer-aware debugging. MIT. [Rendering][pixi-renderers], [a11y][pixi-a11y]. |
| Leafer / Konva | Canvas2D scene graphs; optional editor/transformer tools do not replace Creator's workflow semantics. | Native DOM editor overlay for forms/IME; canvas pixels need keyboard/semantic counterparts. Leafer's text-editor plugin currently specifies web PC. | Retain app document/history/collaboration contracts; adopting optional editor packages is a separate decision. | Browser Canvas2D output must actually work, irrespective of advertised GPU status. Shape/cache/layer inspection replaces ordinary card DOM inspection. MIT cores. [Leafer][leafer-editor], [Konva editing][konva-edit]. |
| CanvasKit / custom GPU | Drawing primitives/backend, not a complete infinite-canvas editor. | Rich shaping is available in CanvasKit; inputs, IME, semantic focus and a11y remain app work. | App owns document operations and synchronization. | WebGL/WASM or custom backend bring explicit native/GPU disposal and more debugging ownership. CanvasKit uses Skia's BSD-style license; custom code owns its compatibility burden. [CanvasKit][canvaskit-start]. |
| GPUCanvas | HTML snapshot/compositing engine with selection/resize binding; not a graph/execution engine. | Live HTML retains browser affordances where supported; parked-page Tab navigation is an acknowledged gap. Qualify actual IME, video and embedded canvas refresh. | Preserve Creator's document operations; snapshots are not a history or collaboration model. | GPU path requires HTML-in-Canvas plus WebGPU; Firefox/Safari currently fall back or reject that path. Early 0.4 API, MIT. Debug DOM, snapshots and atlas state together. [Pinned source][gpu-readme]. |
| tldraw | Complete canvas editor with shapes, tools, selection and bindings; workflow kit adds ports. | DOM custom shapes plus built-in keyboard/announcements; custom controls still need accessible semantics. | Built-in shape clipboard and undo; sync library can supply collaboration, while production server/auth/media integration remain app work. | DOM browser rendering, not a sprite-engine substitute. Production SDK key/license required. [Clipboard][tldraw-clipboard], [history][tldraw-history], [collaboration][tldraw-collaboration]. |
| Rete / LiteGraph | Node/port graph editors and execution infrastructure, currently a mismatch for Creator's card surface. | Rete's DOM renderers can host controls; LiteGraph's Canvas2D UI still needs native editing/a11y integration. | Plugin/engine facilities do not define Creator's document, authorization or collaboration contract. | Core licenses are generally MIT; check Rete's noncommercial advanced-package exceptions. Debug both graph and renderer layers. [Licensing][rete-license], [LiteGraph][litegraph]. |

Across every option, qualify image alpha/color/orientation, current-version identity, live video frames and decoder cleanup.
HTML snapshotting must also prove pixel-only video/WebGL-canvas updates rather than relying on DOM mutation callbacks.
Startup includes code/WASM load, decode/upload, shader compilation and first correct output; idle means no unnecessary
render loop or decoder activity. Context/device-loss detection, recovery and cleanup are separate tests, not consequences
of a successful constructor or render callback. A Chromium-only result does not establish Firefox or Safari behavior.

## Optional 3D backgrounds, plane tilt and model viewers

A glassy/shader background can sit behind any of these surfaces as a separate Three.js canvas, with explicit opacity,
render order, resolution and pause/idle policy. A static effect can render on demand; animated refraction/particles add
ongoing GPU work and intermediate targets. Layering it does not require replacing the media canvas with a 3D editor.

Tilting the whole 2D plane is also different from putting every card into one 3D world. CSS perspective can transform the
surface as a unit, but app picking, drag deltas, wheel anchors, culling and DOM focus overlays must use the corresponding
projected plane geometry. The prototype [projection helper][bench-perspective] and main harness expose project/unproject
APIs; their presence is not yet a passing tilted-input test. A shared 3D world additionally requires depth/occlusion rules,
camera projection, ray picking and editing-overlay behavior; that is a broader architecture decision.

For an isolated interactive model, the [prototype Three viewer][bench-three] uses a separate transparent canvas,
`GLTFLoader`, `OrbitControls`, and explicit geometry/material/texture cleanup. It can start/stop animation independently.
This establishes implementation scope only until a real model is visibly rendered, orbit input works, pause stops work,
and disposal/context-loss checks pass. Model loading success is not image proof. [glTF loader][three-gltf], [orbit controls][three-orbit].

Avoid a separate WebGL renderer/context for every card: browser context limits and duplicated resources become a distinct
constraint. Three's official multi-scene approach uses one renderer with visible viewport/scissor regions. That would be
a separate model-viewer architecture to compare if many cards need simultaneous 3D. [Three multiple scenes][three-scenes].
Pixi documents sharing one **WebGL** context with Three, coordinating render order, clear behavior and renderer state resets;
it explicitly does not make their texture objects interchangeable. This is a possible optimization, not proven here,
and does not establish shared WebGPU-device integration. [Official Pixi/Three guide][pixi-three]. Three's WebGPU renderer
can itself fall back to WebGL2, so record the actual backend and separately qualify shader/material and output parity.
[Three WebGPU guide][three-webgpu].

## Current benchmark interpretation and exclusions

The primary production comparison is `floor=256&view=working`: normally about 20–50 visible media cards, while total
document size varies. `view=fit` is separate all-visible stress. A result for one must not stand in for the other.
Lower thumbnail floors and repeated-asset pools are separate controlled experiments, not the production baseline.
Working tours still cross 256/512 tiers; report which detail sources are cold and which caches are warm.

The common harness isolates renderer/viewport costs, but does not benchmark built-in graph editing, clipboard/history,
collaboration, transport updates, dynamic progress, native IME, or accessibility completeness. Input-to-second-RAF and
animation-frame intervals are scheduling proxies, not physical presentation timestamps. Sampled peak PSS uses periodic
known-process observations; it can miss brief peaks/new processes and does not measure resident VRAM. Nominal texture
bytes, browser process memory and GPU/energy counters must retain their separate labels.

The original forced NVIDIA Vulkan cohort is excluded: ordinary Canvas2D failed solid and photograph drawing even before
Leafer loaded. Leafer's earlier `readyFraction=1` was a false positive despite blank pixels, not evidence of library speed
or a Leafer capacity limit. Original headed diagnostics remain in `outputs/canvas-library-selection/leafer-pixel-probe/`;
the [probe][bench-leafer-probe] is now headless-only and the [adapter][bench-leafer] rejects broken normal Canvas2D output.
Fresh headless coherent-hardware measurements and screenshot pixel gates are the qualification boundary. Do not reuse
the invalid cohort or claim completed tilt/3D/Safari/Firefox/context-loss tests from the base media-card run.

[creator-host]: https://github.com/cozy-creator/cozy-creator/blob/b7fb810bdfdfb5a9f8a0d55d3d0c13c42fea4cc9/web/app/src/canvas/CanvasHost.tsx#L951-L1012
[creator-focus]: https://github.com/cozy-creator/cozy-creator/blob/b7fb810bdfdfb5a9f8a0d55d3d0c13c42fea4cc9/web/app/src/canvas/FocusedViewport.tsx
[creator-buffer]: https://github.com/cozy-creator/cozy-creator/blob/b7fb810bdfdfb5a9f8a0d55d3d0c13c42fea4cc9/web/app/src/canvas/BufferedImage.tsx
[creator-media]: https://github.com/cozy-creator/cozy-creator/blob/b7fb810bdfdfb5a9f8a0d55d3d0c13c42fea4cc9/web/app/src/canvas/Media.tsx
[creator-store]: https://github.com/cozy-creator/cozy-creator/blob/b7fb810bdfdfb5a9f8a0d55d3d0c13c42fea4cc9/web/app/src/canvas/CanvasStore.ts#L200
[pixi-license]: https://github.com/pixijs/pixijs/blob/v8.23.0/LICENSE
[pixi-perf]: https://pixijs.com/8.x/guides/concepts/performance-tips
[pixi-ticker]: https://pixijs.com/8.x/guides/components/application/ticker-plugin
[pixi-events]: https://pixijs.com/8.x/guides/components/events
[pixi-gc]: https://pixijs.com/8.x/guides/concepts/garbage-collection
[pixi-video]: https://github.com/pixijs/pixijs/blob/v8.23.0/src/rendering/renderers/shared/texture/sources/VideoSource.ts
[pixi-text]: https://pixijs.com/8.x/guides/components/scene-objects/text
[pixi-bitmap]: https://pixijs.com/8.x/guides/components/scene-objects/text/bitmap
[pixi-a11y]: https://pixijs.com/8.x/guides/components/accessibility
[pixi-renderers]: https://pixijs.com/8.x/guides/components/renderers
[flow-license]: https://github.com/xyflow/xyflow/blob/main/LICENSE
[flow-perf]: https://reactflow.dev/learn/advanced-use/performance
[flow-api]: https://reactflow.dev/api-reference/react-flow
[flow-a11y]: https://reactflow.dev/learn/advanced-use/accessibility
[leafer-source]: https://github.com/leaferjs/leafer/blob/main/packages/canvas/canvas/src/LeaferCanvasBase.ts
[leafer-core]: https://www.leaferjs.com/ui/en/guide/install/ui/core/
[leafer-home]: https://www.leaferjs.com/en/
[leafer-editor]: https://www.leaferjs.com/ui/en/plugin/in/text-editor/
[konva-perf]: https://konvajs.org/docs/performance/All_Performance_Tips.html
[konva-dirty]: https://konvajs.org/docs/performance/Batch_Draw.html
[konva-edit]: https://konvajs.org/docs/sandbox/Editable_Text.html
[konva-dom]: https://konvajs.org/docs/react/DOM_Portal.html
[konva-license]: https://github.com/konvajs/konva/blob/master/LICENSE
[canvaskit]: https://skia.org/docs/user/modules/canvaskit/
[canvaskit-start]: https://skia.org/docs/user/modules/quickstart/
[skia-license]: https://skia.googlesource.com/skia/+/main/LICENSE
[tldraw-source]: https://github.com/tldraw/tldraw/blob/main/README.md
[tldraw-license]: https://tldraw.dev/community/license
[tldraw-perf]: https://tldraw.dev/sdk-features/performance
[tldraw-culling]: https://tldraw.dev/sdk-features/culling
[tldraw-workflow]: https://tldraw.dev/starter-kits/workflow
[rete-lod]: https://retejs.org/examples/lod-gpu/
[rete-license]: https://retejs.org/docs/licensing/
[litegraph]: https://github.com/jagenjo/litegraph.js
[gpu-react]: https://github.com/berkantay/gpucanvas/blob/338a88d390dfa68ad10e43625d5fbf179f37c38e/src/react.tsx#L34-L123
[gpu-page]: https://github.com/berkantay/gpucanvas/blob/338a88d390dfa68ad10e43625d5fbf179f37c38e/src/page.ts#L20-L73
[gpu-types]: https://github.com/berkantay/gpucanvas/blob/338a88d390dfa68ad10e43625d5fbf179f37c38e/src/types.ts#L7-L29
[gpu-atlas]: https://github.com/berkantay/gpucanvas/blob/338a88d390dfa68ad10e43625d5fbf179f37c38e/src/atlas.ts#L25-L189
[gpu-backend]: https://github.com/berkantay/gpucanvas/blob/338a88d390dfa68ad10e43625d5fbf179f37c38e/src/gpu-backend.ts#L431-L495
[gpu-bench]: https://github.com/berkantay/gpucanvas/blob/338a88d390dfa68ad10e43625d5fbf179f37c38e/demo/bench.ts
[gpu-readme]: https://github.com/berkantay/gpucanvas/blob/338a88d390dfa68ad10e43625d5fbf179f37c38e/README.md
[gpu-license]: https://github.com/berkantay/gpucanvas/blob/338a88d390dfa68ad10e43625d5fbf179f37c38e/LICENSE
[html-canvas]: https://developer.chrome.com/blog/html-in-canvas
[webkit-prefs]: https://github.com/WebKit/WebKit/blob/main/Source/WTF/Scripts/Preferences/UnifiedWebPreferences.yaml#L2855-L2862
[webkit-upload]: https://bugs.webkit.org/show_bug.cgi?id=327115
[webkit-safari]: https://webkit.org/blog/17333/webkit-features-in-safari-26-0/
[tauri-webview]: https://v2.tauri.app/reference/webview-versions/
[webgl-practices]: https://developer.mozilla.org/en-US/docs/Web/API/WebGL_API/WebGL_best_practices
[dessn-post]: https://x.com/eminimnim/status/2121249063576228036
[dessn-pipeline]: https://www.dessn.com/blog/how-dessn-connects-to-your-codebase
[dessn-plugin]: https://github.com/dessn-ai/dessn-agent-plugin
[flow-copy]: https://reactflow.dev/examples/interaction/copy-paste
[tldraw-clipboard]: https://tldraw.dev/sdk-features/clipboard
[tldraw-history]: https://tldraw.dev/sdk-features/history
[tldraw-collaboration]: https://tldraw.dev/docs/collaboration
[pixi-three]: https://pixijs.com/8.x/guides/third-party/mixing-three-and-pixi
[three-gltf]: https://threejs.org/docs/pages/GLTFLoader.html
[three-orbit]: https://threejs.org/docs/pages/OrbitControls.html
[three-scenes]: https://threejs.org/manual/pages/multiple-scenes.html
[three-webgpu]: https://threejs.org/manual/pages/webgpurenderer
[bench-perspective]: ../../benchmarks/canvas-selection/src/perspective.ts
[bench-three]: ../../benchmarks/canvas-selection/src/three-demo.ts
[bench-leafer-probe]: ../../benchmarks/canvas-selection/leafer-probe.mjs
[bench-leafer]: ../../benchmarks/canvas-selection/src/leafer.ts
