"""The reference server for the local API (``docs/spec/local-api.md``).

One process owns one engine and serves any number of local applications over
JSON-RPC 2.0 on a WebSocket, on a Unix domain socket by default. See
:class:`LocalApiServer`, and ``offline-protocol-service`` for the shell entry
point.
"""

from .authz import METHOD_GROUPS, Policy, ServiceOwnership  # noqa: F401
from .codec import RpcError  # noqa: F401
from .dispatch import BEFORE_HELLO, EXPOSED, ID_RESULTS, PLATFORM, SESSION_METHODS  # noqa: F401
from .mux import HELD_TAGS, HOLD_CAPACITY, EventRouter  # noqa: F401
from .server import API_VERSION, MAX_MESSAGE_SIZE, SERVER_NAME, LocalApiServer  # noqa: F401
from .session import Session  # noqa: F401

__all__ = [
    "API_VERSION",
    "BEFORE_HELLO",
    "EXPOSED",
    "EventRouter",
    "HELD_TAGS",
    "HOLD_CAPACITY",
    "ID_RESULTS",
    "LocalApiServer",
    "MAX_MESSAGE_SIZE",
    "METHOD_GROUPS",
    "PLATFORM",
    "Policy",
    "RpcError",
    "SERVER_NAME",
    "SESSION_METHODS",
    "ServiceOwnership",
    "Session",
]
