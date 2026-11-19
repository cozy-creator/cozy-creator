#!/bin/bash
# On the pod: the engine's launcher check (`python -m cozy_runtime.engine.launcher`), found by its command line, sent
# SIGSEGV a second after it starts, once, as a launcher that crashes its driver ends it (engine-routes's `refused`).
for _ in $(seq 1 6000); do
  for d in /proc/[0-9]*; do
    if tr '\0' ' ' < "$d/cmdline" 2> /dev/null | grep -q -- '-m cozy_runtime[.]engine[.]launcher'; then
      p=${d#/proc/}
      sleep 1
      line=$(tr '\0' ' ' < "/proc/$p/cmdline" 2> /dev/null)
      if kill -SEGV "$p" 2> /dev/null; then echo "killed $p (SIGSEGV) a second after it started: $line"; else echo "could not kill $p: $line"; fi
      exit 0
    fi
  done
  sleep 0.1
done
echo "no launcher check seen in 10 minutes"
