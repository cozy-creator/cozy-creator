# Canvas rendering

Migration: [cl-991](https://github.com/cozy-creator/tracker/blob/master/tracker/cozy-creator/cl-991-pixi-canvas-migration.md), [PR #3320](https://github.com/cozy-creator/cozy-creator/pull/3320). Source is under qualification; this document does not establish a release or deployment.

`CanvasStore` owns documents, drafts, selections, commands, history and persisted gestures. `CanvasController` owns logical bounds and camera conversion even when cards are culled. `pixi-viewport` supplies pan, zoom, pinch and camera events. Neither renderer writes project data directly.

`CanvasSurface` owns one PixiJS WebGL renderer and its demand scheduler. `PixiScene` draws passive media, labels, selection, state and progress. A sparse React overlay mounts the card under the pointer; native title input, menus, active playback and the existing Focus editor keep their caret, clipboard, IME and media behavior. Arrow keys select logical cards and Enter opens Focus. React Flow and its provider, node measurement and gesture classes are removed. Ports and connectors are outside Creator's requirements.

Camera motion updates Pixi and the overlay transform directly. The scene culls with a 96-pixel margin; the logical document stays intact. Ordinary idle and hidden scenes stop rendering. Thumbnail arrival, store changes, fonts, resize and context restoration invalidate the scene; visible loading and render animations schedule frames only while active. Loss of WebGL pauses rendering and exposes an accessible status until the browser restores the context.

## Media ownership

The scene uses Calcifer's existing transport and prepared 512-pixel thumbnails, resized for upload with a 256-pixel floor. Zoom and DPR select higher resolutions; the old texture remains until its replacement is ready. Inactive video uses posters. Focus keeps its existing five-picture native buffer and playback ownership.

`MediaCache` owns its source blobs, ImageBitmaps and textures. Six concurrent loads are allowed. Unpinned textures have a 64 MiB LRU allowance; the total 768 MiB target is **soft**, because visible pictures remain pinned. For example, 5,000 distinct 256×144 RGBA textures alone require about 703 MiB. Diagnostics report pins, unpinned bytes and overflow; these estimates exclude driver overhead and must not be added to process memory as independent allocations. Project/private-view teardown cancels requests and destroys scene resources; it also clears cached text metrics containing private titles.

## Qualification

`e2e/cli/web/pixi.js` exercises actual imported pixels, native rename, persisted dragging, delete/undo, Focus/camera return, keyboard navigation, reload, context restoration and idle rendering. `pixi-scale.js` prepares 100/1,000/5,000 unique real-image projects through the CLI, verifies their hashes and layout, and runs three repetitions of working-view pan, Fit all, far-zoom selection, gradual zoom and return. Independent screenshot decoding checks presented pixels; scene counters alone are insufficient.

All automated browsers are headless. Optional `CANDIDATE_DIST` replaces only built static files on the existing Calcifer origin, preserving document security headers and leaving API/media requests real. Hardware benchmark receipts must identify the actual WebGL renderer and reject software rendering. Loading, warm interaction, CPU/PSS and resource estimates are recorded separately. Shared-load or CPU-affined functional runs do not establish performance. The tracker holds receipts and remaining browser/device coverage.
