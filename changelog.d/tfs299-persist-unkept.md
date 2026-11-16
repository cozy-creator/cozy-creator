A LoRA'd page the host tier has no room to keep is still written to disk (tfs-299). TensorFS wrote a derived page only once
the tier kept it, so on a host short of memory nothing was kept or written, and every fill, request and process baked the
LoRA again and re-read the base and its factors from the store: Flux-2 Turbo on an RTX 4090 pod baked 244–265 blocks (5.9–6.3 s
of device time) in each of five requests in one executor. Such a page is now copied out and written from its copy: the copy
is staging in varena's host books while free room allows, else the tier's one copy outside them, which a fill waits for
rather than skip the write. The write goes through the store's admission (evicted at write time, refused by size), and later
fills read the page from disk straight onto the card. The tier's stats count them (`persisted_unkept`, `unkept_degraded`).
