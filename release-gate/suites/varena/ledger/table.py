#!/usr/bin/env python3
"""A GPU's ledger on this machine, read as its peers read it. Runs on the pod, over `cozy machine ssh`.
  table.py                 every row, once
  table.py kill-on-ask     wait for a row asking, kill -9 another row with idle memory, say when
"""
import glob
import os
import signal
import struct
import sys
import time

HEADER, ROW, ROWS = 3, 9, 32


def tables():
    return glob.glob(f"/dev/shm/cozy-varena-{os.geteuid()}/v6-*")


def rows(path):
    with open(path, "rb") as file:
        words = struct.unpack(f"<{HEADER + ROW * ROWS}Q", file.read(8 * (HEADER + ROW * ROWS)))
    names = "pid held idle want answered last_use running short donated".split()
    found = [dict(zip(names, words[HEADER + ROW * i:HEADER + ROW * (i + 1)])) for i in range(ROWS)]
    return dict(zip("generation answers released".split(), words[:HEADER])), [r for r in found if r["pid"]]


def show():
    for path in tables():
        header, live = rows(path)
        print(os.path.basename(path), f"asks={header['generation']} released={header['released'] / 2**30:.2f}GiB")
        for r in live:
            print(f"  pid {r['pid']}: held {r['held'] / 2**30:.2f} idle {r['idle'] / 2**30:.2f} want {r['want'] / 2**30:.2f} "
                  f"running {r['running']} donated {r['donated'] / 2**30:.2f} GiB")


def kill_on_ask():
    while True:
        for path in tables():
            _, live = rows(path)
            asker = next((r for r in live if r["want"]), None)
            donor = next((r for r in live if asker and r["pid"] != asker["pid"] and r["idle"]), None)
            if donor:
                print(f"asker {asker['pid']} wants {asker['want'] / 2**30:.2f} GiB; killing donor {donor['pid']} holding {donor['held'] / 2**30:.2f} GiB")
                os.kill(donor["pid"], signal.SIGKILL)
                return
        time.sleep(0.001)


if __name__ == "__main__":
    {"kill-on-ask": kill_on_ask}.get(sys.argv[1] if len(sys.argv) > 1 else "", show)()
