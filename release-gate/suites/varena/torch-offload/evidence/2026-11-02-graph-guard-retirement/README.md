# Capture guard and intent retirement: bounded proof

Feature bd08f2eedc4a7d6be2a8ebf39851ae25b52afb15 fixes two frontend ownership errors.

P1: the actual read-only binding guard tuple retained every prepared weight Storage.
The real CPU no-user-escape control is red before the fix: Storage stays live even
after export/prepared tables clear. Green: an ended binding and its bound method
remain alive, yet sanctioned guard refs retire and Storage expires. A true detached
alias is still detected and its exact actual Storage remains strongly retained
until the final strong owner drops. Active binding closure refuses; read-only model
and prepared-weight protection remain. No fake CUDA/native resource is used.

P2: capture now selects/restores intent through the epoch's tracked state. Outside
completed capture/routing, known intent deactivates before release/escape classification.
Deactivation failure keeps exact epoch/exports/Regions/Sources/Plan and marks enclosing
owner failed, including no-parent cases. Other uncertainty with owned active intent
also marks owner failed. No external Plan or shared Region is closed/deactivated.
This P2 claim is SOURCE review, not CPU/GPU native Plan qualification.

Ten actual CPU/native-metadata clients pass against the existing auditedb40f SDK/core
plus exact current Python source overlay. Native host guards and absent-handle Plan
refusals do not imply CUDA capture, native active-Plan retirement, Pins or completion.
Exact commands, source/artifact hashes and exit codes are in guard-final-manifest.json.
Earlier receipts, including original red control outputs, remain preserved.

Prepared GPU capture_escape mode is UNRUN: actual detached prebound native Storage
escape, classification refusal, known Plan retired before refusal, subsequent real
model Plan activation usable, and native Pin custody retained even after module alias
clear. The isolated child intentionally keeps an unproven owner until exit; no physical
reclamation claim. Native deactivation failure/uncertain capture-end qualification
remains coordinator-owned. No machine/GPU/rental/native/byte-policy action occurred.
