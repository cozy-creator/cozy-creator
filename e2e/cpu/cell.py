"""What a fixture (`tiny/<package>.py`) declares, and what drive.py records per cell."""

from __future__ import annotations

import contextlib
import json
import os
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import TYPE_CHECKING, NotRequired, Protocol, TypedDict, cast

if TYPE_CHECKING:
    import torch

type Json = None | bool | int | float | str | list[Json] | dict[str, Json]
#: A cell's own judgement of its result document and output files: what is wrong, or None.
type Check = Callable[[Json, list[Path]], str | None]
#: An input file: a path, or made in the given directory at run time.
type Source = Path | Callable[[Path], Path]


class Cell(TypedDict):
    """One request through the executor, and how its output is judged."""

    entry: str
    name: str
    payload: dict[str, Json]
    #: field path (`image`, `assets.0`) -> the file the machine grants there
    files: NotRequired[dict[str, Source]]
    check: NotRequired[Check]
    #: a slot (its path) bound to another of the fixture's models (a ref `models` builds), as `model=` binds it
    models: NotRequired[dict[str, str]]
    #: ranks of a CPU (gloo) group; 1 when absent
    ranks: NotRequired[int]
    #: judged against another cell's output (`entry/name`; that cell's run here, else its reference): a group's output
    #: equals one rank's
    same_as: NotRequired[str]
    #: the mean difference allowed, of 255: an output's thumbnail from its reference's (default 1), and a `same_as`
    #: cell's worst frame from its twin's, sample by sample (default 0.5)
    tolerance: NotRequired[float]
    #: why this cell cannot run on the CPU; printed as SKIP
    skip: NotRequired[str]
    #: outputs (by output id) the function returns empty by design
    empty: NotRequired[list[str]]
    #: each `Weights[M]` parameter's model (a ref `models` builds), and each weights output's byte grant
    sources: NotRequired[dict[str, str]]
    weights: NotRequired[dict[str, int]]


class Fixture(Protocol):
    """A package's tiny models and cells."""

    CELLS: list[Cell]

    def models(self, root: Path, store: Path) -> dict[str, Path | str]:
        """Each model a slot binds (its `default` ref or its path) -> a source folder that TensorFS imports, or a
        `sha256:` checkpoint already written into `store`."""
        ...


class Output(TypedDict):
    #: its output id, and `.audio` for a video's sound
    name: str
    #: how many samples it decodes to (its bytes when it is not a picture, video or sound)
    bytes: int
    #: its thumbnail (`drive.signature`)
    signature: list[float]


class Seen(TypedDict):
    """A cell's judged output: its result document, each output file's size and signature, and each weights output's
    tensors (how many, how many encoded)."""

    result: Json
    outputs: list[Output]
    weights: NotRequired[dict[str, dict[str, int]]]


def size(width: int, height: int) -> Check:
    """The result states this canvas."""

    def check(result: Json, files: list[Path]) -> str | None:
        if not isinstance(result, dict):
            return f"the result is {type(result).__name__}, not an object"
        got = (result.get("width"), result.get("height"))
        return None if got == (width, height) else f"canvas {got[0]}x{got[1]}, not {width}x{height}"

    return check


def answers(count: int) -> Check:
    """The result holds `count` answers (`texts`), none empty."""

    def check(result: Json, files: list[Path]) -> str | None:
        texts = result.get("texts") if isinstance(result, dict) else None
        if not isinstance(texts, list) or len(texts) != count or not all(isinstance(t, str) and t for t in texts):
            return f"the result's texts are {json.dumps(texts)[:200]}, not {count} answers"
        return None

    return check


@contextlib.contextmanager
def defined() -> Iterator[None]:
    """While a fixture builds its models, `torch.empty` fills NaN, so a tensor its class leaves uninitialized is drawn
    from the fixture's seed (its non-finite branch) instead of written as whatever the allocator held: a reference needs
    every byte fixed (seedvr2's modulation vectors took the memory's leftovers, so its output changed run to run)."""
    import torch
    import torch.utils.deterministic as deterministic

    # Its module class holds the switch as a property, which the stubs do not declare.
    was = torch.are_deterministic_algorithms_enabled(), bool(getattr(deterministic, "fill_uninitialized_memory"))
    torch.use_deterministic_algorithms(True, warn_only=True)
    setattr(deterministic, "fill_uninitialized_memory", True)
    try:
        yield
    finally:
        torch.use_deterministic_algorithms(was[0])
        setattr(deterministic, "fill_uninitialized_memory", was[1])


class Pretrained(Protocol):
    def save_pretrained(self, save_directory: str | Path) -> None: ...


def module(value: object) -> torch.nn.Module:
    """A Diffusers or Transformers model as the torch module it is: their decorated constructors are untyped (the
    third-party boundary every fixture crosses)."""
    return cast("torch.nn.Module", value)


def save(value: object, folder: Path) -> None:
    """A Diffusers or Transformers model, config or scheduler in the source format the real model ships in."""
    cast(Pretrained, value).save_pretrained(folder)


def packages() -> Path:
    """The packages checkout under test (drive.py states it): a fixture may read another package's bundled files."""
    return Path(os.environ["CPU_E2E_PACKAGES"])


def qwen_tokenizer(folder: Path) -> None:
    """Qwen's own tokenizer (its byte-level BPE, the Qwen3-VL special tokens) and chat template, as Qwen's releases
    ship them (`tokenizer.json`, `tokenizer_config.json`, `chat_template.json`), from the copy minimax-h3 bundles."""
    from transformers import AutoTokenizer

    source = packages() / "minimax-h3" / "minimax_h3" / "processor"
    folder.mkdir(parents=True, exist_ok=True)
    # Transformers' loader is untyped in some releases a package locks: typed here, at the boundary.
    load: Callable[[Path], object] = AutoTokenizer.from_pretrained
    save(load(source), folder)
    template = json.loads((source / "tokenizer_config.json").read_text())["chat_template"]
    (folder / "chat_template.json").write_text(json.dumps({"chat_template": template}))


#: Why a lane of int8.convrot (or a lane built on it) does not run here.
NARROW = "int8.convrot takes matrices whose columns come in 256s; the tiny model's are narrower"


def quantize(ref: str, lane: str, package: str, release: str) -> Cell:
    """The package's `quantize` writing `lane` from the tiny `ref`, as `cozy model quantize` asks it."""
    return {"entry": "quantize", "name": lane, "payload": {"lane": lane, "package": package, "release": release},
            "sources": {"source": ref}, "weights": {lane: 1 << 34}}
