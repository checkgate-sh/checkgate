"""The examples and tools at least compile (their frameworks are not
installed in the test environment, so they are not run)."""

from __future__ import annotations

import py_compile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
FILES = sorted((ROOT / "examples").rglob("*.py")) + sorted((ROOT / "tool").glob("*.py"))


@pytest.mark.parametrize("path", FILES, ids=[str(p.relative_to(ROOT)) for p in FILES])
def test_compiles(path: Path) -> None:
    py_compile.compile(str(path), doraise=True, cfile=None, optimize=-1, quiet=0)
