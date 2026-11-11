# CPU checkpoint data

These unchanged tokenizer and normalization files are CPU fixture inputs. Their source is the former package bundles at packages commit `de71b31a32b16e4310a5b1fdb0f7aef6c56485cc`; `provenance.json` records every original path, Git blob, byte count and SHA256. Their existing source and licensing apply without relabeling.

The fixtures copy the data into the generated source model or write it into the checkpoint. Runtime loading reads that checkpoint only. No fixture reads removed Anima/SDXL package bundles or fetches historical files over the network.

Both SDXL tokenizer components intentionally receive the same original CLIP vocabulary and merges; the consumer retains each component's distinct padding behavior. H3's latent normalization values remain the exact original learned mean and standard deviation.
