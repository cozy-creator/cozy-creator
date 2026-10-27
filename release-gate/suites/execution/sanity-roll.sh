# Pre-roll sanity hook (release lane): kernel-regret (#2178, a measurement), then SDXL bf16 twice on one executor with
# the second native (sdxl-native-twice.sh). Sourced by gate.sh --cells after the runtime suite's cells (h3,
# conditional-body); `only` picks among h3, conditional-body, kernel-regret, sdxl-bf16-1, sdxl-bf16-2.
. "$HERE/suites/execution/kernel-regret/cells.sh"
. "$HERE/suites/execution/sdxl-native-twice.sh"
