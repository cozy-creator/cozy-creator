# Isolated Hub seed cohort (cl-994)

Seeding the isolated Hub uses the run's candidate Runtime and TensorFS, the same bytes the cells run, never the RC's
rolling build. The candidate action exports the selected wheels and the Runtime/packages commits; `skew/run.sh` serves
the wheels from the fixture's TLS ingress (`/__cozy_sdk/`). `seed_sdk.py pin` points only the private copies of cohort
packages at them before `cozy package publish`, and `seed_sdk.py verify` checks that each returned lock still names
those URLs, versions and hashes. Publication describes the completed lock's Runtime and TensorFS.

A selected wheel must meet each package's declared range; no range is raised or loosened. A native wheel limits the
copy's lock to the runner's OS and architecture, intersected with any authored environments. Old-floor and negative
fixtures keep their own selection, so their floor refusals stay meaningful.

`seed-sdk.json` records each input's version, SHA256 and embedded Runtime commit; `cohort.json` adds the packages
commit, the requested refs and the verified lock hashes, and `cohort.txt` reaches the step summary.
