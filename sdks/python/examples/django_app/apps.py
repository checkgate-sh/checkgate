"""Django: create the client in ``AppConfig.ready()``, close it at exit.

Add ``"django_app"`` (this package) to ``INSTALLED_APPS`` and set
``CHECKGATE_URL`` / ``CHECKGATE_SDK_KEY`` in settings, then use
``django_app.flags.client`` in views (see ``views.py``).

Multiple workers: each worker process needs its own client. With gunicorn
or uWSGI *without* preloading, every worker imports Django and runs
``ready()`` itself, so this just works. With ``--preload`` (gunicorn) or
``lazy-apps = false`` (uWSGI's default!), ``ready()`` runs once in the master
before forking, and the inherited client raises ``CheckgateForkError`` in the
workers. Then either disable preloading (uWSGI: ``lazy-apps = true``), or
create the client in a post-fork hook (gunicorn ``post_fork`` in
gunicorn.conf.py calling ``django_app.flags.start()``; uWSGI
``@uwsgidecorators.postfork``). ``manage.py runserver``'s autoreloader also
runs ``ready()`` in two processes; only the serving one uses its client.
"""

from django.apps import AppConfig


class CheckgateConfig(AppConfig):
    name = "django_app"

    def ready(self) -> None:
        from . import flags

        flags.start()
