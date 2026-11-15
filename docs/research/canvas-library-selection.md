# Creator WebUI canvas library selection

Status: research in progress; no renderer implementation change.

## Scope and priorities

Select the canvas architecture and library for Creator's WebUI from a fresh comparison. Prioritize interaction speed, memory use, CPU/GPU cost, and idle cost. Compare the current React Flow implementation against GPUCanvas, Dessn's published approach, PixiJS, Konva, tldraw, graph-editor alternatives, and a custom renderer. Assess browser and native webview support, media-heavy content, editor behavior, and implementation ownership.

## Research workspace

- Owner: Codex root, with assigned research agents contributing evidence.
- Purpose: source-backed library selection; no migration authorized by this document.
- Branch: `research/canvas-library-selection`.
- Base: `b7fb810bdfdfb5a9f8a0d55d3d0c13c42fea4cc9`.
- Worktree: `/home/fidika/cozy/.worktrees/cozy-creator/canvas-library-selection`.
- Sources inspected: current `origin/master`, official project documentation, source repositories, browser implementation documentation, and the supplied Dessn post.

## Evidence boundary

Source inspection and published measurements do not establish Creator performance. A recommendation must identify its unmeasured assumptions and the matched workloads required to validate it. No benchmark results have been produced for this comparison yet.
