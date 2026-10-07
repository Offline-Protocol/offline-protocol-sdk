"""The HTTP front: call a service on another device with plain HTTP.

See ``docs/spec/http-front.md``. Needs the optional ``http`` extra
(``pip install 'offline-protocol-sdk[http]'``), imported when the front
starts, so importing this package costs nothing without it.
"""

from .envelope import FRONT_APP_ID
from .front import HttpFront
from .hostname import DEFAULT_DOMAIN, Aliases

__all__ = ["DEFAULT_DOMAIN", "FRONT_APP_ID", "Aliases", "HttpFront"]
