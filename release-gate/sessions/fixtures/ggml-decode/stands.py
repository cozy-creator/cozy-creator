"""stands.py <uv.lock> <label> <runtime wheel> <tensorfs wheel>: whether a machine updated with these wheels runs a package
of that lock on them (crates/tensord/src/published/environment.rs `stands_in`, `speaks`, `patch_of`, `rc_build`):
- a lock whose pair is of another series than the machine's can't speak its executor protocol, and the machine's pair goes
  over it (the open RC's TensorFS is 0.27's dev line since xs-216, a released lock's is 0.26);
- otherwise each member must be the lock's version or a later patch of it, or (the Runtime) both dev builds of one series,
  else the run goes on the lock's pair and only warns.
A lock naming a member once per platform (the RC's TensorFS build for Linux x86_64 after the index's for the rest) is read as
TensorD reads it (`published.rs` `read_lock`): its last row.
Exit 1 when the candidates don't stand in."""

import re
import sys

from packaging.version import Version

lock, label, runtime, tensorfs = sys.argv[1:]
text = open(lock).read()


def locked(name: str) -> Version | None:
    rows = re.findall(r'^name = "%s"\nversion = "([^"]+)"' % re.escape(name), text, re.M)
    return Version(rows[-1]) if rows else None


def wheel(path: str) -> Version:
    return Version(path.rsplit("/", 1)[-1].split("-")[1])


ours = {"cozy-runtime": wheel(runtime), "tensorfs": wheel(tensorfs)}
theirs_all = {name: locked(name) for name in ours}
foreign = any(v is not None and v.release[:2] != ours[n].release[:2] for n, v in theirs_all.items())
bad = []
for name, mine in ({} if foreign else ours).items():
    theirs = locked(name)
    if theirs is None:
        continue
    series = mine.release[:2] == theirs.release[:2]
    dev = all(v.is_devrelease and v.pre is None and v.post is None for v in (mine, theirs))
    if not (series and (mine >= theirs or (name == "cozy-runtime" and dev))):
        bad.append(f"{name} {mine} does not stand in for its lock's {theirs}")
pair = ", ".join(f"{n} {v}" for n, v in ours.items())
how = "over a lock of another series" if foreign else "as the lock's or a later patch"
lock_pair = ", ".join(f"{n} {v}" for n, v in theirs_all.items() if v is not None)
print(f"{label}: " + ("; ".join(bad) if bad else f"the candidates stand in, {how} ({pair}; the lock's: {lock_pair})"))
sys.exit(1 if bad else 0)
