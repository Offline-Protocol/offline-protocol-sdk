"""DNS-SD for services: publish this node's registrations on the LAN, and
import what the LAN's neighbours publish, under ``docs/spec/dns-sd-mapping.md``.

Three rules from the chapter shape everything here, each with the failure it
prevents:

1. **A LAN record proves nothing.** A mesh discovery response is signed by
   the provider; a DNS-SD record is an unsigned multicast answer from whoever
   is on the segment. Every import leaves this module as a
   ``service_discovered`` event with ``source: "lan"``, and as a
   :class:`~.services.ServiceRecord` with ``source == "lan"``, so that an
   application cannot mistake it for a discovery without ignoring the field.
2. **A LAN record is never re-advertised on the mesh.** Imports go to this
   bridge's own registry and to the application's event handler, and never
   to :meth:`Services.register`. A registration made on the strength of an
   anonymous LAN record would go out in signed discovery responses under this
   node's identity, and every mesh peer would trust the forged claim on the
   strength of that signature.
3. **A field that does not fit is refused, never truncated.** A truncated
   service id names a different service. A registration whose descriptor
   does not fit the record is not published, and the mesh registration is
   untouched; :meth:`DnsSdBridge.publish` returns ``False`` and the log says
   why.

The bridge needs the optional ``zeroconf`` dependency
(``pip install 'offline-protocol-sdk[lan]'``), imported at :meth:`start` so
the base install never imports it. Records are published under the subtype
``_svc._sub._offlineprotocol._tcp`` of the type the peer-stream transport
already uses; a peer-stream browser ignores any record carrying ``sid``, and
this browser only ever browses the subtype.

The bounds are the chapter's, mirrored here as literals and pinned as
literals in ``test_dnssd_bridge.py`` for the reason the bridge rules give
(C5): a test that read them from this module would agree with any edit.
"""

from __future__ import annotations

import asyncio
import hashlib
import logging
import time
from dataclasses import dataclass
from typing import Any, Callable, Mapping

from .peer_stream_manager import SERVICE_TYPE, advertised_addresses, service_host_name
from .services import SOURCE_LAN, SOURCE_LOCAL, ServiceRecord, Services

logger = logging.getLogger(__name__)

#: Service instances live under this subtype of the peer-stream type.
SUBTYPE = "_svc._sub." + SERVICE_TYPE
#: The first TXT entry, and the only mapping version a reader accepts.
TXT_VERSION = "1"
#: The instance name is this prefix and sixteen hex digits of a digest.
INSTANCE_PREFIX = "svc-"
#: RFC 6763 section 4.1.1: an instance name is at most 63 octets.
MAX_INSTANCE_NAME_OCTETS = 63
#: RFC 6763 section 6.1: one ``key=value`` string is at most 255 bytes.
MAX_TXT_STRING_BYTES = 255
#: The chapter's bound on ``sid``: the engine's 256 does not fit one string.
MAX_SID_BYTES = 200
#: RFC 6763 section 6.2: the whole record, one length octet per string.
MAX_TXT_RECORD_BYTES = 1300
#: What a canonical address starts with; a record's ``addr`` must too.
ADDRESS_PREFIX = "off1"
#: How long an import is kept without resolving again, in seconds. Records
#: age out of a browser's cache after 75 minutes; a headless neighbour that
#: died without a goodbye should not be offered for that long.
DEFAULT_TTL = 300.0
#: How long one resolution may take.
RESOLVE_TIMEOUT_MS = 3000

_TXT_KEYS_FIXED = ("txtvers", "sid", "ver", "addr")
_CAPABILITY_PREFIX = "c."


class RecordRefused(ValueError):
    """A descriptor that does not fit the record, and the reason."""


# -- the mapping, as pure functions ---------------------------------------------


def instance_label(address: str, service_id: str) -> str:
    """``svc-`` plus the first sixteen hex digits of
    ``SHA-256(address || 0x00 || service_id)``."""
    digest = hashlib.sha256(address.encode("utf-8") + b"\x00" + service_id.encode("utf-8"))
    return INSTANCE_PREFIX + digest.hexdigest()[:16]


def service_instance_name(address: str, service_id: str) -> str:
    """The fully qualified instance name under the peer-stream type. The
    subtype is where it is browsed, not part of its name."""
    return f"{instance_label(address, service_id)}.{SERVICE_TYPE}"


def _check_key(key: str) -> None:
    if not key:
        raise RecordRefused("a TXT key cannot be empty")
    for ch in key:
        code = ord(ch)
        if code < 0x20 or code > 0x7E or ch == "=":
            raise RecordRefused(
                f"TXT key {key!r} is not printable ASCII without '=' (RFC 6763 section 6.4)"
            )


def txt_record(record: ServiceRecord, address: str) -> dict[str, str]:
    """Lay a descriptor out as the chapter's TXT record, in the chapter's
    order, or raise :class:`RecordRefused`. Nothing is ever shortened."""
    if not address.startswith(ADDRESS_PREFIX):
        raise RecordRefused(f"{address!r} is not a canonical address")
    sid = record.service_id.encode("utf-8")
    if len(sid) > MAX_SID_BYTES:
        raise RecordRefused(
            f"service id is {len(sid)} bytes; the record carries at most {MAX_SID_BYTES}"
        )
    entries: dict[str, str] = {
        "txtvers": TXT_VERSION,
        "sid": record.service_id,
        "ver": record.version,
        "addr": address,
    }
    for key in sorted(record.capabilities, key=lambda k: k.encode("utf-8")):
        _check_key(key)
        entries[_CAPABILITY_PREFIX + key] = record.capabilities[key]
    for key, value in entries.items():
        string = len(key.encode("utf-8")) + 1 + len(value.encode("utf-8"))
        if string > MAX_TXT_STRING_BYTES:
            raise RecordRefused(
                f"TXT entry {key!r} is {string} bytes; one string carries at most "
                f"{MAX_TXT_STRING_BYTES} (RFC 6763 section 6.1)"
            )
    size = txt_record_size(entries)
    if size > MAX_TXT_RECORD_BYTES:
        raise RecordRefused(
            f"the TXT record is {size} bytes; the chapter allows {MAX_TXT_RECORD_BYTES}"
        )
    return entries


def txt_record_size(entries: Mapping[str, str]) -> int:
    """The record's size on the wire: one length octet per ``key=value``."""
    return sum(1 + len(k.encode("utf-8")) + 1 + len(v.encode("utf-8")) for k, v in entries.items())


def parse_txt(raw: bytes) -> list[tuple[bytes, bytes | None]] | None:
    """The strings of a TXT record as ``(key, value)`` pairs, in order, or
    ``None`` when a length octet runs past the end. A string with no ``=``
    is a key with no value (RFC 6763 section 6.4); an empty string is
    skipped, as the RFC says a reader must."""
    pairs: list[tuple[bytes, bytes | None]] = []
    offset = 0
    while offset < len(raw):
        length = raw[offset]
        offset += 1
        if offset + length > len(raw):
            return None
        string = raw[offset : offset + length]
        offset += length
        if not string:
            continue
        key, sep, value = string.partition(b"=")
        pairs.append((key, value if sep else None))
    return pairs


def record_from_txt(raw: bytes) -> ServiceRecord | None:
    """A LAN neighbour's claim, from the raw TXT record of a resolved
    instance, or ``None`` for a record the chapter says to ignore: another
    ``txtvers``, a missing ``sid`` or ``addr``, an ``addr`` that is not an
    address, a key twice, a value that is not UTF-8, or anything over the
    bounds a publisher would have been refused on."""
    pairs = parse_txt(raw)
    if pairs is None or len(raw) > MAX_TXT_RECORD_BYTES:
        return None
    seen: set[bytes] = set()
    fields: dict[str, str] = {}
    capabilities: dict[str, str] = {}
    for key, value in pairs:
        if key in seen:
            return None
        seen.add(key)
        try:
            name = key.decode("ascii")
            _check_key(name)
            text = "" if value is None else value.decode("utf-8")
        except (UnicodeDecodeError, RecordRefused):
            return None
        if name in _TXT_KEYS_FIXED:
            fields[name] = text
        elif name.startswith(_CAPABILITY_PREFIX) and len(name) > len(_CAPABILITY_PREFIX):
            capabilities[name[len(_CAPABILITY_PREFIX) :]] = text
    if fields.get("txtvers") != TXT_VERSION:
        return None
    service_id = fields.get("sid")
    address = fields.get("addr")
    if not service_id or len(service_id.encode("utf-8")) > MAX_SID_BYTES:
        return None
    if not address or not address.startswith(ADDRESS_PREFIX):
        return None
    return ServiceRecord(
        service_id=service_id,
        version=fields.get("ver", ""),
        capabilities=capabilities,
        provider=address,
        source=SOURCE_LAN,
    )


def lan_service_event(record: ServiceRecord) -> dict[str, Any]:
    """The event a LAN import produces: the mesh ``service_discovered``
    fields, an empty ``query_id`` (no query was sent), ``hop_count`` 0 (the
    record came from the segment) and ``source: "lan"``."""
    return {
        "type": "service_discovered",
        "source": SOURCE_LAN,
        "query_id": "",
        "service_id": record.service_id,
        "version": record.version,
        "provider_peer_id": record.provider,
        "capabilities": dict(record.capabilities),
        "hop_count": 0,
    }


# -- the responder behind the bridge ------------------------------------------


class ZeroconfBackend:
    """The optional ``zeroconf`` package, behind the five calls the bridge
    makes. Tests substitute a fake with the same five."""

    def __init__(self) -> None:
        try:
            from zeroconf import IPVersion, ServiceInfo, ServiceStateChange
            from zeroconf.asyncio import AsyncServiceBrowser, AsyncServiceInfo, AsyncZeroconf
        except ImportError as exc:
            raise ImportError(
                "DNS-SD for services needs the optional dependency: "
                "pip install 'offline-protocol-sdk[lan]'"
            ) from exc
        self._ServiceInfo = ServiceInfo
        self._ServiceStateChange = ServiceStateChange
        self._AsyncServiceBrowser = AsyncServiceBrowser
        self._AsyncServiceInfo = AsyncServiceInfo
        self._zc = AsyncZeroconf(ip_version=IPVersion.All)
        self._browser: Any = None

    async def register(
        self, *, name: str, port: int, txt: Mapping[str, str], server: str, addresses: list[str]
    ) -> Any:
        info = self._ServiceInfo(
            SUBTYPE,
            name,
            port=port,
            properties=dict(txt),
            server=server,
            parsed_addresses=list(addresses),
        )
        await self._zc.async_register_service(info)
        return info

    async def unregister(self, handle: Any) -> None:
        await self._zc.async_unregister_service(handle)

    def browse(self, on_change: Callable[[str, bool], None]) -> None:
        removed = self._ServiceStateChange.Removed

        def handler(zeroconf: Any, service_type: str, name: str, state_change: Any) -> None:
            on_change(name, state_change is removed)

        self._browser = self._AsyncServiceBrowser(self._zc.zeroconf, SUBTYPE, handlers=[handler])

    async def resolve(self, name: str, timeout_ms: int) -> bytes | None:
        info = self._AsyncServiceInfo(SUBTYPE, name)
        if await info.async_request(self._zc.zeroconf, timeout_ms):
            return bytes(info.text)
        return None

    async def close(self) -> None:
        if self._browser is not None:
            await self._browser.async_cancel()
            self._browser = None
        await self._zc.async_close()


@dataclass
class _LanEntry:
    record: ServiceRecord
    seen_at: float


class DnsSdBridge:
    """Publishes a :class:`Services` registry on the LAN and imports the
    LAN's into a registry of its own.

    Start it after ``ProtocolManager.start()``, with the address the engine
    derived (``pm.local_address``); stop it before the manager. Registrations
    made while it runs are published as they happen, through the
    :class:`~.services.ServicesListener` it installs; registrations made
    before are published at :meth:`start`.

    ``on_event`` receives each import as the event
    :func:`lan_service_event` builds, on the event loop's thread. Removal is
    not an event: the mesh has no removal signal either, and
    :meth:`lan_services` is the current view.
    """

    def __init__(
        self,
        services: Services,
        *,
        on_event: Callable[[dict[str, Any]], None] | None = None,
        publish: bool = True,
        browse: bool = True,
        ttl: float = DEFAULT_TTL,
        resolve_timeout_ms: int = RESOLVE_TIMEOUT_MS,
        backend: Any | None = None,
    ) -> None:
        if ttl <= 0:
            raise ValueError("ttl must be positive")
        self._services = services
        self._on_event = on_event
        self._publish = publish
        self._browse = browse
        self._ttl = float(ttl)
        self._resolve_timeout_ms = int(resolve_timeout_ms)
        self._backend_given = backend
        self._backend: Any = None
        self._loop: asyncio.AbstractEventLoop | None = None
        self._address: str | None = None
        self._port = 0
        self._addresses: list[str] = []
        self._published: dict[str, Any] = {}
        self._lan: dict[str, _LanEntry] = {}
        self._tasks: set[asyncio.Task[Any]] = set()
        self._sweeper: asyncio.Task[None] | None = None
        self._running = False

    # -- lifecycle ------------------------------------------------------------

    @property
    def running(self) -> bool:
        return self._running

    @property
    def address(self) -> str | None:
        return self._address

    async def start(
        self,
        *,
        address: str,
        port: int | None = None,
        listen_host: str = "0.0.0.0",
        addresses: list[str] | None = None,
    ) -> None:
        """Publish and browse.

        ``address`` is this node's canonical address, ``port`` its
        peer-stream listening port when it has one (the record's SRV port,
        0 otherwise), and ``addresses`` the interface addresses to publish
        (every non-loopback address of every interface, by default, as the
        peer-stream record does).
        """
        if self._running:
            return
        if not address.startswith(ADDRESS_PREFIX):
            raise ValueError(f"{address!r} is not a canonical address")
        self._loop = asyncio.get_running_loop()
        self._address = address
        self._port = int(port or 0)
        self._backend = self._backend_given if self._backend_given is not None else ZeroconfBackend()
        if self._publish:
            self._addresses = list(addresses) if addresses is not None else advertised_addresses(listen_host)
            if not self._addresses:
                await self._backend.close()
                self._backend = None
                raise RuntimeError("publishing found no interface address to put in the record")
        self._running = True
        self._services.add_listener(self)
        try:
            if self._publish:
                for record in self._services.registered():
                    await self.publish(record)
            if self._browse:
                self._backend.browse(self._on_change)
                self._sweeper = self._loop.create_task(self._sweep_forever())
        except BaseException:
            # A responder that refuses a registration (a name conflict, a
            # closed socket) must not leave a bridge that is listening to
            # the registry and holds an open responder while reporting
            # itself stopped.
            await self.stop()
            raise

    async def stop(self) -> None:
        if not self._running:
            return
        self._running = False
        self._services.remove_listener(self)
        if self._sweeper is not None:
            self._sweeper.cancel()
            self._sweeper = None
        for task in list(self._tasks):
            task.cancel()
        self._tasks.clear()
        for service_id in list(self._published):
            handle = self._published.pop(service_id)
            try:
                await self._backend.unregister(handle)
            except Exception:
                logger.exception("withdrawing %r from the LAN failed", service_id)
        self._lan.clear()
        try:
            await self._backend.close()
        finally:
            self._backend = None

    # -- publishing -----------------------------------------------------------

    def published(self) -> list[str]:
        """The service ids currently on the LAN."""
        return list(self._published)

    async def publish(self, record: ServiceRecord) -> bool:
        """Publish one of this node's registrations. Returns ``False`` when
        the descriptor does not fit the record (rule 3); the mesh
        registration stands either way. A record that is not this node's
        own is refused outright (rule 2)."""
        if record.source != SOURCE_LOCAL:
            raise ValueError("only this node's own registrations are published (dns-sd-mapping invariant 2)")
        if not self._running or not self._publish or self._address is None:
            return False
        try:
            txt = txt_record(record, self._address)
        except RecordRefused as exc:
            logger.warning("service %r not published on the LAN: %s", record.service_id, exc)
            return False
        previous = self._published.pop(record.service_id, None)
        if previous is not None:
            await self._backend.unregister(previous)
        handle = await self._backend.register(
            name=service_instance_name(self._address, record.service_id),
            port=self._port,
            txt=txt,
            server=service_host_name(self._address),
            addresses=self._addresses,
        )
        self._published[record.service_id] = handle
        return True

    async def withdraw(self, record: ServiceRecord) -> None:
        handle = self._published.pop(record.service_id, None)
        if handle is not None and self._backend is not None:
            await self._backend.unregister(handle)

    # ServicesListener: called on whatever thread changed the registry.
    def on_registered(self, record: ServiceRecord) -> None:
        self._later(lambda: self.publish(record))

    def on_unregistered(self, record: ServiceRecord) -> None:
        self._later(lambda: self.withdraw(record))

    def _later(self, make: Callable[[], Any]) -> None:
        loop = self._loop
        if loop is None or not self._running:
            return
        loop.call_soon_threadsafe(self._spawn, make)

    def _spawn(self, make: Callable[[], Any]) -> None:
        if not self._running or self._loop is None:
            return
        task = self._loop.create_task(make())
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)

    # -- importing ------------------------------------------------------------

    def lan_services(self) -> list[ServiceRecord]:
        """What the LAN's neighbours currently claim to offer."""
        return [entry.record for entry in self._lan.values()]

    def _on_change(self, name: str, removed: bool) -> None:
        if removed:
            self._lan.pop(name, None)
            return
        self._spawn(lambda: self._resolve_and_import(name))

    async def _resolve_and_import(self, name: str) -> None:
        if self._backend is None:
            return
        raw = await self._backend.resolve(name, self._resolve_timeout_ms)
        if raw is not None:
            self.import_txt(name, raw)

    def import_txt(self, name: str, raw: bytes, now: float | None = None) -> bool:
        """Take a resolved instance's TXT record into the LAN registry.
        Returns whether an event was delivered: a new instance, or one whose
        record changed. A record the chapter says to ignore removes any
        earlier import under that name; this node's own record is ignored."""
        record = record_from_txt(raw)
        if record is None:
            self._lan.pop(name, None)
            return False
        if record.provider == self._address:
            return False
        seen_at = time.monotonic() if now is None else now
        previous = self._lan.get(name)
        self._lan[name] = _LanEntry(record, seen_at)
        if previous is not None and previous.record == record:
            return False
        self._emit(lan_service_event(record))
        return True

    async def sweep(self, now: float | None = None) -> None:
        """Re-resolve every import older than half the time to live, and
        drop every one older than the whole of it. The periodic task calls
        this; a test calls it with a clock of its own."""
        current = time.monotonic() if now is None else now
        for name, entry in list(self._lan.items()):
            age = current - entry.seen_at
            if age >= self._ttl:
                self._lan.pop(name, None)
            elif age >= self._ttl / 2 and self._backend is not None:
                raw = await self._backend.resolve(name, self._resolve_timeout_ms)
                if raw is not None:
                    self.import_txt(name, raw, now=current)

    async def _sweep_forever(self) -> None:
        while self._running:
            await asyncio.sleep(self._ttl / 2)
            try:
                await self.sweep()
            except Exception:
                logger.exception("the LAN sweep failed; the next one runs on schedule")

    def _emit(self, event: dict[str, Any]) -> None:
        if self._on_event is None:
            return
        try:
            self._on_event(event)
        except Exception:
            logger.exception("event handler failed on %s", event.get("type"))
