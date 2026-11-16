# e2e/link/packages.sh (isolated.sh's `linkpkg`, tfs-300's cold start, cli/link.sh): packages whose install downloads for real
# through the cell's link, published at the isolated Hub's root. A file server on loopback (kept for the suite: E2E_LINK_FILES,
# E2E_LINK_WWW) serves each package's own URL dependencies, so no cache of uv's serves another's:
#   e2e-link-dl-<n>    a 30 MB wheel
#   e2e-link-slow-<n>  an sdist whose build waits 30 s, fetches 60 MB (blob-<n>.bin), waits 60 s: a compile's quiet stretches
# e2e-link-<n>/bare needs no weights; e2e-link-<n>/cold needs pagebox (`spare`). e2e-link-warm's build doesn't wait. The files
# stay out of the published $OUT.
link_packages() {
  local seed=$1 port n
  E2E_LINK_WWW=${RUNNER_TEMP:-$seed}/link-www
  mkdir -p "$E2E_LINK_WWW"
  port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
  (cd / && exec python3 -m http.server "$port" --bind 127.0.0.1 --directory "$E2E_LINK_WWW") > "$seed/link-files.log" 2>&1 &
  E2E_LINK_FILES=http://127.0.0.1:$port
  export E2E_LINK_FILES E2E_LINK_WWW COZY_LOCAL_ORIGINS=$COZY_LOCAL_ORIGINS,$E2E_LINK_FILES
  for n in warm q60a q60c p60a p60c opena openc; do
    python3 -I - "$seed/link-pkgs" "$E2E_LINK_WWW" "$E2E_LINK_FILES" "$n" <<'PY' || return 1
import base64, hashlib, io, os, sys, tarfile, zipfile
pkgs, www, files, n = sys.argv[1:5]
wait = (0, 0) if n == "warm" else (30, 60)
digest = lambda data: "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
with open(f"{www}/blob-{n}.bin", "wb") as blob:
    for _ in range(60):
        blob.write(os.urandom(1 << 20))
# The URL wheel.
name = f"e2e_link_dl_{n}"
members = {f"{name}/__init__.py": b"", f"{name}/data.bin": os.urandom(30 << 20),
           f"{name}-0.0.0.dist-info/METADATA": f"Metadata-Version: 2.1\nName: e2e-link-dl-{n}\nVersion: 0.0.0\n".encode(),
           f"{name}-0.0.0.dist-info/WHEEL": b"Wheel-Version: 1.0\nGenerator: e2e\nRoot-Is-Purelib: true\nTag: py3-none-any\n"}
record = "".join(f"{p},{digest(d)},{len(d)}\n" for p, d in members.items()) + f"{name}-0.0.0.dist-info/RECORD,,\n"
with zipfile.ZipFile(f"{www}/{name}-0.0.0-py3-none-any.whl", "w", zipfile.ZIP_STORED) as wheel:
    for p, d in [*members.items(), (f"{name}-0.0.0.dist-info/RECORD", record.encode())]:
        wheel.writestr(p, d)
# The slow sdist: static metadata (PKG-INFO), so resolving reads it; only its build waits and fetches.
slow, root = f"e2e_link_slow_{n}", f"e2e_link_slow_{n}-0.0.0"
setup = f'''import sys, time, urllib.request
from setuptools import setup
if "bdist_wheel" in sys.argv:
    time.sleep({wait[0]})
    urllib.request.urlretrieve("{files}/blob-{n}.bin", "{slow}/blob.bin")
    time.sleep({wait[1]})
setup(name="e2e-link-slow-{n}", version="0.0.0", packages=["{slow}"], package_data={{"{slow}": ["*.bin"]}})
'''
sdist = {f"{root}/PKG-INFO": f"Metadata-Version: 2.2\nName: e2e-link-slow-{n}\nVersion: 0.0.0\n".encode(),
         f"{root}/pyproject.toml": b'[build-system]\nrequires = ["setuptools>=70"]\nbuild-backend = "setuptools.build_meta"\n',
         f"{root}/setup.py": setup.encode(), f"{root}/{slow}/__init__.py": b""}
with tarfile.open(f"{www}/{root}.tar.gz", "w:gz") as tar:
    for p, d in sdist.items():
        info = tarfile.TarInfo(p)
        info.size, info.mtime = len(d), 1700000000
        tar.addfile(info, io.BytesIO(d))
# The package.
mod, pkg = f"e2e_link_{n}", f"{pkgs}/e2e-link-{n}"
os.makedirs(f"{pkg}/{mod}", exist_ok=True)
open(f"{pkg}/pyproject.toml", "w").write(f'''[build-system]
requires = ["uv_build>=0.12.7,<0.13"]
build-backend = "uv_build"

[project]
name = "e2e-link-{n}"
version = "0.1.0"
description = "tfs-300's cold start: an install that downloads through the link, and a function that needs pagebox"
requires-python = ">=3.12"
dependencies = ["cozy-runtime>=0.45", "msgspec>=0.19", "e2e-link-slow-{n}", "e2e-link-dl-{n}"]

[project.entry-points."cozy.application"]
default = "{mod}:app"

[tool.uv.build-backend]
module-name = "{mod}"
module-root = ""

[tool.uv.sources]
e2e-link-slow-{n} = {{ url = "{files}/{root}.tar.gz" }}
e2e-link-dl-{n} = {{ url = "{files}/{name}-0.0.0-py3-none-any.whl" }}
''')
open(f"{pkg}/package.toml", "w").write(f'[application]\nobject = "{mod}:app"\n')
open(f"{pkg}/{mod}/__init__.py", "w").write('''"""tfs-300's cold start (e2e/link/packages.sh)."""

import msgspec

from cozy_runtime.author import App, Loader, Model

app = App()


class Weights(Model[object]):
    graph: object

    def load(self, loader: Loader) -> None:
        self.graph = loader.construct(lambda config: object())


class Ask(msgspec.Struct, kw_only=True):
    text: str = "cold"


class Done(msgspec.Struct):
    text: str


@app.entrypoint(accelerator=False)
def bare(payload: Ask) -> Done:
    """Its install alone."""
    return Done(payload.text)


@app.entrypoint(accelerator=False, defaults={"weights": "lilylilith/pagebox@0.1.0"})
def cold(payload: Ask, weights: Weights) -> Done:
    """Its install and its weights."""
    _ = weights.graph
    return Done(payload.text)
''')
PY
    publish "$seed/link-pkgs/e2e-link-$n" || return 1
  done
}
