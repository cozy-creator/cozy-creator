# installed_follow (cl-1029): in a dev home whose `local` follows the RC (`dev = true`, an RC build, `[calcifer] packages`), a new
# cozy installed here, as scripts/dev-local.sh installs each build, reaches local's TensorD by itself once this computer is at
# rest: Calcifer's own update (`cozy X (installed)`), which pins it on nothing. Before that, a pin by hand (`--tensord`, as the
# laptop's was on 11-15) shows in `cozy machine ls` and `machine show` with the command that lets it follow again. The other
# build is the open RC's own (its prerelease's cozy): local's TensorD runs it, then the candidate is installed and Calcifer
# restarted on it, and nothing else is asked.
installed_follow() {
step "a dev home's local TensorD follows the cozy installed here, at rest; a pin by hand says so (cl-1029)"
[ -n "$LOCAL" ] || return 0
[ "${RUNNER_OS:-}" = Linux ] && [ "${GITHUB_ACTIONS:-}" = true ] || { echo "installed follow: a fresh GitHub Linux runner only"; return; }
local was rc other cfg=$HOME/.cozy/config.toml co=$OUT/installed-packages saved=$OUT/installed-config.toml back=$OUT/installed-cozy rc_cozy=$OUT/installed-rc-cozy running
was=$("$COZY" --version) && was=${was#cozy }
case $was in *-dev+g*) ;; *) echo "installed follow: cozy $was is no RC build, whose local follows nothing"; return ;; esac
cp "$COZY" "$back"

# Pinned by hand, as `cozy machine update local --tensord ~/.cozy/bin/cozy` pins it.
machine_ready
cozy --json machine update local --tensord "$COZY" -y > "$OUT/installed-pin.json" 2>&1 \
  || { check "installed follow: local refused its cozy by hand: $(tail -c 300 "$OUT/installed-pin.json")"; return; }
machine_ready
J '.held | test("^tensord ")' "$OUT/machine.json" || check "installed follow: machine show doesn't say local is held: $(jq -c .held "$OUT/machine.json")"
cozy machine ls > "$OUT/installed-ls.txt" 2>&1 || check "installed follow: machine ls failed: $(tail -c 300 "$OUT/installed-ls.txt")"
grep -q '^local: held by hand on tensord .*`cozy machine update local` lets it follow again' "$OUT/installed-ls.txt" && grep -qE '^local .*ready · held' "$OUT/installed-ls.txt" \
  || check "installed follow: machine ls doesn't say local is held and how to let it go: $(cat "$OUT/installed-ls.txt")"

# A dev home: local follows the RC, but not while held.
cp "$cfg" "$saved"
rm -rf "$co" && mkdir -p "$co" && git -C "$co" init -q -b rc/e2e && git -C "$co" -c user.name=e2e -c user.email=e2e@localhost commit -q --allow-empty -m checkout \
  || { check "installed follow: the packages checkout wasn't made"; return; }
{ echo 'dev = true'; grep -v '^dev *=' "$saved"; } > "$cfg.next"
if grep -q '^\[calcifer\]' "$cfg.next"; then sed -i "/^\[calcifer\]/a packages = \"$co\"" "$cfg.next"; else printf '\n[calcifer]\npackages = "%s"\n' "$co" >> "$cfg.next"; fi
mv "$cfg.next" "$cfg"
cozy restart > "$OUT/installed-restart.txt" 2>&1 || { check "installed follow: Calcifer didn't restart: $(tail -c 300 "$OUT/installed-restart.txt")"; cp "$saved" "$cfg"; return; }
machine_ready
J '.rc.state == "held"' "$OUT/machine.json" || check "installed follow: held local doesn't say so beside the RC: $(jq -c .rc "$OUT/machine.json")"

# Let go: nothing named moves it to the open RC's Runtime and TensorFS and pins nothing.
cozy --json machine update local -y > "$OUT/installed-release.json" 2>&1 \
  || check "installed follow: local refused to follow again: $(tail -c 300 "$OUT/installed-release.json")"
machine_ready
J '.held == null' "$OUT/machine.json" || check "installed follow: local is still held: $(jq -c .held "$OUT/machine.json")"

# local's TensorD on another build: the open RC's own cozy, installed and started as Calcifer starts any (no update, so no pin).
rc=$(jq -r '.rc.rc // empty' "$OUT/machine.json")
other=$(curl -fsSL "https://github.com/cozy-creator/cozy-creator/releases/download/rc-$rc/SHA256SUMS" 2> /dev/null | awk '$2 ~ /^cozy-.*-linux-x86_64$/ {print $1, $2; exit}')
if [ -z "$other" ]; then
  check "installed follow: rc-${rc:-?} lists no cozy for linux/x86_64"
elif [ "$(cut -d' ' -f2 <<< "$other")" = "cozy-$was-linux-x86_64" ]; then
  echo "installed follow: the open RC's own cozy is this candidate ($was): nothing to move to, so the move isn't asked here"
else
  curl -fsSL -o "$rc_cozy" "https://github.com/cozy-creator/cozy-creator/releases/download/rc-$rc/$(cut -d' ' -f2 <<< "$other")" \
    && echo "$(cut -d' ' -f1 <<< "$other")  $rc_cozy" | sha256sum -c --quiet - \
    || { check "installed follow: the open RC's cozy didn't fetch: $other"; cp "$saved" "$cfg"; CAND_COZY=$back install_candidate_binary; cozy restart > /dev/null 2>&1; return; }
  chmod +x "$rc_cozy"
  CAND_COZY=$rc_cozy install_candidate_binary
  cozy restart > "$OUT/installed-restart.txt" 2>&1 || check "installed follow: Calcifer didn't restart on the RC's cozy: $(tail -c 300 "$OUT/installed-restart.txt")"
  machine_stop > /dev/null 2>&1 || true
  machine_ready
  other=$(jq -r '.status.version' "$OUT/machine.json")
  echo "installed follow: local's TensorD runs the open RC's cozy $other"
  [ "$other" != "$was" ] || check "installed follow: local's TensorD didn't start on the open RC's cozy: it runs the candidate $was"
  # The candidate installed here, as dev-local.sh installs a build, and Calcifer restarted on it: nothing else is asked. local's TensorD
  # runs it within 4 min (60 s at rest, Calcifer's look every 30 s, then its restart).
  CAND_COZY=$back install_candidate_binary
  cozy restart > "$OUT/installed-restart.txt" 2>&1 || check "installed follow: Calcifer didn't restart on the candidate: $(tail -c 300 "$OUT/installed-restart.txt")"
  for _ in $(seq 48); do
    cozy --json machine show local > "$OUT/installed-machine.json" 2> /dev/null && J --arg v "$was" '.status.version == $v' "$OUT/installed-machine.json" && break
    sleep 5
  done
  running=$(jq -r '.status.version // "nothing"' "$OUT/installed-machine.json")
  echo "installed follow: local's TensorD runs $running after cozy $was was installed over $other; it says $(jq -c '{rc, held}' "$OUT/installed-machine.json")"
  [ "$running" = "$was" ] || check "installed follow: local's TensorD still runs $running, not the installed $was: $(jq -c .rc "$OUT/installed-machine.json")"
  J '.held == null' "$OUT/installed-machine.json" || check "installed follow: Calcifer's own move pinned local: $(jq -c .held "$OUT/installed-machine.json")"
fi

# The candidate, off a dev home.
cp "$saved" "$cfg"
CAND_COZY=$back install_candidate_binary
cozy restart > "$OUT/installed-restart.txt" 2>&1 || check "installed follow: Calcifer didn't restart off the dev home: $(tail -c 300 "$OUT/installed-restart.txt")"
machine_ready
}
