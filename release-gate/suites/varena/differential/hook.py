"""Gate hook, loaded by a package copy (cell.sh): this arm, through the Runtime's gate seam
(`cozy_runtime.differential`, unstable, release gate only). It digests every component's outputs and each split
site's decisions into the run record's `execution.loops`. `ARM` turns on varena's checks; `PRESS` changes what the
second request finds: "refill" fills the card once and lets go, so a kept graph's weights left and come back, "hold"
keeps it filled through the request, so they can't all come back and its calls run eagerly, "eager" turns graphs off.
Before Runtime 0.48 this file imported the Runtime's internals itself; its contract refuses that (cr-329)."""

from cozy_runtime import differential

ARM: dict[str, bool] = {}  # cell.sh writes this arm's checks here
PRESS = ""  # and this arm's press

differential.arm(checks=ARM, press=PRESS)  # type: ignore[arg-type]  # cell.sh writes one of the presses
