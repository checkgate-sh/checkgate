"""gunicorn settings for running an app with ``--preload`` (or ``preload_app``)
and Checkgate: create the client after the fork, in each worker."""

preload_app = True
workers = 4


def post_fork(server, worker):
    # Django: django_app.flags.start(); Flask: nothing, get_flags() is lazy.
    from django_app import flags

    flags.start()
