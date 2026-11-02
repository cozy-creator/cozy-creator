# Shared Python owner and prior native host receipts

Current Python binding/native check passes53.93s and both staged fixture scripts
pass Ruff/AST checks. Real GPU execution remains unrun. The paired Runtime has
zero Ruff findings and no new scoped mypy findings relative to61fe3233.

The separately labeled retirement-host receipts bind exact native49685. The first
output-only attempt selected an older default Rust and failed on atomic_try_update;
the corrected explicit installed1.99.1 run passed16 concurrent queue preparations,
three real idle workers, startup outside the held capture mutex and real fork
rejection before the inherited queue mutex. No GPU owners or CUDA/NVML mapping
were constructed. These are host startup/fork results, not GPU retirement or the
subsequent FillResources composition qualification. Frozen49685 was not edited.
