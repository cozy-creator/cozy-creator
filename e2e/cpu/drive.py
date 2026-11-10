"""e2e/cpu: one package's cells through the Runtime's real executor on the CPU, with tiny random weights.

Runs in the package's own environment (run.sh). It stands where TensorD stands: the executor process is the
Runtime's (`python -m cozy_runtime.internal.executor`), and this sends it TensorD's frames (`executor-protocol`) over
the seam: hello, start, load, activate, prepare_request, invoke, shutdown, with the bindings built from the package
interface as `tensord/src/gpu_service.rs` builds them. The weights are the fixture's (`tiny/<package>.py`): the
model's real classes at a tiny config, written into a throwaway store by TensorFS's own import or writer.

    drive.py <package dir> <interface.json> <fixture.py> <out dir> [--refs DIR] [--only cell,...] [--record]

Prints one line per cell: PASS|FAIL|SKIP <package> <entry> <cell> <seconds>s[: first error line]. Exit 1 on any FAIL.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import shutil
import socket
import struct
import subprocess
import sys
import sysconfig
import time
import traceback
from collections.abc import Mapping
from pathlib import Path
from typing import Literal, NotRequired, TypedDict, cast

import numpy as np
import numpy.typing as npt

from cell import Cell, Fixture, Json, Output, Seen

HERE = Path(__file__).resolve().parent
THREADS = os.environ.get("CPU_E2E_THREADS", "2")
HEADER = struct.Struct("!I")


class Failed(Exception):
    pass


def first_line(text: object) -> str:
    return next((line.strip() for line in str(text).splitlines() if line.strip()), "")[:300]


# The package interface, as far as a binding reads it (`cozy-runtime describe --json`).
class Slot(TypedDict):
    path: str
    # `class` is a keyword: read with slot["class"].
    default: NotRequired[str]
    components: NotRequired[list[str]]
    component_use: NotRequired[dict[str, list[str]]]
    weights: NotRequired[bool]
    on_demand: NotRequired[bool]


class Entrypoint(TypedDict):
    name: str
    models: NotRequired[list[Slot]]


class Interface(TypedDict):
    application: str
    entrypoints: list[Entrypoint]


# TensorD's frames (`crates/executor-protocol`), the fields this driver sends.
class Binding(TypedDict):
    application: str
    package_interface: str
    model_class: NotRequired[str]
    model_binding_path: NotRequired[str]
    model_parameter_name: NotRequired[str]
    model_parameter_names: NotRequired[list[str]]
    component: NotRequired[str]
    components: NotRequired[list[str]]
    snapshots: NotRequired[dict[str, str]]
    store: NotRequired[str]
    snapshot: NotRequired[str]
    release: NotRequired[str]
    package: NotRequired[str]
    model: NotRequired[str]


class ModelLoad(TypedDict):
    binding: Binding


class Construction(TypedDict):
    path: str
    manifest: str
    components: list[str]


class InputMetadata(TypedDict):
    input_id: str
    media_type: str
    digest: str
    length: int
    order: int


class InputFile(TypedDict):
    local: str
    media_type: str
    digest: str
    length: int
    order: int


class Start(TypedDict):
    cmd: Literal["start"]
    devices: str
    application: str
    package_interface: str
    sequence_parallel_degree: int
    model_choices: bool
    constructions: list[Construction]


class Load(TypedDict):
    cmd: Literal["load"]
    construction: str
    devices: str
    sequence_parallel_degree: int
    binding: Binding
    models: NotRequired[list[ModelLoad]]


class PrepareRequest(TypedDict):
    cmd: Literal["prepare_request"]
    request_id: str
    construction: str
    entrypoint: str
    payload: dict[str, Json]
    input_metadata: dict[str, InputMetadata]


class Invoke(TypedDict):
    cmd: Literal["invoke"]
    request_id: str
    construction: str
    entrypoint: str
    spool: str
    deadline_s: float
    inputs: dict[str, InputFile]


class Named(TypedDict):
    cmd: Literal["hello", "shutdown"]


class Activate(TypedDict):
    cmd: Literal["activate"]
    construction: str


type Frame = Start | Load | PrepareRequest | Invoke | Named | Activate


class Outcome(TypedDict, total=False):
    terminal: str
    code: str
    message: str


class Reply(TypedDict, total=False):
    reply: str
    event: str
    kind: str
    ok: bool
    code: str
    detail: str
    outcome: Outcome
    outputs: list[dict[str, Json]]
    result: Json


class Executor:
    """One executor process and the seam to it: 4-byte big-endian length, then canonical JSON."""

    def __init__(self, work: Path, store: Path, waive: Mapping[str, str], ranks: int = 1) -> None:
        work.mkdir(parents=True, exist_ok=True)
        self.work, self.ranks = work, ranks
        (work / "seam").unlink(missing_ok=True)
        # A socket path is at most 107 bytes: bound and dialed through this directory's descriptor.
        self.fd = os.open(work, os.O_RDONLY)
        listening = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listening.bind(f"/proc/self/fd/{self.fd}/seam")
        listening.listen(1)
        env = {
            **{k: v for k, v in os.environ.items() if k in ("PATH", "HOME", "LANG", "TMPDIR", "VIRTUAL_ENV")},
            "CUDA_VISIBLE_DEVICES": "",
            "TENSORFS_HOME": str(store),
            "OMP_NUM_THREADS": THREADS,
            "MKL_NUM_THREADS": THREADS,
        }
        if waive:
            # A fixture's waivers (WAIVE): release-geometry pins a tiny model cannot meet, each named with its reason.
            site = Path(sysconfig.get_paths()["purelib"])
            shutil.copyfile(HERE / "waive" / "cpu_e2e_waive.py", site / "cpu_e2e_waive.py")
            (site / "cpu_e2e_waive.pth").write_text("import cpu_e2e_waive\n")
            env["CPU_E2E_WAIVE"] = json.dumps(dict(waive))
        self.stderr = work / "executor.log"
        self.child = subprocess.Popen(
            [sys.executable, "-I", "-m", "cozy_runtime.internal.executor", "--socket", f"/proc/{os.getpid()}/fd/{self.fd}/seam",
             "--root", str(work)],
            env=env, stderr=open(self.stderr, "w"), stdout=subprocess.DEVNULL,
        )
        listening.settimeout(180)
        self.sock, _ = listening.accept()
        listening.close()

    def send(self, frame: Frame) -> None:
        data = json.dumps(frame, separators=(",", ":"), sort_keys=True).encode()
        self.sock.sendall(HEADER.pack(len(data)) + data)

    def recv(self, timeout: float) -> Reply | None:
        head = self._exact(HEADER.size, timeout, boundary=True)
        if head is None:
            return None
        (length,) = HEADER.unpack(head)
        body = self._exact(length, timeout, boundary=False)
        assert body is not None
        return cast(Reply, json.loads(body))

    def _exact(self, want: int, timeout: float, *, boundary: bool) -> bytes | None:
        buf = bytearray()
        self.sock.settimeout(timeout)
        while len(buf) < want:
            chunk = self.sock.recv(want - len(buf))
            if not chunk:
                if boundary and not buf:
                    return None
                raise Failed("the executor's seam closed inside a frame")
            buf += chunk
        return bytes(buf)

    def ask(self, frame: Frame, timeout: float = 900) -> Reply:
        self.send(frame)
        deadline = time.monotonic() + timeout
        while True:
            try:
                got = self.recv(max(1.0, deadline - time.monotonic()))
            except TimeoutError:
                raise Failed(f"{frame['cmd']} did not answer in {timeout:.0f} s") from None
            if got is None:
                raise Failed(f"the executor left during {frame['cmd']}: {self.last_error()}")
            if got.get("reply") == frame["cmd"]:
                return got
            if got.get("event") == "request":
                raise Failed(f"the executor asked TensorD for {got.get('kind')}, which this driver does not stand in for")

    def must(self, frame: Frame, timeout: float = 900) -> Reply:
        reply = self.ask(frame, timeout)
        if not reply.get("ok"):
            raise Failed(f"{frame['cmd']}: {reply.get('code')}: {first_line(reply.get('detail', ''))}")
        return reply

    def last_error(self) -> str:
        lines = [line for line in self.stderr.read_text(errors="replace").splitlines() if line.strip()]
        errors = [line for line in lines if "Error" in line or "error" in line]
        return first_line((errors or lines or ["no stderr"])[-1])

    def close(self) -> None:
        try:
            if self.child.poll() is None:
                self.send({"cmd": "shutdown"})
                self.child.wait(20)
        except (OSError, subprocess.TimeoutExpired):
            pass
        if self.child.poll() is None:
            self.child.kill()
            self.child.wait()
        self.sock.close()
        os.close(self.fd)


def slot_ref(slot: Slot) -> str:
    return slot.get("default") or slot["path"]


def held(manifest: str, store: Path) -> list[str]:
    import tensorfs

    return list(tensorfs.open(manifest, cache_dir=store, local_files_only=True).components)


def bindings(interface: Interface, interface_path: str, entry: Entrypoint, manifests: Mapping[str, str],
             store: Path, release: str, package: str) -> list[Binding]:
    """The entry point's constructed slots as TensorD binds them (`gpu_service.rs`, `Binding`)."""
    rows: list[Binding] = []
    for slot in entry.get("models", []):
        if slot.get("weights"):
            continue
        ref = slot_ref(slot)
        manifest = manifests.get(slot["path"]) or manifests.get(ref)
        if manifest is None:
            raise Failed(f"the fixture builds no model for {slot['path']} ({ref})")
        components = held(manifest, store)
        wanted = slot.get("components") or sorted({c for use in slot.get("component_use", {}).values() for c in use})
        missing = [c for c in wanted if c not in components]
        components = [c for c in components if not wanted or c in wanted]
        if missing or not components:
            raise Failed(f"{slot['path']} uses {', '.join(missing) or 'components'} that the tiny {ref} does not hold")
        parameter = slot["path"].rsplit(".", 1)[-1]
        rows.append({
            "application": interface["application"], "package_interface": interface_path,
            "model_class": cast(dict[str, str], slot)["class"], "model_binding_path": slot["path"],
            "model_parameter_name": parameter, "model_parameter_names": [parameter], "component": components[0],
            "components": components, "snapshots": {c: manifest for c in components}, "snapshot": manifest,
            "store": str(store), "release": release, "package": package, "model": f"tiny/{ref}",
        })
    return rows


def block_means(values: npt.NDArray[np.generic], grid: tuple[int, ...]) -> npt.NDArray[np.float64]:
    """`values` cut into `grid` blocks along its leading axes, each block's mean (its trailing axes kept)."""
    out = values.astype(np.float64)
    for axis, count in enumerate(grid):
        out = np.stack([part.mean(axis=axis) for part in np.array_split(out, min(count, out.shape[axis]), axis=axis)], axis)
    return out


def signature(values: npt.NDArray[np.generic]) -> list[float]:
    """An output's thumbnail, 0-255: a picture 16x16 blocks, a video 8 frames of 8x8, else 1024 runs of its samples.
    Each block of a decoded picture spans a few VAE patches, so another latent shows here where a whole-file mean
    would not."""
    if values.ndim == 4:
        frames = values[np.linspace(0, len(values) - 1, min(8, len(values))).round().astype(int)]
        thumb = block_means(frames, (len(frames), 8, 8))
    elif values.ndim == 3:
        thumb = block_means(values, (16, 16))
    else:
        thumb = block_means(values.reshape(-1), (1024,))
    return [round(float(v), 1) for v in thumb.reshape(-1)]


def shaped(values: npt.NDArray[np.generic], result: Json) -> npt.NDArray[np.generic]:
    """A raw RGB picture as height x width x 3, when the result states its canvas."""
    if values.ndim == 1 and isinstance(result, dict):
        width, height = result.get("width"), result.get("height")
        if isinstance(width, int) and isinstance(height, int) and values.size == width * height * 3:
            return values.reshape(height, width, 3)
    return values


def differ(a: list[float], b: list[float]) -> tuple[float, float]:
    """Mean and largest absolute difference of two thumbnails."""
    gap = np.abs(np.asarray(a) - np.asarray(b))
    return (float(gap.mean()), float(gap.max())) if gap.size else (0.0, 0.0)


def samples(path: Path, media_type: str) -> dict[str, npt.NDArray[np.generic]]:
    """An output's samples as 0-255 numbers: a picture or a video's frames decoded, a video's sound, else its bytes."""
    if media_type.startswith("video/"):
        import av

        found: dict[str, npt.NDArray[np.generic]] = {}
        with av.open(str(path)) as container:
            frames = [frame.to_ndarray(format="rgb24") for frame in container.decode(video=0)]
        if not frames:
            raise Failed(f"{path.name} holds no video frames")
        found[""] = np.stack(frames)
        with av.open(str(path)) as container:
            if container.streams.audio:
                sound = [frame.to_ndarray().astype(np.float32).reshape(-1) for frame in container.decode(audio=0)]
                if sound:
                    found[".audio"] = (np.clip(np.concatenate(sound), -1, 1) * 127.5 + 127.5).astype(np.uint8)
        return found
    if path.suffix != ".raw" and media_type in ("image/png", "image/jpeg", "image/webp"):
        from PIL import Image

        with Image.open(path) as image:
            return {"": np.asarray(image.convert("RGB"))}
    return {"": np.frombuffer(path.read_bytes(), dtype=np.uint8)}


def spread(values: npt.NDArray[np.generic]) -> float:
    """The samples' standard deviation, from at most a million of them."""
    flat = values.reshape(-1)
    return float(flat[:: max(1, flat.size // 1_000_000)].astype(np.float64).std())


def plain(result: Json) -> Json:
    """The result document without what names the attempt or the encoder's exact bytes: an output's `asset_ref` keeps
    its kind and number, and its digest and size go (its samples are judged instead)."""
    if isinstance(result, list):
        return [plain(item) for item in result]
    if not isinstance(result, dict):
        return result
    kept = {k: plain(v) for k, v in result.items() if not ("asset_ref" in result and k in ("digest", "size_bytes"))}
    ref = kept.get("asset_ref")
    if isinstance(ref, str) and ref.startswith("attempt:"):
        kept["asset_ref"] = ref.split("/", 1)[1]
    return kept


def judge(cell: Cell, reply: Reply, spool: Path, reference: Seen | None) -> Seen:
    """The cell's output judged, never only its completion: every declared output present, finite and not flat (or
    declared empty), the cell's own check, and the stored reference's."""
    outcome = reply.get("outcome", {})
    if not reply.get("ok") or outcome.get("terminal") != "succeeded":
        raise Failed(f"{outcome.get('code') or reply.get('code')}: {first_line(outcome.get('message') or reply.get('detail', ''))}")
    result_file = spool / "result.canonical"
    result = cast(Json, json.loads(result_file.read_bytes())) if result_file.exists() else reply.get("result")
    seen: Seen = {"result": plain(result), "outputs": []}
    empty = set(cell.get("empty", []))
    files: list[Path] = []
    for declared in sorted(reply.get("outputs") or [], key=lambda o: str(o.get("output_id"))):
        output, ref, media_type = (str(declared.get(k, "")) for k in ("output_id", "asset_ref", "media_type"))
        found = sorted(spool.glob(ref.split("/", 1)[1].replace("/", "-") + "*"))
        if not found:
            raise Failed(f"output {output} ({ref}) has no file in the spool")
        path = found[0]
        files.append(path)
        if not path.stat().st_size:
            if output not in empty:
                raise Failed(f"output {output} is empty")
            seen["outputs"].append({"name": output, "bytes": 0, "signature": []})
            continue
        for suffix, values in samples(path, media_type).items():
            values = shaped(values, result)
            width = spread(values)
            if not np.isfinite(width):
                raise Failed(f"output {output}{suffix} is not finite")
            if width < 1.0 and media_type.split("/")[0] in ("image", "video", "audio"):
                raise Failed(f"output {output}{suffix} is flat (spread {width:.3f})")
            seen["outputs"].append({"name": output + suffix, "bytes": values.size, "signature": signature(values)})
    if not seen["outputs"] and result in (None, {}, []):
        raise Failed("the reply declares no outputs and an empty result")
    check = cell.get("check")
    if check is not None and (problem := check(result, files)):
        raise Failed(problem)
    if reference is not None:
        compare(seen, reference, float(cell.get("tolerance", 1.0)))
    return seen


def compare(seen: Seen, reference: Seen, tolerance: float) -> None:
    """Same outputs, each decoding to as many samples, thumbnails within `tolerance` on average (and 12 times it at
    most), and the same result document."""
    def shape(outputs: list[Output]) -> list[tuple[str, int]]:
        return [(o["name"], o["bytes"]) for o in outputs]

    want, got = reference["outputs"], seen["outputs"]
    if shape(want) != shape(got):
        raise Failed(f"outputs {shape(got)} differ from the reference's {shape(want)}")
    for a, b in zip(want, got, strict=True):
        mean, worst = differ(a["signature"], b["signature"])
        if mean > tolerance or worst > 12 * tolerance:
            raise Failed(f"output {b['name']} differs from its reference by {mean:.2f}/255 on average, {worst:.0f} at most "
                         f"(tolerance {tolerance})")
    if reference["result"] != seen["result"]:
        raise Failed(f"result {json.dumps(seen['result'])[:160]} differs from the reference's {json.dumps(reference['result'])[:160]}")


MEDIA = {".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".webp": "image/webp", ".mp4": "video/mp4",
         ".wav": "audio/wav", ".flac": "audio/flac", ".mp3": "audio/mpeg", ".json": "application/json"}


def inputs(cell: Cell, payload: dict[str, Json], work: Path, made: Path) -> tuple[dict[str, InputMetadata], dict[str, InputFile]]:
    """The cell's input files as the machine grants them: its bytes in the attempt's own directory, the payload's field
    holding the reference, `PrepareRequest.input_metadata` and `Invoke.inputs` keyed by the field path (`asset_inputs`)."""
    metadata: dict[str, InputMetadata] = {}
    granted: dict[str, InputFile] = {}
    for field, source in cell.get("files", {}).items():
        if callable(source):
            made.mkdir(parents=True, exist_ok=True)
            source = source(made)
        work.mkdir(parents=True, exist_ok=True)
        local = (work / field.replace(".", "-")).with_suffix(source.suffix)
        shutil.copyfile(source, local)
        data = local.read_bytes()
        digest = "sha256:" + hashlib.sha256(data).hexdigest()
        parts = field.split(".")
        node: Json = payload
        for part, after in zip(parts, parts[1:]):
            empty: Json = [] if after.isdigit() else {}
            if isinstance(node, list):
                node.extend([None] * (int(part) + 1 - len(node)))
                node[int(part)] = node[int(part)] or empty
                node = node[int(part)]
            else:
                node = cast(dict[str, Json], node).setdefault(part, empty)
        last = parts[-1]
        # The position in the nearest containing list (`asset_inputs`): `prompt.references.1.file` is 1.
        order = next((int(part) for part in reversed(field.split(".")) if part.isdigit()), 0)
        if isinstance(node, list):
            node.extend([None] * (order + 1 - len(node)))
            node[order] = digest
        else:
            cast(dict[str, Json], node)[last] = digest
        metadata[field] = {"input_id": field, "media_type": MEDIA[source.suffix], "digest": digest, "length": len(data),
                           "order": order}
        granted[field] = {"local": str(local), "media_type": MEDIA[source.suffix], "digest": digest, "length": len(data),
                          "order": order}
    return metadata, granted


def load_fixture(path: Path, package: str) -> Fixture:
    spec = importlib.util.spec_from_file_location(f"tiny_{package.replace('-', '_').replace('.', '_')}", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return cast(Fixture, module)


def main() -> int:
    package_dir, interface_path, fixture_path, out = (Path(a).resolve() for a in sys.argv[1:5])
    flags = sys.argv[5:]
    only = set(flags[flags.index("--only") + 1].split(",")) if "--only" in flags else None
    record = "--record" in flags
    package = package_dir.name
    out.mkdir(parents=True, exist_ok=True)
    store = out / "store"
    interface = cast(Interface, json.loads(interface_path.read_text()))
    fixture = load_fixture(fixture_path, package)
    waive = cast(dict[str, str], getattr(fixture, "WAIVE", {}))
    references_path = (Path(flags[flags.index("--refs") + 1]) if "--refs" in flags else HERE / "refs") / f"{package}.json"
    references = cast(dict[str, Seen], json.loads(references_path.read_text())) if references_path.exists() else {}
    failed = 0

    def line(ok: bool, entry: str, name: str, started: float, why: str = "") -> None:
        nonlocal failed
        failed += not ok
        print(f"{'PASS' if ok else 'FAIL'} {package} {entry} {name} {time.monotonic() - started:.1f}s{': ' + why if why else ''}",
              flush=True)

    started = time.monotonic()
    try:
        import tensorfs

        manifests: dict[str, str] = {}
        os.environ["CPU_E2E_PACKAGES"] = str(package_dir.parent)
        for ref, source in fixture.models(out / "source", store).items():
            manifests[ref] = source if isinstance(source, str) else tensorfs.convert_to_cozytensors(str(source), cache_dir=store).id
        line(True, "-", "tiny-weights", started)
    except Exception as exc:
        with open(out / "tiny-weights.err", "w") as err:
            traceback.print_exc(file=err)
        line(False, "-", "tiny-weights", started, f"{type(exc).__name__}: {first_line(exc)}")
        return 1
    version = next((ln.split('"')[1] for ln in (package_dir / "pyproject.toml").read_text().splitlines() if ln.startswith("version")), "0")
    entries = {row["name"]: row for row in interface["entrypoints"]}
    cells = [c for c in fixture.CELLS if only is None or c["name"] in only or f"{c['entry']}/{c['name']}" in only]
    executors: dict[int, Executor] = {}
    loaded: set[tuple[int, str]] = set()
    #: each cell's judged output this run: a `same_as` cell is judged against its twin's here
    ran: dict[str, Seen] = {}
    try:
        for cell in cells:
            entry, name, ranks = cell["entry"], cell["name"], cell.get("ranks", 1)
            started = time.monotonic()
            request = f"{entry}-{name}"
            if "skip" in cell:
                print(f"SKIP {package} {entry} {name} 0.0s: {cell['skip']}", flush=True)
                continue
            try:
                row = entries.get(entry)
                if row is None:
                    raise Failed(f"the package interface has no entry point {entry}")
                rows = bindings(interface, str(interface_path), row, manifests, store, f"{package}@{version}", package)
                executor = executors.get(ranks)
                if executor is None:
                    executor = executors[ranks] = Executor(out / f"executor-{ranks}", store, waive, ranks)
                    executor.must({"cmd": "hello"})
                    executor.must({
                        "cmd": "start", "devices": "", "application": interface["application"],
                        "package_interface": str(interface_path), "sequence_parallel_degree": ranks, "model_choices": True,
                        "constructions": [{"path": b["model_binding_path"], "manifest": b["snapshot"], "components": b["components"]}
                                          for b in rows if "snapshot" in b and "model_binding_path" in b and "components" in b],
                    })
                construction = f"{entry}@{ranks}"
                if (ranks, construction) not in loaded:
                    load: Load = {"cmd": "load", "construction": construction, "devices": "", "sequence_parallel_degree": ranks,
                                  "binding": rows[0] if rows else {"application": interface["application"],
                                                                   "package_interface": str(interface_path)}}
                    if len(rows) > 1:
                        load["models"] = [{"binding": b} for b in rows]
                    executor.must(load)
                    loaded.add((ranks, construction))
                executor.must({"cmd": "activate", "construction": construction})
                payload = cast(dict[str, Json], json.loads(json.dumps(cell["payload"])))
                metadata, granted = inputs(cell, payload, out / "inputs" / request, out / "made")
                executor.must({"cmd": "prepare_request", "request_id": request, "entrypoint": entry, "payload": payload,
                               "construction": construction, "input_metadata": metadata})
                spool = out / "spool" / request
                spool.mkdir(parents=True)
                reply = executor.ask({"cmd": "invoke", "request_id": request, "entrypoint": entry, "spool": str(spool),
                                      "construction": construction, "deadline_s": 1800, "inputs": granted})
                (out / f"{request}.reply.json").write_text(json.dumps(reply, indent=1))
                key = f"{entry}/{name}"
                same_as = cell.get("same_as")
                against = ran.get(same_as) or references.get(same_as) if same_as else None if record else references.get(key)
                if against is None and not (record and same_as is None):
                    raise Failed(f"no output of {same_as or key} to judge against: run it too, or record (run.sh --record)")
                seen = judge(cell, reply, spool, against)
                ran[key] = seen
                if record and same_as is None:
                    references[key] = seen
                line(True, entry, name, started)
            except Exception as exc:
                why = first_line(exc) if isinstance(exc, Failed) else f"{type(exc).__name__}: {first_line(exc)}"
                line(False, entry, name, started, why)
                dead = executors.pop(ranks, None)
                if dead is not None:  # the next cell starts a fresh executor
                    dead.close()
                    loaded = {key for key in loaded if key[0] != ranks}
    finally:
        for executor in executors.values():
            executor.close()
    if record:
        references_path.write_text(json.dumps(references, indent=1, sort_keys=True) + "\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
