# `cozy render` (cl-956), as an agent runs it, off a terminal on the weightless fixtures (render.py): only what it names renders
# (cl-1192), one reading a box with no take says to render that first, a chain named, -n, an input changed read as it stands, a render
# joined and one whose CLI was killed, a failure stopping only its readers, and --pickup.
render_here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

agent_render() {
[ -n "$LOCAL" ] || return 0
step "cozy render: only what it names, render-first said once, a chain named, -n, an input read as it stands, a joined render, a killed CLI, a failure, --pickup"
COZY=$COZY python3 "$render_here/render.py" "$OUT/agent-render" "$OUT" > "$OUT/agent-render.json" 2> "$OUT/agent-render.err" \
  || check "cozy render: $(jq -c .failures "$OUT/agent-render.json" 2> /dev/null) $(tail -c 500 "$OUT/agent-render.err")"
cat "$OUT/agent-render.json"
}
