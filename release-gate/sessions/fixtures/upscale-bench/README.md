# Upscale bench (se-147)

`degraded/<name>.jpg`: 225 px JPEGs (q50–75, `sources.tsv`) of nine clean pictures: five Tyson Tan mascots (flat
cel shading, black outlines, white ground, like Pokémon official art), Hokusai's wave, a David Revoy painting and two
photos, all Wikimedia Commons (licence and author in `sources.tsv`). `make_set.py --fetch --clean-only` rebuilds
`clean/` (2048², the ground truth) from the originals, each checked by sha256. `metrics.py <runs>` scores
`<runs>/<candidate>/<name>.png` against it (PSNR, LPIPS, DISTS) and without it (MUSIQ, CLIP-IQA); `sheets.py <runs>`
lays them side by side. `../../upscale.sh` runs candidates on a rental and does both.
