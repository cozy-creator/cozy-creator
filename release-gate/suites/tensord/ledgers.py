"""Each GPU's ledger on this machine, read from its page: live rows by GPU uuid. Runs on the machine."""
import glob
import json
import os
import struct

HEADER, ROW, ROWS = 3, 9, 32
tables = {}
for path in glob.glob("/dev/shm/cozy-varena-*/v6-*"):
    words = struct.unpack("<512Q", open(path, "rb").read(4096))
    rows = (words[HEADER + i * ROW : HEADER + (i + 1) * ROW] for i in range(ROWS))
    tables[os.path.basename(path)[3:]] = [
        {"pid": r[0], "held": r[1], "idle": r[2], "running": r[6]} for r in rows if r[0]
    ]
print(json.dumps(tables))
