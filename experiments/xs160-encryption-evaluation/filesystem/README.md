# Local encrypted filesystem evaluation — throwaway

Question: does an existing encrypted filesystem expose the view/lock behavior required by xs-160 without decrypting the project directory into ordinary backing files?

## Observed Linux result

Executed `evaluate.py` using the official securefs v2.0.0 Linux amd64 musl-static release on Linux 7.0.0-42-generic x86_64. The binary reports Crypto++ 8.9/libfuse 29. This is a functional fixture, not an encryption audit or cross-platform qualification.

| Case | Observation |
|---|---|
| Normal application I/O | Create/fsync/read/random seek/rename/in-place overwrite preserved exact bytes for a 4,194,344-byte synthetic file. |
| Media application | ffmpeg wrote a 2-second 320x180/24-fps MPEG-4 file through the mounted path; ffprobe read it and ffmpeg sought/decoded a frame at 1 second. No GUI player was exercised. |
| Backing inspection | Known synthetic marker and expected plaintext filenames were absent from the eight backing files. This scan is not a confidentiality proof. |
| Lock with an open handle | `fusermount -u` returned 1, `Device or resource busy`; view stayed mounted. It must not be labeled locked at this point. |
| Lock after closing handle | Normal unmount succeeded; mountpoint remained an empty directory and files were inaccessible there. |
| Unlock again | Remount reproduced the exact modified file bytes. |
| Idle lock | A configured 3-second idle timeout unmounted after 3.031 seconds in this single sample. |
| Cleanup | No mount or provider process remains; encrypted fixture backing files were preserved. |

The exact receipt is `results.json`. The tiny synthetic video is only a filesystem functional check, not H3 quality/performance evidence.

## Reproduce

Download the official binary separately; it is not vendored here:

```bash
gh release download v2.0.0 --repo netheril96/securefs \
  --pattern securefs-linux-amd64-musl-static-release.zip --dir downloads
unzip downloads/securefs-linux-amd64-musl-static-release.zip -d bin
chmod u+x bin/securefs
uv run --no-project --python /usr/bin/python3 evaluate.py \
  --binary "$PWD/bin/securefs" --artifacts "$PWD/new-fixture"
```

Use a new nonexistent fixture directory; the script refuses reuse. It creates random synthetic passwords in memory, preserves only synthetic artifacts, and unmounts/ends its owned helper in cleanup. It does not accept or access real projects.

Tested executable SHA-256: `a9aa990b007813916b03ec6f8cb83cdc5516bf7950ce432f841b86129b0cf8e5`.

## Rust fit and remaining selection work

securefs is a useful MIT-licensed cross-platform CLI/provider comparison, not a native Rust library or a selected production dependency. Official releases include Windows/macOS/Linux; those other systems were not executed here. [Source](https://github.com/netheril96/securefs), [release](https://github.com/netheril96/securefs/releases/tag/v2.0.0), [license](https://github.com/netheril96/securefs/blob/master/LICENSE.md).

Rust candidates/building blocks:

- **rencfs** exposes a Rust encrypted-filesystem library and FUSE mount on Linux. Its source currently advertises Linux, so it does not establish the required three-OS implementation. License metadata points to Apache-2.0. No rencfs execution/audit was performed in this evaluation. [Source/API](https://github.com/xoriors/rencfs), [crate](https://docs.rs/rencfs/latest/rencfs/), [license](https://github.com/xoriors/rencfs/blob/main/LICENSE-Apache-2.0).
- **fuser** provides Rust FUSE integration, not an encryption format. It is MIT licensed. [Source](https://github.com/cberner/fuser).
- **winfsp-rs** provides Rust WinFsp bindings. Its repository license is GPL-3.0, so compatibility/distribution obligations require review before selecting it. Binding availability does not prove a safe encrypted filesystem or macOS support. [Source](https://github.com/SnowflakePowered/winfsp-rs), [license](https://github.com/SnowflakePowered/winfsp-rs/blob/master/LICENSE.md).
- **Cryptomator CryptoFS** is a Java filesystem library with an established encryption format and desktop platform integration. It is a useful format/behavior reference but not evidence of a native Rust backend. [Library](https://github.com/cryptomator/cryptofs), [format/security](https://docs.cryptomator.org/security/architecture/).

Decide an existing provider/helper versus a reusable Rust core plus OS adapters after a three-OS spike, a license/distribution review, crash/fallback-write tests and the desired open-handle policy. This functional run does not select that architecture. Never use lazy unmount as proof that already-open handles and buffered plaintext have been revoked.
