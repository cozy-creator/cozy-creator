# Root-package relay

`echo_calls.py` is generated from the `minimax-h3-tools` stand-in that `e2e/isolated.sh` seeds (`release-gate/suites/hub/pkg`
renamed), which is an ordinary dependency here. With that seed locked, from this directory:

```sh
cozy-runtime --dir . calls minimax-h3-tools e2e_relay/echo_calls.py
```

`commands.sh` and `source.sh` copy it into their flat caller packages; `commands.sh` also copies `../latents_maker_calls.py`,
generated from its `e2e_latents_maker` seed.
