# cl-1066 (Paul 11-16, "stop doing dumb bullshit like this"): what his standing rules decide, as a person meets it.
# folder_private: a project folder other accounts on this computer may open is shut to them when the project is read, said nothing of:
#   its other bits, and its group bits while its group is the owner's own primary one (`staff`, `users`: every account's). A folder
#   given to another group is a share its owner chose, and keeps that group. Before, every read warned "other accounts on this computer
#   can open <dir>: chmod 700 <dir>" and left it open.
# protocol_free: a project route answers a client that sends no `cozy-project-storage: 3` header; before, it refused it
#   `project_storage_protocol_required`, "update this client".
folder_private() {
step "a project folder others may open is shut to them, a chosen group's share kept, said nothing of (cl-1066)"
[ "${RUNNER_OS:-}" != Windows ] || { echo "folder_private: Unix modes, read on Linux and macOS"; return; }
local dir=$OUT/standing-open shared=$OUT/standing-shared group mode
mode_of() { stat -c %a "$1" 2> /dev/null || stat -f %Lp "$1"; }
# The owner's own group: shut, as every account is in it on a desktop.
mkdir -p "$dir" && chmod 755 "$dir" || { check "folder_private: no folder"; return; }
cozy --json new --unencrypted "$dir" > "$OUT/standing-open-new.json" 2>&1 || { check "folder_private: cozy new: $(tail -c 300 "$OUT/standing-open-new.json")"; return; }
chmod 755 "$dir"
in_project "$dir" status > "$OUT/standing-open-status.txt" 2>&1 || check "folder_private: cozy status: $(tail -c 300 "$OUT/standing-open-status.txt")"
mode=$(mode_of "$dir")
[ "$mode" = 700 ] || check "folder_private: $dir is mode $mode after its project was read, not 700"
! grep -q "other accounts on this computer can open" "$OUT/standing-open-status.txt" \
  || check "folder_private: it warned instead: $(grep 'other accounts' "$OUT/standing-open-status.txt" | head -1)"
# Another group the owner belongs to: a share, kept; only the others go.
group=$(id -Gn | tr ' ' '\n' | grep -vx "$(id -gn)" | head -1 || true)
if [ -n "$group" ]; then
  mkdir -p "$shared" && chgrp "$group" "$shared" && chmod 775 "$shared" || { check "folder_private: no shared folder"; return; }
  cozy --json new --unencrypted "$shared" > "$OUT/standing-shared-new.json" 2>&1 || { check "folder_private: cozy new (shared): $(tail -c 300 "$OUT/standing-shared-new.json")"; return; }
  chmod 775 "$shared"
  in_project "$shared" status > "$OUT/standing-shared-status.txt" 2>&1 || check "folder_private: cozy status (shared): $(tail -c 300 "$OUT/standing-shared-status.txt")"
  mode=$(mode_of "$shared")
  [ "$mode" = 770 ] || check "folder_private: $shared, shared with $group, is mode $mode after its project was read, not 770"
else
  echo "folder_private: this account is in no group but its own: no share to keep"
fi
}

protocol_free() {
step "a project route answers a client sending no protocol header (cl-1066)"
local dir=$OUT/standing-plain holder=$HOME/.cozy/calcifer/calcifer.lock.holder encoded code
cozy --json new --unencrypted "$dir" > "$OUT/standing-plain-new.json" 2>&1 || { check "protocol_free: cozy new: $(tail -c 300 "$OUT/standing-plain-new.json")"; return; }
encoded=$(python3 -c 'from urllib.parse import quote; import os,sys; print(quote(os.path.abspath(sys.argv[1]), safe=""))' "$dir")
code=$(curl -sS -o "$OUT/standing-plain-state.json" -w '%{http_code}' -H "Authorization: Bearer $(jq -r .token "$holder")" \
  "http://$(jq -r .address "$holder")/v1/projects/$encoded/address") || code=none
[ "$code" = 200 ] && J '.id | length > 0' "$OUT/standing-plain-state.json" \
  || check "protocol_free: GET its address with no protocol header answered $code: $(head -c 300 "$OUT/standing-plain-state.json")"
}
