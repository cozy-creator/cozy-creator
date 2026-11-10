"""What a fixture (`tiny/<package>.py`) declares, and what drive.py records per cell."""

from __future__ import annotations

from collections.abc import Callable
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
    #: ranks of a CPU (gloo) group; 1 when absent
    ranks: NotRequired[int]
    #: judged against another cell's reference (`entry/name`): a group's output equals one rank's
    same_as: NotRequired[str]
    #: the largest difference of a 64-part output signature from its reference, of 255
    tolerance: NotRequired[float]
    #: why this cell cannot run on the CPU; printed as SKIP
    skip: NotRequired[str]


class Fixture(Protocol):
    """A package's tiny models and cells."""

    CELLS: list[Cell]

    def models(self, root: Path, store: Path) -> dict[str, Path | str]:
        """Each model a slot binds (its `default` ref or its path) -> a source folder that TensorFS imports, or a
        `sha256:` checkpoint already written into `store`."""
        ...


class Output(TypedDict):
    name: str
    bytes: int
    signature: list[float]


class Seen(TypedDict):
    """A cell's judged output: its result document and each output file's size and signature."""

    result: Json
    outputs: list[Output]


def size(width: int, height: int) -> Check:
    """The result states this canvas."""

    def check(result: Json, files: list[Path]) -> str | None:
        if not isinstance(result, dict):
            return f"the result is {type(result).__name__}, not an object"
        got = (result.get("width"), result.get("height"))
        return None if got == (width, height) else f"canvas {got[0]}x{got[1]}, not {width}x{height}"

    return check


class Pretrained(Protocol):
    def save_pretrained(self, save_directory: str | Path) -> None: ...


def module(value: object) -> torch.nn.Module:
    """A Diffusers or Transformers model as the torch module it is: their decorated constructors are untyped (the
    third-party boundary every fixture crosses)."""
    return cast("torch.nn.Module", value)


def save(value: object, folder: Path) -> None:
    """A Diffusers or Transformers model, config or scheduler in the source format the real model ships in."""
    cast(Pretrained, value).save_pretrained(folder)
