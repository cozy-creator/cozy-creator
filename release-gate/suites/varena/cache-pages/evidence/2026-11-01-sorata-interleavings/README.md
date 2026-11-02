# Preserved cold-start and telemetry/ownership fixture findings

25557 proves the first real Torch OutOfMemoryError under the held GIL, with callback
marker0, followed by actual Cache release and one callback return. The later resize
failed an assertion that did not include its C result. The diagnostic follow-up records
both typed results and all limit/working/Cache snapshots. It also waits for the actual
Runtime int8 startup producer before baseline; the identified background prime allocated
25,600 bytes in a separate source/counter reproduction. No cap or16MiB operation is enlarged.

25560 reaches B retirement and actual A stage2 while the callback runs, then fails an
assumption that the earlier Cache owner must have declined. Componentless NativeSequence
owners/pins do not imply a persistent global Varena replay guard. The ordinary case now
records actual release/refusal and preserves the sequence/output/escaped-alias gates.
A separately labelled actual-live-sequence case explicitly holds Varena's replay scope
and requires the real first-owner refusal plus later logical retirement.

25563 reaches actual output allocation/full readback, a driver OOM, zero remaining Cache
mapping and one callback inside the call interval. Its peer episode reports exhausted
with zero peer bytes, which the fixture had incorrectly required to say local release.
Recovery::next checks the independent local release epoch and permits a final actual
retry; a peer report is not a capacity grant. The follow-up preserves physical/byte/readback
requirements and records the exact peer outcome, never callback-return credit.

All three original failed runs are preserved. These observations are partial evidence,
not completed positive fixtures. Native/helperdfb and Runtime2626 remain unchanged.
