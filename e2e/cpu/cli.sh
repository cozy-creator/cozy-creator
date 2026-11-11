#!/usr/bin/env bash
# e2e/cpu/cli.sh <cozy> <project> <package dir> <out dir>: one package through cozy's CLI and a Calcifer that is up, as
# an author and a person use them. Local code may install; no model function runs and nothing is rented.
#   upload    its code, with an empty file (0.22.1 refused one: file_length_required), sent by content and described
#             (`cozy run <package dir> --describe`): the functions and fields the executor's interface has
#   lock      Calcifer's private lock API leaves the committed uv.lock as it is
#   bind      every cell's request (drive.py's <out>/requests/<request>.json, its input files) bound by Calcifer's binder
#             against the staged local code (the private estimate API; 0.22.1 refused `aspect_ratio=1:1`), a prompt's
#             reference images named as the run's own `references`
# <project> is a project's folder: a run belongs to the project it is sent from. One line per cell, as run.sh's.
set -uo pipefail
COZY=$1 PROJECT=$2 PKG=$3 OUT=$4
p=$(basename "$PKG") cli=$OUT/cli t=$SECONDS
line() { echo "$1 $p $2 $3 $((SECONDS - t))s${4:+: $4}"; t=$SECONDS; }
said() { jq -r '.error.message // .error.code // empty' "$1" 2> /dev/null | head -c 300; }

rm -rf "$cli/src" && mkdir -p "$cli/src"
cp -r "$PKG" "$cli/src/$p" && rm -rf "$cli/src/$p/.venv"
module=$(jq -r '.application | split(":")[0] | split(".")[0]' "$OUT/interface.json")
# Beside its import package or module, flat or under src/.
code=$(find "$cli/src/$p" \( -path "*/$module/__init__.py" -o -name "$module.py" \) -not -path "*/.venv/*" | head -n1)
: > "$(dirname "${code:-$cli/src/$p/x}")/cpu_e2e_empty.py"
# The interface's public functions (neither internal nor the Runtime's own recipes) and each one's request fields.
jq -c '[.entrypoints[] | select((.internal // false | not) and (.invocable.module | startswith("cozy_runtime.") | not))
  | {(.name): [.request.fields[].name] | sort}] | add // {}' "$OUT/interface.json" > "$cli/public.json"
if (cd "$PROJECT" && "$COZY" --json run "$cli/src/$p" --describe > "$cli/describe.json" 2> "$cli/describe.err"); then
  want=$(jq -c 'keys' "$cli/public.json")
  # A package of one function describes that function.
  listed=$(jq -c --argjson want "$want" 'if has("fields") and ($want | length) == 1 then $want else [.functions[]?.function] | sort end' \
    "$cli/describe.json")
  why=''
  [ "$listed" = "$want" ] || why="Calcifer lists $listed, the interface $want"
  for f in $(jq -r '.[]' <<< "$listed"); do
    [ -z "$why" ] || break
    if jq -e 'has("fields")' "$cli/describe.json" > /dev/null; then cp "$cli/describe.json" "$cli/describe-$f.json"
    else
      (cd "$PROJECT" && "$COZY" --json run "$cli/src/$p/$f" --describe > "$cli/describe-$f.json" 2>> "$cli/describe.err") \
        || { why="$f: $(said "$cli/describe-$f.json")"; break; }
    fi
    got=$(jq -c '[.fields[].name] | sort' "$cli/describe-$f.json") need=$(jq -c --arg f "$f" '.[$f]' "$cli/public.json")
    [ "$got" = "$need" ] || why="$f's fields: Calcifer $got, the interface $need"
  done
  if [ -z "$why" ]; then line PASS - upload; else line FAIL - upload "$why"; fi
else
  line FAIL - upload "$(said "$cli/describe.json")"
fi

if python3 "$(dirname "$0")/../package-lock.py" --cozy "$COZY" "$cli/src/$p" > "$cli/lock.json" 2> "$cli/lock.err"; then
  if [ "$(jq -r .changed "$cli/lock.json")" = false ] && cmp -s "$PKG/uv.lock" "$cli/src/$p/uv.lock"; then line PASS - lock
  else line FAIL - lock "the committed uv.lock is stale: $(diff "$PKG/uv.lock" "$cli/src/$p/uv.lock" | grep -m1 '^[<>]')"; fi
elif grep -qE 'not_logged_in|401|403' "$cli/lock.json" "$cli/lock.err"; then
  line SKIP - lock "its index needs a sign-in: $(said "$cli/lock.json")"
else
  line FAIL - lock "$(said "$cli/lock.json")"
fi

# A cell of a public function, as a person sends it: its fields from the payload, its files attached at their paths.
shopt -s nullglob
for request in "$OUT"/requests/*.json; do
  entry=$(jq -r '.entry // empty' "$request")
  [ -n "$entry" ] && jq -e --arg e "$entry" 'has($e)' "$cli/public.json" > /dev/null || continue
  cell=$(basename "$request" .json) cell=${cell#"$entry"-}
  if jq -e .models "$request" > /dev/null; then line SKIP "$entry" "bind/$cell" "it binds a slot to a tiny model, no Hub ref"; continue; fi
  # A prompt's reference goes as a person names one in a run (`references: [{name, image: <file>}]`), its text
  # mentioning it; any other file at its field's path (`--file field.path=FILE`), a media field's at the field.
  jq '. as $req
    | [.files | to_entries[] | select(.key | test("\\.references\\.[0-9]+\\.file$")) | (.key | split(".")) as $parts
       | {at: $parts[:-3], file: .value,
          name: ($req.payload | getpath($parts[:-3] + ["references", ($parts[-2] | tonumber), "key"])),
          kind: (if (.value | test("\\.(mp4|webm|mov)$")) then "video" elif (.value | test("\\.(wav|flac|mp3)$")) then "audio" else "image" end)}]
      as $refs
    | .payload
    | reduce $refs[] as $r (.; setpath($r.at; getpath($r.at) | if type == "object" then .text else . end))
    | if ($refs | length) > 0 then .references = [$refs[] | {name: .name, (.kind): .file}] else . end' \
    "$request" > "$cli/$entry-$cell.payload"
  files=()
  while IFS=$'\t' read -r field file; do files+=(--file "${field%.file}=$file"); done \
    < <(jq -r '.files | to_entries[] | select(.key | test("\\.references\\.[0-9]+\\.file$") | not) | "\(.key)\t\(.value)"' "$request")
  if python3 "$(dirname "$0")/../run-estimate.py" "$cli/src/$p/$entry" --cozy "$COZY" --project "$PROJECT" \
      --input "$cli/$entry-$cell.payload" "${files[@]}" > "$cli/$entry-$cell.bound" 2> "$cli/$entry-$cell.bind.err" \
      && jq -e 'has("error") | not' "$cli/$entry-$cell.bound" > /dev/null; then
    line PASS "$entry" "bind/$cell"
  else
    line FAIL "$entry" "bind/$cell" "$(said "$cli/$entry-$cell.bound")"
  fi
done
