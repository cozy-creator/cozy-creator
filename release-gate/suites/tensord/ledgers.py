"""Each GPU's ledger on this machine, read from its pages: the rows of live processes by GPU uuid. Runs on the machine.
A page's layout is in its name: v6 (TensorFS 0.25) rows are 9 words, v7 (0.26, a `bell` added) 10. A dead owner's row
stays until a peer of its layout buries it, so only pids still running count. Host RAM's page (`host`) is no GPU's."""
import glob
import json
import os
import re
import struct

HEADER, ROWS, WIDTH = 3, 32, {"v6": 9, "v7": 10}
tables: dict[str, list[dict[str, int]]] = {}
for path in sorted(glob.glob("/dev/shm/cozy-varena-*/v[0-9]-*")):
    layout, name = re.match(r"(v\d+)-(.*)", os.path.basename(path)).groups()  # type: ignore[union-attr]
    if layout not in WIDTH or name == "host":
        continue
    row = WIDTH[layout]
    with open(path, "rb") as page:
        words = struct.unpack("<512Q", page.read(4096))
    for i in range(ROWS):
        r = words[HEADER + i * row : HEADER + (i + 1) * row]
        if r[0] and os.path.exists(f"/proc/{r[0]}"):
            tables.setdefault(name, []).append({"pid": r[0], "held": r[1], "idle": r[2], "running": r[6]})
print(json.dumps(tables))
