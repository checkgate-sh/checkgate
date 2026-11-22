"""Checkgate feature flags for Python.

Flags are evaluated locally by Checkgate's Rust engine (the same code the
server and every other SDK run) and kept in sync with the server over
Server-Sent Events, with a polling fallback. See ``CheckgateClient``.
"""

from ._client import (
    OPENFEATURE_REASONS,
    CheckgateClient,
    EvaluationDetails,
    Health,
    Status,
    Variant,
    __version__,
)
from ._errors import (
    CheckgateAuthError,
    CheckgateClosedError,
    CheckgateError,
    CheckgateForkError,
    CheckgateTimeoutError,
)
from ._storage import FileStorage, MemoryStorage, Storage

__all__ = [
    "CheckgateAuthError",
    "CheckgateClient",
    "CheckgateClosedError",
    "CheckgateError",
    "CheckgateForkError",
    "CheckgateTimeoutError",
    "EvaluationDetails",
    "FileStorage",
    "Health",
    "MemoryStorage",
    "OPENFEATURE_REASONS",
    "Status",
    "Storage",
    "Variant",
    "__version__",
]
