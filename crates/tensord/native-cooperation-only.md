# Linux RC native cooperation contract

Every CUDA executor must advertise the current native cooperative owner before Start. Unsupported platforms or an incapable Runtime are refused before CUDA initialization. Start carries exactly one categorical owner descriptor per GPU rank; each rank must acknowledge opening its owner. Import-only and deviceless starts retain an empty vector.

The paired Runtime already enforces this at `internal/executor.py::_initialize_device` (source839ad4662967c43f997340c2a2b0164678bbb6b2): CUDA requires a configured, capable native owner. TensorD no longer chooses Legacy, computes GPU room targets from historical peaks, sends Room pressure commands, adds inferred idle-weight credit, or runs a second GPU pressure controller. Native Varena owns allocation, donation and reclaim decisions.

Driver telemetry and measured request figures remain. Host-memory pressure, disk reclaim, exact idle-worker retirement, cancellation custody, lane scheduling and per-rank validation remain. Explicit warm-level Map changes a requested lifecycle level; it does not authorize TensorD to impose a GPU byte allowance.

The worker-only cut does not change Varena's C ABI or source. The separate Room wire/schema cut is coordinated with Runtime and the SDK. Source and compile checks are bounded regressions; the combined SDXL, Qwen Image2.1, full H3 and multi-GPU paths require actual integrated execution.
