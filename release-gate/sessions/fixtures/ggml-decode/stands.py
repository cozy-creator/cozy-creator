"""stands.py <uv.lock> <label> <runtime wheel> <tensorfs wheel>: whether a machine updated with these wheels runs a package
of that lock on them. TensorD puts the machine's pair over a lock's only where each member is the lock's version or a later
patch of it, or (the Runtime) both are dev builds of one series (crates/tensord/src/published/environment.rs `stands_in`,
`patch_of`, `rc_build`); otherwise the run goes on the lock's pair and only warns. Exit 1 when either doesn't stand in."""

import re
import sys

from packaging.version import Version

lock, label, runtime, tensorfs = sys.argv[1:]
text = open(lock).read()


def locked(name: str) -> Version | None:
    m = re.search(r'^name = "%s"\nversion = "([^"]+)"' % re.escape(name), text, re.M)
    return Version(m.group(1)) if m else None


def wheel(path: str) -> Version:
    return Version(path.rsplit("/", 1)[-1].split("-")[1])


ours = {"cozy-runtime": wheel(runtime), "tensorfs": wheel(tensorfs)}
bad = []
for name, mine in ours.items():
    theirs = locked(name)
    if theirs is None:
        continue
    series = mine.release[:2] == theirs.release[:2]
    dev = all(v.is_devrelease and v.pre is None and v.post is None for v in (mine, theirs))
    if not (series and (mine >= theirs or (name == "cozy-runtime" and dev))):
        bad.append(f"{name} {mine} does not stand in for its lock's {theirs}")
pair = ", ".join(f"{n} {v}" for n, v in ours.items())
print(f"{label}: " + ("; ".join(bad) if bad else f"the candidates stand in ({pair})"))
sys.exit(1 if bad else 0)
