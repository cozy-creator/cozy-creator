# Convolution tuple configuration: actual CPU proof

Feature f0989b046d29719e43ce8683e06437aacca8ab6f records supported scalar attributes
and flat immutable tuples of those scalars, including Conv2d stride/padding/dilation.
No Tensor/mutable tuple contents enter the key. No native/helper/policy change.

The real CPU baseline reproduces the review exactly: Conv2d1x1x8x8/3x3 output6x6
becomes3x3 after stride(1,1)->(2,2), with identical Parameters/buffers and no key change.
Padding/dilation and nested Conv controls also fail old fingerprinting. Fixed all four
actual key controls pass; eager output matches ordinary Torch exactly, original tuple
restores original key, and unsupported Tensor/mutable tuple is not retained. No fake
graph, CUDA tensor, native Plan or allocator substitute is created for these controls.

Eleven actual CPU/native-metadata clients pass on exact current Python overlay plus
owned copied auditedb40f SDK/core; this is not proof about coordinator771/873 H100.
Commands/source/artifact hashes/return codes are in config-final-manifest.json.
Prior guard/Plan feature bd08f2eed and its C10 red-green/host receipts8e4f845e8 remain.
Native capture/Plan cleanup is source-reviewed and GPU-unrun in this lane.

Prepared/unrun GPU frontend cases cover all three Conv tuples: a valid replay, mutate
configuration, require GraphInvalidated before another actual replay hook, preserve
prior output, restore tuple, and reuse the same definition. Existing capture_escape
mode covers true ordinary weight Storage retention, known Plan deactivation before
refusal and subsequent model Plan reuse. That negative child intentionally retains
exact unproven custody until exit; no physical retirement claim. All GPU execution
remains coordinator-owned; no rentals, machine, native or byte-policy actions occurred.
