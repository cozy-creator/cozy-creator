from typing import Literal, Self

from torch.nn import Module

class FlashVSRConditionNet(Module):
    @classmethod
    def from_variant(
        cls,
        variant: Literal["halfS", "S", "M", "B", "L", "XL"] = "S",
        **overrides: object,
    ) -> Self: ...
