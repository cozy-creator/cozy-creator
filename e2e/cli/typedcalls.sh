# xs-198: a CPU caller -> child -> leaf -> private self call through separately installed environments.
# The ordinary suite start owns candidate installation and the project; this cell only prepares local code and uses cozy.
typed_calls() {
step "typed package calls: media, assets, models, failures and dependency closure"
[ -n "$LOCAL" ] || { check "typed calls need the Linux local machine"; return; }
[ -f "$CAND_RT" ] && [[ $CAND_RT == *.whl ]] \
  || { check "typed calls need the exact candidate Runtime wheel (--runtime FILE)"; return; }
[ -f "$CAND_TFS" ] && [[ $CAND_TFS == *.whl ]] \
  || { check "typed calls need the matched TensorFS wheel (--tensorfs FILE)"; return; }
local root=$OUT/typed-calls fixture=$OUT/typed-calls/fixture
mkdir "$root" || { check "typed calls need a fresh receipt directory: $root"; return; }
python3 - "$here/typed-calls" "$fixture" "$CAND_RT" "$CAND_TFS" <<'PY' > "$root/prepare.json" || { check "typed calls: fixture preparation failed"; return; }
import hashlib, json, platform, shutil, sys
from pathlib import Path

source, fixture, runtime, tensorfs = (Path(value).resolve() for value in sys.argv[1:])
runtime_dir = fixture / "runtime"
runtime_dir.mkdir(parents=True)
wheels = {"cozy-runtime": runtime, "tensorfs": tensorfs}
sdk = {}
for distribution, wheel in wheels.items():
    copied = runtime_dir / wheel.name
    shutil.copy2(wheel, copied)
    with copied.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    sdk[distribution] = {"path": "../runtime/" + wheel.name, "sha256": digest}
for name in ("caller", "child", "leaf"):
    target = fixture / name
    shutil.copytree(source / name, target, ignore=shutil.ignore_patterns("__pycache__", "*.pyc", ".venv", "uv.lock", "dist"))
    project = target / "pyproject.toml"
    text = project.read_text()
    # A direct dependency makes the selected transitive TensorFS source participate in uv resolution.
    # Runtime's own TensorFS range still applies; none of the fixture's authored ranges change.
    text = text.replace("dependencies = [", 'dependencies = ["tensorfs", ', 1)
    section = "[tool.uv.sources]"
    if section not in text:
        text += "\n" + section + "\n"
    entries = "".join("\n" + distribution + " = { path = " + json.dumps(value["path"]) + " }" for distribution, value in sdk.items())
    text = text.replace(section, section + entries, 1)
    # This CPU cell uses the runner's native wheel, not an unavailable wheel for every other OS.
    marker = f"sys_platform == '{sys.platform}' and platform_machine == '{platform.machine()}'"
    text += "\n[tool.uv]\nenvironments = " + json.dumps([marker]) + "\n"
    project.write_text(text)
print(json.dumps({"sdk": sdk, "fixture": str(fixture)}, indent=2))
PY
uv lock --project "$fixture/caller" > "$root/lock.log" 2>&1 \
  || { check "typed calls: root lock failed: $(tail -c 500 "$root/lock.log")"; return; }
python3 - "$root/prepare.json" <<'PY' || { check "typed calls: lock did not retain the selected SDK bytes"; return; }
import json, sys, tomllib
from pathlib import Path
receipt = json.loads(Path(sys.argv[1]).read_text())
packages = tomllib.loads((Path(receipt["fixture"]) / "caller/uv.lock").read_text())["package"]
for name, expected in receipt["sdk"].items():
    selected = [package for package in packages if package["name"] == name]
    assert len(selected) == 1, (name, selected)
    assert selected[0]["source"] == {"path": expected["path"]}, selected
    assert [wheel["hash"] for wheel in selected[0]["wheels"]] == ["sha256:" + expected["sha256"]], selected
PY
# Use the public generator and this same pair. It reads the root lock without importing either callee.
local distribution module
for distribution in leaf child; do
  if [ "$distribution" = leaf ]; then module=$fixture/child/typed_child/leaf_calls.py
  else module=$fixture/caller/typed_caller/child_calls.py; fi
  uv run --no-project --with "$CAND_RT" --with "$CAND_TFS" cozy-runtime --dir "$fixture/caller" calls "xs198-typed-$distribution" "$module" \
    > "$root/generate-$distribution.log" 2>&1 \
    || { check "typed calls: $distribution references failed: $(tail -c 500 "$root/generate-$distribution.log")"; return; }
done
uv run --no-project "$here/typed-calls/run.py" --cozy "$COZY" --project "$RUN_PROJECT" \
  --fixture "$fixture" --out "$root/cases" --argv-log "$OUT/argv.log" \
  --model-prefix "e2e-typed-${GITHUB_RUN_ID:-local}-$RANDOM" > "$root/run.log" 2>&1 \
  || { check "typed calls failed: $(tail -c 1000 "$root/run.log")"; return; }
cat "$root/run.log"
[ -s "$root/cases/qualified.json" ] || check "typed calls wrote no qualification receipt"
}
