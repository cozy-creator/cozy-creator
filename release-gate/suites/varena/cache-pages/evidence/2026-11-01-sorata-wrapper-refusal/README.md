# Preserved fixture constructor failure25551

The original dfb fixture failed before the real resize operation: Torch UntypedStorage
rejected Runtime's Device wrapper. The preceding16MiB Cache commit had already happened;
its unproven live backing remains owned until that failed process retires. This is neither
a successful GIL/refusal test nor a clean-zero result. Root owns process retirement.

The fixture-only correction normalizes every direct Tensor/Storage/Stream constructor in
all four parent cells to torch.device("cuda", ctx.device.index or0). Native/helperdfb and
Runtime2626 remain unchanged. The failed raw run and invocation are preserved here.
