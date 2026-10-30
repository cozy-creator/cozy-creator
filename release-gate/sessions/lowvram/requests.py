"""requests.py <out.json> <seed base> <arm>... : one new prompt and seed per request key. ComfyUI and the first Cozy arm share
each (level, model, tier) key so their outputs compare; every later arm gets its own (a repeated request on one
machine could be a memo hit, not an inference)."""
import itertools
import json
import sys

SUBJECTS = ["a lighthouse keeper repairing a brass lamp", "a red fox crossing a frozen river", "a street violinist under neon rain",
            "an old cartographer drawing a coastline", "a glass greenhouse full of orchids", "a hot air balloon over terraced rice fields",
            "a robot barista pouring latte art", "a stone bridge in autumn fog", "a child flying a paper kite on a dune",
            "a tea house at a crowded night market", "a sailboat beside basalt cliffs", "a beekeeper among lavender rows",
            "a snow leopard on a windswept ridge", "a clockmaker's cluttered workbench", "a desert caravan at first light",
            "an astronaut tending tomato plants", "a koi pond beneath cherry blossoms", "a jazz trio in a smoky cellar",
            "a fisherman mending nets at dusk", "a library tower with spiral stairs", "a vintage motorcycle on a coastal road",
            "a monk sweeping temple steps", "a city tram in heavy snowfall", "a potter shaping a tall vase"]
STYLES = ["35mm film photograph, natural light", "oil painting, thick impasto", "isometric 3D render, crisp shadows",
          "watercolor illustration, soft edges", "studio product photo, softbox lighting", "cinematic still, shallow depth of field",
          "ukiyo-e woodblock print", "anime key visual, vivid colors"]
LIGHTS = ["at golden hour", "under an overcast sky", "at night", "in morning mist", "in harsh noon sun", "by candlelight"]
LEVELS, MODELS, TIERS = ["full", "6144", "3072", "4096", "2048", "8192", "12288"], ["sdxl", "anima", "qwen", "qwen4", "krea"], ["cold", "w1", "w2", "w3", "w4", "w5", "w6"]


def main() -> None:
    out, base, arms = sys.argv[1], int(sys.argv[2]), sys.argv[3:]
    pairs = ((f"{subject} {light}", style) for light, style, subject in itertools.product(LIGHTS, STYLES, SUBJECTS))
    table, n = {}, 0
    for arm in ["shared"] + arms[1:]:
        for level, model, tier in itertools.product(LEVELS, MODELS, TIERS):
            subject, style = next(pairs)
            table[f"{arm}/{level}/{model}/{tier}"] = {"prompt": f"{subject[0].upper()}{subject[1:]}, {style}", "seed": base + n}
            n += 1
    prompts = [r["prompt"] for r in table.values()]
    assert len(set(prompts)) == len(prompts) and len({r["seed"] for r in table.values()}) == len(prompts)
    with open(out, "w") as f:
        json.dump({"first_cozy_arm": arms[0] if arms else None, "requests": table}, f, indent=1)


if __name__ == "__main__":
    main()
