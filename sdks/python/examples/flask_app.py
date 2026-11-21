"""Flask: app factory, one client per process, closed at exit.

    pip install checkgate flask
    CHECKGATE_URL=https://flags.example.com CHECKGATE_SDK_KEY=sk_live_... \
        flask --app flask_app run
    # or: gunicorn -w 4 'flask_app:create_app()'   (without --preload; see below)

Flask's ``teardown_appcontext`` runs after every request, so it is the wrong
place to close a long-lived client: close it when the process exits. With
gunicorn ``--preload``, ``create_app()`` runs once in the master before the
workers fork, and a client created there cannot be used in a worker
(``CheckgateForkError``); the lazy ``get_flags()`` below creates the client
on first use inside each worker instead, which works with or without
``--preload``.
"""

import atexit
import os
import threading

from flask import Flask, current_app, jsonify, request

from checkgate import CheckgateClient, CheckgateTimeoutError

_lock = threading.Lock()


def get_flags() -> CheckgateClient:
    """The process's client, created on first use (after any fork)."""
    app = current_app._get_current_object()
    client = app.extensions.get("checkgate")
    if client is None:
        with _lock:
            client = app.extensions.get("checkgate")
            if client is None:
                client = CheckgateClient(app.config["CHECKGATE_URL"], app.config["CHECKGATE_SDK_KEY"])
                try:
                    client.connect(timeout=5)
                except CheckgateTimeoutError:
                    pass  # serve defaults; it keeps connecting in the background
                atexit.register(client.close)
                app.extensions["checkgate"] = client
    return client


def create_app() -> Flask:
    app = Flask(__name__)
    app.config["CHECKGATE_URL"] = os.environ["CHECKGATE_URL"]
    app.config["CHECKGATE_SDK_KEY"] = os.environ["CHECKGATE_SDK_KEY"]

    @app.get("/")
    def index():
        user_id = request.args.get("user", "anonymous")
        enabled = get_flags().is_enabled("new-homepage", user_id, {"plan": request.args.get("plan", "free")})
        return jsonify(homepage="new" if enabled else "classic")

    @app.get("/health/flags")
    def flag_health():
        return jsonify(get_flags().get_health())

    return app
