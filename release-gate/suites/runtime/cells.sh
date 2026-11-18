# Suite `runtime`: H3's own components against their stored times. Every package's default call left with xs-219: suite
# `escaped` runs each package at a 6 GiB cap, and `packages` runs what a release changes.
# H3's video VAE at least as fast as ComfyUI's (cr-306).
source "$HERE/suites/runtime/h3-vae.sh"
# H3's latent upscaler on its int8 lane at its measured speed (cr-327).
source "$HERE/suites/runtime/h3-upscaler.sh"
