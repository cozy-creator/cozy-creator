"""How each model of one installed package pages, so no model surprises a small card: every model slot's default
checkpoint is opened header-only, its graph built on the meta device by the package's own `Model.load`, and each
component's paging units found by the Runtime's own `paging` and `split` on the checkpoint's stored bytes. Prints a
row per component and a `::warning::` for a unit over 1 GiB; reads no weights and fails nothing.
Runs in the release's environment beside probe.py (e2e/imports/run.sh): paging.py <distribution>."""

import importlib.metadata as md
import json
import sys
import typing

try:
    import tensorfs
    import torch
    from tensorfs.raw import parts as stored_parts
    from torch import nn

    from cozy_runtime.author._loader import Config, ModelAssets, _members
    from cozy_runtime.author._model import Model
    from cozy_runtime.internal import canonical, model_config
    from cozy_runtime.internal.fill import ROOT_COMPONENT, _transformers_binding
    from cozy_runtime.internal.residency import _unit, paging, split
except ImportError as missing:  # a Runtime before paging (0.35.7), or a package without Torch
    print(f"paging: skipped ({missing})")
    sys.exit(0)
try:  # fill's Transformers binding reads these when Transformers is installed
    import transformers.conversion_mapping  # noqa: F401
    import transformers.core_model_loading  # noqa: F401
except ImportError:
    pass

GIB, WARN = 1 << 30, 1 << 30


class Built(Exception):
    def __init__(self, obj: object) -> None:
        self.obj = obj


class Loader:
    """Model.load's loader on a header-only checkpoint: `construct` builds on the meta device and ends the load."""

    def __init__(self, config: Config, ck: object) -> None:
        self.config, self.ck = config, ck

    @property
    def assets(self) -> ModelAssets:
        values = {}
        for component, held in self.ck.components.items():
            prefix = "" if self.ck.pipeline is None and component == ROOT_COMPONENT else f"{component}/"
            if held.config:
                values[f"{prefix}{held.config_file}"] = json.dumps(held.config, indent=2).encode()
            for name in held.assets:
                values[f"{prefix}{name}"] = held.asset(name)
        return ModelAssets(values, lambda _: None)

    def construct(self, t: type, *, factory: typing.Callable[[Config], object]) -> object:
        with torch.device("meta"):
            raise Built(factory(self.config))

    def __getattr__(self, name: str) -> object:
        raise RuntimeError(f"load used loader.{name} before construct")


def checkpoint(ref: str) -> tuple[object, str]:
    name, _, release = ref.partition("@")
    failure: Exception | None = None
    for lane in ("int8", "bf16", None):  # the 8-bit default lane, else the 16-bit one, else the model's only
        try:
            return tensorfs.open(name, release=release or None, lane=lane, lazy=True), lane or "its"
        except Exception as exc:  # noqa: BLE001  (a lane this model lacks)
            failure = exc
    raise failure  # type: ignore[misc]


def rows(cls: type, ref: str) -> list[str]:
    ck, lane = checkpoint(ref)
    configs = {n: dict(c.config) for n, c in ck.components.items() if c.config}
    try:
        cls.load(cls.__new__(cls), Loader(Config(canonical.parse_canonical(model_config.construction_config(configs)), "model.config"), ck))
        raise RuntimeError("load constructed nothing")
    except Built as built:
        graph = built.obj
    out = []
    for name, root in _members(graph):
        held = ck.components.get(name)
        if not isinstance(root, nn.Module) or held is None or not held.tensors:
            continue
        bind, consumed = _transformers_binding(dict(held.config or {})), set(root.state_dict(keep_vars=True))
        sizes = {bind(k): sum(p.length for p in stored_parts(ck, name, k).values()) for k in held.tensors}
        sizes = {k: n for k, n in sizes.items() if k in consumed}
        rule, found = paging(root)
        blocks = split(root, found, sizes)
        units: dict[str, int] = {}
        for tensor, n in sizes.items():
            unit = _unit(tensor, set(blocks))
            units[unit] = units.get(unit, 0) + n
        largest = max((n for u, n in units.items() if u), default=0)
        unit, size = max(units.items(), key=lambda item: item[1], default=("", 0))
        rule = rule if blocks == found else f"{rule}+split"
        out.append(f"paging {ref} ({lane} lane) {name}: {type(root).__name__}, rule {rule}, {len(blocks)} blocks, "
                   f"largest block {largest / GIB:.2f} GiB, common {units.get('', 0) / GIB:.2f} GiB")
        if size > WARN:
            out.append(f"::warning::paging: {ref} {name} ({type(root).__name__}): {unit or 'its common weights'} is one "
                       f"{size / GIB:.2f} GiB unit (rule {rule}); a card with less free memory cannot run it")
    return out


name = sys.argv[1]
app = next((p.load() for p in md.distribution(name).entry_points if p.group == "cozy.application"), None)
seen: set[tuple[str, type]] = set()
for registration in getattr(app, "_registry", {}).values():
    hints = typing.get_type_hints(registration.fn)
    classes = {k: v for k, v in hints.items() if isinstance(v, type) and issubclass(v, Model)} | dict(registration.model_classes)
    for slot, ref in registration.model_defaults.items():
        cls = classes.get(slot) or classes.get(slot.split(".")[-1]) or (next(iter(classes.values())) if len(classes) == 1 else None)
        if cls is None or (ref, cls) in seen:
            continue
        seen.add((ref, cls))
        try:
            print("\n".join(rows(cls, ref)))
        except Exception as exc:  # noqa: BLE001  (reported, never a failure: this check is advisory)
            print(f"::warning::paging: {name} {ref}: not checked ({type(exc).__name__}: {str(exc)[:200]})")
