"""Offline persistence of the last flag set.

A storage adapter is any object with ``get_item(key) -> str | None`` and
``set_item(key, value) -> None`` (the shape of the other SDKs' storage
option). Both are called from the client's background thread, never from an
evaluation. Errors are logged and ignored: persistence is best-effort.
"""

from __future__ import annotations

import hashlib
import os
import tempfile
import threading
from pathlib import Path
from typing import Optional, Protocol, Union, runtime_checkable


@runtime_checkable
class Storage(Protocol):
    def get_item(self, key: str) -> Optional[str]: ...

    def set_item(self, key: str, value: str) -> None: ...


class MemoryStorage:
    """Keeps the snapshot in memory (for tests, or to share between clients
    of one process)."""

    def __init__(self) -> None:
        self._items: dict[str, str] = {}
        self._lock = threading.Lock()

    def get_item(self, key: str) -> Optional[str]:
        with self._lock:
            return self._items.get(key)

    def set_item(self, key: str, value: str) -> None:
        with self._lock:
            self._items[key] = value


class FileStorage:
    """One JSON file per cache key in ``directory``.

    File names are a SHA-256 of the key, so no part of the server URL appears
    in them (the key itself already hashes the SDK key). Writes go to a
    temporary file in the same directory and are renamed over the old one, so
    a reader (another process too: several workers may share the directory)
    sees the previous snapshot or the new one, never a partial file. The last
    writer wins. Files are created readable by the owner only.
    """

    def __init__(self, directory: Union[str, os.PathLike[str]]) -> None:
        self.directory = Path(directory)

    def _path(self, key: str) -> Path:
        return self.directory / f"checkgate-{hashlib.sha256(key.encode('utf-8')).hexdigest()[:32]}.json"

    def get_item(self, key: str) -> Optional[str]:
        try:
            return self._path(key).read_text(encoding="utf-8")
        except FileNotFoundError:
            return None

    def set_item(self, key: str, value: str) -> None:
        self.directory.mkdir(parents=True, exist_ok=True)
        target = self._path(key)
        fd, tmp = tempfile.mkstemp(prefix=".checkgate-", suffix=".tmp", dir=self.directory)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                f.write(value)
                f.flush()
                os.fsync(f.fileno())
            os.replace(tmp, target)
        except BaseException:
            try:
                os.unlink(tmp)
            except OSError:
                pass
            raise


def cache_namespace(value: str) -> str:
    """SHA-256 hex of ``value`` (``sdks/nodejs/cache-key.js``): namespaces
    persisted snapshots without putting credentials in storage keys."""
    return hashlib.sha256(value.encode("utf-8")).hexdigest()
