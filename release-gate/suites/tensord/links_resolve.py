# /// script
# requires-python = ">=3.12"
# dependencies = ["cozy-runtime>=0.37", "msgspec>=0.19", "tomli-w>=1.0"]
# ///
"""Every symlink in every environment on a machine resolves. tomli-w is a wheel the image does not bake, so uv unpacks it for
this local code's own environment, and reclaim has to see that environment to keep it (runs 26176 and 26187)."""
import json
import os

import msgspec
import tomli_w

from cozy_runtime.author import App

app = App()
ROOT = "/var/lib/cozy/tensord"


class Ask(msgspec.Struct):
    note: str = "links"


class Seen(msgspec.Struct):
    report: str


@app.entrypoint()
def links_resolve(payload: Ask) -> Seen:
    broken: dict[str, int] = {}
    for root in ("generations", "editable"):
        for base, _, files in os.walk(os.path.join(ROOT, root)):
            for name in files:
                path = os.path.join(base, name)
                if os.path.islink(path) and not os.path.exists(path):
                    env = path[len(ROOT) + 1 :].split("/lib/")[0]
                    broken[env] = broken.get(env, 0) + 1
    return Seen(report=json.dumps({"dangling": sum(broken.values()), "environments": broken, "own": tomli_w.dumps({"a": 1}).strip()}))
