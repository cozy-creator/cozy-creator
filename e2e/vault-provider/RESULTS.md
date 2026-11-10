# Provider execution evidence

The native provider fixture passed on Linux x86_64 on 2026-11-10. This qualifies the actual mounted filesystem adapter, not the complete Creator CLI/WebUI or another OS.

- securefs 2.0.0 lite format, official dynamic Linux archive pinned by `helper/manifest.json`.
- Real file create/fsync/read/random seek/rename/overwrite, 1,116,000 bytes, exact remount parity.
- ffmpeg wrote a two-second synthetic video through the mount and sought/decoded it.
- A normal lock with an open file returned Busy and preserved the live mount; closing the handle allowed a normal unmount.
- Read-only and two-second idle views behaved as requested.
- An unmounted view refused plaintext fallback writes.
- Killing the controller process stopped its provider, removed the kernel FUSE mount, refused private reads and refused fallback writes.
- The known synthetic content marker was absent from inspected backing files; this observation is not a cryptographic audit.

Command (no daemon, account, rental or user project):

```sh
python3 e2e/vault-provider/run.py --probe <built-vault-provider> --securefs <pinned-securefs> --artifacts <new-output-directory>
```

The first development receipt used securefs's `ismount` command, which failed to distinguish a disconnected stale kernel mount. That receipt is superseded. The corrected implementation uses Linux kernel mountinfo and a normal OS unmount; lazy detach is restricted to cleanup after the provider exits. The corrected proof02 passed and left no test mounts.

macOS/Windows installed driver execution is an explicit remaining gate. Use the existing manual `dev-build` workflow, `build=vault-provider`, separately on each supported runner. Full product proof remains the combined CLI e2e run.

Fixture kernel: `7.0.0-42-generic`; helper executable SHA-256: `4d60723df8a8a590f49f65173b6354f91906bb4ab7a103af572aaac99197faba`.
