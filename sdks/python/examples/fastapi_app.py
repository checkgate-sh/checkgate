"""FastAPI: one client per worker process, opened in the lifespan.

    pip install checkgate fastapi uvicorn
    CHECKGATE_URL=https://flags.example.com CHECKGATE_SDK_KEY=sk_live_... \
        uvicorn fastapi_app:app --workers 4

Each uvicorn worker imports the app and runs its own lifespan, so each gets
its own client (and one stream). Evaluation is a local, synchronous call
(microseconds): no need to await it or push it to a thread.
"""

import os
from contextlib import asynccontextmanager

from fastapi import Depends, FastAPI, Request

from checkgate import CheckgateClient, CheckgateTimeoutError, FileStorage


@asynccontextmanager
async def lifespan(app: FastAPI):
    client = CheckgateClient(
        os.environ["CHECKGATE_URL"],
        os.environ["CHECKGATE_SDK_KEY"],
        # Optional: serve the last known flags at once on a restart, even if
        # the server is unreachable.
        storage=FileStorage(os.environ.get("CHECKGATE_CACHE_DIR", "/tmp/checkgate")),
    )
    try:
        # Waits without blocking the event loop. On a timeout the app still
        # starts: flags fall back to defaults (or the offline cache) and the
        # client keeps connecting in the background.
        await client.connect_async(timeout=10)
    except CheckgateTimeoutError:
        pass
    app.state.flags = client
    yield
    client.close()


app = FastAPI(lifespan=lifespan)


def flags(request: Request) -> CheckgateClient:
    return request.app.state.flags


@app.get("/checkout")
def checkout(user_id: str, plan: str = "free", client: CheckgateClient = Depends(flags)):
    if client.is_enabled("new-checkout", user_id, {"plan": plan}):
        return {"checkout": "v2"}
    return {"checkout": "v1"}


@app.get("/health/flags")
def flag_health(client: CheckgateClient = Depends(flags)):
    # Sync diagnostics: state, flag count, config age, last error (no keys or user data).
    return client.get_health()
