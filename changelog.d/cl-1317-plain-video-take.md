`cozy run` takes a plain video file as a pickup's take and as a take to redo, as the page takes an imported video's box:
`pickup.take=./clip.mp4 pickup.out_frame=124`, `video=./clip.mp4`. A take's latents go with it when given
(`pickup.take.latents=<file>`, `video.latents=<file>`), bound by the take's declared type; a function whose take has no latents
says it ignored them. Before, a plain file as `pickup.take` was refused and `video.latents` was dropped as an undeclared field.
