"""gen.py <card> comfy <session> <label,...> | hold <free-mib> | setup <file...> | graphs <on|off>

Renders a self-contained cozy script into gen/<card>/:
- comfy_<session>.py: comfy_bench.py with the labelled rows of cards/<card>/requests.json, their reference images and
  the driver. Rows of one session share one ComfyUI server, as Cozy's requests share an executor.
- vram_hold_<mib>.py: vram_hold.py leaving <mib> MiB free (0 releases).
- comfy_setup.py: comfy_setup.py downloading only the named files.
- graphs_<on|off>.py: graphs_toggle.py, the CUDA-graphs experiment's pod-local switch."""
import ast
import base64
import json
import sys
from pathlib import Path

H = Path(__file__).resolve().parent
D = H.parent


def fill(template: str, subs: dict[str, str], out: Path) -> Path:
    src = (H / template).read_text()
    for old, new in subs.items():
        assert old in src, old
        src = src.replace(old, new)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(src)
    return out






def driver() -> str:
    return (H / "comfy_driver.py").read_text()


def comfy(card: str, session: str, labels: list[str]) -> Path:
    rows = {r["label"]: r for r in json.loads((D / "cards" / card / "requests.json").read_text())}
    jobs = [rows[label] for label in labels]
    assert all(j["engine"] == "comfy" for j in jobs), labels
    images = {j["ref"]: base64.b64encode((D / "refs" / j["ref"]).read_bytes()).decode() for j in jobs if "ref" in j}
    return fill("comfy_bench.py", {"CELL = None  # @CELL@": f"CELL = {session!r}",
                                   "JOBS = None  # @JOBS@": f"JOBS = json.loads({json.dumps(jobs, ensure_ascii=False)!r})",
                                   "IMAGES = None  # @IMAGES@": f"IMAGES = {images!r}",
                                   "DRIVER = None  # @DRIVER@": f"DRIVER = {driver()!r}",
                                   "STEP_OBSERVER = None  # @STEP_OBSERVER@": f"STEP_OBSERVER = {(H / 'comfy_steps_node.py').read_text()!r}"},
                D / "gen" / card / f"comfy_{session}.py")


if __name__ == "__main__":
    card, kind, *rest = sys.argv[1:]
    if kind == "comfy":
        print(comfy(card, rest[0], rest[1].split(",")))
    elif kind == "setup":
        print(fill("comfy_setup.py", {"WANT = None  # @WANT@": f"WANT = {sorted(rest)!r}",
              "IDENTITY = None  # @IDENTITY@": f"IDENTITY = {(H / 'comfy_identity.py').read_text()!r}",
              "REQUIREMENTS = None  # @REQUIREMENTS@": f"REQUIREMENTS = {(D / 'comfy-requirements.txt').read_text()!r}"}, D / "gen" / card / "comfy_setup.py"))
    elif kind == "qwen-setup":
        names = ("qwen_common_files.py", "qwen_comfy_identity.py", "qwen_preflight.py")
        sources = {name: (H / name).read_text() for name in names}
        print(fill("qwen_setup.py", {"SOURCES = None  # @QWEN_SOURCES@": f"SOURCES = {sources!r}"}, D / "gen" / card / "qwen_setup.py"))
    elif kind == "profile-install":
        print(fill("profile_install.py", {"PROFILE_SOURCE = None  # @PROFILE_SOURCE@": f"PROFILE_SOURCE = {(H / 'comfy_profile_node.py').read_text()!r}"}, D / "gen" / card / "profile_install.py"))
    else:
        raise SystemExit(f"unknown kind {kind}")
