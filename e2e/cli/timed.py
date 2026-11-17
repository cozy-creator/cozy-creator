"""stdin's writes, each timed from the start: when the first came, when it ended, how many, and what they said (llm.sh `llm_cli_words`, xs-209)."""

import json
import os
import time

began = time.monotonic()
first: float | None = None
chunks = 0
data = b""
while chunk := os.read(0, 65536):
    first = time.monotonic() - began if first is None else first
    chunks += 1
    data += chunk
print(json.dumps({"first_s": round(first or 0.0, 2), "end_s": round(time.monotonic() - began, 2), "chunks": chunks, "text": data.decode(errors="replace")}))
