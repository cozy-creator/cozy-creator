"""The default lane each first-party model slot resolves to on each GPU family (cl-1076). Paul, 11-16: "someone switched it
from int8 -> bf16 ... please fix everything and make sure this never happens again." "also LTX 2.5 should be at int8-convrot
as well."

Nothing runs. Each package of the checkout is described through cozy (`cozy run <dir> --describe`), and each slot that takes
encoded weights (`encoded_leaves: accept`, not raw `weights`) and names a Hub default that is no LoRA (its factors go on its base
model's lane) is resolved by `cozy model show <default> --gpu <cc> [--components ...]` (dev-only): the Hub resolve TensorD asks
for that slot, against tensorhub.com's releases. Memory
is not asked: at the 8-bit floor it never lowers a lane (Paul 11-06, xs-154 Q1); a 4-bit-floor slot (`min_bits: 4`) may still
go lower on a card short of memory, by design. A lane other than EXPECTED's, or OTHERWISE's for that default, fails: a default
lane moves only with its line here changed on purpose, named in the PR.

    lanes.py --cozy BIN --project DIR --packages DIR --out DIR PACKAGE...

One line per slot, as run.sh's: PASS|FAIL <package> <entry> lanes/<slot> <seconds>s: <default> <lane per family>; the known
gaps last.
"""

import argparse
import concurrent.futures
import json
import os
import pathlib
import subprocess
import sys
import time

# The GPU families the lane rule tells apart (crates/tensorfs/spec/selection.json's features), a card of each for the log.
FAMILIES = {"8.0": "A100", "8.6": "RTX 3090", "8.9": "RTX 4090", "9.0": "H100", "10.0": "B200", "10.3": "B300", "12.0": "RTX 5090"}
# Paul 11-12: int8 ConvRot wherever int8 tensor cores run; fp8 only on sm103, which has none. `fp8|bf16`: fp8 where the
# release has an fp8 lane, else bf16 (no 8-bit lane of it runs there).
EXPECTED = "int8 int8 int8 int8 int8 fp8|bf16 int8"
# Each default (`org/name@release`, or one slot: `<package>:<slot path>`) that resolves otherwise, on purpose: its lane on each
# family in FAMILIES' order, and why. A default that moves fails until its line here changes.
OTHERWISE = {
    "lightricks/ltx-2.5@0.1.0": ("bf16 bf16 bf16 bf16 bf16 bf16 bf16", "se-139: bf16 its only lane; int8 ConvRot coming"),
    "ifm/pan2@0.1.0": ("bf16 bf16 bf16 bf16 bf16 bf16 bf16", "se-139: bf16 its only lane; int8 ConvRot coming"),
    "kopper/relayvsr@0.2.0": ("bf16 bf16 bf16 bf16 bf16 bf16 bf16", "cl-1076: bf16 its only lane, no 8-bit lane published"),
    "lbh-123-ai/h3-latent-upscaler@0.1.1": ("bf16 bf16 bf16 bf16 bf16 bf16 bf16", "cl-1076: bf16 its only lane, no 8-bit lane published"),
    "qwen/qwen3.5-9b-mtp@0.2.0": ("bf16 bf16 bf16 bf16 bf16 bf16 bf16", "cl-1076: bf16 its only lane, no 8-bit lane published"),
    "tencent/hunyuan-image-3-instruct@0.1.1": ("bf16 bf16 bf16 bf16 bf16 bf16 bf16", "cl-1076: bf16 its only lane (its distil has int8)"),
}
PRODUCTION = "https://tensorhub.com"
EIGHT = ("int8", "fp8")


def cozy(binary: str, args: list[str], cwd: str, hub: str = "") -> tuple[dict | None, str]:
    """`cozy --json <args>`: its answer, else why not. A Hub or Calcifer that is away is asked again, twice."""
    env = dict(os.environ, **({"COZY_HUB_URL": hub} if hub else {}))
    said = ""
    for attempt in range(3):
        try:
            p = subprocess.run([binary, "--json", *args], capture_output=True, text=True, timeout=300, cwd=cwd, env=env)
        except subprocess.TimeoutExpired:
            said = f"cozy {' '.join(args)}: no answer in 300 s"
            continue
        try:
            answer = json.loads(p.stdout[p.stdout.find("{"):]) if "{" in p.stdout else None
        except json.JSONDecodeError:
            answer = None
        if p.returncode == 0 and isinstance(answer, dict) and "error" not in answer:
            return answer, ""
        error = (answer or {}).get("error") or {}
        said = error.get("message") or error.get("code") or (p.stderr.strip().splitlines() or ["exit %d" % p.returncode])[-1]
        if str(error.get("kind", "")).lower() != "unavailable":
            break
        time.sleep(5 * (attempt + 1))
    return None, said[:400]


def slots(interface: dict) -> tuple[list[tuple[str, dict]], int]:
    """The (entry, slot) pairs the cell checks, and how many slots it passes over (no Hub default, or plain weights). The
    Runtime's recipes (`cozy model quantize`) are not run's functions."""
    checked, passed = [], 0
    for entry in interface.get("entrypoints") or []:
        if str((entry.get("invocable") or {}).get("module", "")).startswith("cozy_runtime."):
            continue
        for slot in entry.get("models") or []:
            default = slot.get("default")
            hub = isinstance(default, str) and "/" in default and not default.startswith("local/")
            if hub and slot.get("encoded_leaves") == "accept" and slot.get("weights") is not True:
                checked.append((entry.get("name", "-"), slot))
            else:
                passed += 1
    return checked, passed


def expected(default: str, slot_key: str, lanes: list[str]) -> tuple[list[str], str]:
    """The lane each family should resolve `default` to, and why it isn't EXPECTED (empty: it is)."""
    row, why = OTHERWISE.get(slot_key) or OTHERWISE.get(default) or (EXPECTED, "")
    want = []
    for lane in row.split():
        if lane == "fp8|bf16":
            lane = "fp8" if "fp8" in lanes else "bf16"
        want.append(lane)
    return want, why


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cozy", required=True)
    ap.add_argument("--project", required=True)
    ap.add_argument("--packages", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("package", nargs="+")
    args = ap.parse_args()
    # `--gpu` is dev-only (crates/cli/DEV.md): `dev = true` at the config's top while the cell runs, its config as it was after.
    config = pathlib.Path.home() / ".cozy" / "config.toml"
    before = config.read_text() if config.exists() else None
    kept = [line for line in (before or "").splitlines() if not line.replace(" ", "").startswith("dev=")]
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text("\n".join(["dev = true", *kept]) + "\n")
    try:
        return lanes_of(args)
    finally:
        if before is None:
            config.unlink(missing_ok=True)
        else:
            config.write_text(before)


def lanes_of(args: argparse.Namespace) -> int:
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    began = time.monotonic()

    # Each package's interface as Calcifer reads its source.
    def describe(package: str) -> tuple[str, dict | None, str]:
        answer, said = cozy(args.cozy, ["--full", "run", str(pathlib.Path(args.packages) / package), "--describe"], args.project)
        if answer is not None:
            (out / f"{package}.describe.json").write_text(json.dumps(answer, indent=1))
        return package, (answer or {}).get("interface"), said

    with concurrent.futures.ThreadPoolExecutor(4) as pool:
        described = list(pool.map(describe, args.package))
    work: list[tuple[str, str, dict]] = []
    passed = 0
    for package, interface, said in described:
        if not interface:
            print(f"FAIL {package} - lanes 0s: cozy run {package} --describe: {said or 'no interface'}", flush=True)
            continue
        checked, over = slots(interface)
        passed += over
        work += [(package, entry, slot) for entry, slot in checked]

    # Each default's release: its lanes, and whether it is a LoRA, whose factors go on its base model's lane (passed over).
    lanes: dict[str, list[str]] = {}
    loras: set[str] = set()

    def release_lanes(default: str) -> None:
        answer, said = cozy(args.cozy, ["model", "show", default], args.project, PRODUCTION)
        held = (answer or {}).get("lanes", [])
        lanes[default] = [lane["lane"] for lane in held] if answer else [f"unread: {said}"]
        if held and all(lane.get("kind") == "lora" for lane in held):
            loras.add(default)

    with concurrent.futures.ThreadPoolExecutor(4) as pool:
        list(pool.map(release_lanes, sorted({slot["default"] for _, _, slot in work})))
    passed += sum(1 for _, _, slot in work if slot["default"] in loras)
    work = [(package, entry, slot) for package, entry, slot in work if slot["default"] not in loras]
    # Each default and the components its slot reads, once: the lane each family resolves.
    asks = sorted({(slot["default"], tuple(slot.get("components") or [])) for _, _, slot in work})

    def resolve(ask: tuple[str, tuple[str, ...]], family: str) -> tuple[str, float]:
        default, components = ask
        flags = ["--gpu", family] + (["--components", ",".join(components)] if components else [])
        started = time.monotonic()
        answer, said = cozy(args.cozy, ["model", "show", default, *flags], args.project, PRODUCTION)
        lane = answer["resolved"]["lane"] if answer and answer.get("resolved") else f"refused ({said})"
        return lane, time.monotonic() - started

    resolved: dict[tuple[str, tuple[str, ...]], list[str]] = {}
    took: dict[tuple[str, tuple[str, ...]], float] = {}
    with concurrent.futures.ThreadPoolExecutor(4) as pool:
        futures = {(ask, family): pool.submit(resolve, ask, family) for ask in asks for family in FAMILIES}
        for ask in asks:
            each = [futures[(ask, family)].result() for family in FAMILIES]
            resolved[ask], took[ask] = [lane for lane, _ in each], sum(seconds for _, seconds in each)
    (out / "resolved.json").write_text(json.dumps(
        [{"default": d, "components": list(c), "lanes": lanes[d], "resolved": dict(zip(FAMILIES, resolved[(d, c)]))} for d, c in asks],
        indent=1))

    failed, known, used = 0, {}, set()
    for package, entry, slot in work:
        default, path = slot["default"], slot.get("path", "-")
        key = (default, tuple(slot.get("components") or []))
        slot_key = f"{package}:{path}"
        used.update({default, slot_key})
        want, why = expected(default, slot_key, lanes[default])
        got = resolved[key]
        moved = [f"{g} on sm{f.replace('.', '')} ({FAMILIES[f]}), expected {w}" for f, g, w in zip(FAMILIES, got, want) if g != w]
        row = f"{default}{' reading ' + ','.join(key[1]) if key[1] else ''}: {' '.join(got)}"
        if moved:
            failed += 1
            print(f"FAIL {package} {entry} lanes/{path} {took[key]:.0f}s: {default} resolves {'; '.join(moved)}. A default lane "
                  f"moves only with its line in e2e/cpu/lanes.py changed on purpose, named in the PR", flush=True)
        else:
            print(f"PASS {package} {entry} lanes/{path} {took[key]:.0f}s: {row}{f' (known: {why})' if why else ''}", flush=True)
        if why:
            known.setdefault(default, (why, got, set()))[2].add(f"{package} {path}")
    for stale in sorted(set(OTHERWISE) - used):
        print(f"SKIP - - lanes/{stale} 0s: no slot defaults to it any more: drop its line in e2e/cpu/lanes.py", flush=True)

    defaults = sorted({default for default, _ in asks})
    no_fp8 = [default for default in defaults if "fp8" not in lanes[default] and any(lane in EIGHT for lane in lanes[default])]
    said = [f"lanes: {len(work)} slots of {len(described)} packages ({passed} with no Hub default, plain weights or a LoRA passed over), "
            f"{len(defaults)} defaults on {len(FAMILIES)} GPU families ({', '.join('sm' + f.replace('.', '') for f in FAMILIES)}) "
            f"resolved at {PRODUCTION} in {time.monotonic() - began:.0f} s; {failed} moved"]
    for default, (why, got, where) in sorted(known.items()):
        said.append(f"  known gap: {default}: {' '.join(got)} ({why}); {', '.join(sorted(where))}")
    if no_fp8:
        said.append(f"  bf16 on sm103 (B300: no int8 tensor cores), no fp8 lane published: {', '.join(no_fp8)}")
    print("\n".join(said), flush=True)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
            summary.write("### " + said[0] + "\n\n" + "".join(f"- {line.strip()}\n" for line in said[1:]) + "\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
