"""e2e/cpu: one package's cells as a person runs them, `cozy run <package dir>/<function> --on local`, through Calcifer and
the local machine's real TensorD, against the suite's isolated Hub (isolated.sh). The models its functions name by default,
and each one a cell picks, are the fixture's tiny builds (`tiny/<package>.py`), uploaded and published there under those refs
as an admin mirrors an author's model (isolated.sh `mirror`). Outputs are judged as the stand-in's are (drive.py `judged`).

    real.py <cozy> <project> <package dir> <code dir> <interface.json> <fixture.py> <out dir> [--refs DIR] [--only cell,...]
            [--record]

<code dir> is the package as cozy runs it: its checkout, or a copy the cozy under test locked afresh (run.sh --lock fresh).

One line per cell, as drive.py's. A quantize cell is `cozy model quantize`; a two-rank cell is the GPU release gate's.
"""

from __future__ import annotations

import hashlib
import json
import math
import struct
import subprocess
import sys
import time
import traceback
from pathlib import Path
from typing import cast

from cell import Cell, Json, Seen, Source
from drive import (HERE, Failed, Files, Interface, Reply, built, compact, first_line, graphs_off_cuda, judged, load_fixture,
                   slot_ref, steered, stored, tiny, tolerance, twin_gap)

#: An author's page for every tiny model this suite mirrors.
PAGE = "https://github.com/cozy-creator/cozy-creator/tree/master/e2e/cpu"
#: The lane a header's narrowest plain dtype or encoding names (TensorFS spec/selection.json `kinds`), by width.
FAMILIES = {"bf16": ("bf16", 16), "bfloat16": ("bf16", 16), "f16": ("fp16", 16), "float16": ("fp16", 16),
            "f32": ("f32", 32), "float32": ("f32", 32)}
PREFIXES = {"int8.": ("int8", 8), "fp8.": ("fp8", 8), "int4.": ("int4", 4), "nvfp4": ("nvfp4", 4), "mxfp4": ("mxfp4", 4)}
#: The extension `cozy run -o` names a file of each media type with (api::calcifer::output_file_name).
EXTENSIONS = {"image/png": ".png", "image/jpeg": ".jpg", "image/jpg": ".jpg", "image/webp": ".webp", "image/gif": ".gif",
              "image/avif": ".avif", "video/mp4": ".mp4", "video/webm": ".webm", "audio/wav": ".wav", "audio/x-wav": ".wav",
              "audio/wave": ".wav", "audio/mpeg": ".mp3", "audio/flac": ".flac", "audio/ogg": ".ogg",
              "application/json": ".json", "text/plain": ".txt"}


def cozy(binary: str, project: Path, args: list[str], log: Path, timeout: float = 1800) -> dict[str, Json]:
    """`cozy --json <args>` in the project, its output kept in `log`; the run it carried out as it ended (none: {})."""
    with open(log, "w") as out, open(log.with_suffix(".err"), "w") as err:
        done = subprocess.run([binary, "--json", *args], cwd=project, stdout=out, stderr=err, timeout=timeout, check=False)
    runs = [cast(dict[str, Json], row["run"]) for row in map(parsed, log.read_text().splitlines())
            if isinstance(row, dict) and isinstance(row.get("run"), dict)]
    last = runs[-1] if runs else {}
    # A call that read a model, or called a function, its needs rules said its request doesn't use: the rule is wrong, and
    # that model came only when read (TensorD's `needs_rule_missed`, cl-1103).
    missed = [w for w in cast(list[dict[str, Json]], last.get("warnings") or []) if w.get("code") == "needs_rule_missed"]
    if missed:
        raise Failed(f"needs_rule_missed: {first_line(missed[0].get('message', ''))}")
    if done.returncode != 0 or runs and last.get("status") != "completed":
        reason = cast(dict[str, Json], last.get("reason") or {})
        said = reason.get("message") or reason.get("code") or log.with_suffix(".err").read_text().strip()
        raise Failed(f"cozy {' '.join(a for a in args[:3] if a != '--full')}: exit {done.returncode}, "
                     f"{last.get('status') or 'no run'}: {first_line(said)}")
    return last


def parsed(text: str) -> Json:
    """An NDJSON line's value; None for any other line (a JSON document over several lines)."""
    try:
        return cast(Json, json.loads(text))
    except ValueError:
        return None


def lane(manifest: str, store: Path) -> str:
    """The lane a checkpoint's weights are (`commit_lane_mismatch`): its narrowest family of the table, of the most bytes."""
    import tensorfs

    held: dict[tuple[str, int], int] = {}
    for component in tensorfs.open(manifest, cache_dir=store, local_files_only=True).components.values():
        for tensor in component.tensors.values():
            encoding = tensor.encoding
            kind = encoding.scheme if encoding is not None else tensor.dtype.lower()
            family = FAMILIES.get(kind) or next((f for p, f in PREFIXES.items() if kind.startswith(p)), None)
            if family is not None:
                held[family] = held.get(family, 0) + math.prod(tensor.shape) * family[1]
    if not held:
        raise Failed(f"{manifest} holds no tensor of a lane's kind")
    return min(held, key=lambda family: (family[1], -held[family]))[0]


def mirror(binary: str, project: Path, ref: str, manifest: str, store: Path, out: Path) -> None:
    """The tiny checkpoint uploaded by this computer's machine as `<org/name>`'s and published at `ref`'s release, as an admin
    mirrors an author's model (isolated.sh `mirror`)."""
    import tensorfs

    name, _, release = ref.partition("@")
    work = out / "mirror"
    work.mkdir(parents=True, exist_ok=True)
    stem = name.replace("/", "-")
    file = work / f"{stem}.cozytensors"
    checkpoint = tensorfs.open(manifest, cache_dir=store, local_files_only=True)
    checkpoint.write_file(file)
    # A LoRA's checkpoint holds its factors alone: a person uploads it as a LoRA (isolated.sh `standin_lora`).
    kind = "lora" if all(key.endswith((".lora_A.weight", ".lora_B.weight")) for component in checkpoint.components.values()
                         for key in component.tensors) else "model"
    made = cozy(binary, project, [kind, "upload", str(file), name, "--lane", lane(manifest, store), "--mirror", PAGE,
                                  "--on", "local", "-y"], work / f"upload-{stem}.ndjson")
    models = cast(list[dict[str, Json]], cast(dict[str, Json], made.get("result") or {}).get("models") or [{}])
    commit = cast(dict[str, Json], models[0].get("published") or {}).get("commit")
    if not isinstance(commit, str) or not commit:
        raise Failed(f"uploading {name} made no checkpoint")
    cozy(binary, project, [kind, "publish", name, release or "0.1.0", commit, "-y"], work / f"publish-{stem}.ndjson")
    file.unlink()


def wanted(interface: Interface, manifests: dict[str, str], cells: list[Cell]) -> dict[str, str]:
    """Each Hub ref a call reads by default (every slot's `default` the fixture builds), a cell picks (`models`), or the fixture
    builds by its ref (a LoRA a function stacks), and the tiny checkpoint published there. A ref built twice is the fixture's
    error."""
    refs: dict[str, str] = {}

    def want(ref: str, manifest: str | None) -> None:
        if manifest is None or "/" not in ref.partition("@")[0]:
            return
        if refs.setdefault(ref, manifest) != manifest:
            raise Failed(f"the fixture builds two models for {ref}")

    for entry in interface["entrypoints"]:
        for slot in entry.get("models", []):
            if slot.get("weights") or "default" not in slot:
                continue
            want(slot_ref(slot), built(manifests, slot))
    for cell in cells:
        for ref in [*cell.get("models", {}).values(), *cell.get("sources", {}).values()]:
            want(ref, manifests.get(ref))
    # A build of a release the package moved past (`same_model`: its default is now another) is that default's alone.
    names = {ref.partition("@")[0] for ref in refs}
    for ref, manifest in manifests.items():
        if "@" in ref and (ref in refs or ref.partition("@")[0] not in names):
            want(ref, manifest)
    return refs


def made(source: Source, folder: Path) -> Path:
    if callable(source):
        folder.mkdir(parents=True, exist_ok=True)
        return source(folder)
    return source


def request(cell: Cell, folder: Path) -> tuple[dict[str, Json], list[str]]:
    """The cell's request as a person sends one (cli.sh's bind): a prompt's reference images named as the run's own
    `references`, its text mentioning them; any other file attached at its field's path (`--file field.path=FILE`)."""
    payload = cast(dict[str, Json], json.loads(json.dumps(cell["payload"])))
    references: list[Json] = []
    files: list[str] = []
    for field, source in sorted(cell.get("files", {}).items()):
        path = made(source, folder)
        parts = field.split(".")
        if len(parts) >= 4 and parts[-3] == "references" and parts[-2].isdigit() and parts[-1] == "file":
            node: Json = payload
            for part in parts[:-3]:
                node = cast(dict[str, Json], node)[part]
            holder = cast(dict[str, Json], node)
            kind = "video" if path.suffix in (".mp4", ".webm", ".mov") else "audio" if path.suffix in (".wav", ".flac", ".mp3") \
                else "image"
            key = cast(list[dict[str, Json]], holder["references"])[int(parts[-2])]["key"]
            references.append({"name": key, kind: str(path)})
            continue
        files += ["--file", f"{field.removesuffix('.file')}={path}"]
    # Each mentioning prompt as its words: the run's references are what its mentions name.
    for field in cell.get("files", {}):
        parts = field.split(".")
        if len(parts) >= 4 and parts[-3] == "references":
            parent: Json = payload
            for part in parts[:-4]:
                parent = cast(dict[str, Json], parent)[part]
            prompt = cast(dict[str, Json], parent).get(parts[-4])
            if isinstance(prompt, dict):
                cast(dict[str, Json], parent)[parts[-4]] = prompt.get("text", "")
    if references:
        payload["references"] = references
    return payload, files


def outputs(run: dict[str, Json], folder: Path) -> Files:
    """Each output `cozy run -o` downloaded, by its output id (`name`, a list's item `name.<i>`), with its media type."""
    named: Files = {}
    number = run.get("number")
    for item in cast(list[dict[str, Json]], run.get("output") or []):
        if item.get("stays") or item.get("checkpoint"):
            continue
        name, index, media_type = str(item["name"]), int(cast(int, item.get("index") or 0)), str(item.get("media_type", ""))
        # As cozy names it: `<run>-<name>[-<index>]-<sha256 of the name and index>.<extension>`.
        identity = hashlib.sha256(struct.pack("<Q", len(name.encode())) + name.encode() + struct.pack("<I", index)).hexdigest()
        safe = "".join(c if c.isascii() and (c.isalnum() or c in "._-") else "_" for c in name[:128])
        file = folder / (f"{number}-{safe}" + (f"-{index}" if index else "") + f"-{identity}"
                         + EXTENSIONS.get(media_type.split(";")[0].strip().lower(), ""))
        if not file.is_file():
            raise Failed(f"output {name}{f' {index}' if index else ''} was not downloaded to {file.name}: "
                         f"{item.get('export_error') or 'no file'}")
        named[f"{name}.{index - 1}" if index else name] = (file, media_type)
    return named


def quantized(binary: str, project: Path, package_dir: Path, cell: Cell, folder: Path, against: Seen | None) -> tuple[Seen, str]:
    """A `quantize` cell as an author asks it (`cozy model quantize <ref> --lane <lane> --package <dir>`): a checkpoint in that
    lane (its machine uploads only weights as wide as the lane names: `commit_lane_mismatch`), holding as many tensors as the
    stand-in's (the reference's `weights`), as `cozy model show` reads it; and its ref (`<model>@<checkpoint>`)."""
    source = next(iter(cell.get("sources", {}).values()))
    wanted_lane = str(cell["payload"]["lane"])
    run = cozy(binary, project, ["model", "quantize", source, "--lane", wanted_lane, "--package", str(package_dir), "--on", "local",
                                 "-y"], folder / "quantize.ndjson")
    made = next((cast(dict[str, Json], item["checkpoint"]) for item in cast(list[dict[str, Json]], run.get("output") or [])
                 if item.get("checkpoint")), None)
    if made is None or made.get("lane") != wanted_lane:
        raise Failed(f"quantize {wanted_lane} made no checkpoint in its lane: {json.dumps(made)}")
    cozy(binary, project, ["model", "show", f"{made['repo_id']}@{made['revision']}"], folder / "show.json")
    lanes = cast(list[dict[str, Json]], cast(dict[str, Json], json.loads((folder / "show.json").read_text())).get("lanes") or [{}])
    tensors = sum(int(cast(int, component.get("tensors") or 0)) for component in cast(list[dict[str, Json]], lanes[0].get("components") or []))
    recorded: dict[str, dict[str, int]] = against.get("weights", {}) if against is not None else {}
    held = recorded.get(wanted_lane, {}).get("tensors")
    if not tensors or held is not None and held != tensors:
        raise Failed(f"quantize {wanted_lane}'s checkpoint holds {tensors} tensors, the reference's {held}")
    return ({"result": {"lane": wanted_lane}, "outputs": [], "weights": {wanted_lane: {"tensors": tensors}}},
            f"{made['repo_id']}@{made['revision']}")


def main() -> int:
    binary = sys.argv[1]
    project, package_dir, code, interface_path, fixture_path, out = (Path(a).resolve() for a in sys.argv[2:8])
    flags = sys.argv[8:]
    only = set(flags[flags.index("--only") + 1].split(",")) if "--only" in flags else None
    record = "--record" in flags
    package = package_dir.name
    out.mkdir(parents=True, exist_ok=True)
    store = out / "store"
    interface = cast(Interface, json.loads(interface_path.read_text()))
    try:
        fixture = load_fixture(fixture_path, package)
    except Exception as exc:
        with open(out / "fixture.err", "w") as err:
            traceback.print_exc(file=err)
        print(f"FAIL {package} - fixture 0.0s: {type(exc).__name__}: {first_line(exc)}", flush=True)
        return 1
    references_path, unknown = stored(Path(flags[flags.index("--refs") + 1]) if "--refs" in flags else HERE / "refs", package)
    references = cast(dict[str, Seen], json.loads(references_path.read_text())) if references_path.exists() else {}
    failed = 0

    def line(ok: bool, entry: str, name: str, started: float, why: str = "") -> None:
        nonlocal failed
        failed += not ok
        why = "; ".join(filter(None, (why, unknown)))
        print(f"{'PASS' if ok else 'FAIL'} {package} {entry} {name} {time.monotonic() - started:.1f}s{': ' + why if why else ''}",
              flush=True)

    cells = [c for c in fixture.CELLS if only is None or c["name"] in only or f"{c['entry']}/{c['name']}" in only]
    running = [c for c in cells if "skip" not in c and c.get("ranks", 1) == 1]
    started = time.monotonic()
    try:
        manifests = tiny(fixture, package_dir, out, store) if running else {}
        refs = wanted(interface, manifests, running)
        line(True, "-", "tiny-weights", started)
    except Exception as exc:
        with open(out / "tiny-weights.err", "w") as err:
            traceback.print_exc(file=err)
        line(False, "-", "tiny-weights", started, f"{type(exc).__name__}: {first_line(exc)}")
        return 1
    for ref, manifest in sorted(refs.items()):
        started = time.monotonic()
        try:
            mirror(binary, project, ref, manifest, store, out)
            line(True, "-", f"mirror/{ref}", started)
        except Exception as exc:
            line(False, "-", f"mirror/{ref}", started, first_line(exc) if isinstance(exc, Failed) else f"{type(exc).__name__}: {first_line(exc)}")
            return 1

    ran: dict[str, Seen] = {}
    kept: dict[str, Files] = {}
    #: each quantize cell's checkpoint, by its key: a model a later cell binds (`Cell.models`)
    wrote: dict[str, str] = {}
    for cell in cells:
        entry, name = cell["entry"], cell["name"]
        key = f"{entry}/{name}"
        started = time.monotonic()
        if "skip" in cell:
            print(f"SKIP {package} {entry} {name} 0.0s: {cell['skip']}", flush=True)
            continue
        if cell.get("ranks", 1) != 1:
            print(f"SKIP {package} {entry} {name} 0.0s: a {cell.get('ranks')}-rank group runs on GPUs (the release gate's multi-GPU "
                  "cells)", flush=True)
            continue
        folder = out / "cells" / f"{entry}-{name}"
        folder.mkdir(parents=True, exist_ok=True)
        try:
            same_as = cell.get("same_as")
            against = ran.get(same_as) or references.get(same_as) if same_as else None if record else references.get(key)
            if "sources" in cell:
                seen, wrote[key] = quantized(binary, project, code, cell, folder, against)
                named: Files = {}
            else:
                payload, files = request(cell, out / "made")
                (folder / "request.json").write_text(json.dumps(payload, indent=1))
                if absent := [ref for ref in cell.get("models", {}).values()
                              if any(ref == f"{c['entry']}/{c['name']}" for c in fixture.CELLS) and ref not in wrote]:
                    raise Failed(f"nothing built or wrote {absent[0]}: run the cell that writes it first")
                models = [arg for path, ref in sorted(cell.get("models", {}).items())
                          for arg in ("--model", f"{path.rsplit('.', 1)[-1]}={wrote.get(ref, ref)}")]
                run = cozy(binary, project, ["--full", "run", f"{code}/{entry}", "--input", str(folder / "request.json"),
                                             *files, *models, "--on", "local", "-o", str(folder / "out")], folder / "run.ndjson")
                (folder / "run.json").write_text(json.dumps(run, indent=1))
                # Its CUDA graphs fell back to the call as written, as its machine measured it (cr-297).
                graphs_off_cuda(cast(Reply, cast(dict[str, Json], run.get("measurements") or {})))
                if against is None and not (record and same_as is None):
                    raise Failed(f"no output of {same_as or key} to judge against: run it too, or record (run.sh --record)")
                named = outputs(run, folder / "out")
                seen = judged(cell, run.get("result"), named, against, {})
            ran[key], kept[key] = seen, named
            if same_as is not None and same_as in kept:
                where, gap = twin_gap(named, kept[same_as], seen["result"])
                if gap > tolerance(cell):
                    raise Failed(f"{where} differs from {same_as}'s by {gap:.2f}/255 on average (tolerance {tolerance(cell)})")
            said = [steered(seen, other, least, ran.get(other) or references.get(other), cell.get("region"))
                    for other, least in cell.get("apart", {}).items()]
            if record and same_as is None:
                references[key] = seen
            line(True, entry, name, started, "; ".join(said))
        except Exception as exc:
            line(False, entry, name, started, first_line(exc) if isinstance(exc, Failed) else f"{type(exc).__name__}: {first_line(exc)}")
    if record:
        references_path.parent.mkdir(parents=True, exist_ok=True)
        references_path.write_text(compact(references) + "\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
