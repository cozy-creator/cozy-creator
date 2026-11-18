# Suite `rentals`: a rental's life from end to end (cl-1203, th-404). The stop first: the idle cell ends the gate's rental.
. "$HERE/suites/rentals/lib.sh"
rental_stop_closes vast
# gate.sh's `hold` renews the rental's idle clock every 4 minutes while the gate lives, so the rental could never idle out and
# the cell would read the renewals as the updates' doing: it stops here, the last cell (the idle cell ends the gate's rental).
[ -z "${KEEP:-}" ] || { kill "$KEEP" 2>/dev/null; wait "$KEEP" 2>/dev/null; KEEP=; }
rental_idle_through_updates "$M" "$RID"
