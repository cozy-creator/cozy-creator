# TensorFS Python API

Open a model and read its tensors. TensorFS fetches missing data into its shared store
automatically; normal loading does not create a snapshot folder. This guide covers the
Python API, with optional filesystem and repository tools kept separate. The
[Rust API](../tensorfs/docs/rust-api.md) and [file-format specification](../tensorfs/spec/cozytensors.md)
are separate documents.

- [Import and open a model](#python-import-and-open-a-model)
- [Use the returned objects](#python-use-the-returned-objects)
- [Write, convert and publish](#python-write-convert-and-publish)
- [Configuration and advanced operations](#python-configuration-and-advanced-operations)
- [Errors and value types](#errors-and-value-types)

The [type reference](API-TYPES.md) lists every declaration.

## Python: import and open a model

Install the Python package, then import it:

```bash
uv pip install tensorfs
```

```python
import tensorfs

checkpoint = tensorfs.open("me/model", release="1.0", lane="bf16")
weight = checkpoint.components["unet"].tensors.read("conv_in.weight")
```

In these examples `me/model` stands for a repository you can read, with `bf16` and `int8` lanes.

`open` returns a `Checkpoint`. By default (`lazy=True`), it loads metadata; reading
tensors then downloads the objects those tensors need. `release` names a release (none: the
newest) and `lane` its commit of that lane; without a lane the endpoint picks what `gpus` run best
(omitted: the visible GPUs; `[]`: the CPU). `"me/model@sha256:<tree>"` names a commit by id. No separate
download call is required. The same `open` accepts a local `.cozytensors` header or an
explicitly materialized folder; those are optional input forms.

To download the complete checkpoint into the shared store before continuing:

```python
checkpoint = tensorfs.open("me/model", lane="bf16", lazy=False)
```

This waits for all declared chunks and component assets. It does not create a folder,
unpack quantized weights or place them on a GPU. To preload only selected components,
open lazily and call `checkpoint.download(components=["unet"])`.

For a private model, pass `token=...`. Other options are `endpoint`, `cache_dir` and
`local_files_only`. `local_files_only=True` forbids network access and fails if the required
data is unavailable locally. A private CA is set once, with `tensorfs.configure(ca_der=...)`.

## Python: use the returned objects

### Inspect the pipeline and components

```python
print(checkpoint.pipeline.cls if checkpoint.pipeline else "no pipeline")
print(list(checkpoint.components))

unet = checkpoint.components["unet"]
print(unet.cls)
print(unet.config)
```

A `Pipeline` has `cls` and `config`. A `Component` exposes:

| Attribute or call | What you get |
|---|---|
| `name` | Component name, such as `unet`. |
| `cls` | The model class, a `ClassRef` with `library`, `version` and `name` (`ClassRef("", "", "")` when unknown). |
| `config` | Parsed JSON configuration as a dictionary. |
| `config_file` | The file name `config` exports as: `config.json` unless the writer named another. |
| `tensors` | A `Tensors` mapping, described below. |
| `assets` | Supporting filenames mapped to `(sha256_hex, byte_length)` references; this does not load file contents. |
| `asset(name)` | Read a supporting file, fetching it if needed; returns `bytes`. |

An asset is a supporting file needed by the component, such as a tokenizer's
`tokenizer.json`, `vocab.json` or `merges.txt`. All of these files belong to the checkpoint;
they are stored by content hash alongside the tensor data. Native consumers read `config`
and file bytes directly through TensorFS; no directory export is part of loading a
cozytensors checkpoint:

```python
tokenizer = checkpoint.components["tokenizer"]
print(list(tokenizer.assets))
vocab = tokenizer.asset("vocab.json")
```

For import/export compatibility, `config_file` is the filename to use when recreating a
component's configuration file. For example, a scheduler can require
`scheduler_config.json` rather than `config.json`. Loading directly from `config` does not
require a filename; conversion and a `Writer(base=...)` rewrite preserve it.

### Inspect tensor metadata without reading weights

```python
print(list(unet.tensors))
info = unet.tensors["conv_in.weight"]
print(info.dtype, info.shape, info.encoding)
```

`Tensors` behaves like a read-only mapping: `len`, iteration, membership, indexing,
`keys()`, `values()`, `items()` and `get(key, default)` work normally. A missing indexed
key raises `KeyError`; `get` returns its default.

The returned `TensorMetadata` describes the whole tensor: its logical `dtype`, `shape`
and optional `encoding`. It contains metadata, not tensor values. Inspect its shape/dtype,
then call `read`. Ordinary reads hide physical chunk locations and byte offsets;
advanced storage integrations can inspect them through `tensorfs.raw` below.

### Read a tensor, or a component's weights

```python
weight = unet.tensors.read("conv_in.weight")
weights = unet.tensors.read()
selected = unet.tensors.read(["conv_in.weight", "conv_in.bias"])
```

`read(keys=None, device="cpu", decode=False)` accepts a tensor name, a sequence of
names, or no selection. A single name returns one value; a sequence returns a name-to-value
dictionary; no selection (or `None`) reads every tensor in this component. An empty list
returns an empty dictionary without fetching data. Explicit selections keep the requested
order, with duplicate names included once; an all-tensor read uses construction order.
An unknown name raises `KeyError`.

All forms accept `device="cpu"` (the default), `device="cuda"` or `device="cuda:N"`,
and the same `decode` option.

`decode=False` is the default: a plain tensor returns an `Array`, and a quantized tensor
returns `Encoded` with its stored arrays and encoding parameters. Reading does not
automatically expand quantized weights. An `Array` has `dtype`, `shape`, `nbytes` and
`tobytes()`.

With explicit `decode=True`, quantized weights are unpacked into the tensor's declared
logical dtype (`TensorMetadata.dtype`), such as BF16, F16 or F32; BF16 is not forced.
Plain tensors keep their dtype with either setting. The default destination is CPU;
`device="cuda:0"` requests GPU arrays. Supported GPU decoders unpack there; schemes with
a host-only decoder use the CPU fallback.

Decode when the model's compute code needs ordinary floating-point weights. For example,
a standard PyTorch layer can consume a decoded BF16 array, but cannot directly consume
int8 weights together with scales and rotation parameters. A matching quantized compute
kernel can use those encoded arrays directly. Decoding provides compatibility at the
cost of an unpacking step and a larger weight allocation.

`Array` is TensorFS's own array object. It implements DLPack, a standard protocol for
sharing tensor memory between libraries, rather than a separate Python array library.
Frameworks such as PyTorch can wrap a compatible array's existing memory without copying.
For example:

```python
import torch

weight = unet.tensors.read("conv_in.weight", decode=True)
torch_weight = torch.from_dlpack(weight)
state_dict = {name: torch.from_dlpack(value) for name, value in unet.tensors.read(decode=True).items()}
```

On a GPU:

```python
gpu_weights = unet.tensors.read(device="cuda:0", decode=True)
gpu_state_dict = {name: torch.from_dlpack(value) for name, value in gpu_weights.items()}
```

The returned arrays and their framework consumers retain the storage they need.

### Keep quantized weights encoded

```python
quantized = tensorfs.open("me/model", release="1.0", lane="int8").components["unet"]
value = quantized.tensors.read("time_embedding.linear_1.weight")  # decode=False is the default
print(value.scheme, value.params)
print(value.parts.keys())
```

GPU placement does not require decoding:

```python
gpu_value = quantized.tensors.read("time_embedding.linear_1.weight", device="cuda:0")
```

`Encoding` describes how the stored arrays reconstruct the logical tensor: a scheme
name such as `int8.rowwise`, plus its parameters. For example, a logical weight matrix
with shape `(4096, 4096)` can be stored as an int8 array named `weight` with that shape
and a separate scale array named `scale` with shape `(4096,)`. Each row is reconstructed
as `weight[row, column] * scale[row]`.

These are ordinary arrays with their own dtype and shape. Consumers receive their values;
TensorFS handles the underlying chunk locations and byte offsets.

For an encoded tensor, `decode=False` returns `Encoded`: the logical `dtype` and `shape`,
the loaded `scheme` and `params`, and a role-to-`Array` mapping named `parts`. For a plain
tensor it still returns `Array`.
A consumer of encoded weights must understand that scheme and its parameters.
`device` chooses where the arrays live; `decode` chooses their representation. Our
Runtime keeps supported quantized weights encoded for its matching compute kernels.

### Read into an existing allocation

```python
destination = bytearray(weight.nbytes)
unet.tensors.read_into("conv_in.weight", destination, offset=0)
copied_bytes = bytes(destination)

# For stored encoded bytes, size the destination for that part and select its role.
encoded = quantized.tensors.read("time_embedding.linear_1.weight", decode=False)
packed = bytearray(encoded.parts["weight"].nbytes)
quantized.tensors.read_into("time_embedding.linear_1.weight", packed, role="weight", offset=0)

# Or its decoded logical bytes, from byte 64:
window = bytearray(256)
quantized.tensors.read_into("time_embedding.linear_1.weight", window, decode=True, offset=64)
```

`read_into(key, out, *, role=None, decode=False, offset=0)` follows `read`. The destination
determines the number of bytes copied, and `offset` is a byte offset. A plain tensor gives
its data. `role` selects one stored part of an encoded tensor, and `decode=True` gives its
decoded logical bytes. An encoded tensor with neither raises `InvalidError` naming its
roles, and a role with `decode=True` is refused too. This call does not convert to the
destination's dtype; provide the appropriate size and representation.

### Download, verify and manage a checkpoint

```python
def on_progress(stage, completed, total):
    print(f"{stage}: {completed}/{total}")

checkpoint.download(components=["unet"], priority=0, progress=on_progress)
```

`download` returns when all requested chunks and supporting files are verified and locally
available, or raises an error. The open checkpoint retains them; closing its last handle
returns them to the [cache](#the-local-cache).
Progress callbacks receive `(stage, completed, total)`; download counts are locally available
bytes out of requested bytes, while conversion counts are tensors.

For optional availability inspection or an explicit integrity audit:

```python
missing = checkpoint.missing(components=["unet"])
report = checkpoint.verify(components=["unet"])
print(report.ok, report.missing, report.corrupt)
```

`missing` checks local availability without downloading or rehashing. `verify` re-reads
and hashes local objects, returning a `VerificationReport` with separate `missing` and
`corrupt` object-reference lists. `ok` is true when both are empty. Verification never
downloads, repairs or deletes data; other I/O failures raise the usual typed error.
There is no need to call it after every download: incoming data is already verified when
admitted to the store.

All three methods accept the same component selection. `None` covers the checkpoint's
declared chunks and component assets; a component list covers those components' complete
chunks and assets. They do not materialize a repository folder or fetch README/LICENSE
files. Verification always includes the header, even with `components=[]`. A chunk
shared by selected and unselected tensors is checked as one complete stored object.

Additional checkpoint properties and operations:

| Attribute or call | Meaning |
|---|---|
| `id` | Header identity, a `sha256:…` string. |
| `tree` | Commit identity (the repository tree), a `sha256:…` string, when known. |
| `repo_id` | Repository name, when known. |
| `release` | The release it was resolved through, when known. |
| `lane` | Its commit's lane, when known. A standalone header does not store this label. |
| `pipeline`, `components` | Pipeline description and ordered component mapping. |
| `metadata` | Loading/provenance metadata. |
| `chunks` | Chunk object references in header order, as `(sha256_hex, byte_length)` pairs. |
| `close()` | Release this checkpoint handle. Existing child handles remain valid. |

Use `with tensorfs.open(...) as checkpoint:` for automatic close. `Array` implements
`__dlpack__` and `__dlpack_device__`, including stream/device/copy negotiation. Objects
provide normal debug representations (`repr`); an `Encoding` also has a string form.
These representations are not serialization formats.

### Use TensorFS inside an async application

The Python API is synchronous: `open`, `download` and `read` wait for their results.
The native engine can still overlap work internally. In an asyncio application, move
the blocking call to a worker thread so the event loop remains responsive:

```python
import asyncio

async def load_model():
    return await asyncio.to_thread(tensorfs.open, "me/model", lane="bf16", lazy=False)

checkpoint = asyncio.run(load_model())
```

The same pattern works for `checkpoint.download` and tensor reads. Cancelling the
await does not automatically stop an operation already running in the worker thread.

### Advanced: inspect stored ranges

Storage and memory integrations can inspect physical layout without fetching data:

```python
from tensorfs import raw

layout = raw.parts(checkpoint, "unet", "conv_in.weight")
for role, part in layout.items():
    chunk_ref = checkpoint.chunks[part.chunk]
    print(role, part.dtype, part.shape, chunk_ref, part.offset, part.length)
```

`raw.parts` returns `dict[str, raw.Part]`. A plain tensor has one entry with the empty
role `""`; an encoded tensor has its named stored arrays. Each `raw.Part` reports the
stored array's `dtype` and `shape`, a `chunk` index, and byte `offset` and `length` within
that chunk. This call only inspects metadata. Normal model code uses `TensorMetadata`
and `read` instead of managing these ranges.

### Advanced: decode resident encoded parts

`tensorfs.raw.decode_into` is for integrations that already hold encoded parts in memory and own the
destination. It does not open a checkpoint or fetch data. Ordinary model code continues to
use `component.tensors.read(..., decode=True)`. Its signature in `tensorfs.raw` is:

```pyi
def decode_into(
    scheme: str,
    parts: Mapping[str, tuple[str, Sequence[int], Buffer | DLPack]],
    out: Buffer | DLPack,
    *,
    dtype: str,
    shape: Sequence[int],
    params: Mapping[str, object] | None = None,
) -> None: ...
```

Each input is `role: (format_dtype, format_shape, data)`, using the same role names and
encoded representation as `ComponentWriter.put_encoded`. The tuple describes the stored
format; `data` supplies exactly that many contiguous bytes. Its carrier dtype/shape may
differ: packed F4 data can use a U8 carrier, and F8 bytes can use a U8 view. Marshalling
reinterprets those bytes; it does not numerically convert the encoded weights.

`dtype` and `shape` describe the logical result and its final format rounding. `out` must
already have exactly that actual dtype, shape and byte length, with writable contiguous
storage. It is not cast or resized. Primitive buffer outputs support F32 and F16; BF16
outputs use a supported DLPack carrier. For example, this CPU call decodes stored
rowwise INT8 bytes into an existing F32 array:

```python
import numpy as np
from tensorfs import raw

weight = np.array([[1, -2, 3, -4], [5, -6, 7, -8]], dtype=np.int8)
scale = np.array([0.5, 2.0], dtype=np.float32)
out = np.empty((2, 4), dtype=np.float32)

raw.decode_into(
    "int8.rowwise",
    {
        "weight": ("I8", weight.shape, memoryview(weight)),
        "scale": ("F32", scale.shape, scale.tobytes()),
    },
    memoryview(out),
    dtype="F32",
    shape=[2, 4],
    params={},
)
```

Host inputs and a host destination use the existing CPU decoder. The resident
CUDA path supports `int4.convrot`, `w4a8.convrot`, `comfy.asym_w4a8_int8`,
`comfy.w6a8_int8`, `fp8.tensorwise` and `nvfp4`,
with F32, BF16 or F16 output. All parts and the destination must be on the same CUDA
device. Mixed host/device placement and other resident CUDA schemes raise
`UnsupportedError`; there is no CPU fallback or implicit weight upload. Device metadata
conversion and decode scratch may still require temporary allocations.

For NVFP4, packed weights, block scales, global scales and an optional per-column
`pre_quant_scale` are read from the supplied device buffers. The ordinary format rules
still determine nibble order, scale layout and whether global scales multiply or divide;
resident decoding does not invent another quantization convention.

The call is synchronous: success means the output is ready. Do not mutate inputs or
access the destination concurrently with the call. Nonempty input/output spans must not overlap;
duplicate read-only input spans are allowed. The binding holds buffer leases and DLPack
owners through completion and orders CUDA work against their producer streams.

Buffer/descriptor and overlap refusals occur before output writes. Unknown schemes raise
`UnknownSchemeError`; invalid encoding/value cases can raise `DecodeError`. After a
value or runtime error, output contents are unspecified. If CUDA completion cannot be
proved, the binding retains the actual input/output owners rather than freeing storage
still in use; the output is not a valid result.

## Python: write, convert and publish

### Quantize supplied tensor rows

`raw.quantize` runs a versioned native tensor encoder without importing a framework or model.
The `int8-convrot256/1` strategy is the production H256/16 rotation followed by
per-row F32 scales and ties-to-even I8 rounding. Its storage scheme is `int8.convrot`;
the producer revision is separate from the encoding parameters used by readers.

```python
from tensorfs import raw

encoded = raw.quantize(
    "int8.convrot", f32_little_endian_bytes,
    dtype="BF16", shape=[rows, columns], strategy="int8-convrot256/1",
    params={"rotation": "kron_regular_h4", "rotation_size": 256},
)
```

The input is immutable `bytes` containing finite little-endian F32 values, regardless of the
logical output `dtype` (F32, F16 or BF16). For ConvRot, the shape is a nonempty matrix whose columns
are divisible by 256. The result is the same `Encoded`/`Array` representation returned by
stored reads; pass its parts to the existing `ComponentWriter.put_encoded` to save them.

`raw.quantize_measured` takes the same arguments and returns `(encoded, measured)`, using the
same native producer once. The statistics dictionary contains `amax`, `max_abs_error`, `sse`,
`energy` and integer `saturated`. It compares the supplied F32 values with the stored representation
before logical F16/BF16 rounding. Sums accumulate in F64; chunked sums may differ by floating
reduction order. `amax` is the maximum of the supplied chunk, not the optional whole-tensor
`tensor_amax` bound. These are numerical acceptance facts, not storage or transfer measurements.
The original `raw.quantize` return type and validation remain unchanged.

The `nvfp4-min-sse/1` strategy preserves Runtime's three-candidate block-scale search,
including clipping and smaller-scale tie breaking. It requires the `nvfp4` scheme with
`block_size=16`, `global_scale="multiply"`, `nibble_order="low_first"` and
`scale_layout="row_major"`. Supply `tensor_amax=` as the finite, nonnegative maximum absolute
F32 value of the **complete tensor**, including when passing one row chunk. Each chunk is
checked against that bound. The statistic is separate from stored encoding params; TensorFS
never silently substitutes a chunk-local maximum. ConvRot refuses `tensor_amax` because its
row scales need no shared statistic.

An all-zero tensor uses NVFP4 global scale 1. A positive maximum whose global scale
underflows to zero or overflows F32 refuses before returning parts. This fixes a production
producer edge case: the old Runtime implementation can emit global scale zero for a tiny
nonzero tensor after a zero-denominator calculation. Such inputs are explicit refusals here,
not a claim of byte parity. The existing Rust RTN encoder remains a separate legacy operation;
`nvfp4-min-sse/1` does not silently select it.

The arithmetic releases the GIL, uses one row of floating scratch for ConvRot or one 16-value
block for NVFP4, and allocates the encoded
output; the binding then copies each part into an aligned `Array`. Callers can supply bounded
complete-row chunks without changing payload or scale
bytes. This operation does not read a checkpoint, resolve a model recipe, collect calibration,
write output or compute quality statistics. Unknown strategies and incompatible inputs refuse;
the existing error family for encoder refusals is `DecodeError`. Cancellation between bounded
calls remains the caller's responsibility. A complete native checkpoint conversion runner is
separate work.

### Save a checkpoint file

`Writer` writes the cozytensors format. To create Safetensors files, first commit the
checkpoint and then use `tensorfs.materialize(..., format="safetensors")`. This writes
weight files plus the separate configuration and supporting files required by that layout.

```python
from array import array
import tensorfs

with tensorfs.Writer("./model.cozytensors") as writer:
    writer.metadata["source"] = "my-training-run"
    writer.pipeline(("my_package", "1.0", "MyPipeline"), {"version": 1})
    model = writer.component("model", cls=("my_package", "1.0", "MyModel"), config={"width": 2})
    model.put("weight", array("f", [1, 2]), dtype="F32", shape=[2])
    model.asset("vocab.txt", b"one\ntwo\n")

saved = writer.commit()
print(saved.id)       # immutable header identity
print(saved.tree)     # repository-tree identity, when present
```

The context commits on normal exit. `commit()` returns a `Checkpoint` and returns the same
result on later successful calls. A raw Python buffer needs explicit `dtype` and `shape`;
a DLPack input supplies them, and contradictory explicit values are refused.

A `.cozytensors` file is a small handle into the shared local store, not a self-contained
file. The destination gets only the header; tensor and supporting-file bytes are admitted
once into the store:

```text
./model.cozytensors          # binary metadata and content-hash references
$TENSORFS_HOME/blobs/...     # shared tensor and supporting-file bytes
```

The header contains pipeline/component configuration, tensor names, dtypes, shapes,
encodings and references. Store paths, credentials and endpoint URLs stay in configuration,
not in the file. Opening it uses the configured store and, when needed and allowed, the
configured source.

The store keeps the saved data while that file exists and holds this header. `open(path)` on a
header file holds its data the same way where it is opened, so a moved or copied file is held
once it is opened. Without such a file the data is an ordinary [cache](#the-local-cache) entry;
open checkpoints keep their bytes meanwhile. To copy a checkpoint to another machine, write the
portable form with `tensorfs.materialize(saved, local_dir=...)` (described below).

The file identifies one checkpoint, not an enclosing repository tree. It contains no release
label or repository README/LICENSE files; those remain available on a checkpoint opened
through its repository.

`Writer(dest=None, ...)` keeps the checkpoint in the shared store as a cache entry, without a
header file. `dest` is a literal file path, not a directory. A `repo_id` upload names its
commit's `lane`.

Its options and members are:

| Option or member | Use |
|---|---|
| `lane` | The uploaded commit's lane; with `repo_id`. |
| `base` | Start from an existing checkpoint and reuse unchanged data. |
| `resume` | Resume keyed local write state. |
| `repo_id`, `endpoint`, `token` | Stream the new checkpoint to a repository using supplied access. |
| `cache_dir` | Choose the local store/cache. |
| `metadata` | Mutable loading/provenance metadata dictionary. |
| `local_checkpoint` | A committed local result retained after an upload failure, if one exists. |
| `pipeline(cls, config)` | Set the pipeline description; `cls` is a `ClassRef` or a `(library, version, name)` tuple. |
| `component(name, cls=..., config=..., config_file=None)` | Create or reopen a component and return its `ComponentWriter`. `config_file` names the file its config exports as (`scheduler_config.json`); the default keeps a reopened or base component's, else `config.json`. |
| `has(component, key)` | Check a current/resumed write, excluding untouched tensors inherited from the base. |
| `commit()` | Finish and return the checkpoint; a first commit error is cached and re-raised. |
| `abandon()` | End without producing a new checkpoint. |

An exceptional context exit abandons the writer unless a resume key requests retained state.
An upload failure remains an error; it can also carry `error.checkpoint` and populate
`writer.local_checkpoint`. That result can be published again without rerunning training for as
long as the [cache](#the-local-cache) keeps it: 7 days after its last use.

A `ComponentWriter` supports:

| Call | Use |
|---|---|
| `put(key, data, dtype=None, shape=None)` | Write plain values from a buffer or an array implementing DLPack, such as a Torch tensor. |
| `put_encoded(key, dtype=..., shape=..., scheme=..., params=..., parts=...)` | Preserve encoded data; each part is `role: (dtype, shape, buffer)`. |
| `copy(key, source_component, as_key=None)` | Reuse source key `key`, optionally renaming its destination. |
| `asset(name, data)` | Write a supporting file. |
| `order(keys)` | Set tensor construction order. |
| `has(key)` | Check a current/resumed write. |

### Import an existing model

```python
checkpoint = tensorfs.convert_to_cozytensors("hf://hf-internal-testing/tiny-sdxl-pipe")
print(list(checkpoint.components))
```

`convert_to_cozytensors` imports an existing model into the cozytensors format and shared
store, then returns a `Checkpoint` you can read normally. It accepts one source or a
sequence: supported local files/folders, Hugging Face, Civitai and HTTPS. Options are
`converter`, `variant`, `cache_dir`, `token` and `progress`.
For a Hugging Face source, `hf://owner/model@revision` selects an upstream tag, branch or
commit. Without `@revision`, the importer resolves Hugging Face's `main` revision. It pins
and records the exact upstream commit automatically before importing; no separate
resolution call is needed. These are upstream source revisions, not Tensorhub branches.

## Python: configuration and advanced operations

### Choose defaults and credentials

```python
tensorfs.configure(endpoint="https://tensorhub.com", cache_dir="./tensorfs-cache", token=my_token,
                   ca_der=private_ca_der)
print(tensorfs.__version__)
```

`my_token` in the examples is a credential you obtained for your endpoint, and `private_ca_der` your CA's DER bytes.

`configure` optionally sets process-wide defaults for future calls. It returns `None`,
does not write a configuration file and does not change existing checkpoint handles.
You can instead pass settings directly to each operation:

```python
checkpoint = tensorfs.open("me/model", lane=16, endpoint="https://tensorhub.com", token=my_token)
```

The default endpoint is `https://tensorhub.com`; environment settings include
`TENSORFS_ENDPOINT`, `TENSORFS_HOME` and `TENSORFS_TOKEN`. Public reads need no account.

`configure` updates only non-`None` arguments. Its token applies to open, materialize,
`download_file`, `upload_folder` and `Writer`; pass provider-specific tokens explicitly to
`convert_to_cozytensors`. `ca_der` (DER bytes) adds a private CA as a trusted root, on top of
the system roots, for every later call in the process; no other call takes it. An explicitly
constructed `Endpoint` has its own `token` and `ca_der`.

For refreshable access, supply a callable or object whose `headers(method, url, headers,
payload)` returns request headers. `payload` is the known body SHA-256, not body contents.
An optional `retry(method, url, headers, payload, status, response_headers)` returns whether
to try again after a response. These are duck-typed protocols, not classes you must inherit.
Progress and repository-change callback exceptions can propagate normally. An ordinary
exception from a token provider is wrapped as `DeniedError`; a TensorFS exception retains
its family. Credentials do not follow cross-origin redirects.

### The local cache

Every process on a machine shares one store: `cache_dir=`, else `TENSORFS_HOME`, else
`~/.cache/tensorfs`. Local copies expire 7 days after their last use, and the least recently
used are evicted first when the disk runs low. Header files (written by `Writer(dest)` or
opened by path) and `local/` references keep their data while they exist. There is no public
cache API.

### Optional filesystem escape hatches

These operations are for tools that require paths or portable folders. They are not
steps in the normal `open`/`read` workflow.

#### Materialize a checkpoint or repository

`materialize` creates a regular folder and returns its path. Pass an open checkpoint to
write that checkpoint, or a repository name to reproduce repository content:

```python
folder = tensorfs.materialize(saved, local_dir="./portable")
st_folder = tensorfs.materialize(
    saved, local_dir="./safetensors-model", format="safetensors", decode=True
)
repo_folder = tensorfs.materialize("me/model", release="1.0", lane="bf16", local_dir="./repo")
```

The default `format="cozytensors"` writes a native portable folder: `model.cozytensors`
plus `chunks/`, retaining the same header bytes and content hashes.
`format="safetensors"` writes Safetensors weights with ordinary configuration and supporting
files. It retains supported encoded representations, or with `decode=True` writes each
tensor in its logical dtype. Native cozytensors materialization preserves encodings and rejects
`decode=True`.

Materialization fetches required missing objects or fails; it does not silently create an
incomplete output. A checkpoint produces its own data and any known repository files.
A bare header cannot supply an unknown README or LICENSE. With a repository name in native
mode, `components` can select which components' payloads are materialized.

Supply `local_dir` for a checkpoint or for Safetensors output. A checkpoint already has
its source and credentials, so repository-selection/configuration keywords do not apply.
Safetensors output does not accept `components`. Native repository mode also accepts
`release`, `lane`, `components`, `local_files_only`, `cache_dir`, `endpoint` and `token`; without `local_dir`, it returns a managed snapshot folder.

#### Export for file-based libraries

Direct TensorFS/cozytensors loading is the normal integration path, including in our
Runtime. Libraries should consume component configuration, supporting-file bytes and
tensor arrays directly. Export is a convenience for libraries that still require
filenames; it is not a required preparation step.

```python
# Only for a consumer that requires a directory:
from transformers import CLIPTokenizer

tokenizer = CLIPTokenizer.from_pretrained(checkpoint.components["tokenizer"].export_files())
folder = checkpoint.components["tokenizer"].export_files("./tokenizer-files")
```

`Component.export_files(directory=None)` writes configuration (as `config_file`) and
supporting files, fetching any missing asset bytes, and returns the folder's path. It never
exports tensor weights. Without a directory it writes to a folder the cache manages and
evicts like its other entries; with one it writes there and leaves it to you. The
full-checkpoint `tensorfs.materialize` operation also writes weight files when that is
needed.

#### Download files or publish a folder

```python
license_path = tensorfs.download_file("me/model", "LICENSE")
commit = tensorfs.upload_folder(repo_id="me/model", folder_path="./portable", lane="bf16", token=my_token)
```

`download_file` returns one file path and accepts release, lane, location and authentication
options, without component selection.

`upload_folder` uploads the folder (one `.cozytensors` file and its repo files) as a new commit
in `lane` and returns its id. It never publishes: a release is `cozy model publish`.

### Resolve a model

```python
endpoint = tensorfs.Endpoint("https://tensorhub.com", token=my_token)
resolved = endpoint.resolve("me/model", release="1.0", gpus=["8.9"])
```

`Endpoint(url, token=None, ca_der=None, cache_dir=None)` provides `resolve(repo_id, release=,
lane=, gpus=)`: `{release, lane, commit, header, reasons}`, without opening or caching it.

### All Python entry points at a glance

| Entry point | Returns |
|---|---|
| `configure` | `None` |
| `open` | `Checkpoint`; `lazy=True` loads metadata, `lazy=False` waits for all checkpoint data locally |
| `convert_to_cozytensors` | `Checkpoint` |
| `materialize` | Local folder path; native cozytensors or Safetensors output |
| `download_file` | Local file path |
| `upload_folder` | The commit id |

Normal loading uses `open` followed by component tensor reads. Use
`checkpoint.download(components=...)` to preload data into the shared store;
`checkpoint.missing(components=...)` inspects availability, and
`checkpoint.verify(components=...)` returns a `VerificationReport` for an explicit local
integrity audit. `materialize` and `download_file` are optional filesystem conveniences.
All of these Python calls are synchronous.

The returned data types are `Checkpoint`, `Pipeline`, `Component`, `Tensors`,
`TensorMetadata`, `Encoding`, `Array`, `Encoded` and `VerificationReport`. Writing uses `Writer` and `ComponentWriter`;
repository references use `Endpoint`. They are covered above. Frameworks own tensor allocation and
residency; use `Tensors.read_into` to fill their writable buffers or DLPack arrays.

## Errors and value types

Catch `tensorfs.Error` to handle TensorFS failures; inspect `.code` and `.detail`.
More specific Python exception classes are:

| Family | Python exception |
|---|---|
| Malformed request, document or value | `InvalidError` |
| Wrong document kind or major version | `FormatError` |
| Missing object/reference | `NotFoundError` |
| Hash/length mismatch | `IntegrityError` |
| Unknown encoding | `UnknownSchemeError` |
| Decode failure | `DecodeError` |
| Storage I/O | `StorageError` |
| Insufficient storage | `DiskFullError` |
| Network | `NetworkError` |
| Permission | `DeniedError` |
| Unsupported operation | `UnsupportedError` |
| Concurrent update | `ConflictError` |

Ordinary argument and mapping errors can still be `TypeError` or `KeyError`;
progress and repository-change callbacks can propagate their own exceptions.

Loading metadata and encoding parameters accept booleans, integers in the signed-64 through
unsigned-64 range, strings, lists and string-keyed maps. They do not accept floats, `None`
or bytes. Pipeline/component configuration is ordinary JSON, including finite floats and
null values.

### Native checkpoint quantization

`Writer.normalize_bf16` writes a resolved selection as ordinary BF16 tensors through
the same native reader, conversion control, staging and Writer transaction:

```python
stats = writer.normalize_bf16(source, [{
    "component": "net", "key": "proj.weight",
    "source_dtype": "F32", "shape": [128, 256],
}], max_buffer_bytes=8 << 20, max_new_bytes=128 * 256 * 2,
   control=tensorfs.ConversionControl())
```

The fixed `bf16-rne/1` operation accepts plain F16/F32 of any rank, including scalars
and empty tensors, and parameter-free rank-2 `fp8.rowwise`/`int8.rowwise` inputs with
floating logical dtypes. It decodes rowwise values directly to F32, then applies the
existing BF16 round-to-nearest-even cast. Plain BF16 and other inputs are preserved
by the caller's selection policy, not silently recast. Nonfinite sources refuse.
Round-trip max error must be finite and at most `amax / 256`; BF16 overflow and tiny
values that violate that exact threshold refuse before publishing the tensor.

Supply the same full selection on resume, including completed keys. The existing
journal binds normalization separately from encoded quantization and reserves both
operations' full tensor sets, so they can coexist only for disjoint keys. A pending
tensor cannot be claimed by the other operation after interruption. There is no new
checkpoint format or conversion worker. A legacy journal without reserved keys can
resume its own operation, but cannot admit a second operation whose disjointness
cannot be established. This method never commits the checkpoint.

The buffer grant covers source/decode/output windows and bounded numerical scratch.
Plain inputs stream by element windows; encoded inputs use complete source rows and
refuse if a row cannot fit. Selection/checkpoint metadata and the existing reader,
store and Writer I/O buffers retain their own lifetimes outside this working-buffer
metric. Output grants count newly written BF16 payload bytes before deduplication.
Cancellation uses `ConversionControl` between read/cast/write windows and before
publication; callers must join before closing Output.

Results include newly written `converted_keys`, `reused_keys`, actual stored
`source_bytes_read`, separate `logical_source_bytes_read` for the previous Runtime
counter, `new_bytes_written`, `required_buffer_bytes`, per-tensor RNE error statistics
and `worst_relative_frobenius`. SSE and energy accumulate in F64. Reused tensors have
no recomputed measurements, so aggregate worst is `None` if any were reused.
`raw.normalization_implementation()` supplies the reader/cast/journal implementation
digest for Runtime provenance. An empty selection is an observational no-op.

`Writer.quantize` converts an exact, fully resolved selection using the existing Writer.
It neither selects model layers nor commits the checkpoint. The package/Runtime still owns
recipe selection, calibration that executes a model, output grants and final publication.
The initial route accepts plain F16/BF16/F32 rank-2 weights and the explicit producers
`int8-convrot256/1` and `nvfp4-min-sse/1` with their exact existing format parameters.

```python
control = tensorfs.ConversionControl()
selected = [{
    "component": "unet", "key": "proj.weight",
    "source_dtype": "BF16", "shape": [1024, 1024], "dtype": "BF16",
    "encoding": {"scheme": "int8.convrot", "params": {
        "rotation": "kron_regular_h4", "rotation_size": 256,
    }},
    "strategy": "int8-convrot256/1",
}]
writer = tensorfs.Writer("output.cozytensors", base=source, resume="resolved-job-key")
stats = writer.quantize(source, selected, max_buffer_bytes=8 << 20,
                        max_new_bytes=2 << 20, control=control)
output = writer.commit()
```

Supply the **full same selection** on resume; the existing journal binds it to the source
header, inherited base, numerical revisions/implementation digest and quality bound before trusting completed keys. The buffer
size may change; changing the source, selection or numerical recipe refuses. Inherited
unselected tensors/config/assets retain the ordinary base behavior. Output budget counts all
new encoded payload/companion bytes before store deduplication, matching an output grant;
physical disk growth is a separate quantity. A repeated completed selection reads/writes zero.

The native loop releases the GIL. Another thread can call `control.cancel()` or inspect
`control.progress()` (source bytes, completed tensors, bytes supplied to Writer); observations never drive or acknowledge
work. Keep the caller/request alive until the native call returns. Commit/abandon/other writes
refuse while conversion owns the Writer. Cancellation stops between bounded windows and before
publication; an already-started filesystem operation may finish. Complete earlier tensors stay
in the existing resume journal; an interrupted current tensor is recomputed. No partial tensor
is a completed entry. Temporary encoded parts live in that Writer's existing work directory.

`component="name"` optionally executes just that component of the **same full selection**.
Each call supplies the identical selection and calibration checkpoint; the execution filter
is excluded from journal identity. This permits component-at-a-time callers to share one
Writer without fitting future components early. An unknown component refuses before any
write. Result counters and the working/output grants apply only to the executed component;
unfiltered calls execute the full selection as before.

The buffer bound includes simultaneous source, F32 conversion, encoder outputs and numerical
scratch. The existing downloader and Writer additionally retain their own bounded I/O buffers;
it is not a process RSS cap. One row that cannot fit refuses. NVFP4 reads the whole tensor in
bounded windows to establish a finite maximum, then makes the bounded encoding pass. Both reads
are counted. No expanded whole-tensor code array or unbounded companion buffer is retained.

`stats` reports identity, encoded/reused keys, source bytes, logical new bytes, saturation,
worst relative Frobenius and per-tensor acceptance facts. Reused entries have no recomputed
quality measurement, so aggregate worst is `None` if any were reused. Bytes match the primitive
producer; SSE/energy use compensated F64 accumulation and may vary at reduction-rounding precision with chunk
size or the NumPy reference. Max error, finiteness, saturation and the production round-trip
tripwires are preserved; optional `max_relative_frobenius` refuses before publishing that tensor.
This is an implementation candidate: real default-CLI conversion/parity/resume proof is required
before Runtime producer cutover. Encoded-source conversions and calibrated recipes are separate.

TensorFS0.15.5 extends native checkpoint quantization input to parameter-free rank-2
`fp8.rowwise` (F8_E4M3/F8_E5M2 codes) and `int8.rowwise` (I8 codes), with F32/F16/BF16 row
scales. The immutable checkpoint's stored roles/geometry are checked by the existing format
registry. Bounded code and scale windows reconstruct directly into F32 using that decoder,
without narrowing through logicalBF16/F16 metadata. No new decoder or resident-device capability
is introduced. Buffer admission includes decoder I32/F32 temporaries. Read counters count stored
weight+scale bytes per pass. Resume identity includes the reader/shared decoder implementation;
previous native conversions under another implementation refuse reuse instead of mixing bytes.
Runtime admission and producer cutover need the separate real rowwise conversion gate.

#### Completed SVDQ calibration and additional producers

The explicit `fp8-rowwise-rne/1` and `w4a8-convrot256-fixed/1` strategies produce
parameter-free `fp8.rowwise` and `w4a8.convrot` with group size 16 and H256 rotation.
They run in the same bounded Writer loop; the W4A8 codebook is written once per tensor.
The existing format encoders with different fitting rules keep their own identities.

`svdq-nvfp4-r32-faer/1` fits a complete weight using a finished immutable calibration
checkpoint. Runtime owns collection. TensorFS owns smoothing, the eight alpha candidates,
randomized rank-32 fitting (16 oversampling columns, four subspace iterations), BF16
factors, activation scoring and final NVFP4 residual encoding. The stored scheme remains
`svdq.nvfp4`; existing artifacts retain their decoder semantics.

```python
selected = [{
    "component": "net", "key": "proj.weight",
    "source_dtype": "BF16", "shape": [128, 256], "dtype": "BF16",
    "encoding": {"scheme": "svdq.nvfp4", "params": {
        "block_size": 16, "global_scale": "multiply",
        "nibble_order": "low_first", "scale_layout": "row_major",
    }},
    "strategy": "svdq-nvfp4-r32-faer/1",
    "fit": {"seed": 12345, "component": "net",
            "xmax": "proj.weight.xmax", "rows": "proj.weight.rows"},
}]
stats = writer.quantize(source, selected, calibration=calibration,
                        max_buffer_bytes=64 << 20, max_new_bytes=1 << 20,
                        control=tensorfs.ConversionControl())
```

`xmax` must be plain F32 `[K]`; captured `rows` must be plain BF16/F16/F32 `[S,K]`
with at least one sample. Weight dimensions and arithmetic are checked before fitting;
nonfinite data, unsupported encoding/geometry and inadequate workspace refuse. There is
no callback, model object or mutable Python array at this boundary. The native call owns
both checkpoint references until it returns.

The fitting budget includes the complete decoded weight and calibration, current/best
factors, all numerical scratch, encoded companions and bounded read/staging temporaries.
It does not infer a grant from host RAM. Refusals state required bytes. This initial fitter
retains its full decoded input; it is not a streaming large-matrix fitter. Per-tensor `fit`
details and aggregate `required_buffer_bytes` report the admitted working requirement.
Reader/Writer I/O and OS page cache remain outside this conversion-buffer metric.

`measured` and `relative_frobenius` describe the whole logical weight before its final
logical-dtype cast. `fit.residual` describes only the NVFP4 residual and supplies its
existing max-error tripwire. `fit` also records strategy, selected alpha, calibration
error, plain-NVFP4 error and calibration bytes read. Source and calibration reads are
separate result counters; progress reports their sum. Neither is physical disk traffic.
Reused measurements remain unknown rather than fabricated as zero.

Resume binds the calibration checkpoint and complete fit descriptor in addition to the
weight source, strategy and implementation. `raw.quantization_implementation()` returns
the same full producer/source-reader digest as Writer resume for Runtime memo provenance
(a build without store support reports only its raw numerical implementation). A seed names this Rust producer's
sketch; it does not reproduce Torch's CPU/CUDA RNG stream or singular-vector signs.
Cancellation is checked between input windows, numerical passes, candidates and library
factorizations, then before publication. A single QR/SVD call is not interruptible.
CPU conversion, calibrated GPU consumption and model quality require separate release-gate
evidence; source/build success alone establishes none of those outcomes.
