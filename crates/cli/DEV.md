# Dev-only CLI features

Ours, not the official interface: in no `--help`, and refused unless `~/.cozy/config.toml` has, at
its top (above any `[section]`):

```toml
dev = true
```

Anyone may turn them on; the Hub does not gate them. Mark a new one in `src/dev.rs` (`DEV_ONLY`)
and list it here.

| Command | What |
|---|---|
| `cozy machine rent <sku> --provider runpod\|vast` | Quote and buy on that provider only. The Hub prefers a datacenter holding one of our NFS drives; the quote names provider and datacenter, and `cozy machine show` (`--json`: `rental.provider`, `rental.datacenter`, `rental.nfs_drive`) says where it landed and whether the drive mounted. |
