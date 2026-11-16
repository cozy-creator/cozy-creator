A LoRA's baked page is kept, or at least written to disk, on a host short of memory (tfs-299). TensorFS kept a derived page
only where the host tier had free room, never asked idle processes for theirs while the tier was over its room, and wrote a
page to disk only once kept: Flux-2 Turbo on an RTX 4090 pod kept nothing and baked 244–265 blocks (5.9–6.3 s of device time)
in each of five requests in one executor, re-reading the base and its factors from the store. Keeping a derived page is now a
demand on the tier: it lets go of this process's pages of sets the request has not used, least recently used first (where
that makes it fit), then asks idle processes for the rest, whatever the room's sign. With no room even so, the page is copied
out and written from its copy: staging in varena's host books from free room, else the tier's one copy outside them, which a
fill waits for rather than skip the write. The write goes through the store's admission (evicted at write time, refused by
size), and later fills read the page from disk straight onto the card. The tier counts them (`persisted_unkept`,
`unkept_degraded`).
