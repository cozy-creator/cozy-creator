"""Varena's public GPU allocator API.

Gpu owns this process's device memory; Region, working allocations, Cache and Plan
share that native allocator. Low-level consumers must name their actual CUDA stream.
Install tensorfs.varena.torch for Torch allocations. Runtime's source/graph adapter
is private and intentionally not re-exported here.
"""

from .api import (
    Allocation,
    Busy,
    Cache,
    CacheView,
    Capturing,
    Error,
    Exhausted,
    Fault,
    Gpu,
    Invalid,
    Plan,
    Region,
    SourceError,
    Split,
    Unsupported,
    Use,
    core_identity,
    open,
    retirement_stats,
)
from .sources import Source

__all__ = ['Allocation', 'Busy', 'Cache', 'CacheView', 'Capturing', 'Error', 'Exhausted', 'Fault', 'Gpu', 'Invalid', 'Plan', 'Region', 'Source', 'SourceError', 'Split', 'Unsupported', 'Use', 'core_identity', 'open', 'retirement_stats']
