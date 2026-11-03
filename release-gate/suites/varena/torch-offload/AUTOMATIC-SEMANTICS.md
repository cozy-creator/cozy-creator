# Actual default-wrapper semantics gate

`automatic_semantics.py` calls the real public graphs=auto API. No graph, event,
native resource or module forward is substituted. It checks reviewed Identity
returns the identical input; a custom parameter-owning module returns its original
dict/tuple/None plus duplicate Tensor/view aliases; a custom RNG module advances
its own REAL generator exactly once per request; and in-place ReLU mutates/returns
its actual input. Custom Python state/call counters must increment once only.

The RNG control uses two independently owned, equally seeded real generators.
Neither is rewound/restored during execution. Extra authored warmup would change
the sequence and fail the comparison. No RNG capture eligibility is inferred.

```sh
python release-gate/suites/varena/torch-offload/automatic_semantics.py --device cpu
# Assigned operator's isolated guarded ordinary-Cozy child, matched SDK:
python release-gate/suites/varena/torch-offload/automatic_semantics.py --device cuda:0
```

CPU mode proves actual eager wrapper/return/RNG behavior with CUDA uninitialized.
The GPU mode additionally requires zero captures/replays/cache registrations;
Identity has exactly one cold alias refusal, unknown custom modules and in-place
ReLU have no cold observations. Their reasons must remain the bounded effect/alias
exclusions. GPU assertions are prepared/unrun here. This does not replace the strict
two-block first gate or the separately owned four-mode native overlap clients.

Typed admission is now SOURCE-COMPOSED by root, not an unimplemented blocker.
Native dcbc141b2 and adapter0883 use the exact public RuntimeError subclass only at
the existing overlap entry branch. Matching native import/dispatch and GPU matrix
proof remain separately pending; no new torch.py is overlaid onto an old extension.
