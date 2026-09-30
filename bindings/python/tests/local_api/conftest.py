"""Fixtures for the local API: a server over a Unix socket in the test's
temporary directory, and a small JSON-RPC client that keeps responses and
notifications apart."""

from __future__ import annotations

import asyncio
import itertools
import json
import os
import shutil
import tempfile
from pathlib import Path
from typing import Any, Callable

import pytest
from websockets.asyncio.client import connect, unix_connect

from offline_protocol_sdk.local_api import LocalApiServer, Policy
from offline_protocol_sdk.offline_protocol import OverflowPolicy, ProtocolConfig
from offline_protocol_sdk.protocol_manager import ProtocolManager


def make_config(**overrides: Any) -> ProtocolConfig:
    """A config with the Internet transport on (the validator refuses a
    config with every transport off) and encryption off, so a test never
    needs a key exchange."""
    fields: dict[str, Any] = dict(
        app_id="server-app",
        profile="server-user",
        ble_enabled=False,
        wifi_direct_enabled=False,
        internet_enabled=True,
        reticulum_enabled=False,
        nostr_enabled=False,
        prefer_online=True,
        initial_ttl=3,
        encryption_enabled=False,
        auto_key_exchange=False,
        store_pending=True,
        require_encryption=False,
        max_pending_per_peer=100,
        max_pending_global=1000,
        pending_ttl_ms=60000,
        overflow_policy=OverflowPolicy.DROP_OLDEST,
    )
    fields.update(overrides)
    return ProtocolConfig(**fields)


class RpcClient:
    """One connection: ``call`` awaits the matching response; notifications
    land in :attr:`events` and can be awaited by tag."""

    def __init__(self, websocket: Any) -> None:
        self._ws = websocket
        self._ids = itertools.count(1)
        self._pending: dict[Any, asyncio.Future[dict[str, Any]]] = {}
        self.events: list[dict[str, Any]] = []
        self._event_waiters: list[tuple[Callable[[dict[str, Any]], bool], asyncio.Future[dict[str, Any]]]] = []
        self.closed: asyncio.Future[Any] = asyncio.get_running_loop().create_future()
        self._reader = asyncio.ensure_future(self._read())

    async def _read(self) -> None:
        try:
            async for raw in self._ws:
                message = json.loads(raw)
                if "id" in message:
                    waiter = self._pending.pop(message["id"], None)
                    if waiter is not None and not waiter.done():
                        waiter.set_result(message)
                    continue
                event = message["params"]
                self.events.append(event)
                for predicate, future in list(self._event_waiters):
                    if not future.done() and predicate(event):
                        future.set_result(event)
                        self._event_waiters.remove((predicate, future))
        except Exception as exc:  # the socket closed
            if not self.closed.done():
                self.closed.set_result(exc)
        else:
            if not self.closed.done():
                self.closed.set_result(None)

    async def send_raw(self, text: str) -> None:
        await self._ws.send(text)

    async def call_raw(self, request: dict[str, Any]) -> dict[str, Any]:
        future: asyncio.Future[dict[str, Any]] = asyncio.get_running_loop().create_future()
        self._pending[request["id"]] = future
        await self._ws.send(json.dumps(request))
        return await asyncio.wait_for(future, 10)

    async def call(self, method: str, params: dict[str, Any] | None = None) -> Any:
        request: dict[str, Any] = {"jsonrpc": "2.0", "id": next(self._ids), "method": method}
        if params is not None:
            request["params"] = params
        response = await self.call_raw(request)
        if "error" in response:
            raise RpcFailure(response["error"])
        return response["result"]

    async def hello(self, app_id: str, **extra: Any) -> dict[str, Any]:
        return await self.call("hello", {"app_id": app_id, **extra})

    def event_of(self, tag: str, **fields: Any) -> asyncio.Future[dict[str, Any]]:
        def matches(event: dict[str, Any]) -> bool:
            return event.get("type") == tag and all(event.get(k) == v for k, v in fields.items())

        future: asyncio.Future[dict[str, Any]] = asyncio.get_running_loop().create_future()
        for event in self.events:
            if matches(event):
                future.set_result(event)
                return future
        self._event_waiters.append((matches, future))
        return future

    async def wait_event(self, tag: str, timeout: float = 10, **fields: Any) -> dict[str, Any]:
        return await asyncio.wait_for(self.event_of(tag, **fields), timeout)

    def events_of(self, tag: str) -> list[dict[str, Any]]:
        return [e for e in self.events if e.get("type") == tag]

    async def close(self) -> None:
        await self._ws.close()
        await asyncio.gather(self._reader, return_exceptions=True)


class RpcFailure(Exception):
    def __init__(self, error: dict[str, Any]) -> None:
        super().__init__(error.get("message"))
        self.code = error["code"]
        self.message = error.get("message")
        self.variant = (error.get("data") or {}).get("variant")


class ServerHarness:
    """Starts servers under a short temporary directory and closes them after.

    Short on purpose: a Unix socket path is limited to about a hundred bytes,
    and pytest's own ``tmp_path`` is longer than that on macOS.
    """

    def __init__(self, tmp_path: Any) -> None:
        self._tmp = Path(tempfile.mkdtemp(prefix="opsvc-", dir="/tmp" if os.path.isdir("/tmp") else None))
        self._servers: list[LocalApiServer] = []
        self._clients: list[RpcClient] = []
        self._count = itertools.count(1)

    async def server(
        self,
        *,
        config: ProtocolConfig | None = None,
        policy: Policy | None = None,
        tcp: bool = False,
        health: bool = True,
        manager: ProtocolManager | None = None,
    ) -> LocalApiServer:
        n = next(self._count)
        if manager is None:
            manager = ProtocolManager(config or make_config(profile=f"user-{n}"))
        if tcp:
            server = LocalApiServer(
                manager,
                policy=policy,
                tcp_port=0,
                token_path=self._tmp / f"token-{n}",
                health=health,
            )
        else:
            server = LocalApiServer(
                manager,
                policy=policy,
                socket_path=self._tmp / f"run-{n}" / "api.sock",
                health=health,
            )
        await server.start()
        self._servers.append(server)
        return server

    async def client(self, server: LocalApiServer) -> RpcClient:
        if server.socket_path is not None:
            websocket = await unix_connect(str(server.socket_path), uri="ws://localhost/")
        else:
            websocket = await connect(f"ws://127.0.0.1:{server.port}/")
        client = RpcClient(websocket)
        self._clients.append(client)
        return client

    async def close(self) -> None:
        for client in self._clients:
            await client.close()
        for server in self._servers:
            await server.stop()
        shutil.rmtree(self._tmp, ignore_errors=True)


@pytest.fixture
async def harness(tmp_path):
    h = ServerHarness(tmp_path)
    try:
        yield h
    finally:
        await h.close()
