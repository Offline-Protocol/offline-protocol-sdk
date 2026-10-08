"""A local API client that stays connected, for a process that is one.

The front is a client of the local API like any other (``docs/bridges/
local-api.md`` holds for it unchanged): it opens the socket, declares its
application id in ``hello``, subscribes, and calls the engine's methods by
name. Unlike a one-shot client it must survive the server restarting, so it
reconnects with a backoff, and every ``hello`` hands it the events the server
held for it while it was away.
"""

from __future__ import annotations

import asyncio
import itertools
import json
import logging
from pathlib import Path
from typing import Any, Awaitable, Callable

from websockets.asyncio.client import connect, unix_connect
from websockets.exceptions import WebSocketException

logger = logging.getLogger(__name__)

RECONNECT_INITIAL_DELAY = 0.5
RECONNECT_MAX_DELAY = 5.0


class NotConnected(Exception):
    """The client has no connection to the local API at the moment."""


class LocalApiError(Exception):
    """An error object the server answered with."""

    def __init__(self, error: dict[str, Any]) -> None:
        super().__init__(error.get("message", "local API error"))
        self.code = error.get("code")
        self.message = error.get("message", "")
        self.variant = (error.get("data") or {}).get("variant")


class LocalApiClient:
    """One persistent connection.

    Parameters
    ----------
    app_id:
        Declared in ``hello``; the server stamps it on every send.
    subscribe:
        The event tags to receive.
    on_event:
        Called on the event loop for every event, in order.
    on_connect:
        Awaited after every successful ``hello`` and ``subscribe``, before
        the client counts as connected for callers, with the ``hello``
        result. It may :meth:`call`.
    socket_path, or tcp_port with tcp_host and token or token_file:
        The server's carrier. A token file is read at every connect, since
        the server writes a new token at every launch.
    """

    def __init__(
        self,
        *,
        app_id: str,
        subscribe: list[str],
        on_event: Callable[[dict[str, Any]], None],
        on_connect: Callable[[dict[str, Any]], Awaitable[None]] | None = None,
        socket_path: str | Path | None = None,
        tcp_port: int | None = None,
        tcp_host: str = "127.0.0.1",
        token: str | None = None,
        token_file: str | Path | None = None,
        client_name: str = "offline-protocol-http-front",
    ) -> None:
        if (socket_path is None) == (tcp_port is None):
            raise ValueError("pass exactly one of socket_path or tcp_port")
        self._app_id = app_id
        self._subscribe = subscribe
        self._on_event = on_event
        self._on_connect = on_connect
        self._socket_path = str(socket_path) if socket_path is not None else None
        self._tcp_port = tcp_port
        self._tcp_host = tcp_host
        self._token = token
        self._token_file = Path(token_file) if token_file is not None else None
        self._client_name = client_name
        self._ids = itertools.count(1)
        self._pending: dict[int, asyncio.Future[Any]] = {}
        self._ws: Any = None
        self._connected = asyncio.Event()
        #: ``hello`` succeeded on the current socket: ``on_connect`` may call
        #: before the client counts as connected for everyone else.
        self._ready = False
        self._stopping = False
        self._task: asyncio.Task[None] | None = None
        self.hello_result: dict[str, Any] | None = None

    @property
    def connected(self) -> bool:
        return self._connected.is_set()

    @property
    def local_address(self) -> str | None:
        return (self.hello_result or {}).get("local_address")

    async def start(self, wait: float | None = 10.0) -> None:
        """Starts connecting; waits up to ``wait`` seconds for the first
        connection, and raises :class:`NotConnected` if it did not come."""
        self._stopping = False
        self._task = asyncio.create_task(self._run(), name="local-api-client")
        if wait is not None:
            try:
                await asyncio.wait_for(self._connected.wait(), wait)
            except asyncio.TimeoutError:
                raise NotConnected("the local API did not answer in time") from None

    async def stop(self) -> None:
        self._stopping = True
        ws = self._ws
        if ws is not None:
            await ws.close()
        if self._task is not None:
            self._task.cancel()
            await asyncio.gather(self._task, return_exceptions=True)
            self._task = None
        self._fail_pending()

    async def call(self, method: str, params: dict[str, Any] | None = None) -> Any:
        if not self._ready:
            raise NotConnected(f"cannot call {method}: not connected")
        return await self._send(method, params)

    async def _send(self, method: str, params: dict[str, Any] | None) -> Any:
        ws = self._ws
        if ws is None:
            raise NotConnected(f"cannot call {method}: not connected")
        request_id = next(self._ids)
        future: asyncio.Future[Any] = asyncio.get_running_loop().create_future()
        self._pending[request_id] = future
        try:
            await ws.send(
                json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params or {}})
            )
            return await future
        except (WebSocketException, OSError) as exc:
            raise NotConnected(f"{method}: {exc}") from None
        finally:
            self._pending.pop(request_id, None)

    async def _open(self) -> Any:
        if self._socket_path is not None:
            return await unix_connect(self._socket_path, uri="ws://localhost/", max_size=None)
        return await connect(f"ws://{self._tcp_host}:{self._tcp_port}/", max_size=None)

    def _hello_params(self) -> dict[str, Any]:
        params: dict[str, Any] = {"app_id": self._app_id, "client": self._client_name}
        token = self._token
        if self._token_file is not None:
            token = self._token_file.read_text(encoding="utf-8").strip()
        if token is not None:
            params["token"] = token
        return params

    async def _run(self) -> None:
        delay = RECONNECT_INITIAL_DELAY
        while not self._stopping:
            reader: asyncio.Task[None] | None = None
            try:
                self._ws = await self._open()
                reader = asyncio.create_task(self._read(self._ws))
                self.hello_result = await self._send("hello", self._hello_params())
                await self._send("subscribe", {"types": self._subscribe})
                self._ready = True
                if self._on_connect is not None:
                    await self._on_connect(self.hello_result)
                self._connected.set()
                delay = RECONNECT_INITIAL_DELAY
                logger.info("connected to the local API as %s", self._app_id)
                await reader
            except asyncio.CancelledError:
                raise
            except LocalApiError as exc:
                logger.error("the local API refused this client: %s", exc)
            except (OSError, WebSocketException, NotConnected) as exc:
                logger.warning("local API connection: %s", exc)
            except Exception:
                logger.exception("local API client failed")
            finally:
                self._ready = False
                self._connected.clear()
                if reader is not None:
                    reader.cancel()
                    await asyncio.gather(reader, return_exceptions=True)
                self._ws = None
                self._fail_pending()
            if self._stopping:
                break
            await asyncio.sleep(delay)
            delay = min(delay * 2, RECONNECT_MAX_DELAY)

    async def _read(self, ws: Any) -> None:
        try:
            async for raw in ws:
                try:
                    message = json.loads(raw)
                except ValueError:
                    continue
                if not isinstance(message, dict):
                    continue
                if message.get("method") == "event":
                    params = message.get("params")
                    if isinstance(params, dict):
                        try:
                            self._on_event(params)
                        except Exception:
                            logger.exception("event handler failed on %s", params.get("type"))
                    continue
                future = self._pending.get(message.get("id"))  # type: ignore[arg-type]
                if future is None or future.done():
                    continue
                if "error" in message:
                    future.set_exception(LocalApiError(message["error"]))
                else:
                    future.set_result(message.get("result"))
        except WebSocketException:
            pass

    def _fail_pending(self) -> None:
        for future in self._pending.values():
            if not future.done():
                future.set_exception(NotConnected("the local API connection closed"))
        self._pending.clear()
