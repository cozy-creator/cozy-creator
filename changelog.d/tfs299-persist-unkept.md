A LoRA'd page the host tier has no room to keep is still written to disk (tfs-299). TensorFS wrote a derived page only once
the tier kept it, so on a host short of memory nothing was kept or written, and every fill, request and process baked the
LoRA again and re-read the base and factors from the store: Flux-2 Turbo on a 4090 baked 244–265 blocks (5.9–6.3 s of
device time) in each of five requests in one executor. Such a page is now copied out and written from its copy, at most
512 MiB of copies at once (one that would pass that is written at a later fill), and later fills read it from disk straight
onto the card. The tier's stats count them (`persisted_unkept`).
