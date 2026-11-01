# V3 owner actions and ordinary machine mode: independent audit

Read-only reviewer: `/root/p3_owner_restart`. GPU operator: `/root`.
Branch: `codex/xs138-p3-v3-gpu-audit`, from freshly fetched
Creator `1b77958bdfb3f699db8340951c9b5cc1805d4243`.
No product source, installed software, rental or GPU state was changed by this audit.

## Exact cohort

- Native `0657802458dfbf4fb7e66613e30c9baef1308e68`, TensorFS `0.17.16+dev.g06578024`.
- Runtime `6c8122412bf502f2913c35eaba6cf9d263d314b5`, version `0.36.23+dev.g6c812241`.
- Worker and mode CLI `f57bf6eaa7ad94ec9f3e66d1d7b85b5ba6ae4784`, version `0.15.7+dev.gf57bf6ea`.
- Owner fixture `6e85733ef6d3cea3697b30c10fb4eef1cf9c1297`; mode fixture
  `fea8109ed8318e6a00015cacb8f6051195278037`.
- Sorata, RTX 4090, UUID `GPU-846689c6-daf3-15b1-580e-f2a78bdf6748`, driver 595.91.07.
- Torch 2.14.0+cu130 / `08187d9e0fba026dc8217405802ab5381dc88d90` is asserted by the executed owner fixture.

The audit rehashes all staged fixture files against their Git objects, both wheels
and the CLI binary, the executed Runtime owner module, TensorFS extension and Torch
shim. Raw run detail equals the separately extracted result. Exact hashes and
asserted values are in [audit.json](audit.json); the verifier is [audit.py](audit.py).

Ordinary update **25735** completed from a96/v2 before the v3 owner run. Its recorded
command uses update/drain/restart and no table-reset command. The fixture successfully
asserts protocol 3. This cell does not contain before/after table images; the v2→v3
locking/tombstone migration proof remains the separate frozen CPU protocol evidence.
No downgrade or second table namespace is inferred or authorized.

## Owner action run 25737: pass

The actual Runtime owner was Active on PID 232527, birth tick 111795946, row
`{slot:0, incarnation:1}`. It had 64 MiB of live Cache, zero immediately reclaimable
bytes, and a requestable owner. An independent requester selected it in Allocation
round 34 for an owner action with zero ready offer.

The first callback declined after an actual OFD publication lock was held. While
service publication was excluded, the fixture freed and trimmed **32 MiB of unrelated
working backing**. Its Cache remained 64 MiB and the second callback did not run.
After publication resumed, the second callback released ownership of 32 MiB behind
the actual CUDA stream fence. The capped native service completed exactly **33,554,432
bytes**, with no pending credit and one callback of each kind. A returned callback
or unrelated global release did not supply this donor's byte credit.

The surviving 32 MiB Cache range and 8 MiB working allocation passed full readback;
the requester then allocated and checked another 32 MiB. Its final allocated,
mapped, pending, pool and event-handle values were zero. The donor returned exactly
to its initial eight asserted ownership/accounting values: allocated 17,039,360,
mapped 20,971,520, with no pending bytes, pools, event handles or Cache reservation.
Context joins, event waits, terminal OOMs and physical OOMs stayed zero. The fixture
ran for 11,511 ms; it is a mechanism check, not a model benchmark.

Qualification boundaries:

- The requester explicitly asks; this is scoped native owner servicing, not a new
  automatic physical-OOM recovery proof. The donor temporarily opts into Shared
  inside this fixture and restores its original setting.
- The shared-device usage sample fell 46,858,240 bytes; this is not attributed donor
  credit. Exact donor credit comes from the capped pass and managed physical state.
- `Requester.close` kills the owned child if still running and waits for exit. This
  does not prove the child's graceful `Peer.close` completed before termination.
- Final baseline is task-owned quiescence, not zero whole-process GPU use. No
  foreign-growth/H reserve, full-model or new shared-client `db701` claim is made.

## Ordinary machine mode: pass

| Update | Probe | Live worker mode | Runtime Start | Actual native row |
| --- | --- | --- | --- | --- |
| 25739 | 25741 | shared | `shared=True`, priority 0 | Shared, slot 0/incarnation 3, PID 232721 |
| 25742 | 25743 | dedicated | `shared=False`, priority 0 | Dedicated, slot 0/incarnation 4, PID 232771 |

Both updates completed through the frozen f57 CLI's normal typed `--mode` route.
Software wheel hashes stayed unchanged. Worker start times advanced and the probe's
worker-parent and executor births changed. Each probe saw an active actual Runtime
owner and matching PID/birth in its native row. No fixture calls `Owner.configure`
in this mode proof. The final live status is restored **dedicated**.

This qualifies propagation through ordinary restart and cold Start. It does not
qualify in-place mode switching while a donation runs, numeric priority behavior,
or exact prior-executor exit timing. The probe retains a 16 MiB tensor and checks
its endpoints; it is not full-range content validation. The periodic row physical
snapshot can precede that tensor allocation and is not used as an exact live-size
assertion. The parent reports libcuda loaded, so import-only parent behavior is
not claimed.

## Reproduce this audit

From this worktree, with original preserved workspace receipts:

```sh
nice -n 19 python3 release-gate/suites/varena/evidence/ledger-v3-gpu-20261101/audit.py /home/fidika/cozy_v2
```

Raw receipts are copied beneath `raw/`; their original paths and SHA-256 values
are recorded in audit.json. This branch carries evidence only. No merge, release,
broader P3 completion or provider-cleanup result is claimed by the reviewer.
