# cl-991 canvas migration — implementation in progress

Owner: Codex root. Branch: `feat/cl-991-pixi-canvas`. Release base: `b7fb810bdfdfb5a9f8a0d55d3d0c13c42fea4cc9`; refreshed integration base: `bd43af76eb4fcf32878d2719d083c0783a00fc15` (`rc/0.26.0`).

[Tracker plan](https://github.com/cozy-creator/tracker/blob/master/tracker/cozy-creator/cl-991-pixi-canvas-migration.md) owns acceptance. Implementation is not yet qualified.

The application store remains the document/history owner. A small viewport controller owns logical card boxes and camera transforms; pixi-viewport handles navigation. PixiJS WebGL draws passive cards and cached media. React keeps focused editing, title inputs, menus, active media and accessible controls. No ports or connector engine is introduced.

Assigned lanes: viewport/controller (`feat/cl-991-viewport`), scene/media (`feat/cl-991-pixi-scene`), root product integration, qualification inventory. Each lane has one writer; root integrates commits and owns all browser runs. Automated browser runs remain headless.
