# Shared Fill and Arena retirement source checks

Product source: `569649a8cdcd4db8214624e4d08866e27700a954`.
Draft PR: https://github.com/cozy-creator/cozy-creator/pull/2426
Owner: `/root/async_integration_review`.

- Native library and six-mode async example: PASS, 1.27 seconds.
- Complete `tensorfs-py` bindings: PASS, 3 minutes 41 seconds with a fresh independent target.
- Both commands used locked/offline Cargo, one build job and nice19. Existing vendored faer
  warnings are recorded; they are not new Varena diagnostics.
- The original five fixture cases are byte-identical to8b after removing the added module
  and dispatcher line. The sixth real GPU case is `arena_retirement`.
- P3 owner reviewed the queue composition. Its one fixture finding was corrected: a refused
  Arena handoff preserves the returned Arc before the isolated helper reports failure.

These are source compilation checks only. No GPU mode ran, no portable shared-library cohort
was built here, and no rental or installation occurred. The manifest records exact input
commits, source hashes and log hashes. General Source/TensorFS asynchronous adoption and the
external P4 public adapter remain separate work. Keep this PR draft.
