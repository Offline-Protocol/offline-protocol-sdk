"""One connection to the local API, subscribed to every event.

Deliberately small and separate from the HTTP front's persistent client: a
verifier command is short-lived, matches events by their own identifiers,
and must not depend on the front's package. Only :func:`watch` in
``commands.py`` reconnects, around this class.
"""

from __future__ import annotations

import asyncio
import itertools
import json
from pathlib import Path
from typing import Any, Callable

from websockets.asyncio.client import connect, unix_connect

#: The application id every verifier connection declares. A received
#: message is routed to the clients of the application its sender stamped,
#: so a verifier on the sending device and one on the receiving device must
#: declare the same id.
DEFAULT_APP_ID = "offline-protocol-verify"


class RpcFailure(Exception):
    """A JSON-RPC error: ``code`` is the number, ``variant`` the engine's name."""

    def __init__(self, error: dict[str, Any]) -> None:
        super().__init__(error.get("message") or "local API error")
        self.code = error.get("code")
        self.variant = (error.get("data") or {}).get("variant")


class VerifyClient:
    """``call`` awaits the matching response; every event lands in a queue."""

    def __init__(self, websocket: Any) -> None:
        self._ws = websocket
        self._ids = itertools.count(1)
        self._pending: dict[int, asyncio.Future[dict[str, Any]]] = {}
        self._events: asyncio.Queue[dict[str, Any] | None] = asyncio.Queue()
        self.hello_result: dict[str, Any] = {}
        self._reader = asyncio.ensure_future(self._read())

    @classmethod
    async def open(
        cls,
        *,
        socket_path: str | Path | None = None,
        tcp_port: int | None = None,
        token_file: str | Path | None = None,
        app_id: str = DEFAULT_APP_ID,
    ) -> "VerifyClient":
        """Connects, declares ``app_id`` and subscribes to every event."""
        if (socket_path is None) == (tcp_port is None):
            raise ValueError("pass exactly one of socket_path or tcp_port")
        token: str | None = None
        if socket_path is not None:
            websocket = await unix_connect(str(socket_path), uri="ws://localhost/", max_size=None)
        else:
            if token_file is None:
                raise ValueError("the TCP carrier needs the service's token file")
            # Read at every connect: the service writes a new token at every launch.
            token = Path(token_file).read_text(encoding="ascii").strip()
            websocket = await connect(f"ws://127.0.0.1:{tcp_port}/", max_size=None)
        client = cls(websocket)
        try:
            params: dict[str, Any] = {"app_id": app_id, "client": "offline-protocol-verify"}
            if token is not None:
                params["token"] = token
            client.hello_result = await client.call("hello", params)
            await client.call("subscribe", {"types": "all"})
        except BaseException:
            await client.close()
            raise
        return client

    @property
    def local_address(self) -> str | None:
        return self.hello_result.get("local_address")

    async def _read(self) -> None:
        reason = "the local API connection closed"
        try:
            async for raw in self._ws:
                try:
                    message = json.loads(raw)
                except ValueError:
                    continue
                if not isinstance(message, dict):
                    continue
                if message.get("method") == "event":
                    params = message.get("params")
                    if isinstance(params, dict):
                        self._events.put_nowait(params)
                    continue
                waiter = self._pending.pop(message.get("id"), None)  # type: ignore[arg-type]
                if waiter is not None and not waiter.done():
                    waiter.set_result(message)
        except Exception as exc:
            reason = f"the local API connection closed: {exc}"
        finally:
            for waiter in self._pending.values():
                if not waiter.done():
                    waiter.set_exception(ConnectionError(reason))
            self._pending.clear()
            # Wakes a reader of the queue: nothing more will arrive.
            self._events.put_nowait(None)

    async def call(self, method: str, params: dict[str, Any] | None = None) -> Any:
        request_id = next(self._ids)
        future: asyncio.Future[dict[str, Any]] = asyncio.get_running_loop().create_future()
        self._pending[request_id] = future
        await self._ws.send(json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params or {}}))
        response = await future
        if "error" in response:
            raise RpcFailure(response["error"])
        return response.get("result")

    async def next_event(self, timeout: float | None) -> dict[str, Any]:
        """The next event in arrival order. Raises ``TimeoutError`` when
        ``timeout`` runs out and ``ConnectionError`` once the socket closed."""
        try:
            event = await asyncio.wait_for(self._events.get(), timeout)
        except asyncio.TimeoutError:
            raise TimeoutError("no event in time") from None
        if event is None:
            self._events.put_nowait(None)
            raise ConnectionError("the local API connection closed")
        return event

    async def wait_for(
        self,
        accept: Callable[[dict[str, Any]], bool],
        timeout: float,
        on_other: Callable[[dict[str, Any]], None] | None = None,
    ) -> dict[str, Any]:
        """The first event ``accept`` takes; every other one goes to ``on_other``."""
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        while True:
            remaining = deadline - loop.time()
            if remaining <= 0:
                raise TimeoutError("no matching event in time")
            event = await self.next_event(remaining)
            if accept(event):
                return event
            if on_other is not None:
                on_other(event)

    async def close(self) -> None:
        await self._ws.close()
        await asyncio.gather(self._reader, return_exceptions=True)
