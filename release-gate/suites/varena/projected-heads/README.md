# Actual H3 projected-head regression

Fixture work in progress for xs-138. It will construct actual package `_Heads`, encoded Q/K/V projections, real Rows/baked hooks, compiled norm/RoPE and key-mean entries, and the installed INT4QK ABI inside an actual Runtime Request/phase. Whole and constrained head choices must preserve output and seed values, refuse at minimum capacity before mutation, avoid legacy retries after acceptance, and retire native ownership.

No executable fixture or GPU qualification is claimed by this planning commit. Frozen native base is the independently qualified bounded-trim artifact b20f699c; this branch does not change allocator source.
