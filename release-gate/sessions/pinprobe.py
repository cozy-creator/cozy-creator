

# Appended to a package's module by idle-neighbour's `probe` (var-030 §9, proof only, never merged).
def _pinprobe(when: str) -> list[str]:
    """PINPROBE: the residency blocks still bound, the varena pins, and the CUDA storages alive
    with what holds each view; at the start, again after a garbage collection."""
    import collections
    import gc
    import time

    import torch as _torch

    lines: list[str] = []
    began = time.perf_counter()
    try:
        from cozy_runtime.internal import residency as _res

        memories = list(_res._MEMORIES.values())

        def pinned() -> str:
            said = []
            for mem in memories:
                native = getattr(mem, "native", None)
                if native is not None:
                    s = native.stats()
                    said.append(f"{s['pinned_bytes'] / 2**30:.2f} GiB pinned of {s['resident_bytes'] / 2**30:.2f} resident")
            return "; ".join(said) or "no native memory"

        def blocks() -> None:
            tally: dict[tuple[object, ...], list[int]] = {}
            for mem in memories:
                for model in list(mem.models.values()):
                    for b in model.groups():
                        held = tuple(getattr(b, f, None) is not None for f in ("data", "native_use", "final_native_use", "prefilled"))
                        if not any(held) and not b.count and not b.retained:
                            continue
                        row = tally.setdefault((model.component, b.count, b.retained, b.loading, *held), [0, 0])
                        row[0] += 1
                        row[1] += int(getattr(b, "allocation_bytes", 0) or b.nbytes)
            for k, (n, size) in sorted(tally.items(), key=lambda kv: -kv[1][1])[:6]:
                lines.append(f"PINPROBE {when} blocks {k[0]}: {n}, {size / 2**30:.2f} GiB; count {k[1]} retained {k[2]} "
                             f"loading {k[3]}; data {k[4]} use {k[5]} final_use {k[6]} prefilled {k[7]}")
            if not tally:
                lines.append(f"PINPROBE {when} blocks: none bound")

        def storages() -> tuple[list[object], dict[int, list]]:
            objs = gc.get_objects()
            found: dict[int, list] = {}
            for o in objs:
                try:
                    if isinstance(o, _torch.Tensor) and o.is_cuda:
                        st = o.untyped_storage()
                        if st.data_ptr():
                            found.setdefault(st.data_ptr(), [st.nbytes(), []])[1].append(o)
                except Exception:
                    continue
            return objs, found

        def label(o: object, held: object) -> tuple[str, bool]:
            """`o`, which refers to `held`, as a word; whether to go on up from it."""
            t = type(o)
            name = t.__name__
            if t is dict:
                keys = [str(k)[:40] for k, v in list(o.items())[:20000] if v is held][:2]
                return f"dict[{'/'.join(keys)}]", True
            if t in (list, tuple, set, frozenset, collections.deque):
                return f"{name}({len(o)})", True
            if name == "cell":
                return "cell", True
            if name == "frame":
                return f"frame {o.f_code.co_name}@{o.f_code.co_filename.rsplit('/', 1)[-1]}:{o.f_lineno}", False
            if name in ("generator", "coroutine", "async_generator"):
                code = getattr(o, "gi_code", None) or getattr(o, "cr_code", None) or getattr(o, "ag_code", None)
                return f"{name} {getattr(code, 'co_qualname', '?')}", False
            if name in ("function", "method", "builtin_function_or_method", "functools.partial", "partial"):
                return f"{name} {getattr(o, '__qualname__', '?')}", False
            attrs = []
            d = getattr(o, "__dict__", None)
            if d is held:
                attrs.append("__dict__")
            elif isinstance(d, dict):
                attrs += [str(k) for k, v in d.items() if v is held][:2]
            for s in getattr(t, "__slots__", ()) or ():
                try:
                    if getattr(o, s, None) is held:
                        attrs.append(s)
                except Exception:
                    pass
            return f"{t.__module__.rsplit('.', 1)[-1]}.{name}{('.' + '/'.join(attrs[:2])) if attrs else ''}", True

        def holders(found: dict[int, list], objs: list[object]) -> None:
            big = {ptr: row for ptr, row in found.items() if row[0] >= 16 << 20}
            if not big:
                lines.append(f"PINPROBE {when} storages: {len(found)} CUDA, {sum(r[0] for r in found.values()) / 2**30:.2f} GiB; none of 16 MiB or more")
                return
            mine = {id(objs), id(found), id(big)} | {id(r) for r in found.values()} | {id(r[1]) for r in found.values()}
            level = {id(t): t for row in big.values() for t in row[1]}
            words = {i: "" for i in level}  # id -> the chain above it, filled level by level
            up: dict[int, list[object]] = {}
            for depth in range(3):
                if not level:
                    break
                refs: dict[int, list[object]] = {}
                mine.add(id(refs))
                for o in objs:
                    if id(o) in mine:
                        continue
                    try:
                        for r in gc.get_referents(o):
                            if id(r) in level:
                                refs.setdefault(id(r), []).append(o)
                    except Exception:
                        continue
                nxt: dict[int, object] = {}
                for i, held in level.items():
                    up[i] = [o for o in refs.get(i, []) if id(o) not in mine][:3]
                    for o in up[i]:
                        word, more = label(o, held)
                        if more and depth < 2 and id(o) not in nxt:
                            nxt[id(o)] = o
                        up.setdefault(id(o), [])
                        words.setdefault(id(o), word)
                level = nxt

            def chain(i: int, depth: int = 0) -> str:
                above = up.get(i, [])
                if not above:
                    return ""
                parts = []
                for o in above[:2]:
                    parts.append(words.get(id(o), type(o).__name__) + (chain(id(o), depth + 1) if depth < 2 else ""))
                return " <- " + " | ".join(parts)

            kinds: dict[str, list] = {}
            for ptr, (nbytes, tensors) in big.items():
                said = [chain(id(t)) for t in tensors]
                seen = sorted({c for c in said if c})
                key = "; ".join(c[:260] for c in seen[:3]) or " (no Python referrer)"
                row = kinds.setdefault(key, [0, 0, ""])
                row[0] += 1
                row[1] += nbytes
                row[2] = f"{tensors[0].dtype} x{len(tensors)} views, {said.count('')} with no Python referrer"
            lines.append(f"PINPROBE {when} storages: {len(found)} CUDA, {sum(r[0] for r in found.values()) / 2**30:.2f} GiB; "
                         f"{len(big)} of 16 MiB or more, {sum(r[0] for r in big.values()) / 2**30:.2f} GiB")
            for key, (count, nbytes, example) in sorted(kinds.items(), key=lambda kv: -kv[1][1])[:6]:
                lines.append(f"PINPROBE {when} {count} storages, {nbytes / 2**30:.2f} GiB, {example}:{key}"[:900])

        lines.append(f"PINPROBE {when} varena: {pinned()}")
        blocks()
        objs, found = storages()
        holders(found, objs)
        del objs, found
        if when == "start":
            freed = gc.collect()
            objs, found = storages()
            big = sum(r[0] for r in found.values() if r[0] >= 16 << 20)
            lines.append(f"PINPROBE {when} after gc.collect ({freed} freed): varena {pinned()}; storages of 16 MiB or more {big / 2**30:.2f} GiB")
            del objs, found
    except Exception as error:  # the probe never fails the request
        lines.append(f"PINPROBE {when} failed: {type(error).__name__}: {error}"[:300])
    lines.append(f"PINPROBE {when} took {time.perf_counter() - began:.1f} s")
    return lines

