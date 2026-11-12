# e2e/cli/released.sh: a project the newest release made reads the same on the candidate (cl-930's check that project.yaml's
# removal loses nothing). Calcifer's store is one way (version 8): once the candidate has run, a release refuses it. So the
# release makes its project before the candidate's first start (`released_made`, in run.sh's `start`): a starter project it
# edits, imports into, renames and deletes in, read as it reads it; the candidate, starting on the release's store, then
# reads every box, version, standing, history entry and canvas row as the release did (`released_project`).
released_made() {
step "the newest release makes a project"
local P=$OUT/made-by-release released pid
released=$(newest_release) || { check "released project: no release to make it"; return; }
(unset COZY_DOWNLOAD_URL; cozy --json update "$released") > "$OUT/released-update.json" 2>&1 || { check "released project: update $released: $(tail -c 300 "$OUT/released-update.json")"; install_candidate_binary; return; }
[ "$(cozy --version)" = "cozy $released" ] || { check "released project: cozy is $(cozy --version), not $released"; install_candidate_binary; return; }
if starter_project "$P" > "$OUT/released-new.json" 2>&1; then
  tiny "$OUT/released.png"
  project_api "$P" /assets/kyogre -X PATCH -H 'Content-Type: application/json' -d '{"fields": {"aspect_ratio": "1:1"}}' > /dev/null || check "released project: edit"
  project_import "$P" "$OUT/released.png" picture > /dev/null || check "released project: import"
  project_api "$P" /rename -X POST -H 'Content-Type: application/json' -d '{"from": "coast", "to": "shore"}' > /dev/null || check "released project: rename"
  project_api "$P" /assets/erupt -X DELETE > /dev/null || check "released project: delete"
  released_state "$P" > "$OUT/released-before.json" || check "released project: $released can't read it"
else
  check "released project: $released made none: $(tail -c 300 "$OUT/released-new.json")"
fi
# The release's Calcifer and any machine it started end before the candidate starts on its store.
pid=$(jq -r '.pid // empty' "$HOME/.cozy/calcifer/calcifer.lock.holder" 2> /dev/null) || pid=
systemctl --user stop cozy-calcifer > /dev/null 2>&1 || true
[ -z "$pid" ] || kill "$pid" 2> /dev/null || true
for _ in $(seq 30); do [ -z "$pid" ] || ! kill -0 "$pid" 2> /dev/null && break; sleep 1; done
machine_stop > /dev/null 2>&1 || true
install_candidate_binary
echo "released project: $released made $P"
}

released_project() {
step "a project the newest release made: the candidate reads it as the release did"
local P=$OUT/made-by-release before after
before=$(cat "$OUT/released-before.json" 2> /dev/null) && [ -n "$before" ] || { check "released project: the release left none to read"; return; }
after=$(released_state "$P") || { check "released project: the candidate can't read it: $(tail -c 300 "$OUT/status.err")"; return; }
[ "$after" = "$before" ] || check "released project: the candidate reads the release's project otherwise: $(diff <(jq . <<< "$before") <(jq . <<< "$after") | head -30)"
echo "released project: the release's $(jq '.assets | length' <<< "$before") boxes, $(jq '[.assets[].versions] | add' <<< "$before") versions, $(jq '.history | length' <<< "$before") history entries and $(jq '.canvas | length' <<< "$before") rows read the same on $(cozy --version)"
}

# What a person sees of a project: each box (its tool, fields, kind, versions, approval and standing), its history and its canvas rows. An
# Edit's `node_kind: video` is left out: the RC infers it where 0.24.0 showed none (cc3c1606d), and the project holds the same.
released_state() {
  local state history canvas
  state=$(status "$1") && history=$(project_api "$1" /history) && canvas=$(project_api "$1" /canvas) || return
  jq -cS -n --argjson s "$state" --argjson h "$history" --argjson c "$canvas" \
    '{id: $s.id, assets: [$s.assets[] | {name, tool, kind, fields: (.fields | del(.node_kind)), versions, approved, standing}], history: [$h.entries[] | {id, words, names, undone: (.undone != null)}], canvas: $c.cards}'
}
