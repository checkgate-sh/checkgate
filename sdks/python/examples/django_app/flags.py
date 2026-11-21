"""The process's Checkgate client."""

import atexit
from typing import Optional

from django.conf import settings

from checkgate import CheckgateClient, CheckgateTimeoutError

client: Optional[CheckgateClient] = None


def start() -> CheckgateClient:
    global client
    if client is None:
        client = CheckgateClient(settings.CHECKGATE_URL, settings.CHECKGATE_SDK_KEY)
        try:
            # Blocks startup up to 5 s for the first flag set; after that,
            # evaluations fall back to defaults until it arrives.
            client.connect(timeout=5)
        except CheckgateTimeoutError:
            pass
        atexit.register(client.close)
    return client
