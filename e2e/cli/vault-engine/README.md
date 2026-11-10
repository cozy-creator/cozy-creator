# Project-vault engine proof

This fixture executes actual Argon2id/AES-GCM encryption, keybag persistence, slot/context authentication, wrong/tampered credentials, wrapping-slot replacement, fresh generations, bounded
parsing/KDF requests and real-clock idle/absolute expiry. It uses synthetic data and the
production Argon2id cost (256 MiB, three iterations); there are no mocks or reduced-cost crypto parameters.

Run with a new owned output directory:

```sh
nice -n 19 cargo run --manifest-path e2e/cli/vault-engine/Cargo.toml --locked -j 1 -- /path/to/proof-output
```

The native fixture is only an engine/API proof. It does not qualify Calcifer's lock routes,
plaintext-cache coverage, a mount driver, a player or cross-platform packaging. Those use the
installed candidate through `e2e/cli/run.sh --installed --shard projectvault --out ...`, which
runs the actual CLI, native mounted media and the real browser flow without mocked HTTP.
Coordinate the installed candidate with the release owner: all of this uses the default
`~/.cozy`, not a second daemon or an isolated `COZY_HOME`.

The CLI/browser proof generates and removes its own credential files. It does not emit
recovery secrets, take recovery-code screenshots, rent compute; the Linux CLI shard also runs a real private CPU fixture before rekey. Its
project fixtures and encrypted native receipt are deliberately synthetic.
