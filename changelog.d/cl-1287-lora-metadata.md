Every LoRA uploaded to TensorHub carries its trigger words and its default strength. `cozy lora upload` (and the page's upload)
takes them from `--trigger`/`--strength`, else from its source (Civitai's trained words, a Hugging Face card's `instance_prompt`),
else what the card already has; a LoRA with none from any of these gets no trigger and strength 1.0, and the upload says so in one
line each. `cozy lora ls <org/name> --json` reads a LoRA's triggers and source.
