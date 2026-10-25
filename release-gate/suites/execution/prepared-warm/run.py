"""Normal CLI source/upload/warm/two-input reuse and held-back keeper qualification."""

from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path


def records(text):
    decoder, rows = json.JSONDecoder(), []
    while text.strip():
        value, end = decoder.raw_decode(text.lstrip())
        rows.append(value)
        text = text.lstrip()[end:]
    return rows


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--machine", required=True)
    parser.add_argument(
        "--model",
        required=True,
        help="Owned private model for the tiny BF16 checkpoint",
    )
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    package = Path(__file__).resolve().parent
    args.out.mkdir(parents=True, exist_ok=False)

    def call(name, words):
        with (
            (args.out / f"{name}.json").open("w") as out,
            (args.out / f"{name}.err").open("w") as err,
        ):
            result = subprocess.run(
                ["cozy", *map(str, words), "--json"], stdout=out, stderr=err
            )
        events = records((args.out / f"{name}.json").read_text())
        assert result.returncode == 0, (name, events[-1:] if events else None)
        run = events[-1].get("run", events[-1])
        assert run["status"] == "completed", (name, run)
        return run

    source = call(
        "source", ["run", package / "source", "seed=103", "--on", args.machine]
    )
    upload = call(
        "upload",
        [
            "model",
            "upload",
            f"run:{source['number']}#model",
            args.model,
            "--lane",
            "bf16",
            "--on",
            args.machine,
        ],
    )
    (published,) = [
        item["published"]
        for item in upload["result"]["models"]
        if item.get("published", {}).get("destination") == args.model
    ]
    selected = args.model + "@" + published["commit"]
    common = ["run", package / "gpu", f"model.model={selected}", "--on", args.machine]

    def infer(name, seed, steps, *, required=True, batch_rows=1):
        run = call(
            name,
            [
                *common,
                f"seed={seed}",
                f"steps={steps}",
                f"batch_rows={batch_rows}",
                f"require_prepared={str(required).lower()}",
                "--gpus=1",
            ],
        )
        assert run["result"]["passed"] is True
        row = json.loads(run["result"]["detail"])
        return {"number": run["number"], **row}

    warm = call("warm", [*common, "seed=11", "steps=8", "--warm", "gpu"])
    assert warm["result"]["set"][0]["prepared"] is True
    first, second = infer("first", 19, 8), infer("second", 29, 8)
    assert first["pid"] == second["pid"] and first["definition"] == second["definition"]
    assert first["captures"] == second["captures"] == 1
    assert first["factory_calls"] == second["factory_calls"] == 1
    assert (first["handler_calls"], second["handler_calls"]) == (1, 2)
    assert first["phase_calls"] == second["phase_calls"]
    assert first["launches_this_request"] == second["launches_this_request"] == 1
    assert first["value"] != second["value"]
    changed = call(
        "changed", [*common, "seed=31", "steps=17", "batch_rows=2", "--warm", "gpu"]
    )
    assert changed["result"]["set"][0]["prepared"] is True
    third = infer("third", 37, 17, batch_rows=2)
    assert third["captures"] == 2 and third["factory_calls"] == 2
    assert (
        third["pid"] == first["pid"]
        and first["batch_rows"] == 1
        and third["batch_rows"] == 2
    )
    assert first["input_shape"] == [1, 256] and third["input_shape"] == [2, 256]
    assert first["output_shape"] == [1, 16] and third["output_shape"] == [2, 16]

    # A real live Torch allocation lowers observed driver-free bytes below a measured threshold.
    held_back = call(
        "held-back",
        [
            *common,
            "steps=23",
            "batch_rows=2",
            "pressure_bytes=268435456",
            "--warm",
            "gpu",
        ],
    )
    row = held_back["result"]["set"][0]
    assert row["prepared"] is False and row["level"] == "host" and row["held_back"]
    # These real calls each trigger the existing keeper after returning. They never call
    # the factory themselves; the next call reveals whether a keeper retried it.
    declined = [
        infer(f"declined-{i}", 41 + i, 23, required=False, batch_rows=2)
        for i in range(3)
    ]
    assert all(
        not row["prepared"] and row["factory_calls"] == 3 and row["captures"] == 2
        for row in declined
    )
    capacity = declined[0]["capacity"]
    assert (
        capacity["ballast_bytes"] == 268435456
        and capacity["allocated_delta"] >= 268435456
    )
    assert (
        capacity["free_while_live"]
        < capacity["required_free_bytes"]
        < capacity["free_before"]
    )
    assert capacity["free_after_release"] >= capacity["required_free_bytes"]
    retry = call(
        "retry",
        [
            *common,
            "seed=43",
            "steps=23",
            "batch_rows=2",
            f"min_free_bytes={capacity['required_free_bytes']}",
            "--warm",
            "gpu",
        ],
    )
    assert retry["result"]["set"][0]["prepared"] is True
    fourth = infer("fourth", 47, 23, batch_rows=2)
    assert fourth["factory_calls"] == 4 and fourth["captures"] == 3
    proof = {
        "source": source["number"],
        "upload": upload["number"],
        "model": selected,
        "warm": warm["number"],
        "first": first,
        "second": second,
        "changed": changed["number"],
        "third": third,
        "held_back": held_back["number"],
        "capacity": capacity,
        "declined": declined,
        "retry": retry["number"],
        "fourth": fourth,
    }
    (args.out / "proof.json").write_text(json.dumps(proof, indent=2) + "\n")
    print(json.dumps(proof, indent=2))


if __name__ == "__main__":
    main()
