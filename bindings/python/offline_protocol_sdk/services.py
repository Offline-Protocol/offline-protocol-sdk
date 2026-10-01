"""Application-level service wrappers over the generated ``MeshServices``.

The engine keeps the registry of what this node offers, and it cannot list
it: the FFI has register, unregister, discover, request and respond, and no
enumeration. Anything that needs the list (a DNS-SD publisher, a status page,
a restart that re-announces) keeps a copy. This module keeps that copy once,
beside the calls that change it, so that the copy and the engine's registry
move together: the engine call runs first, and the copy changes only when
the engine accepted.

What is here is the mesh side only. A LAN import under
``docs/spec/dns-sd-mapping.md`` is a claim by an unsigned record, and it is
never registered with the engine through this module or any other: that
would sign, under this node's identity, a claim made by whoever answered on
the segment (invariant 2 of the chapter). :class:`ServiceRecord` carries a
``source`` so that a reader can tell the two apart, and only records with
``source == "local"`` ever reach :meth:`Services.register`.

The response status set is closed. The engine accepts ``ok``, ``not_found``
and ``error``, refuses any other status on send, and drops a response with
any other status on receipt; a status the guide once called
"application-defined" is a response nobody receives. The set is mirrored
here as a literal, pinned by ``test_services.py`` the way the bridge rules
pin every hand-mirrored constant (C5), so that a respond that would be
refused fails here with the reason instead of as an opaque core error.
"""

from __future__ import annotations

import logging
import threading
from dataclasses import dataclass, field
from typing import Any, Callable, Mapping, Protocol

from .offline_protocol import MeshServices, OfflineProtocol

logger = logging.getLogger(__name__)

#: The response statuses the engine accepts (``VALID_SERVICE_STATUSES`` in
#: the services crate). Mirrored as a literal; see the module docstring.
VALID_STATUSES: tuple[str, ...] = ("ok", "not_found", "error")

#: A registration this node made.
SOURCE_LOCAL = "local"
#: An import from an unsigned LAN record (``docs/spec/dns-sd-mapping.md``).
SOURCE_LAN = "lan"


@dataclass(frozen=True)
class ServiceRecord:
    """One service, as this node offers it or as a LAN neighbour claims to.

    ``provider`` is ``None`` for this node's own registrations and the
    claimed address for a LAN import. ``capabilities`` is a copy: a caller
    mutating its own map afterwards does not change what was registered.
    """

    service_id: str
    version: str = ""
    capabilities: Mapping[str, str] = field(default_factory=dict)
    provider: str | None = None
    source: str = SOURCE_LOCAL

    def __post_init__(self) -> None:
        object.__setattr__(self, "capabilities", dict(self.capabilities))

    @property
    def is_local(self) -> bool:
        return self.source == SOURCE_LOCAL


class ServicesListener(Protocol):
    """What a publisher (the DNS-SD bridge) hears from the registry.

    Both are called outside the registry's lock, on the thread that made the
    change, after the engine accepted it. A listener that raises is logged
    and does not undo the registration: the mesh registration is the primary
    and a LAN copy that could not be published is the LAN's loss only.
    """

    def on_registered(self, record: ServiceRecord) -> None: ...

    def on_unregistered(self, record: ServiceRecord) -> None: ...


class Services:
    """Register, unregister, discover, request and respond, with the copy of
    this node's registrations the engine cannot provide.

    Construct it over a live :class:`OfflineProtocol` (``pm.protocol``); the
    generated :class:`MeshServices` is built here. The engine takes a
    registration before ``start()`` as well as after, so a host may register
    everything it offers before it starts.
    """

    def __init__(self, protocol: OfflineProtocol, *, mesh_services: Any | None = None) -> None:
        self._mesh: Any = mesh_services if mesh_services is not None else MeshServices(protocol)
        self._lock = threading.Lock()
        self._local: dict[str, ServiceRecord] = {}
        self._listeners: list[ServicesListener] = []

    # -- the registry ---------------------------------------------------------

    def register(
        self,
        service_id: str,
        version: str = "",
        capabilities: Mapping[str, str] | None = None,
    ) -> ServiceRecord:
        """Register a service this node offers.

        Registering an id again replaces the descriptor, as the engine does;
        listeners hear the old record unregistered and the new one registered.
        Raises the core's error when the engine refuses the id (empty,
        whitespace, over 256 bytes, or a reserved ``__`` prefix).
        """
        record = ServiceRecord(service_id, version, capabilities or {})
        self._mesh.register_service(service_id, version, dict(record.capabilities))
        with self._lock:
            previous = self._local.get(service_id)
            self._local[service_id] = record
            listeners = list(self._listeners)
        if previous is not None:
            self._notify(listeners, "on_unregistered", previous)
        self._notify(listeners, "on_registered", record)
        return record

    def unregister(self, service_id: str) -> bool:
        """Unregister a service. Returns what the engine returns: whether it
        was registered. The copy is kept in step with the engine's answer,
        so an id the engine did not hold is not in the copy afterwards
        either, whatever this side believed."""
        found = bool(self._mesh.unregister_service(service_id))
        with self._lock:
            previous = self._local.pop(service_id, None)
            listeners = list(self._listeners)
        if previous is not None:
            self._notify(listeners, "on_unregistered", previous)
        return found

    def registered(self) -> list[ServiceRecord]:
        """This node's registrations, in registration order."""
        with self._lock:
            return list(self._local.values())

    def get(self, service_id: str) -> ServiceRecord | None:
        with self._lock:
            return self._local.get(service_id)

    # -- the calls that carry no state -----------------------------------------

    def discover(self, service_id: str | None = None) -> str:
        """Broadcast a discovery query; returns the query id the
        ``service_discovered`` events will carry."""
        return str(self._mesh.discover_services(service_id))

    def request(self, provider: str, service_id: str, method: str, body: str) -> str:
        """Send a request to a provider; returns the request id the
        ``service_response_received`` event will carry."""
        return str(self._mesh.send_service_request(provider, service_id, method, body))

    def respond(self, request_id: str, requester: str, service_id: str, status: str, body: str) -> str:
        """Answer a ``service_request_received`` event.

        ``status`` must be one of :data:`VALID_STATUSES`; the engine refuses
        any other and the requester drops any other, so it is refused here
        with the reason.
        """
        if status not in VALID_STATUSES:
            raise ValueError(
                f"status {status!r} is not one the engine sends or a requester keeps; "
                f"use one of {', '.join(VALID_STATUSES)}"
            )
        return str(self._mesh.respond_to_service_request(request_id, requester, service_id, status, body))

    # -- listeners ------------------------------------------------------------

    def add_listener(self, listener: ServicesListener) -> None:
        with self._lock:
            if listener not in self._listeners:
                self._listeners.append(listener)

    def remove_listener(self, listener: ServicesListener) -> None:
        with self._lock:
            if listener in self._listeners:
                self._listeners.remove(listener)

    @staticmethod
    def _notify(listeners: list[ServicesListener], method: str, record: ServiceRecord) -> None:
        for listener in listeners:
            callback: Callable[[ServiceRecord], None] = getattr(listener, method)
            try:
                callback(record)
            except Exception:
                logger.exception("service listener %r failed in %s", listener, method)
