"""Names for this package's annotations, imported by type checkers only."""

from typing import TypedDict

import torch

type Parts = dict[str, tuple[str, tuple[int, ...], torch.Tensor]]
#: `specimen`'s parts, decode params and expected values.
type Specimen = tuple[Parts, dict[str, object], torch.Tensor]

Case = TypedDict(
    "Case",
    {
        "shape": tuple[int, ...],
        "layout": str,
        "order": str,
        "global": str,
        "pre": bool,
        "dtype": str,
        "sha256": str,
    },
)
