"""Exceptions raised by the Checkgate client."""

from __future__ import annotations


class CheckgateError(Exception):
    """Base class of every exception the client raises."""


class CheckgateAuthError(CheckgateError):
    """The server rejected the SDK key (HTTP 401 or 403).

    The client stops retrying: a rejected key does not become valid by asking
    again. Flags already loaded keep being served. Call ``connect()`` again to
    retry, e.g. after the key was restored.
    """

    def __init__(self, status: int) -> None:
        super().__init__(
            f"[Checkgate] The server rejected the SDK key (HTTP {status}). "
            "Not retrying; check the key, then call connect() again."
        )
        self.status = status


class CheckgateTimeoutError(CheckgateError, TimeoutError):
    """``connect()`` did not complete within its timeout.

    The client keeps connecting in the background; flags arrive as soon as
    the server is reachable. Also a ``TimeoutError``.
    """


class CheckgateClosedError(CheckgateError):
    """The client was closed: ``close()`` is terminal for syncing.

    Raised by ``connect()`` after ``close()``, and by a ``connect()`` that was
    still waiting when ``close()`` was called.
    """


class CheckgateForkError(CheckgateError):
    """The client was created in a parent process and used after ``os.fork()``.

    A forked child inherits the client's memory but not its background thread,
    and possibly locks that a parent thread held at the instant of the fork.
    Create the client in each worker process instead (for example in
    gunicorn's ``post_fork`` hook, or lazily on first use).
    """
