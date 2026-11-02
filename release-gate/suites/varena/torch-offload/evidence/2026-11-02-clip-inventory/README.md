# Stock CLIP inventory declaration regression

Stock SDXL run 26210 completed its 20-step baseline, then rejected the initialized
`text_encoder` declaration during CPU adoption. Transformers 5.17.0 declares the
CLIP no-split classes as a list, then `PreTrainedModel.post_init` converts the
instance declaration to a set (`modeling_utils.py:1310`).

The production change accepts that concrete set type alongside tuple/list. String
member validation remains; declaration order never drives boundary order, which
continues to follow module traversal.

- Baseline source: a247d24a3bb172df790445cc0ad6a06c557d709a, inventory SHA256
  `ef343a7f45421e1faf0999e09aafc5a64287c5000c9c10dff424be6e7d9c6648`.
- Fixed inventory SHA256:
  `973aca63ca767d0754bbbf70ada84a89808618c552d5db7cf45e180e4e4ad011`.
- `clip-red.json`: real tiny CLIPTextModel and CLIPTextModelWithProjection both
  fail with `text_encoder: invalid _no_split_modules declaration`; process exit 1.
- `clip-green.json`: both pass, preserving exact CPU outputs, Parameter identities,
  storage pointers, requires_grad flags and deterministic boundary order; exit 0.
- `inventory-green.json`: existing tied-Parameter/shared-slot/storage-view and
  refusal-preserves-model contract passes, including new tuple/list/set ordering
  and invalid string/dict/iterator/scalar/nonstring-member refusals; exit 0.

Validation used Torch 2.14.1+cpu and Transformers 5.17.0, one CPU thread, nice 19,
`CUDA_VISIBLE_DEVICES='' HF_HUB_OFFLINE=1 OMP_NUM_THREADS=1 MKL_NUM_THREADS=1
OPENBLAS_NUM_THREADS=1 PYTHONDONTWRITEBYTECODE=1`. Existing Torch CPU environment:
`xs138-p4-torch-offload/.task/torch-cpu/bin/python`.
Transformers and tokenizers were read without installation from the existing uv
archive paths `iARbooqMq1gitwIr` and `Owwa8vcIIZ4u0Vh_` through `PYTHONPATH`.

Run `clip_inventory_cpu.py --inventory-source /path/to/_offload_inventory.py`
and `inventory_cpu.py` from the sibling suite directory. The CLIP probe also
supports a CUDA-built Torch package while keeping all model tensors on CPU and
checking CUDA remains uninitialized. Receipts precede that probe-only convenience
and metadata-field addition; the model and inventory assertions are unchanged.

The first green attempt failed because its test expected the older CLIPTextModel
`text_model` prefix. Transformers 5.17.0 puts this model's embeddings and encoder
directly on the root; CLIPTextModelWithProjection retains the nested prefix.
The corrected test derives the three expected CLIP boundaries from the real
model's named module traversal. No production change beyond set acceptance.

These are CPU inventory results; no GPU adoption, native, rental, deployment,
release, or full stock SDXL output parity claim is made here.
