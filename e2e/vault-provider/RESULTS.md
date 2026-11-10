# Provider execution evidence

These real native-provider checks run in disposable GitHub-hosted machines, using mounted filesystems, actual OS storage providers, SQLite and ffmpeg. They qualify the provider and packaging. The complete installed Creator CLI/WebUI and Calcifer session policy remain the combined product gate in PR #2612.

## Qualified runs

| Platform | Source SHA | Run | Scope |
| --- | --- | --- | --- |
| Linux x86_64 | `88632aa3718a0f469e94bd86cb67db91ea2e90fc` | [42809936629](https://github.com/cozy-creator/cozy-creator/actions/runs/42809936629) | Bundled securefs, SQLite/media/lock/remount/controller kill |
| Linux ARM64 | `88632aa3718a0f469e94bd86cb67db91ea2e90fc` | [42809940765](https://github.com/cozy-creator/cozy-creator/actions/runs/42809940765) | Same; native ARM64 helper |
| Windows x86_64 | `88632aa3718a0f469e94bd86cb67db91ea2e90fc` | [42809839677](https://github.com/cozy-creator/cozy-creator/actions/runs/42809839677) | Bundled securefs + WinFsp 2.1, SQLite/media/lock/remount/controller kill |
| Windows ARM64 | `88632aa3718a0f469e94bd86cb67db91ea2e90fc` | [42809843162](https://github.com/cozy-creator/cozy-creator/actions/runs/42809843162) | Same; x64 helper under Windows11 ARM emulation |
| macOS26 ARM64 | `48a92fe1c582c5a21b873477468f581dfcf413d9` | [42809661772](https://github.com/cozy-creator/cozy-creator/actions/runs/42809661772) | Native encrypted APFS, SQLite/media/lock/remount/controller kill, supervisor kill/reconciliation |
| macOS26 Intel | `48a92fe1c582c5a21b873477468f581dfcf413d9` | [42809665451](https://github.com/cozy-creator/cozy-creator/actions/runs/42809665451) | Native encrypted APFS, SQLite/media/lock/remount/controller kill, supervisor kill/reconciliation |

All six jobs completed successfully on 2026-11-10; their result artifacts were downloaded and checked. The native macOS source is byte-identical between the two listed SHAs. The later fixture adds a fresh-read check after successful unmount, relevant to Windows, and documentation. No complete-product release is implied by this matrix.

## What ran

- A random internal 43-character secret passed over stdin. Linux/Windows used a checksum-pinned embedded securefs 2.0.0 lite helper; the fixture verified the extracted executable and redistribution notices. macOS used system `hdiutil`, with the single password NUL-terminated and no keychain-store operation.
- Actual file create, write, fsync, read, random seek, overwrite and rename of 1,116,000 bytes; byte-identical content after complete unmount/remount.
- A real SQLite WAL transaction with `synchronous=FULL`, checkpoint, database reopen, then full encrypted unmount/remount and payload equality. This verifies successful fsync/FULL operations and clean-remount persistence; it does not simulate a hardware power loss.
- ffmpeg wrote a two-second synthetic video through the mounted path, then sought and decoded a frame.
- Linux and macOS returned Busy for a held file and preserved the mount; closing that handle permitted normal locking. Windows accepted normal unmount with the held handle; an actual fresh read through that previously-open unread handle then failed as required.
- Read-only views enforced on Linux/macOS; Windows read-only requests explicitly refused rather than silently granting writes.
- Helper idle timeout enforced on Linux/Windows. Native APFS explicitly requires Calcifer-managed session expiration; it does not pretend to implement an independent idle timer.
- After normal unmount, the mount status was absent and writes through the absent/sealed path could not fall through to plaintext storage.
- Controller SIGKILL removed the owned provider/mount and refused private reads and fallback writes. macOS checked both the mount table and image registry: a hidden unlocked kernel image was not accepted as locked.
- Both Mac architectures also killed the supervisor while the controller survived, called `Mounted::reconcile()`, and required its exact image/device association to disappear.
- The known synthetic marker and private media filename were absent from inspected encrypted backing. This is a functional observation, not a cryptographic audit.

The Mac backend is a different physical format. Raw APFS vault copies require macOS; these runs do not claim raw ciphertext interoperability with securefs. Logical Automerge and decoded media bytes remain independent of local storage format. Native disk-image confidentiality is not described as authenticated per-file AEAD. The kernel attachment limit after simultaneous forced controller and supervisor death is documented in the [provider README](../../crates/project-mount/README.md).

## Corrected failures retained

- The first Linux development receipt used securefs's custom `ismount`, which could overlook a disconnected stale kernel FUSE mount. That receipt is superseded. Linux now checks kernel mountinfo; normal locking uses OS unmount, and lazy detach is restricted to cleanup after provider exit. Corrected local proof02 and the hosted runs passed.
- The initial macFUSE FSKit route was tested with its signed modules enabled only for the disposable test user and settings restored afterward. [42804959021](https://github.com/cozy-creator/cozy-creator/actions/runs/42804959021) failed at actual `File::create` with ENOSYS78, before write/fsync. No I/O check was waived; native APFS replaced that route.
- The first native Mac run rejected the `APFSX` alias. `hdiutil` uses `Case-sensitive APFS`; corrected runs created and mounted the encrypted image.
- Native runs [42808967626](https://github.com/cozy-creator/cozy-creator/actions/runs/42808967626) and [42809020622](https://github.com/cozy-creator/cozy-creator/actions/runs/42809020622) passed normal I/O but failed controller-kill cleanup. APFS attach output and image registry could name different device layers. Recording and checking identity from the same image registry fixed cleanup; the later controller-kill proofs passed.

Rerun (no account, rental, daemon, model or user project):

```sh
python3 e2e/vault-provider/run.py --probe <built-vault-provider> --securefs <prepared-provider> --artifacts <new-output-directory>
```

The existing manual `dev-build` workflow provides `build=vault-provider` for each standard runner. Release preparation verifies helper archive hashes at download and build, embeds them in the single self-updating Cozy executable, and retains license notices. Linux optional encryption requires glibc2.34+ and FUSE2; ordinary projects retain the CLI's existing platform floor. macOS needs no extra filesystem driver; Windows requires WinFsp and the x64 VC runtime.
