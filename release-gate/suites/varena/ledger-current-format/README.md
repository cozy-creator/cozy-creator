# Current ledger format

The real native `ledger_peer` example links the same dynamic Varena core as other
clients. Build it with `RUSTFLAGS='-C prefer-dynamic' cargo build --offline --locked
-j1 -p varena --example ledger_peer`; set `LD_LIBRARY_PATH` to its `target/debug/deps`
and the matching `rustc --print target-libdir` when invoking it directly.

Run `python3 prove.py --peer <ledger_peer> --old-peer <frozen-v2-peer> --output <new-directory>`.
The old peer is only a rejection control, built from the preserved
`c3bf579ae82b42b254aa30fe54c609fc8e89afcb` source with the historical ledger-v3 gate.
It is never linked into the current product. All temporary tables are task-owned in
`/dev/shm` and all subprocesses are reaped. No device is initialized or GPU byte credited.

The gate checks live current reopen, wrong version and size, unpublished initialization,
real empty/live v2 refusal with identical inode and bytes, nonblocking publication refusal,
and a pathname replacement while a real opener is observed waiting in `fcntl_setlk`.
The fixture replaces only its own empty table; product code never replaces a table.

Committed host evidence includes the broader existing ledger/fork proof and stopped
publisher checks. Its first run used an obsolete v2 reader offset and failed to observe
an in-flight write. That receipt remains; after updating the fixture to the current
v3 layout, it intercepted a genuine uncommitted bank and proved recovery. The product
binary was unchanged. User-namespace transition was denied by the host and remains unrun.
