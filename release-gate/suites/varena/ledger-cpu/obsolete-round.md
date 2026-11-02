# Obsolete round regression

Build the actual `varena` example `ledger_peer`, then run
`obsolete_round.py PEER NEW_OUTPUT_DIRECTORY` against the preserved pre-fix source,
or add `--fixed` for the withdrawal implementation. Three real processes own real
OFD row locks in a private tmpfs. Donor A is SIGSTOPed; B changes Dedicated to
Shared at lower priority. The supplied byte counts are metadata, not GPU evidence.

Before the fix, the second fresh ask still joins A's unfinished round. After the
fix, only the original requester's exact-generation withdrawal aborts that round;
the next ask selects B. The driver sample and completed-release count are unchanged.
A foreign joiner can clear its own want without aborting the original round or
clearing its requester's demand. No elapsed time is used as proof of process death.
