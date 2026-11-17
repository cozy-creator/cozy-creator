"""tfs-299 proof session 3: session 2's cells (a new prompt each request) without the 0.25.0 arms, the fix arms on
TensorFS <fix2 sha> (idle pinning asks idle peers). python3 gen3.py <fix2 creator run> <fix2 sha> > params3.json"""
import json, sys
run, sha = sys.argv[1:3]
d = json.load(open("params2.json"))
cells = [c for c in d["cells"] if c["name"][0] in "hf" or c["name"].startswith("wh-")]
for c in cells:
    if c["name"][0] == "f" and "update" in c:
        c["update"]["tensorfs"] = f"artifact:{run}/tensorfs-wheel-{sha}"
print(json.dumps({**d, "cells": cells, "need_s": 2700}))
