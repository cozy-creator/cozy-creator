"""tfs-299 host room proof: 0.25.0 (r) vs RC (h) vs fix (f), uncapped Flux-2 turbo and Ideogram 4 on one RTX 4090,
the host held to ~58 GiB available at each arm's start (cr-297's 62.6 GiB pod cheza). Arms forward r h f, then reverse
f h r on other seeds (the call memo keys Runtime/TensorFS builds and seeds). Each arm: Flux-2 x3, Ideogram x2, Flux-2.
Session 2 (gen.py --fresh): every request a new prompt, so each one runs its text encoder (no stage memo hit).
python3 gen.py [--fresh] <runtime run> <base creator run> <base sha> <fix creator run> <fix sha> > params.json"""
import json, sys
FX = "prompt=A red vintage bicycle leaning on a sunlit yellow wall in a narrow street, photo"
IDEO = ('prompt={"high_level_description":"A bakery storefront with a sign that reads FRESH BREAD","style_description":'
        '"photograph, soft morning daylight","compositional_deconstruction":{"background":"a brick shop front with a wide '
        'window of loaves and a green door","elements":[{"type":"text","bbox":[90,180,230,820],"text":"FRESH BREAD",'
        '"desc":"a painted wooden sign above the window"}]}}')
REL_RT = "https://github.com/cozy-creator/cozy-runtime/releases/download/v0.46.0/cozy_runtime-0.46.0-cp312-abi3-manylinux_2_28_x86_64.whl"
REL_TFS = "https://github.com/cozy-creator/cozy-creator/releases/download/tensorfs-v0.25.1/tensorfs-0.25.1-cp311-abi3-manylinux_2_28_x86_64.whl"
M = {"x": ("flux-2/generate_turbo", [FX, "prompt_upsampling:=false"]),
     "i": ("ideogram-4/generate", [IDEO, "megapixels:=1", "aspect_ratio=1:1", "steps:=48", "guidance:=7", "prompt_upsampling:=false"])}
ARM = [("x", 1), ("x", 2), ("x", 3), ("i", 1), ("i", 2), ("x", 4)]
FREE = 58
FRESH = "--fresh" in sys.argv
rt, base_run, base_sha, fix_run, fix_sha = [a for a in sys.argv[1:] if a != "--fresh"]
SCENES = ["at dawn", "in light rain", "under a red awning", "with pigeons on the cobbles", "beside a blue door",
          "at dusk with string lights", "in deep afternoon shadow", "after snowfall"]
stack = {"r": {"runtime": REL_RT, "tensorfs": REL_TFS},
         "h": {"runtime": rt, "tensorfs": f"artifact:{base_run}/tensorfs-wheel-{base_sha}"},
         "f": {"runtime": rt, "tensorfs": f"artifact:{fix_run}/tensorfs-wheel-{fix_sha}"}}


def cell(name, key, seed, arm, first, fresh=None):
    target, args = M[key]
    if fresh is not None:  # a prompt of its own: Flux-2's scene, Ideogram's sign
        args = [f"{args[0]}, {SCENES[fresh]}" if key == "x" else args[0].replace("FRESH BREAD", f"FRESH BREAD {fresh + 1}"),
                *args[1:]]
    d = {"name": name, "target": target, "args": [*args, f"seed:={seed}"]}
    if arm == "r":
        d["target"] = target + "@0.9.0"  # 0.25.0's releases
    else:
        d["tree"] = "pk"
    if first:
        d["update"] = {**stack[arm], "tensord": True, "gpu_memory_limit": "off"}
        d["host_free_gib"] = FREE
    return d


base = 150000 if FRESH else 140000
cells = [cell(f"w{arm}-{k}", k, base + 10 * j + n, arm, n == 0, 7 if FRESH else None)
         for j, arm in enumerate("rh") for n, k in enumerate("xi")]
for d, order in ((1, "rhf"), (2, "fhr")):
    for arm in order:
        cells += [cell(f"{arm}{d}-{k}{n}", k, base + 1000 * d + 10 * (k == "i") + n, arm, i == 0,
                       (3 * (d - 1) + n - 1) % 7 if FRESH else None)
                  for i, (k, n) in enumerate(ARM)]
compare = [[f"h{d}-{k}{n}", f"f{d}-{k}{n}"] for d in (1, 2) for k, n in ARM]
print(json.dumps({"cells": cells, "compare": compare, "need_s": 6600, "max_fails": 4, "floor_mb_s": 50, "fetch_mbs": 50}))
