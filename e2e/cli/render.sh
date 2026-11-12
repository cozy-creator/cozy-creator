# `cozy render` (cl-956), as an agent runs it, off a terminal on the weightless fixtures (render.py): a chain rendered from what it
# reads, nothing to render, --new and -n, a render joined and one whose CLI was killed, a failure stopping only its readers,
# --outdated and --pickup.
render_here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

agent_render() {
[ -n "$LOCAL" ] || return 0
step "cozy render: a chain, nothing to render, --new and -n, a joined render, a killed CLI, a failure, --outdated, --pickup"
COZY=$COZY python3 "$render_here/render.py" "$OUT/agent-render" "$OUT" > "$OUT/agent-render.json" 2> "$OUT/agent-render.err" \
  || check "cozy render: $(jq -c .failures "$OUT/agent-render.json" 2> /dev/null) $(tail -c 500 "$OUT/agent-render.err")"
cat "$OUT/agent-render.json"
}
