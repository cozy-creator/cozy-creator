"""Private native image identity. Independent of the public Varena C ABI version."""

from __future__ import annotations

import ctypes as C
import struct
from pathlib import Path


def _identity(read, mapped: bool) -> str:
    header = read(0, 64)
    if header[:6] == b"\x7fELF\x02\x01":
        phoff = struct.unpack_from("<Q", header, 32)[0]
        size, count = struct.unpack_from("<HH", header, 54)
        if size != 56 or count > 4096 or phoff > 1 << 20:
            raise ValueError("invalid ELF program headers")
        programs = [
            struct.unpack("<IIQQQQQQ", read(phoff + i * size, size))
            for i in range(count)
        ]
        for kind, _, offset, address, _, filesz, memsz, _ in programs:
            if kind != 4 or filesz > 1 << 20:
                continue
            if mapped and not any(
                p[0] == 1 and p[3] <= address and address + memsz <= p[3] + p[6]
                for p in programs
            ):
                continue
            notes = read(address if mapped else offset, filesz)
            at = 0
            while at + 12 <= len(notes):
                namesz, descsz, tag = struct.unpack_from("<III", notes, at)
                at += 12
                name = notes[at : at + namesz]
                at += (namesz + 3) & ~3
                value = notes[at : at + descsz]
                at += (descsz + 3) & ~3
                if (
                    name == b"GNU\0"
                    and tag == 3
                    and len(value) == descsz
                    and descsz >= 16
                ):
                    return "elf-" + value.hex()
    elif header[:4] == b"\xcf\xfa\xed\xfe":
        count, size = struct.unpack_from("<II", header, 16)
        if count > 4096 or size > 1 << 20:
            raise ValueError("invalid Mach-O load commands")
        commands = read(32, size)
        at = 0
        for _ in range(count):
            kind, length = struct.unpack_from("<II", commands, at)
            if length < 8 or at + length > len(commands):
                raise ValueError("invalid Mach-O command")
            if kind == 0x1B and length == 24:
                return "macho-" + commands[at + 8 : at + 24].hex()
            at += length
    elif header[:2] == b"MZ":
        pe = struct.unpack_from("<I", header, 60)[0]
        if pe > 1 << 20:
            raise ValueError("invalid PE header")
        coff = read(pe, 24)
        if coff[:4] != b"PE\0\0":
            raise ValueError("invalid PE signature")
        sections, optional_size = struct.unpack_from("<H12xH", coff, 6)
        optional = read(pe + 24, optional_size)
        if struct.unpack_from("<H", optional)[0] != 0x20B:
            raise ValueError("only 64-bit PE is supported")
        image_size = struct.unpack_from("<I", optional, 56)[0]
        debug, debug_size = struct.unpack_from("<II", optional, 112 + 6 * 8)
        table = read(pe + 24 + optional_size, sections * 40)

        def rva(address, length):
            if address + length > image_size:
                raise ValueError("PE address outside image")
            if mapped:
                return read(address, length)
            for i in range(sections):
                _, start, size, offset = struct.unpack_from("<IIII", table, i * 40 + 8)
                if start <= address and address + length <= start + size:
                    return read(offset + address - start, length)
            raise ValueError("PE address has no file section")

        if debug_size > 1 << 20:
            raise ValueError("invalid PE debug directory")
        entries = rva(debug, debug_size)
        for at in range(0, len(entries), 28):
            kind, size, address = struct.unpack_from("<III", entries, at + 12)
            if kind == 2 and 24 <= size <= 1 << 20:
                codeview = rva(address, size)
                if codeview[:4] == b"RSDS":
                    return "pe-" + codeview[4:24].hex()
    raise ValueError("native image has no supported linker build identity")


def file_identity(path: Path) -> str:
    with path.open("rb") as file:

        def read(offset, size):
            file.seek(offset)
            value = file.read(size)
            if len(value) != size:
                raise ValueError("truncated native image")
            return value

        return _identity(read, False)


def loaded_identity(library, windows: bool = False) -> str:
    symbol = C.cast(library.vr_v1_core_identity, C.c_void_p)
    if windows:
        kernel = C.WinDLL("kernel32", use_last_error=True)
        module = C.c_void_p()
        lookup = kernel.GetModuleHandleExW
        lookup.argtypes = [C.c_uint32, C.c_void_p, C.POINTER(C.c_void_p)]
        lookup.restype = C.c_int
        if not lookup(0x6, symbol, C.byref(module)):
            raise OSError(C.get_last_error(), "GetModuleHandleExW")
        base = module.value
    else:

        class Info(C.Structure):
            _fields_ = [
                ("name", C.c_char_p),
                ("base", C.c_void_p),
                ("symbol", C.c_char_p),
                ("address", C.c_void_p),
            ]

        lookup = C.CDLL(None).dladdr
        lookup.argtypes = [C.c_void_p, C.POINTER(Info)]
        lookup.restype = C.c_int
        info = Info()
        if not lookup(symbol, C.byref(info)):
            raise OSError("cannot locate loaded Varena core")
        base = info.base
    if not base:
        raise ValueError("loaded core has no image base")
    return _identity(lambda offset, size: C.string_at(base + offset, size), True)
