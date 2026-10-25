# Bearer forwarding after a controller restart

An accepted remote download must keep receiving the controller's refreshed Hub bearer after
Calcifer restarts. Reattaching its observer must not block on the Hub, change the run's identity,
resubmit it, or prevent cancellation and outcome delivery when authentication fails.

## Failure receipt

Production run **17344** (`margarette`, RTX 3090, Qwen Image 2.1 int8, 2026-10-25) was accepted
at 02:36:57 UTC. Calcifer stopped at 02:37:09 with two owed runs and resumed both. TensorD then
logged 260 credential refusals for the owner's bearer, whose advertised expiry was 02:39:20.
The executor ultimately failed reading object
`c0284b582e14987fbd3d5a2cb2bd139084371ed9acbae488829a1c900833c680` with HTTP 401.

Calcifer's forwarding registrations were lost at restart. Accepted runs reattached without
registering them again. TensorD's own refusals distinguish this failure from Runtime cr-224,
where TensorD kept receiving tokens but a busy executor did not read its bearer frames.
Worker timestamps were about 24 seconds behind the controller; do not compare their absolute
timestamps as if they share a clock.

## Real release-gate procedure (pending)

The release coordinator owns the default `~/.cozy`, shared controller, and named test rental.
Do not restart that controller during another session's work. No alternate home, raw HTTP,
mock Hub, or second daemon is used.

1. Use a real Hub, account and fresh remote store. Prefer a dedicated proof Hub with ordinary
   short-lived credentials; production credentials otherwise require a download that remains
   active past their expiry. Record exact CLI/TensorD versions and both clocks.
2. Start `cozy --json model download <model> --lane <lane> --on <machine>`. This uses TensorD's
   download path and isolates the controller repair from Runtime's separate Load-reader fix.
   Record its run ID and nonzero transfer progress before restarting anything.
3. Perform the coordinator's normal CLI update/restart, while leaving the remote machine and
   accepted run alive. Reattach with `cozy --json run show <run>`. Capture Calcifer's restart
   and resumed-run log, then cross at least two ordinary credential refresh points.
4. Require continued byte progress and successful completion with the same accepted run ID
   and one attempt. Capture machine logs proving no credential-expiry failure; repeat the
   download to verify the completed store is reused.
5. Repeat with a second machine already holding a live bearer before the first reattaches.
   The resumed machine must receive the current bearer immediately, without waiting until
   the other machine's next refresh point.
6. Exercise refusal with a dedicated proof account: after acceptance, make that account's
   credential unavailable and restart the controller. The observer must still deliver the
   machine's actual outcome and allow `cozy run cancel <run>`; it must not synthesize a new
   failure, resubmit the work, or block while minting a bearer.

Record the run IDs, exact candidate commit, transfer counts, credential-refresh boundaries,
attempts and results in the PR's `## Proof`. Source checks alone do not pass this cell.
