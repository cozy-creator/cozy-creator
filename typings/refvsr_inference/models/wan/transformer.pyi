from typing import Self

from torch.nn import Module

class WanTransformer3DModel(Module):
    # The converter installs the projection before collecting its state_dict.
    lq_proj: Module

    @classmethod
    def from_config(cls, config: dict[str, object]) -> Self: ...

    def build_lq_proj(self) -> Module: ...
