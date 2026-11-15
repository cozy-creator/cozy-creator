"""The tensorfs suite's quantize-fp8 cell (cl-974): ideogram-4's int8 recipe for its DiTs, written from Ideogram's fp8,
on a model class that only names the components it reads. Nothing serves it: the cell asks `cozy model quantize` of a
copy of Ideogram 4's fp8 checkpoint with this local package."""

from cozy_runtime.author import App, Model, QuantizationPlan, Recipe, uses_components

app = App()
_DITS = ("transformer", "unconditional_transformer")


class Ideogram4(Model[None]):
    """Ideogram 4's checkpoint layout, as far as the recipe reads it."""

    @uses_components("transformer", "unconditional_transformer")
    def denoise(self) -> None:
        raise NotImplementedError("the quantize-fp8 cell serves nothing")


INT8 = Recipe(
    lane="int8",
    model=Ideogram4,
    plan=QuantizationPlan(
        encoding="int8.convrot",
        components=_DITS,
        include=tuple(
            f"{dit}.layers.*.{linear}.weight"
            for dit in _DITS
            for linear in ("attention.to_[qkv]", "attention.to_out.0", "feed_forward.w[123]", "adaln_modulation")
        ),
        output_precision="preserve",
    ),
)
