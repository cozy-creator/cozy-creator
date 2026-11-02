"""Fresh-process passive Working custody; no Runtime execution client or Torch."""

from __future__ import annotations

import hashlib
import json
import os
import sys
import traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from driver import Driver, Probe, Stream, until


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    payload = json.loads(Path(sys.argv[1]).read_text())
    result = {
        "passed": False,
        "case": "passive_custody",
        "pid": os.getpid(),
        "input": payload,
    }
    public = []
    allocations = []
    driver = work = control = helper = None
    gate = output = None
    from tensorfs import _ext, varena

    try:
        assert payload["gpu_uuid"] != "UNBOUND", (
            "bind the observed rental GPU before execution"
        )
        assert "torch" not in sys.modules
        assert digest(_ext.__file__) == payload["extension_sha256"]
        binaries = Path(__file__).parent / "bin"
        assert digest(binaries / "public-probe.so") == payload["helper_sha256"]
        helper = Probe(binaries / "public-probe.so")
        assert (
            helper.lib.probe_core_identity()
            == _ext._varena_core_identity()
            == varena.core_identity()
        )
        mapped = {
            line.split(maxsplit=5)[-1]
            for line in Path("/proc/self/maps").read_text().splitlines()
            if len(line.split(maxsplit=5)) == 6
        }
        rust_std = {
            Path(path).name
            for path in mapped
            if Path(path).name.startswith("libstd-") and path.endswith(".so")
        }
        assert rust_std == {payload["std_name"]}, rust_std
        for name, wanted in (
            ("libvarena.so", payload["core_sha256"]),
            (payload["std_name"], payload["std_sha256"]),
        ):
            paths = {path for path in mapped if Path(path).name == name}
            assert len(paths) == 1 and digest(next(iter(paths))) == wanted, (
                name,
                paths,
            )
        driver = Driver(retain_primary=True)
        expected_uuid = (
            payload["gpu_uuid"]
            .lower()
            .removeprefix("gpu-")
            .removeprefix("mig-")
            .replace("-", "")
        )
        assert driver.uuid() == expected_uuid
        work, control = Stream(driver, helper), Stream(driver, helper)
        first, second = varena.open(0), varena.open(0)
        public.extend((first, second))
        # The fixture helper calls the one native default_directory implementation.
        until(
            lambda: (
                helper.call("observer", 0, payload["gpu_uuid"].encode()),
                True,
            )[1],
            label="closed metadata observer admission",
        )
        observed = until(helper.facts, label="initial observer metadata")
        assert observed["closed"] and not observed["active"]
        assert observed["clients"] == 2 and observed["passive_holds"] == 1
        assert observed["measured_requests"] == 0
        assert observed["row"] is not None and observed["row"]["pid"] == os.getpid()
        row_id = observed["owner"]
        result["observer_after_closed_lease"] = observed
        initial = helper.working_stats()
        assert initial["allocated_bytes"] == initial["mapped_bytes"] == 0, initial
        count = 8 << 20
        gate = helper.allocate(4, control)
        output = helper.allocate(count, work)
        control.write_gate(gate, 0)
        control.finish()
        baseline = helper.working_stats()
        assert baseline["allocated_bytes"] == count + 4, baseline
        data = first.alloc(count, work.handle)
        allocations.append(data)
        old_address = data.ptr
        work.wait_gate(gate)
        work.fill(old_address, count, 61)
        work.copy(output, old_address, count)
        assert work.pending(), "real gate did not establish pending GPU work"
        data.close()  # native records its completion after the queued D2D, without waiting
        first.close()
        second.close()
        result["closed_public_handles"] = [first._handle, second._handle, data._handle]
        assert result["closed_public_handles"] == [0, 0, 0]

        def passive_pending():
            facts = helper.facts()
            result["last_pending_observation"] = facts
            assert (
                facts["failed"] is None
                and not facts["stopping"]
                and not facts["retired"]
                and facts["measured_requests"] == 0
            ), facts
            assert (
                facts["closed"] and not facts["active"] and facts["passive_holds"] == 1
            ), facts
            row = facts["row"]
            assert row is not None and row["id"] == row_id, facts
            return (
                facts
                if facts["clients"] == 0 and row["working"]["pending"] >= count
                else None
            )

        pending = until(
            passive_pending, label="passive-only service with gated Working"
        )
        assert pending["activity"] == pending["row"]["activity"] == "Unknown", pending
        pending_stats = helper.working_stats()
        assert pending_stats["allocated_bytes"] == baseline["allocated_bytes"]
        assert work.pending()
        result["pending"] = {
            "facts": pending,
            "working": pending_stats,
            "old_address_record_only": old_address,
            "baseline": baseline,
        }
        # Only the separately retained output is read after public data was freed.
        control.write_gate(gate, 1)
        control.finish()
        work.finish()
        read = work.read(output, count)
        assert read == bytes([61]) * count
        result["readback"] = {
            "bytes": count,
            "sha256": hashlib.sha256(read).hexdigest(),
            "source": "still-owned native output",
        }

        def passive_ready():
            facts = helper.facts()
            result["last_ready_observation"] = facts
            assert (
                facts["owner"] == row_id
                and facts["clients"] == 0
                and facts["passive_holds"] == 1
            ), facts
            assert (
                facts["failed"] is None
                and not facts["retired"]
                and not facts["stopping"]
            ), facts
            row = facts["row"]
            return (
                facts
                if row
                and row["working"]["pending"] == 0
                and row["working"]["idle"] >= count
                else None
            )

        ready = until(passive_ready, label="actual pending-to-ready publication")
        result["ready"] = {"facts": ready, "working": helper.working_stats()}
        third = varena.open(0)
        public.append(third)
        before_reuse = third.stats()
        reused = third.alloc(count, work.handle)
        allocations.append(reused)
        after_reuse = third.stats()
        result["reuse"] = {
            "before": before_reuse,
            "after": after_reuse,
            "new_owned_address": reused.ptr,
            "old_address_record_only": old_address,
        }
        assert after_reuse["working_mapped"] == before_reuse["working_mapped"]
        assert after_reuse["driver_calls"] == before_reuse["driver_calls"]
        work.fill(reused.ptr, count, 93)
        work.copy(output, reused.ptr, count)
        assert work.read(output, count) == bytes([93]) * count
        assert until(helper.facts)["owner"] == row_id
        reused.close()
        work.finish()
        third.close()
        until(
            lambda: f if (f := helper.facts())["clients"] == 0 else None,
            label="new public client retirement",
        )
        result["reuse"]["full_readback_bytes"] = count
        result["passed"] = True
    except BaseException as error:  # noqa: BLE001 - preserve original failure before cleanup
        result["error"] = repr(error)
        result["traceback"] = traceback.format_exc()
    finally:
        # Release the actual device gate on a failed predicate too, then establish
        # completion before freeing fixture buffers. Never mask the original failure.
        try:
            if control is not None and gate is not None:
                control.write_gate(gate, 1)
                control.finish()
            if work is not None:
                work.finish()
            for value in reversed(allocations):
                value.close()
            for value in reversed(public):
                value.close()
            if helper is not None and output is not None:
                helper.call("free", 0, output, work.handle)
                output = None
            if helper is not None and gate is not None:
                helper.call("free", 0, gate, control.handle)
                gate = None
            if work is not None:
                work.finish()
            if control is not None:
                control.finish()
            if helper is not None:
                helper.call("trim", 0)
            from tensorfs import varena

            def retired():
                value = varena.retirement_stats()
                assert value["quarantined"] == 0, value
                return value if value["queued"] == 0 else None

            result["retirement"] = until(
                retired, label="public native retirement tickets"
            )
            if work is not None:
                work.close()
            if control is not None:
                control.close()
            if helper is not None:
                final = helper.working_stats()
                result["final_working"] = final
                assert all(
                    final[name] == 0
                    for name in (
                        "allocated_bytes",
                        "mapped_bytes",
                        "pending_bytes",
                        "pools_bytes",
                    )
                ), final
                facts = until(
                    lambda: (
                        f
                        if (f := helper.facts())["row"]
                        and sum(f["row"]["working"].values()) == 0
                        else None
                    ),
                    label="zero-byte passive row",
                )
                result["final_service"] = facts
                assert (
                    facts["closed"]
                    and facts["clients"] == 0
                    and facts["passive_holds"] == 1
                    and not facts["retired"]
                    and facts["measured_requests"] == 0
                )
            if driver is not None:
                driver.close()
        except BaseException as cleanup:  # noqa: BLE001 - cleanup cannot mask the original failure
            result["cleanup_error"] = repr(cleanup)
            result["cleanup_traceback"] = traceback.format_exc()
            result["passed"] = False
    print(json.dumps(result, sort_keys=True), flush=True)
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
