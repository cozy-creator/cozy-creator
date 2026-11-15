# xs-198: a CPU caller -> child -> leaf -> private self call through separately installed environments.
# The ordinary suite start owns candidate installation and the project; this cell only prepares local code and uses cozy.
typed_calls() {
step "typed package calls: media, assets, models, failures and dependency closure"
[ -n "$LOCAL" ] || { check "typed calls need the Linux local machine"; return; }
[ -f "$CAND_RT" ] && [[ $CAND_RT == *.whl ]] \
  || { check "typed calls need the exact candidate Runtime wheel (--runtime FILE)"; return; }
local root=$OUT/typed-calls fixture=$OUT/typed-calls/fixture
mkdir "$root" || { check "typed calls need a fresh receipt directory: $root"; return; }
python3 - "$here/typed-calls" "$fixture" "$CAND_RT" <<'PY' > "$root/prepare.json" || { check "typed calls: fixture preparation failed"; return; }
import hashlib, json, shutil, sys
from pathlib import Path

source, fixture, runtime = (Path(value).resolve() for value in sys.argv[1:])
runtime_dir = fixture / "runtime"
runtime_dir.mkdir(parents=True)
wheel = runtime_dir / runtime.name
shutil.copy2(runtime, wheel)
for name in ("caller", "child", "leaf"):
    target = fixture / name
    shutil.copytree(source / name, target, ignore=shutil.ignore_patterns("__pycache__", "*.pyc", ".venv", "uv.lock", "dist"))
    project = target / "pyproject.toml"
    text = project.read_text()
    section = "[tool.uv.sources]"
    if section not in text:
        text += "\n" + section + "\n"
    text = text.replace(section, section + "\ncozy-runtime = { path = " + json.dumps("../runtime/" + wheel.name) + " }", 1)
    project.write_text(text)
with wheel.open("rb") as stream:
    digest = hashlib.file_digest(stream, "sha256").hexdigest()
print(json.dumps({"runtime": str(wheel), "sha256": digest, "fixture": str(fixture)}, indent=2))
PY
uv lock --project "$fixture/caller" > "$root/lock.log" 2>&1 \
  || { check "typed calls: root lock failed: $(tail -c 500 "$root/lock.log")"; return; }
# Use the public generator and this same wheel. It reads the one root lock and does not import either callee.
local distribution module
for distribution in leaf child; do
  if [ "$distribution" = leaf ]; then module=$fixture/child/typed_child/leaf_calls.py
  else module=$fixture/caller/typed_caller/child_calls.py; fi
  uv run --no-project --with "$CAND_RT" cozy-runtime --dir "$fixture/caller" calls "xs198-typed-$distribution" "$module" \
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
