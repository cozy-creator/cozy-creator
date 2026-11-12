# e2e/cli/released.sh: a project the newest release made reads the same on the candidate (cl-930's check that project.yaml's
# removal loses nothing). After `update` (commands.sh `update_to_newest`) put the newest release in place and kept the candidate
# as $OUT/cozy-under-test: the release makes a starter project, edits, imports, renames, deletes and renders a box there, then the
# candidate goes back in place and every box, version, standing, history entry and canvas row must read as the release read them.
released_project() {
step "a project the newest release made: the candidate reads it as the release did"
local P=$OUT/made-by-release back=$OUT/cozy-under-test released before after
[ -x "$back" ] || { check "released project: update kept no candidate"; return; }
released=$(cozy --version)
[ "$released" != "$("$back" --version)" ] || { check "released project: cozy is still the candidate"; return; }
starter_project "$P" > "$OUT/released-new.json" 2>&1 || { check "released project: $released made none: $(tail -c 300 "$OUT/released-new.json")"; return; }
tiny "$OUT/released.png"
project_api "$P" /assets/kyogre -X PATCH -H 'Content-Type: application/json' -d '{"fields": {"aspect_ratio": "1:1"}}' > /dev/null || check "released project: edit"
project_import "$P" "$OUT/released.png" picture > /dev/null || check "released project: import"
project_api "$P" /rename -X POST -H 'Content-Type: application/json' -d '{"from": "coast", "to": "shore"}' > /dev/null || check "released project: rename"
project_api "$P" /assets/erupt -X DELETE > /dev/null || check "released project: delete"
if [ -n "$LOCAL" ]; then
  project_api "$P" /assets -X POST -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg tool "$here/project/draw.py/draw" '{name: "drawn", exact: true, tool: $tool, kind: "image", fields: {instructions: "A red square."}}')" > /dev/null \
    || check "released project: a box to render"
  project_api "$P" /runs -X POST -H 'Content-Type: application/json' -H "Idempotency-Key: released-$RANDOM$RANDOM" -d '{"names": ["drawn"], "on": ["local"]}' \
    > "$OUT/released-render.json" 2>&1 || check "released project: render: $(tail -c 300 "$OUT/released-render.json")"
  for _ in $(seq 600); do J '(.busy | not) and any(.assets[]; .name == "drawn" and .versions > 0)' <<< "$(status "$P")" && break; sleep 0.5; done
  J 'any(.assets[]; .name == "drawn" and .versions > 0 and .standing == "current")' <<< "$(status "$P")" \
    || check "released project: drawn has no current version: $(status "$P" | jq -c '.assets[] | select(.name == "drawn") | {versions, standing, failure}')"
fi
before=$(released_state "$P") || { check "released project: $released can't read it"; return; }
CAND_COZY=$back install_candidate_binary
cozy --json doctor > "$OUT/released-doctor.json" || check "released project: cozy doctor on the candidate"
[ "$(cozy --version)" = "$("$back" --version)" ] || { check "released project: cozy is $(cozy --version), not the candidate"; return; }
after=$(released_state "$P") || { check "released project: the candidate can't read it: $(tail -c 300 "$OUT/status.err")"; return; }
[ "$after" = "$before" ] || check "released project: the candidate reads $released's project otherwise: $(diff <(jq . <<< "$before") <(jq . <<< "$after") | head -30)"
echo "released project: $released's $(jq '.assets | length' <<< "$before") boxes, $(jq '[.assets[].versions] | add' <<< "$before") versions, $(jq '.history | length' <<< "$before") history entries and $(jq '.canvas | length' <<< "$before") rows read the same on $(cozy --version)"
}

# What a person sees of a project: each box (its tool, fields, kind, versions, approval and standing), its history and its canvas rows. An
# Edit's `node_kind: video` is left out: the RC infers it where 0.24.0 showed none (cc3c1606d), and the project holds the same.
released_state() {
  local state history canvas
  state=$(status "$1") && history=$(project_api "$1" /history) && canvas=$(project_api "$1" /canvas) || return
  jq -cS -n --argjson s "$state" --argjson h "$history" --argjson c "$canvas" \
    '{id: $s.id, assets: [$s.assets[] | {name, tool, kind, fields: (.fields | del(.node_kind)), versions, approved, standing}], history: [$h.entries[] | {id, words, names, undone: (.undone != null)}], canvas: $c.cards}'
}
