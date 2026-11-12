"""e2e/cpu: one package's cells through the Runtime's real executor on the CPU, with tiny random weights.

Runs in the package's own environment (run.sh). It stands where TensorD stands: the executor process is the
Runtime's (`python -m cozy_runtime.internal.executor`), and this sends it TensorD's frames (`executor-protocol`) over
the seam: hello, start, load, prepare_request, invoke, shutdown, with the bindings built from the package
interface as `tensord/src/gpu_service.rs` builds them. The weights are the fixture's (`tiny/<package>.py`): the
model's real classes at a tiny config, written into a throwaway store by TensorFS's own import or writer.

    drive.py <package dir> <interface.json> <fixture.py> <out dir> [--refs DIR] [--only cell,...] [--record]

Prints one line per cell: PASS|FAIL|SKIP <package> <entry> <cell> <seconds>s[: first error line]. Exit 1 on any FAIL.
"""

from __future__ import annotations

import hashlib
import contextlib
import importlib.util
import itertools
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
from collections.abc import Callable, Iterator, Mapping
from pathlib import Path
from typing import Literal, NotRequired, TypedDict, cast

import numpy as np
import numpy.typing as npt

from cell import Cell, Fixture, Json, Output, Seen, Source

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
    #: the functions it calls (an orchestrator's segments)
    calls: NotRequired[list[dict[str, Json]]]


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
    components: NotRequired[list[str]]
    store: NotRequired[str]
    snapshot: NotRequired[str]
    release: NotRequired[str]
    package: NotRequired[str]
    model: NotRequired[str]


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


type InputHistory = dict[str, dict[str, InputFile] | None]


class CallRow(TypedDict):
    """One function an attempt may call (`Start.call_interfaces`): here a package's own (`self`)."""

    module: str
    export: str
    self: bool


class Start(TypedDict):
    cmd: Literal["start"]
    devices: str
    application: str
    package_interface: str
    sequence_parallel_degree: int
    model_choices: bool
    constructions: list[Construction]
    call_interfaces: NotRequired[list[CallRow]]


class Load(TypedDict):
    cmd: Literal["load"]
    construction: str
    devices: str
    sequence_parallel_degree: int
    #: every slot's binding (Runtime 0.44, xs-081)
    models: list[Binding]


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
    #: each `Weights[M]` parameter's checkpoint, and each weights output's granted bytes
    sources: NotRequired[dict[str, str]]
    weights: NotRequired[dict[str, int]]
    operation: NotRequired[str]


class Grant(TypedDict):
    """A child's output projected into its caller's spool (`ByteGrant`)."""

    output_id: str
    kind: str
    digest: str
    length: int
    content_bytes: int
    media_type: str
    local: str


class Answer(TypedDict):
    """TensorD's answer to the executor's durable request `seq` (`author._executor_requests`): a verdict, and for a
    call its state (`CallState`), for a memo its entry (`MemoEntry`, `MemoStored`)."""

    event: Literal["answer"]
    seq: int
    ok: bool
    code: str
    detail: str
    state: NotRequired[str]
    child_request_id: NotRequired[str]
    result: NotRequired[str]
    byte_grants: NotRequired[list[Grant]]
    local: NotRequired[str]
    stored: NotRequired[bool]
    reason: NotRequired[str]
    digest: NotRequired[str]
    length: NotRequired[int]
    sequence: NotRequired[int]


class Item(TypedDict):
    """A weights output's local commit, as the executor names it to TensorD (`checkpoint_item`)."""

    output: str
    repo_id: str
    revision: str


class Named(TypedDict):
    cmd: Literal["hello", "shutdown"]


class Watch(TypedDict):
    """A client watches `request_id`'s previews for `for_ms`; out of band, unanswered."""

    cmd: Literal["preview"]
    request_id: str
    for_ms: int


type Command = Start | Load | PrepareRequest | Invoke | Named
type Frame = Command | Answer | Watch


class Outcome(TypedDict, total=False):
    terminal: str
    code: str
    message: str


class Saved(TypedDict):
    asset_ref: str
    digest: str
    media_type: str


class Reply(TypedDict, total=False):
    reply: str
    event: str
    kind: str
    seq: int
    call_index: int
    module: str
    export: str
    payload: str
    saved: list[Saved]
    digest: str
    size_bytes: int
    output: str
    repo_id: str
    revision: str
    ok: bool
    code: str
    detail: str
    outcome: Outcome
    outputs: list[dict[str, Json]]
    result: Json
    #: A hello's: the executor makes previews; a progress frame's preview, and a log's message
    previews: int
    preview: dict[str, Json]
    name: str


class Executor:
    """One executor process and the seam to it: 4-byte big-endian length, then canonical JSON."""

    def __init__(self, work: Path, store: Path, waive: Mapping[str, str], patch: Mapping[str, str], ranks: int = 1) -> None:
        work.mkdir(parents=True, exist_ok=True)
        self.work, self.ranks = work, ranks
        (work / "seam").unlink(missing_ok=True)
        # A socket path is at most 107 bytes: bound and dialed through this directory's descriptor.
        self.fd = os.open(work, os.O_RDONLY)
        listening = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listening.bind(f"/proc/self/fd/{self.fd}/seam")
        listening.listen(1)
        env = {
            **{k: v for k, v in os.environ.items() if k in ("PATH", "HOME", "LANG", "TMPDIR", "VIRTUAL_ENV", "TENSORFS_ENDPOINT")},
            "CUDA_VISIBLE_DEVICES": "",
            "TENSORFS_HOME": str(store),
            "OMP_NUM_THREADS": THREADS,
            "MKL_NUM_THREADS": THREADS,
        }
        if waive or patch:
            # A fixture's waivers (WAIVE, PATCH): release-geometry pins a tiny model cannot meet, each named with its reason.
            site = Path(sysconfig.get_paths()["purelib"])
            shutil.copyfile(HERE / "waive" / "cpu_e2e_waive.py", site / "cpu_e2e_waive.py")
            (site / "cpu_e2e_waive.pth").write_text("import cpu_e2e_waive\n")
            env["CPU_E2E_WAIVE"] = json.dumps(dict(waive))
            env["CPU_E2E_PATCH"] = json.dumps(dict(patch))
        self.stderr = work / "executor.log"
        self.child = subprocess.Popen(
            [sys.executable, "-I", "-m", "cozy_runtime.internal.executor", "--socket", f"/proc/{os.getpid()}/fd/{self.fd}/seam",
             "--root", str(work)],
            env=env, stderr=open(self.stderr, "w"), stdout=subprocess.DEVNULL,
        )
        listening.settimeout(180)
        self.sock, _ = listening.accept()
        listening.close()
        self.items: list[Item] = []
        #: TensorD's call broker for the attempt being invoked, when its function calls others
        self.broker: Broker | None = None
        #: The attempt's previews (cr-282) and what it said of its `preview` lines, when its hello says it makes them
        self.previewing = False
        self.previews: list[dict[str, Json]] = []
        self.preview_said: list[str] = []

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

    def ask(self, frame: Command, timeout: float = 900) -> Reply:
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
                if frame["cmd"] == "hello":
                    self.previewing = bool(got.get("previews"))
                return got
            if got.get("event") == "progress" and got.get("kind") == "preview":
                self.previews.append(got.get("preview") or {})
            elif got.get("event") == "progress" and str(got.get("name", "")).startswith("preview: "):
                self.preview_said.append(str(got.get("name")))
            elif got.get("event") == "request" and got.get("kind") == "checkpoint_item":
                # A weights output committed in the local store: TensorD records it (and uploads it only for a remote
                # destination, which no cell names).
                self.items.append({"output": got.get("output", ""), "repo_id": got.get("repo_id", ""),
                                   "revision": got.get("revision", "")})
                self.send({"event": "answer", "seq": got.get("seq", 0), "ok": True, "code": "", "detail": ""})
            elif got.get("event") == "request" and self.broker is not None and got.get("kind", "") in Broker.KINDS:
                self.send(self.broker.answer(got))
            elif got.get("event") == "request":
                raise Failed(f"the executor asked TensorD for {got.get('kind')}, which this driver does not stand in for")

    def must(self, frame: Command, timeout: float = 900) -> Reply:
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


#: A child call: (its function, its request, its input files by field path, its request id) -> its reply and spool.
type RunChild = Callable[[str, dict[str, Json], dict[str, Path], str], tuple[Reply, Path]]


class Broker:
    """TensorD's call broker (`tensord/src/calls.rs`) for one attempt: a child call runs to its end on the child executor
    before it is answered, so every poll finds it settled and the attempt never waits on a nudge (`child_events`). Its
    outputs are projected into the caller's spool as TensorD projects them (`receive`): each named by its sha256, granted
    by its result path, and held for a later child."""

    KINDS = frozenset({"child_call", "child_poll", "child_cancel", "child_forget", "gpu_release", "stage_memo_lookup",
                       "stage_memo_store", "publish"})

    def __init__(self, request: str, spool: Path, run: RunChild) -> None:
        self.request, self.spool, self.run = request, spool, run
        #: each file the caller holds, by digest: what it saved and passed, and what its children returned
        self.held: dict[str, Path] = {}
        self.settled: dict[int, Answer] = {}

    def answer(self, got: Reply) -> Answer:
        seq, kind, index = got.get("seq", 0), got.get("kind", ""), got.get("call_index", -1)
        verdict: Answer = {"event": "answer", "seq": seq, "ok": True, "code": "", "detail": ""}
        settled: Answer = verdict
        if kind == "child_call":
            settled = self.call(got).copy()
        elif kind == "child_poll":
            settled = self.settled[index].copy() if index in self.settled else verdict | {
                "ok": False, "code": "child_call_refused", "detail": "parent call has not been activated"}
        elif kind == "stage_memo_lookup":
            settled = verdict | {"local": ""}
        elif kind == "stage_memo_store":
            settled = verdict | {"stored": False, "reason": "no memo store here"}
        elif kind == "publish":  # shown to no one here (`sequence` 0), as a child call's product is
            settled = verdict | {"digest": got.get("digest", ""), "length": got.get("size_bytes", 0), "sequence": 0}
        settled["seq"] = seq
        return settled

    def call(self, got: Reply) -> Answer:
        index = got.get("call_index", -1)
        if index in self.settled:
            return self.settled[index]
        for saved in got.get("saved", []):
            self.held[saved["digest"]] = spooled(self.spool, saved["asset_ref"])
        # Every call is a serving call (`calls.rs` serving_call): its request, and the model choices it passes (`models`),
        # which a cell's calls leave at their defaults.
        document = cast(dict[str, Json], json.loads(got.get("payload", "{}")))
        request = document.get("payload")
        if not isinstance(request, dict):
            raise Failed("a child call has no request object")
        files = {path: self.held[digest] for path, digest in digests(request) if digest in self.held}
        child = f"{self.request}-call-{index}"
        reply, spool = self.run(got.get("export", ""), request, files, child)
        outcome = reply.get("outcome", {})
        verdict: Answer = {"event": "answer", "seq": 0, "ok": True, "code": "", "detail": ""}
        if not reply.get("ok") or outcome.get("terminal") != "succeeded":
            verdict |= {"ok": False, "code": outcome.get("code") or reply.get("code") or "child_failed",
                        "detail": (outcome.get("message") or reply.get("detail") or "")[:1024]}
        else:
            result, grants = self.receive(reply, spool, index)
            verdict |= {"state": "succeeded", "child_request_id": child, "result": result, "byte_grants": grants}
        self.settled[index] = verdict
        return verdict

    def receive(self, reply: Reply, spool: Path, index: int) -> tuple[str, list[Grant]]:
        result_file = spool / "result.canonical"
        value = cast(Json, json.loads(result_file.read_bytes())) if result_file.exists() else reply.get("result")
        directory = self.spool / "child-results" / str(index)
        directory.mkdir(parents=True, exist_ok=True)
        grants: list[Grant] = []
        for declared in reply.get("outputs") or []:
            output, ref, media_type = (str(declared.get(k, "")) for k in ("output_id", "asset_ref", "media_type"))
            path = spooled(spool, ref)
            data = path.read_bytes()
            sha = hashlib.sha256(data).hexdigest()
            local = directory / f"{len(grants)}-{sha}"
            shutil.copyfile(path, local)
            with contextlib.suppress(FileExistsError):
                os.link(local, self.spool / f"sha256-{sha}")
            digest = f"sha256:{sha}"
            node: Json = value
            for part in output.split(".") if output else []:
                node = node[int(part)] if isinstance(node, list) else cast(dict[str, Json], node)[part]
            if not isinstance(node, dict):
                raise Failed(f"child output {output} is not a file in its result")
            node.update(asset_ref=digest, digest=digest, size_bytes=len(data), media_type=media_type)
            grants.append({"output_id": output, "kind": str(node.get("kind", "")), "digest": digest, "length": len(data),
                           "content_bytes": len(data), "media_type": media_type, "local": str(local)})
            self.held[digest] = local
        return json.dumps(value, separators=(",", ":"), sort_keys=True), grants


def spooled(spool: Path, asset_ref: str) -> Path:
    """The file an attempt saved under `asset_ref` (`attempt:<id>/video/0001`), in its spool; or one it holds by its
    digest (`sha256:<hex>`: a child's output it returns as its own, held as `sha256-<hex>`)."""
    if asset_ref.startswith("sha256:"):
        held = spool / asset_ref.replace(":", "-", 1)
        if not held.exists():
            raise Failed(f"{asset_ref} is not held in {spool.name}'s spool")
        return held
    found = sorted(spool.glob(asset_ref.split("/", 1)[-1].replace("/", "-") + "*"))
    if not found:
        raise Failed(f"{asset_ref} has no file in {spool.name}'s spool")
    return found[0]


def digests(value: Json, path: str = "") -> list[tuple[str, str]]:
    """Every `sha256:` string in a request, by its field path (`payload.first_frame`)."""
    if isinstance(value, str):
        return [(path, value)] if value.startswith("sha256:") and len(value) == 71 else []
    items = enumerate(value) if isinstance(value, list) else value.items() if isinstance(value, dict) else ()
    return [found for key, item in items for found in digests(item, f"{path}.{key}" if path else str(key))]


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
            "model_parameter_name": parameter, "components": components, "snapshot": manifest,
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
    its kind and number, its digest and size go (its samples are judged instead), and a checkpoint's id is only an id
    (its tensors are judged instead: `written`)."""
    if isinstance(result, list):
        return [plain(item) for item in result]
    if isinstance(result, str) and len(result) == 71 and result.startswith("sha256:"):
        return "sha256:*"
    if not isinstance(result, dict):
        return result
    kept = {k: plain(v) for k, v in result.items() if not ("asset_ref" in result and k in ("digest", "size_bytes"))}
    ref = kept.get("asset_ref")
    if isinstance(ref, str) and ref.startswith("attempt:"):
        kept["asset_ref"] = ref.split("/", 1)[1]
    return kept


def written(cell: Cell, items: list[Item], store: Path) -> dict[str, dict[str, int]]:
    """Each weights output the cell asks for, committed: its checkpoint's tensors, and how many are encoded."""
    import tensorfs

    found: dict[str, dict[str, int]] = {}
    for output in cell.get("weights", {}):
        commits = [item["revision"] for item in items if item["output"] == output]
        if not commits:
            raise Failed(f"weights output {output} was never committed")
        checkpoint = tensorfs.open(commits[-1], cache_dir=store, local_files_only=True)
        rows = [row for component in checkpoint.components.values() for row in component.tensors.values()]
        found[output] = {"tensors": len(rows), "encoded": sum(row.encoding is not None for row in rows)}
        if not rows:
            raise Failed(f"weights output {output} holds no tensors")
    return found


#: an output id -> its file and media type
type Files = dict[str, tuple[Path, str]]


def judge(cell: Cell, reply: Reply, spool: Path, reference: Seen | None,
          committed: Callable[[], dict[str, dict[str, int]]]) -> tuple[Seen, Files]:
    """The cell's output judged, never only its completion: every declared output present, finite and not flat (or
    declared empty), the cell's own check, and the stored reference's."""
    outcome = reply.get("outcome", {})
    if not reply.get("ok") or outcome.get("terminal") != "succeeded":
        raise Failed(f"{outcome.get('code') or reply.get('code')}: {first_line(outcome.get('message') or reply.get('detail', ''))}")
    result_file = spool / "result.canonical"
    result = cast(Json, json.loads(result_file.read_bytes())) if result_file.exists() else reply.get("result")
    seen: Seen = {"result": plain(result), "outputs": []}
    weights = committed()
    if weights:
        seen["weights"] = weights
    empty = set(cell.get("empty", []))
    files: list[Path] = []
    named: Files = {}
    for declared in sorted(reply.get("outputs") or [], key=lambda o: str(o.get("output_id"))):
        output, ref, media_type = (str(declared.get(k, "")) for k in ("output_id", "asset_ref", "media_type"))
        path = spooled(spool, ref)
        files.append(path)
        named[output] = (path, media_type)
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
    if not seen["outputs"] and not weights and result in (None, {}, []):
        raise Failed("the reply declares no outputs and an empty result")
    check = cell.get("check")
    if check is not None and (problem := check(result, files)):
        raise Failed(problem)
    if reference is not None:
        compare(seen, reference, tolerance(cell))
    return seen, named


def tolerance(cell: Cell) -> float:
    """A cell's allowed mean difference, of 255: its own, else 1 from a stored reference and 0.5 from its one-rank twin
    (a CPU group's output is that twin's to the bit; a rank on another rank's rotary rows is 2/255 off)."""
    return float(cell.get("tolerance", 0.5 if "same_as" in cell else 1.0))


def frames(path: Path, media_type: str, result: Json) -> Iterator[npt.NDArray[np.int16]]:
    """An output's samples a frame at a time: a video's pictures; else the output whole. A video's sound is not here:
    its encoder (AAC) turns a last-bit difference between a group's sum and one rank's into whole-band noise, so a sound
    is judged by its thumbnail (`compare`)."""
    if media_type.startswith("video/"):
        import av

        with av.open(str(path)) as container:
            for frame in container.decode(video=0):
                yield frame.to_ndarray(format="rgb24").astype(np.int16)
        return
    for values in samples(path, media_type).values():
        yield shaped(values, result).astype(np.int16)


def twin_gap(mine: Files, theirs: Files, result: Json) -> tuple[str, float]:
    """How far a group's outputs are from its one-rank twin's, sample by sample: the worst frame's (or picture's) mean
    absolute difference, of 255, and where. A group computes what one rank does, so its pictures are that twin's to the
    bit; a rank that reads another rank's rows, or runs without an overlay the others armed, moves whole frames."""
    worst, where = 0.0, ""
    for output in sorted(mine):
        if output not in theirs:
            raise Failed(f"output {output} is not among its twin's {sorted(theirs)}")
        (a, kind), (b, _) = mine[output], theirs[output]
        if not a.stat().st_size and not b.stat().st_size:
            continue
        count = 0
        for index, (x, y) in enumerate(itertools.zip_longest(frames(a, kind, result), frames(b, kind, result))):
            if x is None or y is None or x.shape != y.shape:
                raise Failed(f"output {output} has another shape than its twin's at frame {index}")
            gap = float(np.abs(x - y).mean())
            count += 1
            if gap > worst:
                worst, where = gap, f"{output} frame {index}"
        if not count:
            raise Failed(f"output {output} decodes to nothing")
    return where, worst


def compare(seen: Seen, reference: Seen, tolerance: float) -> None:
    """Same outputs, each decoding to as many samples, thumbnails within `tolerance` on average (and 12 times it at
    most), and the same result document."""
    def shape(outputs: list[Output]) -> list[tuple[str, int]]:
        return [(o["name"], o["bytes"]) for o in outputs]

    want, got = reference["outputs"], seen["outputs"]
    if shape(want) != shape(got):
        raise Failed(f"outputs {shape(got)} differ from the reference's {shape(want)}")
    for a, b in zip(want, got, strict=True):
        if len(a["signature"]) != len(b["signature"]):
            raise Failed(f"output {b['name']}'s reference is a {len(a['signature'])}-part signature, not this "
                         f"{len(b['signature'])}-part thumbnail: re-record it (run.sh --record)")
        mean, worst = differ(a["signature"], b["signature"])
        allowed = max(tolerance, 1.0) if b["name"].endswith(".audio") else tolerance  # an encoded sound (`frames`)
        if mean > allowed or worst > 12 * allowed:
            raise Failed(f"output {b['name']} differs from its reference by {mean:.2f}/255 on average, {worst:.0f} at most "
                         f"(tolerance {allowed})")
    if reference.get("weights") != seen.get("weights"):
        raise Failed(f"weights {seen.get('weights')} differ from the reference's {reference.get('weights')}")
    if reference["result"] != seen["result"]:
        raise Failed(f"result {json.dumps(seen['result'])[:160]} differs from the reference's {json.dumps(reference['result'])[:160]}")


MEDIA = {".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".webp": "image/webp", ".mp4": "video/mp4",
         ".wav": "audio/wav", ".flac": "audio/flac", ".mp3": "audio/mpeg", ".json": "application/json"}


def inputs(cell: Cell, payload: dict[str, Json], work: Path, made: Path,
           shared: InputHistory) -> tuple[dict[str, InputMetadata], dict[str, InputFile]]:
    """The cell's input files as the machine grants them: its bytes in the attempt's own directory, the payload's field
    holding the reference, `PrepareRequest.input_metadata` and `Invoke.inputs` keyed by the field path (`asset_inputs`)."""
    metadata: dict[str, InputMetadata] = {}
    granted: dict[str, InputFile] = {}
    files: Mapping[str, Source] = cell.get("files", {})
    same_as = cell.get("same_as")
    reference = shared.get(same_as) if same_as else None
    if same_as and files and reference is None:
        raise Failed(f"file inputs for {same_as} are unavailable: include that reference cell")
    if reference is not None:
        if set(files) != set(reference):
            raise Failed(f"file fields differ from the comparison cell {same_as}")
        files = {field: Path(file["local"]) for field, file in reference.items()}
    for field, source in files.items():
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
    if reference is not None and {k: v["digest"] for k, v in granted.items()} != {k: v["digest"] for k, v in reference.items()}:
        raise Failed(f"input bytes differ from the comparison cell {same_as}")
    key = f"{cell['entry']}/{cell['name']}"
    if key in shared:
        frozen: dict[str, InputFile] = {}
        for field, file in granted.items():
            # Snapshot before the model runs. A model only receives its per-cell copy.
            source_path = Path(file["local"])
            cache = work.parent / ".shared" / (file["digest"].removeprefix("sha256:") + source_path.suffix)
            cache.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source_path, cache)
            frozen[field] = {**file, "local": str(cache)}
        shared[key] = frozen
    return metadata, granted


def grant(field: str, path: Path) -> tuple[InputMetadata, InputFile]:
    """A file the machine grants at `field` (its own path): `PrepareRequest.input_metadata`'s row and `Invoke.inputs`'."""
    data = path.read_bytes()
    digest = "sha256:" + hashlib.sha256(data).hexdigest()
    order = next((int(part) for part in reversed(field.split(".")) if part.isdigit()), 0)
    media_type = MEDIA.get(path.suffix, "application/octet-stream")
    return ({"input_id": field, "media_type": media_type, "digest": digest, "length": len(data), "order": order},
            {"local": str(path), "media_type": media_type, "digest": digest, "length": len(data), "order": order})


def compact(value: object, depth: int = 0) -> str:
    """JSON with one key a line, each list of numbers (a thumbnail) on one line."""
    pad, inner = " " * depth, " " * (depth + 1)
    if isinstance(value, dict) and value:
        rows = [f"{inner}{json.dumps(k)}: {compact(v, depth + 1)}" for k, v in sorted(value.items())]
        return "{\n" + ",\n".join(rows) + f"\n{pad}}}"
    if isinstance(value, list) and value and not all(isinstance(v, (int, float)) and not isinstance(v, bool) for v in value):
        return "[\n" + ",\n".join(f"{inner}{compact(v, depth + 1)}" for v in value) + f"\n{pad}]"
    return json.dumps(value, separators=(",", ":"))


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
    try:
        fixture = load_fixture(fixture_path, package)
    except Exception as exc:  # a fixture that can't import (a helper the Runtime refuses) is the package's failure
        with open(out / "fixture.err", "w") as err:
            traceback.print_exc(file=err)
        print(f"FAIL {package} - fixture 0.0s: {type(exc).__name__}: {first_line(exc)}", flush=True)
        return 1
    waive = cast(dict[str, str], getattr(fixture, "WAIVE", {}))
    #: `module:NAME` -> the literal it takes in the executor (a release geometry shrunk)
    patch = {k: repr(v) for k, v in cast(dict[str, object], getattr(fixture, "PATCH", {})).items()}
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
    module = interface["application"].split(":")[0]
    #: the package's functions its orchestrators call, as TensorD lists them on a Start (`calls::call_interfaces`)
    calls: list[CallRow] = [{"module": module, "export": export, "self": True} for export in sorted(
        {str(call.get("function")) for row in interface["entrypoints"] for call in row.get("calls", [])})]
    children: dict[str, Executor] = {}

    #: a call that constructs nothing loads one binding naming only its package, as TensorD sends it
    weightless: Binding = {"application": interface["application"], "package_interface": str(interface_path)}

    def run_child(export: str, request: dict[str, Json], files: dict[str, Path], child: str) -> tuple[Reply, Path]:
        """A child call through the child executor, as TensorD runs it on its own (GPU) executor: the callee's slots
        bound and loaded once, its input files granted where its request names them."""
        row = entries.get(export)
        if row is None:
            raise Failed(f"a call names {export}, which the package interface does not have")
        rows = bindings(interface, str(interface_path), row, manifests, store, f"{package}@{version}", package)
        executor = children.get("child")
        if executor is None:
            executor = children["child"] = Executor(out / "executor-child", store, waive, patch)
            executor.must({"cmd": "hello"})
            executor.must({"cmd": "start", "devices": "", "application": interface["application"],
                           "package_interface": str(interface_path), "sequence_parallel_degree": 1, "model_choices": True,
                           "constructions": []})
        construction = f"{export}@child"
        if (0, construction) not in loaded:
            load: Load = {"cmd": "load", "construction": construction, "devices": "", "sequence_parallel_degree": 1,
                          "models": rows or [weightless]}
            executor.must(load)
            loaded.add((0, construction))
        rows_in = {field: grant(field, path) for field, path in files.items()}
        executor.must({"cmd": "prepare_request", "request_id": child, "entrypoint": export, "payload": request,
                       "construction": construction, "input_metadata": {k: v[0] for k, v in rows_in.items()}})
        spool = out / "spool" / child
        spool.mkdir(parents=True)
        executor.broker = Broker(child, spool, run_child)
        executor.broker.held |= {rows_in[field][1]["digest"]: path for field, path in files.items()}
        reply = executor.ask({"cmd": "invoke", "request_id": child, "entrypoint": export, "spool": str(spool),
                              "construction": construction, "deadline_s": 1800, "inputs": {k: v[1] for k, v in rows_in.items()}},
                             timeout=1860)
        executor.broker = None
        (out / f"{child}.reply.json").write_text(json.dumps(reply, indent=1))
        return reply, spool

    #: each cell's judged output this run: a `same_as` cell is judged against its twin's here
    ran: dict[str, Seen] = {}
    #: and its output files, for a twin compared sample by sample
    kept: dict[str, Files] = {}
    shared_inputs: InputHistory = {cell["same_as"]: None for cell in cells if "same_as" in cell}
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
                payload = cast(dict[str, Json], json.loads(json.dumps(cell["payload"])))
                metadata, granted = inputs(cell, payload, out / "inputs" / request, out / "made", shared_inputs)
                # The request as a person sends it, for cli.sh to bind: its fields and its files by path.
                (out / "requests").mkdir(exist_ok=True)
                (out / "requests" / f"{request}.json").write_text(json.dumps({
                    "entry": entry, "payload": cell["payload"], "files": {f: g["local"] for f, g in granted.items()},
                    "models": bool(cell.get("models"))}))
                # A cell may bind a slot to another of the fixture's models (`model=` on a request).
                chosen = {**manifests, **{path: manifests[ref] for path, ref in cell.get("models", {}).items()}}
                rows = bindings(interface, str(interface_path), row, chosen, store, f"{package}@{version}", package)
                executor = executors.get(ranks)
                if executor is None:
                    executor = executors[ranks] = Executor(out / f"executor-{ranks}", store, waive, patch, ranks)
                    executor.must({"cmd": "hello"})
                    start: Start = {
                        "cmd": "start", "devices": "", "application": interface["application"],
                        "package_interface": str(interface_path), "sequence_parallel_degree": ranks, "model_choices": True,
                        "constructions": [{"path": b["model_binding_path"], "manifest": b["snapshot"], "components": b["components"]}
                                          for b in rows if "snapshot" in b and "model_binding_path" in b and "components" in b],
                    }
                    if calls:
                        start["call_interfaces"] = calls
                    executor.must(start)
                construction = f"{entry}@{ranks}" + "".join(f":{k}={v}" for k, v in sorted(cell.get("models", {}).items()))
                if (ranks, construction) not in loaded:
                    load: Load = {"cmd": "load", "construction": construction, "devices": "", "sequence_parallel_degree": ranks,
                                  "models": rows or [weightless]}
                    executor.must(load)
                    loaded.add((ranks, construction))
                executor.must({"cmd": "prepare_request", "request_id": request, "entrypoint": entry, "payload": payload,
                               "construction": construction, "input_metadata": metadata})
                spool = out / "spool" / request
                spool.mkdir(parents=True)
                invoke: Invoke = {"cmd": "invoke", "request_id": request, "entrypoint": entry, "spool": str(spool),
                                  "construction": construction, "deadline_s": 1800, "inputs": granted}
                if "sources" in cell:
                    invoke |= {"sources": {k: manifests[ref] for k, ref in cell["sources"].items()},
                               "weights": cell.get("weights", {}), "operation": f"{package}/{entry}"}
                executor.items.clear()
                # A client watches every attempt's previews: each `preview` line runs on the loop's real layout. A tiny
                # fixture's latents have a few channels, not its VAE's: that alone is what such a line may say.
                executor.previews.clear()
                executor.preview_said.clear()
                if executor.previewing:
                    executor.send({"cmd": "preview", "request_id": request, "for_ms": 3_600_000})
                # TensorD answers every attempt's durable requests: a call, a GPU release, a stage memo.
                executor.broker = Broker(request, spool, run_child)
                executor.broker.held |= {g["digest"]: Path(g["local"]) for g in granted.values()}  # it may pass them on
                # As long as the deadline it gave the attempt: a busy computer's 1080p decode outlasts 900 s.
                reply = executor.ask(invoke, timeout=invoke["deadline_s"] + 60)
                executor.broker = None
                (out / f"{request}.reply.json").write_text(json.dumps(reply, indent=1))
                (out / f"{request}.previews.json").write_text(json.dumps({
                    "previews": [{k: v for k, v in one.items() if k != "data"} for one in executor.previews],
                    "said": executor.preview_said,
                }, indent=1))
                wrong = [said for said in executor.preview_said if "where this VAE's latents have" not in said]
                if wrong:
                    raise Failed(wrong[0])
                key = f"{entry}/{name}"
                same_as = cell.get("same_as")
                against = ran.get(same_as) or references.get(same_as) if same_as else None if record else references.get(key)
                if against is None and not (record and same_as is None):
                    raise Failed(f"no output of {same_as or key} to judge against: run it too, or record (run.sh --record)")
                items = executor.items
                seen, named = judge(cell, reply, spool, against, lambda: written(cell, items, store))
                ran[key], kept[key] = seen, named
                if same_as is not None and same_as in kept:
                    where, gap = twin_gap(named, kept[same_as], seen["result"])
                    if gap > tolerance(cell):
                        raise Failed(f"{where} differs from {same_as}'s by {gap:.2f}/255 on average "
                                     f"(tolerance {tolerance(cell)})")
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
        for executor in [*executors.values(), *children.values()]:
            executor.close()
    if record:
        references_path.parent.mkdir(parents=True, exist_ok=True)
        references_path.write_text(compact(references) + "\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
