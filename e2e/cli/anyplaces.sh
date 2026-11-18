# any_places (cl-1244). Found by cl-1224's cell on rented pods, and again on pod-session 44383243364: `cozy run <local code> --on any`
# never ran. Its "sync local/… (source changed)" job stood for good at "waiting for a machine that can take it: <pod> can't run it
# ("sha256:…/" is not org/package/function[@release] or sha256:<hex>/function)", for every rental of the account and this computer.
# Calcifer's queue asked each machine when it would finish the job as a function, and a job that calls none (a package's install or
# sync, a model's) has no function to name. In 0.26.0 and 0.26.1 too.
# One stood-in rental (rcmoves.sh's: `cozy machine serve --rental` on this runner, on the newest release's image), ready, beside this
# computer's machine. Each of these is sent `any`, which is also where a job naming no machine goes on an OS that runs no models
# itself while a rental is up (`runs::default_on`). Each is placed on the rental at once and completes there:
#   a package's install            cozy package install <local code> --on any
#   a model's install              cozy model install <a .safetensors file> local/<name> --on any
#   a run whose code needs a sync  cozy run <that code, changed>/reverse --on any: its sync, then its call
# Linux, a GitHub runner; its own shard.
any_places() {
step "what calls no function, sent any, goes to a machine that takes work: an install, a model, a run's sync (cl-1244)"
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "any places: a fresh GitHub Linux runner only"; return 0; }
RM_DIR=$OUT/anyplaces RM_CFG=$HOME/.cozy/config.toml RM_SAVED=$OUT/anyplaces-config.toml
cp "$RM_CFG" "$RM_SAVED" || { check "any places: no config.toml to keep"; return; }
rm_rentals stand-d && rm_start && ap_cell
rm_quit
}

ap_cell() {
local d=$RM_DIR/stand-d pkg=$RM_DIR/weightless
rm_serve stand-d
for _ in $(seq 360); do rm_show stand-d > "$d/show-ready.json" && J '.status.phase == "ready"' "$d/show-ready.json" && break; sleep 0.5; done
J '.status.phase == "ready"' "$d/show-ready.json" || { check "any places: stand-d wasn't ready in 3 min: $(tail -c 600 "$d/machine.log")"; return; }
cp -r "$here_rcmoves/weightless" "$pkg" && safetensors_file "$RM_DIR/tiny.safetensors" || { check "any places: its package and model file weren't made"; return; }
echo "any places: the machines here: $(cozy --json machine ls | jq -c '[.machines[] | {name, kind, state}]')"
ap_send "a package's install" install package install "$pkg" || true
ap_send "a model's install" model model install "$RM_DIR/tiny.safetensors" local/any-places --source-format as-is || true
# Its code changed since that install: the run's first job is its sync.
echo "# changed $RANDOM" >> "$pkg/e2e_weightless/__init__.py"
ap_send "a run whose code needs a sync" run run "$pkg/reverse" "text=any$RANDOM" || return 0
rm_runs stand-d "$RM_DIR/runs.json"
echo "any places: what stand-d ran: $(jq -c '[.runs | sort_by(.created_at_ms)[] | {name, kind, status, target: .target[0:60]}]' "$RM_DIR/runs.json")"
}

# ap_send <what> <name> <cozy args...>: `cozy --json <args> --on any` from the project, followed to its end. stand-d takes work the
# whole time, so it is placed at once: one that said it waits for a machine and is still going a minute on is cut there.
ap_send() {
local what=$1 name=$2 pid code=0 waits=''
shift 2
(cd "$RUN_PROJECT" && exec timeout 900 "$COZY" --json "$@" --on any) > "$RM_DIR/$name.ndjson" 2> "$RM_DIR/$name.err" & pid=$!
for _ in $(seq 60); do kill -0 "$pid" 2> /dev/null || break; sleep 1; done
if kill -0 "$pid" 2> /dev/null; then
  waits=$({ grep -ohE 'waiting for a machine that can take it.*' "$RM_DIR/$name.ndjson" "$RM_DIR/$name.err" || true; } | tail -1 | cut -c1-400)
  [ -z "$waits" ] || kill "$pid" 2> /dev/null || true
fi
wait "$pid" || code=$?
[ -z "$waits" ] || { check "any places: $what was placed nowhere: $waits"; return 1; }
[ "$code" = 0 ] && J '.run.status == "completed" and .run.machine == "stand-d"' <(tail -1 "$RM_DIR/$name.ndjson") \
  || { check "any places: $what didn't complete on stand-d (exit $code): $(tail -1 "$RM_DIR/$name.ndjson" | jq -c '.run | {status, machine, reason}' 2> /dev/null) $(tail -c 300 "$RM_DIR/$name.err")"; return 1; }
echo "any places: $what: $(tail -1 "$RM_DIR/$name.ndjson" | jq -c '.run | {name, kind, target, machine, status}') $(grep -ohE 'Cozy placed it[^"]*' "$RM_DIR/$name.ndjson" | head -1)"
}
