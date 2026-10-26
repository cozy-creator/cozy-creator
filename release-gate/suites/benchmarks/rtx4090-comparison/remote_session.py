"""One bounded remote-only comparison window, reusing the retained default-CLI owner boundaries.

No action without --execute. This is an output-only proof driver, not a rental service.
"""

import argparse
import datetime
import fcntl
import hashlib
import importlib.util
import json
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

sys.dont_write_bytecode = True
CLI = Path.home() / ".cozy/bin/cozy"


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def save(folder, name, value):
    (folder / name).write_text(json.dumps(value, indent=2) + "\n")


def documents(text):
    decoder = json.JSONDecoder()
    while text.strip():
        row, end = decoder.raw_decode(text.lstrip())
        yield row
        text = text.lstrip()[end:]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--stack-lock-fd", type=int,
                        help="Borrow the parent's inherited shared stack lock through rental cleanup")
    args = parser.parse_args()
    config = json.loads(args.config.read_text())
    for path, sha in config["source_sha256"].items():
        assert digest(path) == sha, path
    cohort = json.loads(Path(config["cohort"]).read_text())
    assert digest(config["cohort"]) == config["cohort_sha256"]
    qualification = load("svdq_remote_qualification", config["qualifier"])
    qualification.source_check(cohort)
    borrowed = None
    if args.stack_lock_fd is not None:
        borrowed = qualification.validate_stack_lock(args.stack_lock_fd)
        qualification.STACK_LOCK_FD = args.stack_lock_fd
    supported = {"rtx-4090": ("NVIDIA GeForce RTX 4090", "sm89"),
                 "rtx-5090": ("NVIDIA GeForce RTX 5090", "sm120"),
                 "h100-pcie": ("NVIDIA H100 PCIe", "sm90"),
                 "h100-sxm5-80gb": ("NVIDIA H100 80GB HBM3", "sm90")}
    assert config["sku"] in supported and config["gpus"] == 1
    assert (config["gpu_name"], config["gpu_family"]) == supported[config["sku"]]
    assert (
        config["rental_wall_seconds"] == 3300
        and config["budget_usd_micros"] == 5_000_000
    )
    assert config["rental_wall_seconds"] + config["cleanup_reserve_seconds"] <= 3600
    if not args.execute:
        print("Frozen inputs and budget checked; no rental, SDK or GPU action")
        return
    args.out.mkdir(parents=True, exist_ok=False)
    save(args.out, "config.json", config)
    if borrowed is not None:
        save(args.out, "borrowed-stack-lock.json", borrowed)
    # Remote-only: use the ordinary installed CLI; never install/restore the local SDK.
    # Measured calls acquire the shared lock briefly and wait outside timing for the
    # frozen normal caller to return after the primary's own installation window.
    caller_before = digest(CLI)
    save(args.out, "caller-before.json", {"path": str(CLI), "sha256": caller_before,
                                       "local_sdk_mutated": False})
    owned = None
    rental = None
    observer = None
    rejected_order = threading.Event()
    stop_requested = threading.Event()
    owner_lock = threading.Lock()
    rent_started = deadline = None
    ended = False

    def call(label, argv, check=True):
        with (
            (args.out / (label + ".json")).open("w") as stdout,
            (args.out / (label + ".err")).open("w") as stderr,
        ):
            began = time.monotonic_ns()
            result = subprocess.run(
                [str(CLI), *map(str, argv), "--json"],
                stdin=subprocess.DEVNULL,
                stdout=stdout,
                stderr=stderr,
                check=False,
                close_fds=True,
                timeout=(90 if deadline is None else
                         min(35, max(1, deadline + config["cleanup_reserve_seconds"] - time.time()))
                         if label.startswith(("end", "ready", "owned", "machines", "quote", "catalog"))
                         else max(1, deadline - time.time())),
            )
        save(args.out, label + "-command.json", [str(CLI), *map(str, argv), "--json"])
        save(
            args.out,
            label + "-timing.json",
            {
                "cli_wall_ms": (time.monotonic_ns() - began) / 1e6,
                "returncode": result.returncode,
            },
        )
        rows = list(documents((args.out / (label + ".json")).read_text()))
        if check:
            assert result.returncode == 0, (label, rows)
        return rows

    def claim(machine):
        nonlocal owned
        # From this rent's own progress/result, never chosen from a list difference.
        assert machine["kind"] == "rental" and machine["rental_id"] not in prior_ids
        assert machine["rental"]["rented_at_ms"] >= int((rent_started - 10) * 1000)
        assert machine["rental_id"].startswith("pr-")
        with owner_lock:
            if owned is not None:
                assert owned["rental_id"] == machine["rental_id"]
                return
            owned = machine
            save(args.out, "owned.json", machine)
            save(args.out, "owned-watchdog-binding.json", {
                "rental_id": machine["rental_id"], "deadline_epoch": deadline,
                "watchdog_armed_before_order": True,
            })
            terms = machine["rental"]
            if (
                machine.get("hub") != config["hub"]
                or terms["sku"] != config["sku"]
                or terms["gpus"] != 1
                or terms["usd_micros_per_hour"] > config["max_hourly_usd_micros"]
                or terms["disk_gb"] != quote["container_disk_gb"]
            ):
                save(args.out, "order-terms-refused.json", terms)
                rejected_order.set()  # Ownership is recorded first, so finally can end it.

    def end_owned():
        nonlocal ended
        if owned is None:
            return
        alias = owned["rental_id"]
        # Existing bounded teardown retries; none creates another rental.
        for attempt in range(4):
            try:
                observed = call(f"end-before-{attempt}", ["machine", "show", alias])[-1]
                assert observed["rental_id"] == owned["rental_id"], (
                    "rental identity changed; do not touch it"
                )
                if observed["state"] != "ended":
                    call(
                        f"end-{attempt}",
                        ["machine", "end", alias, "--force", "--yes"],
                        check=False,
                    )
                observed = call(f"end-after-{attempt}", ["machine", "show", alias])[-1]
                listing = call(f"end-list-{attempt}", ["machine", "list"])[-1]
                assert observed["rental_id"] == owned["rental_id"]
                absent = not any(
                    m.get("rental_id") == owned["rental_id"] or m.get("rental_id") == alias
                    for m in listing["machines"]
                )
                if (
                    observed["state"] == "ended"
                    and not observed.get("unverified")
                    and absent
                    and not listing.get("unanswered")
                ):
                    ended = True
                    save(
                        args.out,
                        "ended-confirmed.json",
                        {
                            "machine": observed,
                            "absent_from_listing": True,
                            "scope": "Observed ended/absent, not settled provider billing",
                        },
                    )
                    return
            except Exception as error:
                save(
                    args.out,
                    f"end-error-{attempt}.json",
                    {"error": repr(error), "rental_id": owned["rental_id"]},
                )
            time.sleep(5)
        raise RuntimeError(
            "Owned rental teardown not verified; retain watchdog/evidence and alert root"
        )

    def stop_owner(signum, frame):
        if rental is not None and owned is None and rental.poll() is None:
            # Do not manufacture an unacknowledged paid order by stopping this follower
            # before it has supplied its identity. Keep following the same request.
            stop_requested.set()
            return
        raise SystemExit(
            f"owner interrupted by signal {signum}; normal cleanup follows"
        )

    signal.signal(signal.SIGTERM, stop_owner)
    signal.signal(signal.SIGINT, stop_owner)
    try:
        # Current cozy routes through its normal config/environment/default Hub.
        # whoami's account URL comes from that exact Origin; no retired selector
        # flag or alternate home is used to force a different service.
        account = call("caller-route", ["whoami"])[-1]
        assert account["logged_in"] and account["buy_credits"] == config["hub"].rstrip("/") + "/account"
        prior = call("machines-before-rent", ["machine", "list"])[-1]
        prior_ids = {m["rental_id"] for m in prior["machines"] if m.get("rental_id")}
        quoted = call(
            "quote",
            ["machine", "rent", config["sku"], "--gpus=1", f"--disk-gb={config['disk_gb']}"],
            check=False,
        )
        assert quoted[-1]["error"]["code"] == "quote_unagreed"
        quote = quoted[0]["quote"]
        assert (
            quote["sku"] == config["sku"]
            and quote["gpus"] == 1
            and quote["container_disk_gb"] >= config["disk_gb"]
        )
        assert 0 < quote["usd_micros_per_hour"] <= config["max_hourly_usd_micros"]
        catalog = call("catalog", ["machine", "rent"])[-1]
        product = next(p for p in catalog if p["name"] == config["sku"])
        width = next(w for w in product["widths"] if w["accelerator_count"] == 1)
        storage = width["storage_usd_micros_per_hour"] * quote["container_disk_gb"] / width["container_disk_gb"]
        traffic = width.get("bandwidth_usd_micros_per_gb", 0) * config["traffic_gb"]
        all_in_hour = config["max_hourly_usd_micros"] + storage + traffic
        assert all_in_hour <= config["budget_usd_micros"], ("GPU/storage/traffic exceed budget", all_in_hour)
        save(args.out, "all-in-quote.json", {"gpu_usd_micros_per_hour": quote["usd_micros_per_hour"],
             "storage_usd_micros_per_hour": storage, "traffic_gb": config["traffic_gb"],
             "traffic_allowance_usd_micros": traffic, "one_hour_all_in_usd_micros": all_in_hour})
        rent_started = time.time()
        deadline = rent_started + config["rental_wall_seconds"]
        save(
            args.out,
            "budget.json",
            {
                "rent_start_epoch": rent_started,
                "deadline_epoch": deadline,
                "quoted_usd_micros_per_hour": quote["usd_micros_per_hour"],
                "quoted_gpu_hour_usd_micros": quote["usd_micros_per_hour"],
                "budget_usd_micros": config["budget_usd_micros"],
                "scope": "Paid rental window; local SDK setup/restore timed separately",
            },
        )
        save(args.out, "watchdog-context.json", {
            "hub": config["hub"], "prior_ids": sorted(prior_ids), "sku": config["sku"],
            "rent_start_epoch": rent_started, "hard_deadline_epoch": rent_started + 3600,
        })
        with (args.out / "watchdog.log").open("w") as watch_log:
            watcher = subprocess.Popen([sys.executable, config["watchdog"], "--out", str(args.out),
                                       "--deadline", str(deadline)], stdout=watch_log,
                                      stderr=subprocess.STDOUT, start_new_session=True, close_fds=True)
        save(args.out, "watchdog-owner.json", {"pid": watcher.pid, "deadline_epoch": deadline,
                                             "armed_before_order": True})
        arm_deadline = time.monotonic() + 10
        while not (args.out / "watchdog.json").exists():
            if watcher.poll() is not None or time.monotonic() >= arm_deadline:
                save(args.out, "order-not-started.json", {"reason": "watchdog did not acknowledge readiness"})
                raise RuntimeError("Independent expiry was not ready; no rental was ordered")
            time.sleep(0.1)
        armed = json.loads((args.out / "watchdog.json").read_text())
        assert armed["pid"] == watcher.pid and armed["deadline_epoch"] == deadline
        with (
            (args.out / "rent.txt").open("w") as stdout,
            (args.out / "rent.err").open("w") as stderr,
        ):
            rental = subprocess.Popen(
                [
                    str(CLI),
                    "machine",
                    "rent",
                    config["sku"],
                    "--gpus=1",
                    f"--disk-gb={config['disk_gb']}",
                    "--yes",
                ],
                stdin=subprocess.DEVNULL,
                stdout=stdout,
                stderr=subprocess.PIPE,
                text=True,
                close_fds=True,
            )

            def follow_boot():
                for line in rental.stderr:
                    stderr.write(line)
                    stderr.flush()
                    match = re.match(r"^([a-z][a-z0-9-]*): ", line)
                    if match and owned is None:
                        try:
                            claim(call("owned-boot", ["machine", "show", match[1]])[-1])
                        except Exception as error:
                            save(
                                args.out,
                                "boot-observation-error.json",
                                {
                                    "error": repr(error),
                                    "policy": "continue same order; do not rent again",
                                },
                            )

            observer = threading.Thread(target=follow_boot, daemon=True)
            observer.start()
            # No ten-minute timeout/restart: keep following this exact order until actual
            # termination/readiness or the explicit overall budget. Ctrl-C only ends following.
            while (
                rental.poll() is None
                and time.time() < deadline
                and not rejected_order.is_set()
                and not (stop_requested.is_set() and owned is not None)
            ):
                time.sleep(1)
            if rental.poll() is None:
                rental.send_signal(signal.SIGINT)
            try:
                status = rental.wait(timeout=15)
            except subprocess.TimeoutExpired:
                rental.kill()  # only this CLI follower; independent watchdog retains order ownership
                status = rental.wait(timeout=5)
            observer.join(timeout=50)
            if owned is None:
                match = re.search(
                    r"^machine\s+(\S+)",
                    (args.out / "rent.txt").read_text(),
                    re.MULTILINE,
                )
                if match:
                    claim(call("owned-ready", ["machine", "show", match[1]])[-1])
            assert owned is not None, (
                "rent identity unresolved; do not create another order; root must inspect retained response"
            )
            assert (
                status == 0
                and time.time() < deadline
                and not rejected_order.is_set()
                and not stop_requested.is_set()
            ), (
                "real terminal rent failure, rejected quote terms, or overall budget exhausted"
            )
        alias = owned["rental_id"]
        assert owned["rental"]["sku"] == config["sku"] and owned["rental"]["gpus"] == 1
        assert (
            owned["rental"]["usd_micros_per_hour"] <= config["max_hourly_usd_micros"]
        ), "fresh agreed rate exceeded cap; end owned rental"
        observed = call("ready", ["machine", "show", alias])[-1]
        assert (
            observed["state"] == "ready" and observed["rental_id"] == owned["rental_id"]
        )
        install = call(
            "install-remote",
            [
                "machine",
                "update",
                alias,
                "--gpu-memory-limit",
                "off",
                "--tensord",
                cohort["artifacts"]["tensord"]["path"],
                "--runtime",
                cohort["artifacts"]["runtime"]["path"],
                "--tensorfs",
                cohort["artifacts"]["tensorfs"]["path"],
            ],
        )
        runs = [r["run"] for r in install if isinstance(r.get("run"), dict)]
        assert runs and runs[-1]["status"] == "completed"
        for kind in ("runtime", "tensorfs"):
            assert (
                runs[-1]["result"]["to"][kind + "_sha256"]
                == cohort["artifacts"][kind]["sha256"]
            )
        observed = qualification.idle(
            args.out, "remote-installed", alias, cohort["creator_source"]
        )
        gpus = observed["status"]["gpus"]
        assert (
            len(gpus) == 1
            and gpus[0]["name"] == config["gpu_name"]
            and gpus[0]["family"] == config["gpu_family"]
            and gpus[0]["memory_bytes"] >= config["minimum_reported_vram_bytes"]
        )
        assert time.time() < deadline - config["teardown_reserve_seconds"], (
            "budget too short; end without starting another request"
        )
        proof_dir = args.out / "qualification"
        proof_dir.mkdir()
        cohort["deadline_epoch"] = deadline
        assert cohort["caller_sha256"] == config["caller_sha256"]
        proof = qualification.qualify(cohort, proof_dir, alias)
        assert proof["valid_comparisons"]
        save(args.out, "qualification-summary.json", proof)
    except BaseException as error:
        save(
            args.out,
            "failure.json",
            {
                "error": repr(error),
                "owned_rental_id": None if owned is None else owned["rental_id"],
                "policy": "Stop after this failure; no inference retry or second rental. Cleanup receipts remain separate.",
            },
        )
        raise
    finally:
        try:
            if rental is not None and rental.poll() is None:
                rental.send_signal(
                    signal.SIGINT
                )  # Stop following; this is not rental cancellation.
                try:
                    rental.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    rental.kill()
                    rental.wait(timeout=5)
            if observer is not None and observer.is_alive():
                observer.join(timeout=50)
            if owned is None:
                save(args.out, "stop-now.json", {"reason": "controller stopped without a claimed identity"})
                save(args.out, "ownership-unresolved.json", {"cleanup_verified": False, "rent_start_epoch": rent_started, "policy": "No second order or unrelated rental adoption; independent watchdog retains own response recovery."})
            end_owned()
        finally:
            save(args.out, "controller-cleanup.json", {
                "local_sdk_mutated": False,
                "caller_sha256_before": caller_before,
                "caller_sha256_after": digest(CLI),
                "remote_ended_verified": ended,
                "owned_rental_id": None if owned is None else owned["rental_id"],
                "scope": "No local CLI/SDK action by this controller; other owners may use their own lock window.",
            })


if __name__ == "__main__":
    main()
