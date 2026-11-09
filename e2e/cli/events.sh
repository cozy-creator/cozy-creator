#!/usr/bin/env bash
# e2e/cli/events.sh --out DIR: GET /v1/events and the live views on it, alone: this computer's cozy as it is, in the default home,
# on a local machine. Calcifer must be a build that serves the route. The project suite runs the same steps, and a restart too.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/project.sh"
. "$here/../lib.sh"
parse "$@"
ALONE=1
hub_checked() { :; }
ADDR=$(jq -r .address "$HOME/.cozy/calcifer/calcifer.lock.holder") TOKEN=$(jq -r .token "$HOME/.cozy/calcifer/calcifer.lock.holder")
event_stream
live_views
finish events
