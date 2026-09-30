#!/usr/bin/env python3
"""A client of the local API: one application talking to a running
``offline-protocol-service``. In order: ``hello`` with an application id,
``subscribe``, ``send_message`` and its ``message_delivered`` event, a
document edit read back, the one-shot idiom (a fresh connection: ``hello``,
one request, close), and, with ``--wait``, one inbound ``message_received``.
Errors print as the JSON-RPC code and the engine's variant name, which is
what a client switches on. Needs only the ``websockets`` package the SDK
depends on. Contract: docs/spec/local-api.md; guide: docs/local-api.md.

Usage:
  python examples/local_api_client.py --socket /run/example/api.sock --app-id notes
  python examples/local_api_client.py --tcp 7800 --token-file /run/example/token \
      --app-id notes --to off1... --wait 30
"""

from __future__ import annotations

import argparse
import asyncio
import itertools
import json
import sys
from pathlib import Path
from typing import Any

from websockets.asyncio.client import connect, unix_connect
from websockets.exceptions import WebSocketException


class RpcFailure(Exception):
    """A JSON-RPC error: ``code`` is the number, ``variant`` the engine's name."""

    def __init__(self, error: dict[str, Any]) -> None:
        super().__init__(error.get("message"))
        self.code = error["code"]
        self.variant = (error.get("data") or {}).get("variant")


class Client:
    """One connection: ``call`` awaits the matching response, events queue up."""

    def __init__(self, websocket: Any) -> None:
        self._ws = websocket
        self._ids = itertools.count(1)
        self._pending: dict[int, asyncio.Future[dict[str, Any]]] = {}
        self.events: asyncio.Queue[dict[str, Any]] = asyncio.Queue()
        self._reader = asyncio.ensure_future(self._read())

    async def _read(self) -> None:
        reason = "connection closed"
        try:
            async for raw in self._ws:
                message = json.loads(raw)
                if "id" in message:  # a response; events carry no id
                    waiter = self._pending.pop(message["id"], None)
                    if waiter is not None:
                        waiter.set_result(message)
                else:
                    self.events.put_nowait(message["params"])
        except Exception as exc:  # the socket closed without a normal close frame
            reason = f"connection closed: {exc}"
        finally:
            # Nothing pending will be answered now: fail it rather than hang.
            for waiter in self._pending.values():
                if not waiter.done():
                    waiter.set_exception(ConnectionError(reason))
            self._pending.clear()

    async def call(self, method: str, params: dict[str, Any] | None = None) -> Any:
        request_id = next(self._ids)
        future: asyncio.Future[dict[str, Any]] = asyncio.get_running_loop().create_future()
        self._pending[request_id] = future
        await self._ws.send(json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params or {}}))
        response = await future
        if "error" in response:
            raise RpcFailure(response["error"])
        return response["result"]

    async def next_event(self, tag: str | tuple[str, ...], timeout: float = 30.0, **fields: Any) -> dict[str, Any]:
        """The next event with one of these tags whose fields match; others are skipped."""
        tags = (tag,) if isinstance(tag, str) else tag
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            remaining = deadline - asyncio.get_running_loop().time()
            event = await asyncio.wait_for(self.events.get(), max(remaining, 0.01))
            if event.get("type") in tags and all(event.get(k) == v for k, v in fields.items()):
                return event

    async def close(self) -> None:
        await self._ws.close()
        await asyncio.gather(self._reader, return_exceptions=True)


async def open_client(socket: str | None, tcp: int | None, token_file: str | None) -> tuple[Client, str | None]:
    """Connects on the carrier the service uses; returns the client and the token."""
    if socket:
        return Client(await unix_connect(socket, uri="ws://localhost/")), None
    token = Path(token_file or "").read_text(encoding="ascii").strip()  # written 0600 at launch
    return Client(await connect(f"ws://127.0.0.1:{tcp}/")), token


async def hello(client: Client, app_id: str, token: str | None) -> dict[str, Any]:
    params: dict[str, Any] = {"app_id": app_id, "client": "local_api_client.py"}
    if token is not None:
        params["token"] = token
    return await client.call("hello", params)


async def send_and_confirm(client: Client, recipient: str, content: str) -> dict[str, Any]:
    """Sends, then waits for the terminal event naming the returned id:
    ``message_delivered``, or ``message_undeliverable`` when the engine gives up."""
    message_id = await client.call("send_message", {"recipient": recipient, "content": content, "priority": "Medium"})
    return await client.next_event(("message_delivered", "message_undeliverable"), message_id=message_id)


async def edit_document(client: Client, space_id: str, doc_id: str, key: str, text: str) -> dict[str, Any]:
    """Creates the document (a repeat is a no-op), sets one text key, reads it back.

    A written value is tagged with its kind (`{"kind": "text", "value": ...}`);
    `doc_json` reads back plain JSON."""
    await client.call("data.create_doc", {"space_id": space_id, "doc_id": doc_id})
    value_json = json.dumps({"kind": "text", "value": text})
    await client.call(
        "data.map_set",
        {"space_id": space_id, "doc_id": doc_id, "collection": "fields", "key": key, "value_json": value_json},
    )
    return json.loads(await client.call("data.doc_json", {"space_id": space_id, "doc_id": doc_id}))


async def one_shot(socket: str | None, tcp: int | None, token_file: str | None, app_id: str, method: str) -> Any:
    """The one-shot idiom: a connection that sends hello, one request, and closes."""
    client, token = await open_client(socket, tcp, token_file)
    try:
        await hello(client, app_id, token)
        return await client.call(method)
    finally:
        await client.close()


async def run(args: argparse.Namespace) -> int:
    client: Client | None = None
    try:
        client, token = await open_client(args.socket, args.tcp, args.token_file)
        greeting = await hello(client, args.app_id, token)
        print(json.dumps({"hello": greeting}))
        await client.call("subscribe", {"types": ["message_received", "message_delivered", "message_undeliverable"]})
        if args.to:
            outcome = await send_and_confirm(client, args.to, args.content)
            print(json.dumps({outcome["type"].removeprefix("message_"): outcome}))
        document = await edit_document(client, args.space, args.doc, "edited_by", args.app_id)
        print(json.dumps({"document": document}))
        print(json.dumps({"one_shot": await one_shot(args.socket, args.tcp, args.token_file, args.app_id, "local_address")}))
        if args.wait:
            received = await client.next_event("message_received", timeout=args.wait)
            print(json.dumps({"received": received}))
    except RpcFailure as exc:
        print(json.dumps({"error": {"code": exc.code, "variant": exc.variant, "message": str(exc)}}), file=sys.stderr)
        return 1
    except (OSError, TimeoutError, asyncio.TimeoutError, WebSocketException) as exc:
        # Not a refusal from the engine: the socket, the token file, or a wait ran out.
        print(json.dumps({"error": {"message": str(exc) or type(exc).__name__}}), file=sys.stderr)
        return 1
    finally:
        if client is not None:
            await client.close()
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="A local API client.")
    carrier = parser.add_mutually_exclusive_group(required=True)
    carrier.add_argument("--socket", help="the service's Unix socket path")
    carrier.add_argument("--tcp", type=int, metavar="PORT", help="the service's loopback TCP port")
    parser.add_argument("--token-file", help="the token file the service wrote (with --tcp)")
    parser.add_argument("--app-id", default="notes")
    parser.add_argument("--to", help="an off1... address to send one message to")
    parser.add_argument("--content", default="hello from the local API")
    parser.add_argument("--space", default="notes-1")
    parser.add_argument("--doc", default="todo")
    parser.add_argument("--wait", type=float, default=0, help="seconds to wait for one inbound message")
    args = parser.parse_args(argv)
    if args.tcp is not None and not args.token_file:
        parser.error("--tcp needs --token-file")
    return asyncio.run(run(args))


if __name__ == "__main__":
    sys.exit(main())
