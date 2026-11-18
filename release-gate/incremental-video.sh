# incremental-video.sh, a --cells hook (`-f cells=release-gate/incremental-video.sh -f runtime_wheel=<cozy-runtime dev-build run, wheels>
# -f cozy=<dev-build run> -f packages_ref=<packages commit> -f publish=false`): cl-1210's proof (Paul, 11-18: "we re-assemble all the
# videos and re-encode them, then we re-download them all over again"). One three-segment H3 Turbo shot, one seed, on one pod, twice:
# - before: the open RC's own Runtime build at the candidate's merge-base with it, and the packages commit `packages_ref` branched from
#   (its merge-base with the packages RC). A pod's own Runtime is the last release's, below the RC packages' floor.
# - after: the candidate Runtime, and `packages_ref`.
# Both on the candidate cozy and the pod's TensorFS, so the arms differ by the two changes alone (`iv-arms`).
# Each arm, as it runs (incremental-video.py `tap`): every event of its run as it reaches this computer, and five times a second the TCP
# payload received from the machine. Every revision of its video as it lands is exported, probed and decoded whole. iv.json (`report`)
# is each revision's length, what it kept of the one before (the event's `appended_from`), and what this computer received from the
# machine while it landed, beside Calcifer's own account, the run's record (`measurements.transfer`): the wire takes nobody's word,
# and the proof is that the two agree. Then its last segment's decode (frames, ms) and its last denoise to the take on disk.
# Then the pictures and sound (`pictures`, iv-pictures.json and its strips): AFTER against the whole-shot decode of its own latents
# (`vae_round_trip`, the latents named by their hash: they stay on the machine), and against BEFORE over segment 1, which both arms make
# alike; the step at each seam in both; clicks and drift at each seam. The shot is three 8 s beats: two never share a segment.
# The rental ends once the whole decode is in: the pictures are compared after it.
ONLY=  # the suites' --only chose their cells, not these
IV_RC=rc/0.27.0  # the open RC both bases are read from
IV_PY=(python3 -I "$HERE/incremental-video.py")
IV_AT= IV_BASE_AT= IV_BASE= IV_BASE_V= IV_LAT= IV_LANE=  # set -u: said even when a step that sets them didn't run
IV_SHOT=(summary="A lighthouse on a rocky coast at dusk, the sea rising." soundscape="Waves breaking, wind, gulls." turbo:=true seed:=$((R + 1210))
  "beats:=$(jq -cn '[{duration: 8, description: "[Shot 1] The beam sweeps over the dark sea as waves roll in and break on the rocks."},
    {duration: 8, description: "The camera rises along the lighthouse as the wind picks up and spray climbs the tower."},
    {duration: 8, description: "From the gallery the camera looks out to sea, where a ship passes under the beam."}]')")

# iv_base: the base arm's Runtime into IV_BASE (its manylinux wheel, here) and IV_BASE_V (the version it reports): the RC's build
# (rc-dev's, on the `rc-<x.y.z>` release) of the commit the candidate branched from, which its wheel names (`_build_provenance`).
iv_base() {
  local release=${IV_RC/\//-} name
  IV_AT=$(unzip -p "$RWF" cozy_runtime/_build_provenance.py 2> /dev/null | sed -n 's/^COMMIT = "\([0-9a-f]\{40\}\)".*/\1/p')
  [ -n "$IV_AT" ] || { log "incremental-video: $RWF names no commit"; return 1; }
  IV_BASE_AT=$(gh api "repos/cozy-creator/cozy-runtime/compare/$IV_RC...$IV_AT" -q .merge_base_commit.sha 2> iv-base.err)
  [ -n "$IV_BASE_AT" ] || { log "incremental-video: no merge-base of $IV_AT with cozy-runtime $IV_RC: $(tail -c 200 iv-base.err)"; return 1; }
  [ "$IV_BASE_AT" != "$IV_AT" ] || { log "incremental-video: the candidate Runtime $IV_AT is on $IV_RC already: nothing to compare it with"; return 1; }
  name=$(gh release view "$release" -R cozy-creator/cozy-runtime --json assets -q '.assets[].name' 2>> iv-base.err |
    grep -F -- "+g${IV_BASE_AT:0:8}-" | grep -m1 'manylinux.*x86_64\.whl$')
  [ -n "$name" ] || { log "incremental-video: cozy-runtime's $release holds no manylinux wheel of $IV_BASE_AT: $(tail -c 200 iv-base.err)"; return 1; }
  mkdir -p "$OUT/candidate" && gh release download "$release" -R cozy-creator/cozy-runtime -p "$name" -D "$OUT/candidate" --clobber 2>> iv-base.err ||
    { log "incremental-video: $name not downloaded: $(tail -c 200 iv-base.err)"; return 1; }
  IV_BASE=$OUT/candidate/$name IV_BASE_V=${name#cozy_runtime-}
  IV_BASE_V=${IV_BASE_V%%-cp*}
}

# iv_runtime <runtime> <version it reports> <arm>: the pod on that Runtime, as `machine show` says; what it runs, for the report.
iv_runtime() {
  local i said=
  "$CLI" --json machine update "$M" --runtime "$1" -y > "iv-$3-update.json" 2> "iv-$3-update.log" \
    || { log "incremental-video: update to $2: $(tail -c 300 "iv-$3-update.log" | tr '\n' ' ')"; return 1; }
  for i in $(seq 60); do
    said=$("$CLI" --json machine show "$M" 2> /dev/null | jq -c '.status // {} | {version, runtime, tensorfs}')
    [ "$(jq -r '.runtime // ""' <<< "$said" 2> /dev/null)" = "$2" ] && { echo "$said" > "iv-$3.machine"; return 0; }
    sleep 5
  done
  log "incremental-video: machine show says ${said:-nothing}, not Runtime $2"
  return 1
}

# iv_watch <arm>: each revision of the arm's video as it lands, until its run settles or its cell is over (`iv-<arm>.celled`): exported,
# probed and decoded whole. A revision whose export isn't its length (a newer one landed meanwhile) is looked at again.
iv_watch() {
  local arm=$1 name=iv-$1 n rev want last=0 show f at once=
  : > "iv-$arm.revs"
  while [ ! -s "$name.number" ]; do [ ! -e "iv-$arm.celled" ] || return 0; sleep 1; done
  n=$(cat "$name.number")
  [ "$n" != none ] || return 0
  while :; do
    if show=$(cd "${RUN_PROJECT:-$PWD}" && "$CLI" --json run watch "$(run_ref "$OUT" "$name" "$n")" --once 2> /dev/null); then
      rev=$(jq -r '[.output[]? | select(.name == "video" and .url) | .rev] | max // 0' <<< "$show")
      want=$(jq -r '[.output[]? | select(.name == "video" and .url) | .length] | max // 0' <<< "$show")
      if [ "$rev" -gt "$last" ]; then
        at=$(date +%s%3N)
        rm -rf "iv-$arm.out"
        (cd "${RUN_PROJECT:-$PWD}" && "$CLI" run watch "$(run_ref "$OUT" "$name" "$n")" --once -o "$OUT/iv-$arm.out") > /dev/null 2>&1
        f=$(find "iv-$arm.out" -name '*.mp4' 2> /dev/null | head -1)
        # A revision of a run still going has no digest on its record, so `run watch -o` exports none of it (44275320398 saw only
        # the take): it is the project's file of that length, where Calcifer landed it.
        [ -n "$f" ] || f=$(find "$OUT/project/files/" -maxdepth 1 -type f -size "${want}c" 2> /dev/null | head -1)
        if [ -n "$f" ] && [ "$(stat -c %s "$f")" = "$want" ]; then
          last=$rev
          mkdir -p "iv-$arm.revisions" && cp "$f" "iv-$arm.revisions/rev-$rev.mp4"
          printf '%s\t%s\t%s\t%s\t%s\n' "$rev" "$at" "$want" \
            "$(ffprobe -v error -count_packets -show_entries format=duration:stream=codec_type,nb_read_packets,start_time -of json "$f" | jq -c '{duration: (.format.duration | tonumber? // 0), video: ([.streams[] | select(.codec_type == "video") | .nb_read_packets | tonumber] | add // 0), audio: ([.streams[] | select(.codec_type == "audio") | .nb_read_packets | tonumber] | add // 0), starts: ([.streams[] | {key: .codec_type, value: (.start_time | tonumber? // null)}] | from_entries)}')" \
            "$(nice -n 19 ffmpeg -v error -i "$f" -f null - 2>&1 | head -c 200 | tr '\n\t' '  ')" >> "iv-$arm.revs"
        fi
      fi
      case $(jq -r .status <<< "$show") in
        completed) jq -e '.output[]? | select(.name == "video" and .url and .status == "completed")' <<< "$show" > /dev/null && [ "$rev" = "$last" ] && break ;;
        failed|canceled) break ;;
      esac
    fi
    # Its cell over, one more look takes what landed last.
    if [ -e "iv-$arm.celled" ]; then [ -z "$once" ] || break; once=1; fi
    sleep 1
  done
}

# iv_arm <arm> <target>: the shot, watched and tapped.
iv_arm() {
  local arm=$1 name=iv-$1
  rm -f "iv-$arm.stop" "iv-$arm.tap" "iv-$arm.celled"
  "${IV_PY[@]}" tap "$OUT/$name.ndjson" "$OUT/iv-$arm.tap" --stop "$OUT/iv-$arm.stop" 2> "iv-$arm.tap.err" & local tapper=$!
  iv_watch "$arm" & local watcher=$!
  cell "$name" "$2" "${IV_SHOT[@]}"
  : > "iv-$arm.celled"
  wait "$watcher"
  sleep 2
  : > "iv-$arm.stop"
  wait "$tapper"
}

if [ -z "$RWF" ] || [ ! -f "$RWF" ] || [ -z "$PKGS" ]; then
  expect incremental-video FAIL "needs a candidate Runtime wheel (runtime_wheel: ${RWF:-none}) and a packages_ref"
  return 0
fi
# Its merge-base as GitHub reads it, as iv_base reads the Runtime's: the checkout is one commit deep (44273987649 found none in it).
IV_PKG_BASE=$(gh api "repos/cozy-creator/packages/compare/$IV_RC...$(git -C "$PKGS" rev-parse HEAD)" -q .merge_base_commit.sha 2> iv-base.err)
[ -n "$IV_PKG_BASE" ] && [ "$IV_PKG_BASE" != "$(git -C "$PKGS" rev-parse HEAD)" ] && git -C "$PKGS" fetch -q --depth=1 origin "$IV_PKG_BASE" 2>> iv-base.err \
  && git -C "$PKGS" worktree add -q --detach "$OUT/packages-before" "$IV_PKG_BASE" \
  || { expect incremental-video FAIL "no merge-base of packages_ref with the packages $IV_RC to compare it with (${IV_PKG_BASE:-none}): $(tail -c 200 iv-base.err 2> /dev/null)"; return 0; }
ln -sfn "$OUT/packages-before/minimax-h3" "$OUT/minimax-h3-before"
# Its Triton sets, built as gate.sh built the candidate's while the rental booted: each arm's first request finds its own.
if [ -x "$PKGS/scripts/triton.sh" ]; then
  ( "$PKGS/scripts/triton.sh" "$OUT/packages-before/minimax-h3" > kernels-minimax-h3-before.log 2>&1; echo $? > kernels-minimax-h3-before.status ) &
fi

if iv_base; then
  log "incremental-video: before on Runtime $IV_BASE_V ($IV_BASE_AT), packages $IV_PKG_BASE; after on Runtime $rt ($IV_AT), packages $(git -C "$PKGS" rev-parse HEAD); cozy $v and TensorFS $tfs in both"
  while [ -x "$PKGS/scripts/triton.sh" ] && [ ! -s kernels-minimax-h3-before.status ]; do sleep 2; done
  [ ! -x "$PKGS/scripts/triton.sh" ] || [ "$(cat kernels-minimax-h3-before.status)" = 0 ] ||
    log "incremental-video: before's Triton sets did not build, so its run compiles them: $(tail -c 200 kernels-minimax-h3-before.log | tr '\n' ' ')"
  iv_runtime "$IV_BASE" "$IV_BASE_V" before && iv_arm before ./minimax-h3-before/generate
else
  expect incremental-video FAIL "no base Runtime, so no before arm: $(tail -n 1 gate.log | cut -c1-200)"
fi
iv_runtime "$RWF" "$rt" after && iv_arm after "$(pkg minimax-h3 generate)"

# The arms side by side, and the checks.
"${IV_PY[@]}" report "$OUT" > iv.json 2> iv-report.err || log "incremental-video: no report: $(tail -c 300 iv-report.err)"
iv=$(cat iv.json)
log "incremental-video: $(jq -c 'map_values({take, expected_total, wire_total, record, last_segment: (.last_segment | {frames, decode_video_ms}), last_denoise_to_take_ms})' <<< "$iv")"
expect iv-arms "$(jq -r 'if .before.status == "completed" and .after.status == "completed" and .before.cozy == .after.cozy and .before.tensorfs == .after.tensorfs
  and .before.runtime != .after.runtime and .before.lanes == .after.lanes then "PASS" else "FAIL" end' <<< "$iv" 2> /dev/null || echo FAIL)" \
  "$(jq -c 'map_values({run, status, cozy, runtime, tensorfs, lanes})' <<< "$iv" 2> /dev/null) packages ${IV_PKG_BASE:0:8} and $(git -C "$PKGS" rev-parse --short=8 HEAD), Runtime ${IV_BASE_AT:0:8} and ${IV_AT:0:8}"
# Only the newest piece moves, by Calcifer's own account (the record's `transfer`): three revisions or more, each after the first keeping
# bytes of the one before, and no more read than the take is long (its indexes are read again: within 5%).
expect iv-reads "$(jq -r '.after as $a | if ($a.revisions | length) >= 3 and $a.record.read != null and $a.record.read <= $a.take * 1.05 and all($a.revisions[1:][]; .appended_from > 0) then "PASS" else "FAIL" end' <<< "$iv" 2> /dev/null || echo FAIL)" \
  "$(jq -c '{before: (.before | {record, take}), after: (.after | {record, take, appended_from: [.revisions[] | .appended_from]})}' <<< "$iv" 2> /dev/null)"
# The wire agrees with it, revision by revision, in both arms: what this computer received on the machine's output lanes (its bulk
# sockets; the control connection's events and status are outside the account, Predictive Downloads 11-18) while a revision landed is
# what its event said was new, the arm's sum is the record's `read` (each within 5%: a tie, Paul 11-15), and what the events said it
# kept is the record's `reused`, to the byte. Every socket's bytes and how it was classified are in iv.json (`sockets`).
expect iv-reads-agree "$(jq -r 'def near($a; $b): $a != null and $b != null and $b > 0 and (($a - $b) | fabs) <= $b * 0.05;
  if all(.before, .after; . as $arm | ($arm.revisions | length) > 0 and all($arm.revisions[]; near(.wire_bulk; .expected_read))
    and near($arm.wire_bulk_total; $arm.record.read) and $arm.record.reused == ([$arm.revisions[] | .appended_from] | add))
  then "PASS" else "FAIL" end' <<< "$iv" 2> /dev/null || echo FAIL)" \
  "$(jq -c 'map_values({peer: .peer.by, control: .sockets.control, wire_bulk_total, wire_total, record, revisions: [.revisions[] | {rev, length, appended_from, expected_read, wire_bulk, wire_control, wire}]})' <<< "$iv" 2> /dev/null)"
expect iv-no-reencode "$(jq -r '.after.last_segment as $s | if $s.frames > 0 and $s.frames < .before.last_segment.frames then "PASS" else "FAIL" end' <<< "$iv" 2> /dev/null || echo FAIL)" \
  "$(jq -c '{before: .before.last_segment, after: .after.last_segment, last_denoise_to_take_ms: map_values(.last_denoise_to_take_ms)}' <<< "$iv" 2> /dev/null)"
expect iv-playable "$(jq -r 'if (.after.revisions | length) > 0 and all(.before.revisions[], .after.revisions[]; .probe.duration > 0 and .probe.video > 0 and .probe.audio > 0 and .decode_errors == "") then "PASS" else "FAIL" end' <<< "$iv" 2> /dev/null || echo FAIL)" \
  "$(jq -c '[.after.revisions[] | {rev, duration: .probe.duration, video: .probe.video, audio: .probe.audio, decode_errors}]' <<< "$iv" 2> /dev/null)"

# The pictures and sound: AFTER against its own latents decoded whole, and against BEFORE.
IV_BEFORE=$(run_outputs "$OUT" iv-before | grep -E '\.mp4$' | head -1)
IV_AFTER=$(run_outputs "$OUT" iv-after | grep -E '\.mp4$' | head -1)
IV_WHOLE=
if [ -n "$IV_AFTER" ]; then
  # Its latents stay on the machine (a `Latents` result): named by their hash, which the project resolves and Calcifer fetches.
  IV_LAT=$(run_get "$OUT" iv-after '[.output[]? | select(.name == "latents") | .sha256][0]')
  IV_LANE=$(jq -r '.after.lanes | if length == 1 then .[0] else "" end' <<< "$iv" 2> /dev/null)
  if [ -n "$IV_LAT" ]; then
    # vae_round_trip also encodes a video it is given: the take's first 129 frames, the shape its own cell runs (suites/runtime/h3-vae.sh).
    nice -n 19 ffmpeg -nostdin -v error -y -i "$IV_AFTER" -frames:v 129 -an -c:v libx264 -pix_fmt yuv420p -crf 12 "$OUT/iv-whole-in.mp4"
    cell iv-whole "$(pkg minimax-h3 vae_round_trip)" --file video="$OUT/iv-whole-in.mp4" --file latents="$IV_LAT" ${IV_LANE:+--lane "$IV_LANE"}
    IV_WHOLE=$(run_outputs "$OUT" iv-whole | grep -E '\.mp4$' | head -1)
    [ -n "$IV_WHOLE" ] || log "incremental-video: no whole decode of the after take's latents: run $(cat iv-whole.number 2> /dev/null) $(run_get "$OUT" iv-whole .status) $(run_get "$OUT" iv-whole .reason.message | head -c 200)"
  else
    log "incremental-video: the after take's record names no latents, so no whole decode of them"
  fi
fi
# Every measurement that needs the pod is in: it ends here, not after the comparison (gate.sh's own end finds nothing left).
end_it
if [ -n "$IV_AFTER" ]; then
  seams=$(jq -r '.result.seams // [] | map(tostring) | join(",")' iv-after.json)
  joins=$(jq -r '[.after.revisions[:-1][] | .probe.video // empty] | map(tostring) | join(",")' <<< "$iv")
  nice -n 19 uv run -q --no-project --with numpy python -I "$HERE/incremental-video.py" pictures "${IV_BEFORE:--}" "$IV_AFTER" "${IV_WHOLE:--}" \
    --seams "$seams" --joins "$joins" --out "$OUT/iv-pictures" > iv-pictures.json 2> iv-pictures.err
  pics=$(cat iv-pictures.json)
  whole_lanes=$(jq -c '[.measurements.models[]?.lane] | unique' iv-whole.json 2> /dev/null)
  log "incremental-video pictures: $(jq -c '{frames, after_vs_whole: (.after_vs_whole | del(.near)), segment_1: (.after_vs_before.segment_1 // {} | del(.near)), later: (.after_vs_before.later // {} | del(.near)), steps, sound, brightness}' <<< "$pics" 2> /dev/null || tail -c 300 iv-pictures.err)"
  # A/V sync against BEFORE, which libavformat muxed: segment 1's sound within 1 ms of it, and every revision's tracks starting where
  # its take's do (a priming edit off by one AAC frame is 21-23 ms).
  expect iv-sync "$(jq -r --slurpfile iv iv.json '($iv[0].before.revisions[-1].probe.starts) as $want | if $want != null and ((.sound.segment_1.lag_ms // 99) | fabs) <= 1
    and all($iv[0].after.revisions[]; .probe.starts == $want) then "PASS" else "FAIL" end' <<< "$pics" 2> /dev/null || echo FAIL)" \
    "$(jq -c --slurpfile iv iv.json '{segment_1: .sound.segment_1, before_starts: $iv[0].before.revisions[-1].probe.starts, after_starts: [$iv[0].after.revisions[] | .probe.starts]}' <<< "$pics" 2> /dev/null)"
  # The same latents decoded in parts and whole, on the take's own lane: a take with no whole decode to hold it to fails.
  expect iv-same-pictures "$(jq -r --argjson lanes "$(jq -c '.after.lanes' <<< "$iv" 2> /dev/null || echo '[]')" --argjson whole "${whole_lanes:-[]}" \
    'if .after_vs_whole.psnr_least != null and .after_vs_whole.psnr_least >= 40 and ($whole | length) > 0 and ($whole - $lanes | length) == 0
      and (.after_vs_before.segment_1.psnr_mean // 0) >= 45 then "PASS" else "FAIL" end' <<< "$pics" 2> /dev/null || echo FAIL)" \
    "$(jq -c '{after_vs_whole: (.after_vs_whole | del(.near)), segment_1: (.after_vs_before.segment_1 // {} | del(.near))}' <<< "$pics" 2> /dev/null) lanes: take $(jq -c '.after.lanes' <<< "$iv" 2> /dev/null), whole ${whole_lanes:-unread}"
  # A seam steps or clicks no more than today's there (or 3x the median change, 1.5x the loudest step); a join of parts not at all, and
  # every join of parts is looked at. Clicks are compared at the same frame: AFTER's list holds its joins too (44284366904 compared its
  # seam 1 with today's seam 2).
  expect iv-seams "$(jq -r '.steps as $s | .sound as $a | if ($s.before | length) > 0 and ([range(0; $s.after | length)] | all(. as $i | $s.after[$i].ratio <= ([3, (($s.before[$i].ratio // 0) + 1)] | max)))
    and ($s.after_joins | length) == ($s.after | length) and all($s.after_joins[]; .ratio <= 3)
    and (($a.before.clicks | map({key: (.at | tostring), value: (.click // 0)}) | from_entries) as $today
      | all($a.after.clicks[]; (.click // 0) <= (if $today[.at | tostring] == null then 1.5 else ([1.5, $today[.at | tostring] + 0.5] | max) end)))
    and ($a.after.drift_ms | fabs) <= 50 then "PASS" else "FAIL" end' <<< "$pics" 2> /dev/null || echo FAIL)" \
    "$(jq -c '{steps, sound}' <<< "$pics" 2> /dev/null)"
else
  expect iv-same-pictures FAIL "no after take to compare (before ${IV_BEFORE:-none})"
fi
