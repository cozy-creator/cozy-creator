# Bundled filesystem provider

Cozy embeds the unmodified securefs 2.0.0 release executable. Its MIT license is in
`LICENSE.securefs.md`; upstream source and build scripts are available at
<https://github.com/netheril96/securefs/tree/v2.0.0>.

The upstream build manifest names Argon2, Crypto++, TCLAP, SQLite, Abseil,
uni-algo and Protocol Buffers. The upstream build also supports mimalloc and
uses doctest for its own build tests. Their redistribution notices are retained
in `licenses/`; `licenses/sources.json` records the source and digest of each
notice. SQLite is dedicated to the public domain:
<https://www.sqlite.org/copyright.html>.

The Linux archive is upstream's **musl-static** build
(`securefs-linux-<arch>-musl-static-release.zip`), built on Alpine Linux 3.22 by
upstream's release workflow. It links in:

- musl libc 1.2.5 as Alpine packages it, MIT license: `licenses/musl.txt`;
  source <https://musl.libc.org/releases/musl-1.2.5.tar.gz> and Alpine's
  patches <https://gitlab.alpinelinux.org/alpine/aports/-/tree/3.22-stable/main/musl>.
- libfuse 2.9.9, LGPL-2.1, unmodified: `licenses/libfuse.txt`; source
  <https://github.com/libfuse/libfuse/releases/tag/fuse-2.9.9>. Alpine's
  `fuse-static` patches only the separate `fusermount` program:
  <https://gitlab.alpinelinux.org/alpine/aports/-/tree/3.22-stable/community/fuse>.
- GCC 14.2's libstdc++ and libgcc, under the GCC Runtime Library Exception.

Cozy does not modify or rebuild the executable. To relink it with a modified
libfuse, rebuild securefs from the source above (upstream's `build-alpine` job
in `.github/workflows/release.yml`) and point `COZY_SECUREFS_PROVIDER` at the
result. The kernel's FUSE device and the system's `fusermount3` (or
`fusermount`) setuid helper are separately installed, not bundled.

WinFsp is a separately installed Windows filesystem driver; its own license
and installation terms apply. Native macOS storage uses Apple's system hdiutil
and APFS implementation, not this bundled executable or a macFUSE driver. Cozy
never silently changes boot/security policy.

The optional `COZY_SECUREFS_PROVIDER` environment variable selects an explicit
local securefs executable before the bundled one. It permits using a locally
rebuilt provider, including one relinked with a modified libfuse; the supported
securefs command/format version is still checked. It is a local executable
choice, never a path supplied by TensorHub or a project invitation. No runtime
network download of executable code is performed.
