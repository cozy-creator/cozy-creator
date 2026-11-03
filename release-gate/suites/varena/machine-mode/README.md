# Ordinary Shared/Dedicated mode propagation

This completes the missing cl-694 route in source. It does not add a byte control.
The CLI sends typed `mode`, the existing update waits for the normal drain, commits
the requested machine settings in one validated config replacement, and restarts.
Mode-only updates do not select a new SDK or take the unchanged-software shortcut.
Success requires a live post-restart `status.mode`; a software `to` record cannot
stand in for that observation. Old workers reject the unknown update field before
draining. Install a supporting worker first using the ordinary update route.

Root builds/operates the candidate CLI and worker. Use the default home and the
existing daemon; this fixture creates neither a daemon nor a rental and installs no
software. Stage the two-app bundle with this branch's
`../idle-lifecycle/stage.py`, binding the exact unchanged SDK versions, extension
hash and actual GPU UUID. Then:

```bash
python3 prove.py --cli /absolute/candidate/cozy --on OWNED \
  --bundle /absolute/bound-bundle --out /absolute/mode-proof
```

The script saves ordinary Shared and Dedicated update receipts, requires each live
machine report, and runs a GPU app under each setting. The actual executor-created
Runtime owner and its native row must match; the app never calls configure. The
original effective mode is restored through the same route in `finally`. A failed
update is preserved and cleanup still attempts to observe/restore the setting.
This restores the effective mode, not whether an older config expressed its default
implicitly. No GPU qualification follows from source compilation or fixture staging.

The task still needs categorical priorities. Start currently sends `priority:0`;
this mode setter neither changes that constant nor proves active/idle priority
ordering. Cache owner action, foreign attribution/latency and legacy TensorD byte
controller retirement remain separate gates. The existing sampler27e/Runtime907
and lifecycle616 checkpoints remain frozen in their own worktrees.
