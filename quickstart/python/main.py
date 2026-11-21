"""Evaluates `new-checkout` for two users every second, locally, and prints
when the answer changes. Toggle the flag in the dashboard and watch."""
import os
import time

from checkgate import CheckgateClient

USERS = {"user-pro": {"plan": "pro"}, "user-free": {"plan": "free"}}

with CheckgateClient(
    f"http://localhost:{os.environ.get('CHECKGATE_PORT', '3000')}",
    os.environ["CHECKGATE_SDK_KEY"],
) as client:
    client.connect(timeout=10)
    print("Connected. Toggle new-checkout in the dashboard (Ctrl-C to stop).")
    last = ""
    try:
        while True:
            now = "  ".join(f"{u}={client.is_enabled('new-checkout', u, a)}" for u, a in USERS.items())
            if now != last:
                print(f"{time.strftime('%H:%M:%S')}  new-checkout: {now}")
                last = now
            time.sleep(1)
    except KeyboardInterrupt:
        pass
