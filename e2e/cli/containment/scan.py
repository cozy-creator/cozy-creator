"""Read-only canary scans. Receipts expose counts and path hashes, never matched values/paths."""
from __future__ import annotations

import base64
from collections import Counter
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import sqlite3
import stat
import struct
from urllib.parse import quote
import zipfile
import zlib

from common import Failure, require, sha


class Scanner:
    def __init__(self):
        self.markers: list[bytes] = []
        self.images: set[bytes] = set()
        self.counts: Counter[str] = Counter()
        self.matches: set[str] = set()

    def add(self, value: str | bytes) -> None:
        raw = value.encode() if isinstance(value, str) else value
        self.markers.append(raw)

    def patterns(self) -> list[bytes]:
        patterns = set()
        for raw in self.markers:
            patterns.update((raw, raw.hex().encode(), base64.b64encode(raw), base64.urlsafe_b64encode(raw)))
            try:
                value = raw.decode()
                patterns.update((value.encode("utf-16le"), value.encode("utf-16be"), quote(value, safe="").encode(), json.dumps(value)[1:-1].encode()))
            except UnicodeDecodeError:
                pass
        return sorted(patterns, key=len, reverse=True)

    def found(self, content: bytes, label: str, patterns: list[bytes]) -> None:
        if any(pattern in content for pattern in patterns):
            self.matches.add(label)

    def decoded(self, data: bytes, label: str, patterns: list[bytes], depth: int = 0) -> None:
        self.found(data, label, patterns)
        if depth > 2:
            return
        for magic, decoder in ((b"\x1f\x8b", gzip.decompress), (b"x\x01", zlib.decompress), (b"x\x9c", zlib.decompress), (b"x\xda", zlib.decompress)):
            start = data.find(magic)
            if start >= 0:
                try:
                    expanded = decoder(data[start:])
                except (OSError, zlib.error, EOFError):
                    continue
                self.counts["compressed_streams"] += 1
                self.decoded(expanded, label, patterns, depth + 1)
        if data.startswith(b"SQLite format 3\0"):
            # Deserialize into RAM. No temporary copy, journal or SHM is created by this scanner.
            database = sqlite3.connect(":memory:")
            try:
                database.deserialize(data)
                for (table,) in database.execute("SELECT name FROM sqlite_schema WHERE type='table'"):
                    name = '"' + table.replace('"', '""') + '"'
                    for row in database.execute("SELECT * FROM " + name):
                        self.counts["sqlite_rows"] += 1
                        for value in row:
                            if isinstance(value, str):
                                self.decoded(value.encode(), label, patterns, depth + 1)
                            elif isinstance(value, bytes):
                                self.decoded(value, label, patterns, depth + 1)
            except sqlite3.DatabaseError:
                # Raw DB and every WAL/SHM byte are always scanned independently. An active
                # inconsistent logical snapshot is not silently certified as a logical pass.
                self.counts["sqlite_unreadable"] += 1
            finally:
                database.close()
        if data.startswith(b"PK\x03\x04"):
            try:
                with zipfile.ZipFile(io.BytesIO(data)) as archive:
                    for item in archive.infolist():
                        self.found(item.filename.encode(), label, patterns)
                        if item.file_size > 64 << 20:
                            self.counts["archive_oversize"] += 1
                            continue
                        self.decoded(archive.read(item), label, patterns, depth + 1)
                        self.counts["archive_members"] += 1
            except (OSError, ValueError, zipfile.BadZipFile):
                self.counts["archive_unreadable"] += 1
        # Chrome LevelDB SST data blocks can be Snappy compressed, unlike its raw log records.
        if data[-8:] == bytes.fromhex("57fb808b247547db"):
            try:
                import snappy
                def varint(blob: bytes, at: int) -> tuple[int, int]:
                    value = shift = 0
                    while shift < 64:
                        byte = blob[at]; at += 1
                        value |= (byte & 127) << shift
                        if byte < 128:
                            return value, at
                        shift += 7
                    raise ValueError()
                def block(offset: int, length: int) -> bytes:
                    raw = data[offset:offset + length]
                    kind = data[offset + length]
                    return raw if kind == 0 else snappy.decompress(raw) if kind == 1 else b""
                footer = data[-48:]
                _, at = varint(footer, 0); _, at = varint(footer, at)
                offset, at = varint(footer, at); length, at = varint(footer, at)
                index = block(offset, length)
                end = len(index) - 4 * (struct.unpack_from("<I", index, len(index) - 4)[0] + 1)
                at = 0
                while at < end:
                    _, at = varint(index, at)
                    unshared, at = varint(index, at); size, at = varint(index, at)
                    at += unshared
                    handle = index[at:at + size]; at += size
                    offset, pos = varint(handle, 0); length, _ = varint(handle, pos)
                    self.decoded(block(offset, length), label, patterns, depth + 1)
                    self.counts["leveldb_blocks"] += 1
            except (ImportError, ValueError, IndexError, struct.error):
                self.counts["leveldb_unreadable"] += 1
        if self.images and data[:4] in {b"\x89PNG", b"RIFF", b"\xff\xd8\xff\xe0", b"\xff\xd8\xff\xe1"}:
            try:
                from PIL import Image
                with Image.open(io.BytesIO(data)) as picture:
                    picture = picture.convert("L")
                    bits = bytes(1 if picture.getpixel((min(picture.width - 1, (2 * x + 1) * picture.width // 32), min(picture.height - 1, (2 * y + 1) * picture.height // 32))) > 127 else 0 for y in range(16) for x in range(16))
                    if bits in self.images:
                        self.matches.add(label)
                    self.counts["images_decoded"] += 1
            except (ImportError, OSError, ValueError):
                self.counts["images_unreadable"] += 1

    def scan(self, roots: list[Path], excluded_roots: list[Path], receipt: Path) -> dict:
        self.counts.clear(); self.matches.clear()
        patterns = self.patterns()
        require(patterns, "scan_markers_missing")
        excluded = [path.resolve() for path in excluded_roots]
        seen: set[tuple[int, int]] = set()
        overlap = max(map(len, patterns)) - 1
        for root in roots:
            if not root.exists():
                continue
            def unreadable(_error):
                self.counts["unreadable"] += 1
            for directory, dirs, names in os.walk(root, followlinks=False, onerror=unreadable):
                parent = Path(directory)
                dirs[:] = [name for name in dirs if not any((parent / name).absolute().is_relative_to(skip) for skip in excluded)]
                for name in dirs + names:
                    path = parent / name
                    if path == receipt or any(path.absolute().is_relative_to(skip) for skip in excluded):
                        continue
                    label = sha(os.fsencode(path))
                    self.found(os.fsencode(path), label, patterns)
                    try:
                        metadata = path.lstat()
                        if stat.S_ISLNK(metadata.st_mode):
                            self.found(os.fsencode(os.readlink(path)), label, patterns)
                            continue
                        if not stat.S_ISREG(metadata.st_mode) or (metadata.st_dev, metadata.st_ino) in seen:
                            continue
                        seen.add((metadata.st_dev, metadata.st_ino))
                        self.counts["files"] += 1
                        self.counts["bytes"] += metadata.st_size
                        tail = b""
                        data = bytearray() if metadata.st_size <= 64 << 20 else None
                        with path.open("rb") as source:
                            while chunk := source.read(1 << 20):
                                self.found(tail + chunk, label, patterns)
                                tail = (tail + chunk)[-overlap:]
                                if data is not None:
                                    data.extend(chunk)
                        if data is not None:
                            self.decoded(bytes(data), label, patterns)
                        else:
                            self.counts["large_raw_only"] += 1
                    except FileNotFoundError:
                        self.counts["vanished"] += 1
                    except PermissionError:
                        self.counts["unreadable"] += 1
        return {**dict(self.counts), "matches": len(self.matches), "path_sha256": sorted(self.matches)}


def tmpfs(path: Path) -> bool:
    resolved = path.resolve()
    mounts = []
    for line in Path("/proc/self/mountinfo").read_text().splitlines():
        before, after = line.split(" - ", 1)
        mount = Path(before.split()[4].replace("\\040", " "))
        if resolved.is_relative_to(mount):
            mounts.append((len(mount.parts), after.split()[0]))
    return bool(mounts) and max(mounts)[1] == "tmpfs"
