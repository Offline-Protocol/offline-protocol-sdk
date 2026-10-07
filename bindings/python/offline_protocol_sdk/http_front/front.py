"""The HTTP front: plain HTTP in, sealed messages between hosts.

``docs/spec/http-front.md`` is the contract. An application sends an HTTP
request to ``<service>.<device>.<domain>``; this front carries it to that
device as the content of an end-to-end encrypted direct message, the front
there calls the HTTP callback the providing application registered, and the
answer comes back the same way.

Two rules shape everything here, each with the failure it prevents:

* **A body leaves the host only inside a sealed message.** Requests and
  responses go through ``send_message``, never the service request path,
  whose bodies are signed plaintext: on a LAN with no hop encryption that
  path shows every body to every forwarder.
* **Only a message the engine decrypted is acted on.** The engine refuses a
  wire sender that differs from the MLS-authenticated one, so for an
  ``encrypted`` message ``sender`` is the device that sealed it, and that is
  what a provider is told. A plaintext envelope is dropped: anything could
  have written it.

``aiohttp`` is imported when the front starts, behind the ``http`` extra.
"""

from __future__ import annotations

import asyncio
import collections
import json
import logging
import os
import secrets
import time
import uuid
from pathlib import Path
from typing import Any

from .client import LocalApiClient, LocalApiError, NotConnected
from .envelope import (
    CLOCK_SLACK_SECONDS,
    DEFAULT_DEADLINE_SECONDS,
    ERRORS,
    FRONT_APP_ID,
    MAX_BODY_BYTES,
    MAX_DEADLINE_SECONDS,
    MAX_HEADER_BYTES,
    MAX_PATH_BYTES,
    MIN_CALLBACK_SECONDS,
    EnvelopeError,
    Request,
    Response,
    carried,
    decode,
    header_bytes,
)
from .hostname import DEFAULT_DOMAIN, Aliases, HostError, is_label, parse_host
from .registry import Registration, Registry

logger = logging.getLogger(__name__)

FRONT_NAME = "offline-protocol-http-front"
LOOPBACK_HOSTS = frozenset({"127.0.0.1", "::1", "localhost"})
TOKEN_HEADER = "X-Offline-Protocol-Token"
TIMEOUT_HEADER = "X-Offline-Protocol-Timeout"
ERROR_HEADER = "X-Offline-Protocol-Error"
SENDER_HEADER = "X-Offline-Protocol-Sender"
REQUEST_ID_HEADER = "X-Offline-Protocol-Request-Id"
SERVICE_HEADER = "X-Offline-Protocol-Service"
#: One discovery query per browsed service per this many seconds, never one
#: per browser or per request: a query is a broadcast gossiped ten hops.
BROWSE_INTERVAL_SECONDS = 60.0
#: A provider that answers no query for this many rounds is reported gone.
BROWSE_SILENT_ROUNDS = 2
BROWSE_KEEPALIVE_SECONDS = 15.0
#: Failure events for a message whose id the send call has not returned yet.
EARLY_EVENTS_CAPACITY = 1024
SUBSCRIPTIONS = ["message_received", "message_failed", "message_undeliverable", "service_discovered"]


class _Failed(Exception):
    def __init__(self, token: str) -> None:
        super().__init__(token)
        self.token = token


def _write_token(path: Path) -> str:
    """A fresh token, written ``0600`` in the same call that creates the file."""
    token = secrets.token_hex(32)
    try:
        path.unlink()
    except FileNotFoundError:
        pass
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        handle.write(token + "\n")
    return token


async def _read_limited(stream: Any, limit: int) -> bytes | None:
    """The whole stream, or ``None`` once it passes ``limit`` bytes."""
    chunks: list[bytes] = []
    size = 0
    while True:
        chunk = await stream.read(65536)
        if not chunk:
            return b"".join(chunks)
        size += len(chunk)
        if size > limit:
            return None
        chunks.append(chunk)


class _Browse:
    def __init__(self, service: str) -> None:
        self.service = service
        self.entries: dict[str, dict[str, Any]] = {}
        self.queues: set[asyncio.Queue[tuple[str, dict[str, Any]]]] = set()
        self.task: asyncio.Task[None] | None = None

    def publish(self, kind: str, entry: dict[str, Any]) -> None:
        for queue in self.queues:
            queue.put_nowait((kind, entry))


class HttpFront:
    """Serves HTTP on ``host:port`` and speaks to the engine through the
    local API at ``socket_path`` (or ``tcp_port`` with its token).

    Parameters
    ----------
    host, port:
        Where to serve. Off loopback a ``token_path`` is required.
    domain:
        The hostname suffix (default ``offline.protocol.internal``).
    aliases:
        The operator's device aliases.
    registry_path:
        Where registrations are kept across restarts; in memory when absent.
    token_path:
        Write a fresh token here at every start and require it in
        ``X-Offline-Protocol-Token``.
    """

    def __init__(
        self,
        *,
        socket_path: str | Path | None = None,
        tcp_port: int | None = None,
        tcp_host: str = "127.0.0.1",
        local_api_token: str | None = None,
        local_api_token_file: str | Path | None = None,
        host: str = "127.0.0.1",
        port: int = 8080,
        domain: str = DEFAULT_DOMAIN,
        aliases: Aliases | None = None,
        registry_path: str | Path | None = None,
        token_path: str | Path | None = None,
    ) -> None:
        if host not in LOOPBACK_HOSTS and token_path is None:
            raise ValueError(f"serving on {host} needs a token file: off loopback the front is never open")
        self.host = host
        self.port = port
        self.domain = domain.lower().rstrip(".")
        self.aliases = aliases or Aliases()
        self.registry = Registry(registry_path)
        self.token_path = Path(token_path) if token_path is not None else None
        self.token: str | None = None
        self.client = LocalApiClient(
            app_id=FRONT_APP_ID,
            subscribe=SUBSCRIPTIONS,
            on_event=self._on_event,
            on_connect=self._on_connect,
            socket_path=socket_path,
            tcp_port=tcp_port,
            tcp_host=tcp_host,
            token=local_api_token,
            token_file=local_api_token_file,
        )
        self._pending: dict[str, tuple[str, asyncio.Future[Response]]] = {}
        self._by_message: dict[str, str] = {}
        self._early: collections.OrderedDict[str, str] = collections.OrderedDict()
        self._browses: dict[str, _Browse] = {}
        self._tasks: set[asyncio.Task[Any]] = set()
        self._runner: Any = None
        self._session: Any = None
        self._aiohttp: Any = None

    # -- lifecycle --------------------------------------------------------------

    async def start(self) -> None:
        try:
            import aiohttp
            from aiohttp import web
        except ImportError as exc:
            raise ImportError("the HTTP front needs the optional dependency: pip install 'offline-protocol-sdk[http]'") from exc
        self._aiohttp = aiohttp
        if self.token_path is not None:
            self.token = _write_token(self.token_path)
        self._session = aiohttp.ClientSession(auto_decompress=True)
        app = web.Application(client_max_size=MAX_BODY_BYTES + 1)
        app.router.add_route("*", "/{tail:.*}", self._dispatch)
        self._runner = web.AppRunner(app, access_log=None)
        await self._runner.setup()
        site = web.TCPSite(self._runner, self.host, self.port)
        try:
            await site.start()
            sockets = getattr(site._server, "sockets", None) or []
            if sockets:
                self.port = sockets[0].getsockname()[1]
            await self.client.start()
        except BaseException:
            await self.stop()
            raise
        logger.info("HTTP front serving on %s:%s for *.%s", self.host, self.port, self.domain)

    async def stop(self) -> None:
        for browse in self._browses.values():
            if browse.task is not None:
                browse.task.cancel()
        for task in list(self._tasks):
            task.cancel()
        await asyncio.gather(*self._tasks, return_exceptions=True)
        await self.client.stop()
        if self._runner is not None:
            await self._runner.cleanup()
            self._runner = None
        if self._session is not None:
            await self._session.close()
            self._session = None
        for _, future in self._pending.values():
            if not future.done():
                future.set_exception(_Failed("not_connected"))
        if self.token_path is not None and self.token is not None:
            try:
                self.token_path.unlink()
            except FileNotFoundError:
                pass
            self.token = None

    async def _on_connect(self, hello: dict[str, Any]) -> None:
        """Registers every kept service again: the engine and the server's
        ownership table both forget them when either restarts."""
        for registration in self.registry.all():
            try:
                await self._register_with_engine(registration)
            except LocalApiError as exc:
                logger.error("could not register %s again: %s", registration.service, exc)

    async def _register_with_engine(self, registration: Registration) -> None:
        await self.client._send(
            "services.register_service",
            {
                "service_id": registration.service,
                "version": registration.version,
                "capabilities": registration.capabilities,
            },
        )

    def _spawn(self, coro: Any) -> None:
        task = asyncio.ensure_future(coro)
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)

    # -- HTTP -------------------------------------------------------------------

    def _error(self, token: str, detail: str | None = None) -> Any:
        web = self._aiohttp.web
        body = {"error": token}
        if detail:
            body["detail"] = detail
        return web.json_response(body, status=ERRORS[token], headers={ERROR_HEADER: token})

    async def _dispatch(self, request: Any) -> Any:
        if self.token is not None and not secrets.compare_digest(
            request.headers.get(TOKEN_HEADER, "").encode(), self.token.encode()
        ):
            return self._error("unauthorized")
        try:
            target = parse_host(request.headers.get("Host"), self.domain)
        except HostError as exc:
            return self._error("bad_host", str(exc))
        if target is None:
            return await self._control(request)
        return await self._invoke(request, target.service, target.device)

    async def _invoke(self, request: Any, service: str, device: str) -> Any:
        web = self._aiohttp.web
        address = self.aliases.resolve(device)
        if address is None:
            return self._error("unknown_device", f"{device} is neither an address nor a configured alias")
        path = request.raw_path
        if len(path.encode()) > MAX_PATH_BYTES:
            return self._error("bad_request", "path over the limit")
        headers = carried(request.headers)
        if header_bytes(headers) > MAX_HEADER_BYTES:
            return self._error("bad_request", "headers over the limit")
        body = await _read_limited(request.content, MAX_BODY_BYTES)
        if body is None:
            return self._error("body_too_large")
        timeout = float(DEFAULT_DEADLINE_SECONDS)
        raw_timeout = request.headers.get(TIMEOUT_HEADER)
        if raw_timeout is not None:
            try:
                timeout = min(max(float(raw_timeout), 1.0), float(MAX_DEADLINE_SECONDS))
            except ValueError:
                return self._error("bad_request", f"{TIMEOUT_HEADER} must be a number of seconds")
        envelope = Request(
            id=uuid.uuid4().hex,
            service=service,
            method=request.method,
            path=path,
            headers=headers,
            body=body,
            deadline=int(time.time() + timeout),
        )
        try:
            if address == self.client.local_address:
                response = await self._serve(envelope, address)
                if response is None:
                    raise _Failed("deadline_exceeded")
            else:
                response = await self._send_and_wait(envelope, address, timeout)
        except _Failed as failure:
            return self._error(failure.token)
        out_headers = dict(response.headers)
        if response.error is not None:
            out_headers[ERROR_HEADER] = response.error
        return web.Response(status=response.status, body=response.body, headers=out_headers)

    async def _send_and_wait(self, envelope: Request, address: str, timeout: float) -> Response:
        future: asyncio.Future[Response] = asyncio.get_running_loop().create_future()
        self._pending[envelope.id] = (address, future)
        message_id: str | None = None
        try:
            try:
                message_id = await self.client.call(
                    "send_message", {"recipient": address, "content": envelope.encode(), "priority": "High"}
                )
            except NotConnected:
                raise _Failed("not_connected") from None
            except LocalApiError as exc:
                raise _Failed("unknown_device" if exc.variant == "InvalidArgument" else "send_failed") from None
            early = self._early.pop(message_id, None)
            if early is not None:
                raise _Failed(early)
            self._by_message[message_id] = envelope.id
            try:
                return await asyncio.wait_for(asyncio.shield(future), timeout)
            except asyncio.TimeoutError:
                raise _Failed("deadline_exceeded") from None
        finally:
            self._pending.pop(envelope.id, None)
            if message_id is not None:
                self._by_message.pop(message_id, None)

    # -- the front's own endpoints -----------------------------------------------

    async def _control(self, request: Any) -> Any:
        web = self._aiohttp.web
        parts = [p for p in request.path.split("/") if p]
        method = request.method
        if parts == ["health"] and method == "GET":
            return web.json_response(
                {
                    "name": FRONT_NAME,
                    "version": _package_version(),
                    "connected": self.client.connected,
                    "local_address": self.client.local_address,
                }
            )
        if parts == ["services"] and method == "GET":
            return web.json_response({"services": [r.to_json() for r in self.registry.all()]})
        if len(parts) == 2 and parts[0] == "services" and method in ("PUT", "DELETE"):
            return await self._services(request, parts[1])
        if len(parts) == 2 and parts[0] == "browse" and method == "GET":
            if not is_label(parts[1]):
                return web.json_response({"error": "bad_request", "detail": "not a service label"}, status=400)
            return await self._browse(request, parts[1])
        return web.json_response({"error": "not_found"}, status=404)

    async def _services(self, request: Any, service: str) -> Any:
        web = self._aiohttp.web
        if request.method == "DELETE":
            if self.registry.get(service) is None:
                return web.json_response({"error": "not_found"}, status=404)
            try:
                await self.client.call("services.unregister_service", {"service_id": service})
            except NotConnected:
                return self._error("not_connected")
            except LocalApiError as exc:
                logger.warning("unregistering %s: %s", service, exc)
            self.registry.remove(service)
            return web.json_response({"service": service, "registered": False})
        raw = await _read_limited(request.content, MAX_HEADER_BYTES)
        try:
            registration = Registration.parse(service, json.loads(raw or b"null"))
        except ValueError as exc:
            return web.json_response({"error": "bad_request", "detail": str(exc)}, status=400)
        try:
            await self._register_with_engine(registration)
        except NotConnected:
            return self._error("not_connected")
        except LocalApiError as exc:
            if exc.variant == "PermissionDenied":
                return web.json_response({"error": "conflict", "detail": exc.message}, status=409)
            return web.json_response({"error": "bad_request", "detail": exc.message}, status=400)
        self.registry.put(registration)
        return web.json_response(registration.to_json())

    async def _browse(self, request: Any, service: str) -> Any:
        web = self._aiohttp.web
        response = web.StreamResponse(headers={"Content-Type": "text/event-stream", "Cache-Control": "no-cache"})
        await response.prepare(request)
        browse = self._browses.get(service)
        if browse is None:
            browse = self._browses[service] = _Browse(service)
        queue: asyncio.Queue[tuple[str, dict[str, Any]]] = asyncio.Queue()
        browse.queues.add(queue)
        for entry in list(browse.entries.values()):
            queue.put_nowait(("added", entry))
        if browse.task is None or browse.task.done():
            browse.task = asyncio.ensure_future(self._browse_loop(browse))
        try:
            while True:
                try:
                    kind, entry = await asyncio.wait_for(queue.get(), BROWSE_KEEPALIVE_SECONDS)
                except asyncio.TimeoutError:
                    await response.write(b": keepalive\n\n")
                    continue
                public = {k: v for k, v in entry.items() if k != "last_seen"}
                await response.write(f"event: {kind}\ndata: {json.dumps(public)}\n\n".encode())
        except (ConnectionResetError, asyncio.CancelledError):
            pass
        finally:
            browse.queues.discard(queue)
            if not browse.queues and browse.task is not None:
                browse.task.cancel()
                browse.task = None
        return response

    async def _browse_loop(self, browse: _Browse) -> None:
        while browse.queues:
            try:
                await self.client.call("services.discover_services", {"service_id": browse.service})
            except (NotConnected, LocalApiError) as exc:
                logger.warning("discovery for %s: %s", browse.service, exc)
            await asyncio.sleep(BROWSE_INTERVAL_SECONDS)
            horizon = time.monotonic() - BROWSE_INTERVAL_SECONDS * BROWSE_SILENT_ROUNDS
            for device, entry in list(browse.entries.items()):
                if entry["last_seen"] < horizon:
                    del browse.entries[device]
                    browse.publish("gone", entry)

    def _on_discovered(self, event: dict[str, Any]) -> None:
        browse = self._browses.get(event.get("service_id", ""))
        device = event.get("provider_peer_id")
        if browse is None or not isinstance(device, str):
            return
        entry = {
            "device": device,
            "alias": self.aliases.alias_of(device),
            "service": browse.service,
            "version": event.get("version", ""),
            "capabilities": event.get("capabilities") or {},
            "hops": event.get("hop_count", 0),
            "source": "lan" if event.get("source") == "lan" else "mesh",
            "last_seen": time.monotonic(),
        }
        known = browse.entries.get(device)
        browse.entries[device] = entry
        if known is None or {k: v for k, v in known.items() if k != "last_seen"} != {
            k: v for k, v in entry.items() if k != "last_seen"
        }:
            browse.publish("added", entry)

    # -- events -----------------------------------------------------------------

    def _on_event(self, event: dict[str, Any]) -> None:
        kind = event.get("type")
        if kind == "message_received":
            self._on_message(event)
        elif kind in ("message_failed", "message_undeliverable"):
            token = "send_failed" if kind == "message_failed" else "recipient_unreachable"
            self._on_failure(event.get("message_id"), token)
        elif kind == "service_discovered":
            self._on_discovered(event)

    def _on_failure(self, message_id: Any, token: str) -> None:
        if not isinstance(message_id, str):
            return
        request_id = self._by_message.get(message_id)
        if request_id is None:
            # The send call has not returned its id yet: an event the engine
            # emits inside the call reaches the caller before the result.
            self._early[message_id] = token
            while len(self._early) > EARLY_EVENTS_CAPACITY:
                self._early.popitem(last=False)
            return
        pending = self._pending.get(request_id)
        if pending is not None and not pending[1].done():
            pending[1].set_exception(_Failed(token))

    def _on_message(self, event: dict[str, Any]) -> None:
        if event.get("app_id") != FRONT_APP_ID:
            return
        content = event.get("content")
        sender = event.get("sender")
        if not isinstance(content, str) or not isinstance(sender, str):
            return
        if event.get("encrypted") is not True:
            logger.warning("dropped a plaintext envelope from %s: only a decrypted message is acted on", sender)
            return
        try:
            envelope = decode(content)
        except EnvelopeError as exc:
            logger.warning("dropped a malformed envelope from %s: %s", sender, exc)
            return
        if isinstance(envelope, Response):
            pending = self._pending.get(envelope.id)
            if pending is None or pending[0] != sender:
                return  # not waited for, or not from the device asked
            if not pending[1].done():
                pending[1].set_result(envelope)
            return
        self._spawn(self._provide(envelope, sender))

    async def _provide(self, envelope: Request, sender: str) -> None:
        response = await self._serve(envelope, sender)
        if response is None:
            return
        try:
            await self.client.call(
                "send_message", {"recipient": sender, "content": response.encode(), "priority": "High"}
            )
        except (NotConnected, LocalApiError) as exc:
            logger.warning("could not answer %s for %s: %s", envelope.id, sender, exc)

    async def _serve(self, envelope: Request, sender: str) -> Response | None:
        """Calls the registered callback; ``None`` when the requester has
        stopped waiting and there is no one to answer."""
        if envelope.version != 1:
            return Response.failure(envelope.id, "unsupported_version")
        remaining = envelope.deadline - time.time()
        if remaining < -CLOCK_SLACK_SECONDS:
            logger.info("dropped request %s from %s: past its deadline", envelope.id, sender)
            return None
        registration = self.registry.get(envelope.service)
        if registration is None:
            return Response.failure(envelope.id, "unknown_service")
        timeout = min(max(remaining, MIN_CALLBACK_SECONDS), MAX_DEADLINE_SECONDS)
        headers = dict(envelope.headers)
        headers[SENDER_HEADER] = sender
        headers[REQUEST_ID_HEADER] = envelope.id
        headers[SERVICE_HEADER] = envelope.service
        url = registration.callback.rstrip("/") + envelope.path
        aiohttp = self._aiohttp
        try:
            async with self._session.request(
                envelope.method,
                url,
                headers=headers,
                data=envelope.body or None,
                timeout=aiohttp.ClientTimeout(total=timeout),
                allow_redirects=False,
            ) as reply:
                body = await _read_limited(reply.content, MAX_BODY_BYTES)
                if body is None:
                    return Response.failure(envelope.id, "response_too_large")
                reply_headers = carried(reply.headers)
                if header_bytes(reply_headers) > MAX_HEADER_BYTES:
                    return Response.failure(envelope.id, "response_too_large")
                return Response(envelope.id, reply.status, reply_headers, body)
        except (aiohttp.ClientError, asyncio.TimeoutError, OSError) as exc:
            logger.warning("callback for %s failed: %s", envelope.service, exc)
            return Response.failure(envelope.id, "callback_failed")


def _package_version() -> str:
    try:
        from importlib.metadata import version

        return version("offline-protocol-sdk")
    except Exception:
        return "0"
