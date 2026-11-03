# H3 lifecycle pair: sampled media review

Actual full runs25482 and25499 have362 decoded frames at1344×768 and24fps, with matching
15.083333s video format. Saved video-latent metadata matches and both10,354,176-value F16
tensors are finite. All four output hashes match the independent execution audit.

Eight matched frames (0,52,103,155,206,258,309,361) were inspected side by side. Both show a
detailed clockmaker's workbench, warm sunlight and coherent tool silhouettes. Candidate
composition differs substantially: a prominent lamp and differently placed machinery/tools.
No obvious candidate corruption, systematic blur or reduced scene detail was visible in
these samples. The sampled visual quality looks comparable; the outputs are not identical.
No full-motion playback or audio listening assessment was performed.

Full decoded comparison reports SSIM0.723110 and PSNR16.331089dB. Latent relative L2 is
0.699477 and cosine0.755860, with RMS1.059088 versus1.061225. These are material numerical
changes, not one-channel-level noise and not evidence of a perceptual failure by themselves.
Decoded PCM also differs (966,656 interleaved S16 samples, maximum difference380); that
comparison makes no perceptual audio-equivalence claim. Audio latents are not exported.

The original execution audit discloses control memoize=True/cold miss versus candidate
memoize=False, one sample per arm, and differing clocks/power throttling. Denoise/store-byte
improvements do not imply a blanket no-worse claim: store synchronous-read time increased.
Source, performance and media quality remain separately reported boundaries.

The saved orchestration used one allowed CPU, nice19, ffmpeg/filter threads1, and existing
comparison scripts. Contact sheets are lossless WebP; full original videos remain in their
owned run outputs, identified by SHA256 in numeric-review.json. No GPU action was performed
by this review lane.
