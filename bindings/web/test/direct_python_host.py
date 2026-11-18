"""The Python host as the native end direct.html dials (W16, ADR 0039).

The same contract as the WebRTC Direct crate's example listener
(``crates/offline-protocol-webrtc-direct/examples/listener/main.rs``), so the
page runs unchanged against either: ``DIRECT_LISTENER=python`` makes
test/run.mjs start this instead. Run with the Python package's interpreter,
which has the package and a library built with the ``webrtc-direct``
feature (``bindings/python/scripts/build-desktop.sh``)::

    bindings/python/.venv/bin/python bindings/web/test/direct_python_host.py <private-ip> [--port N] [--app-id ID]

It starts a ``ProtocolManager`` with a fresh identity over in-memory stores
and the peer-stream slot only, starts its ``PeerStreamManager`` with no TCP
listener, and opens the manager's WebRTC Direct listener on ``<private-ip>``.
What the page checks is the manager's bridge: one table, the newer of two
dialled streams kept, one announcement and one loss per address.

Standard output carries one JSON object per line, with the example's types
and fields: ``listening`` first (``code``, ``address``, ``host``,
``port``), then ``proved``, ``refused``, ``lost``, ``dropped`` (read from
the manager's diagnostics), ``neighbor_discovered``, ``neighbor_lost``,
``session_ready``, ``accepted``, ``message``, ``sent``, ``delivered``,
``failed``, ``closed`` and ``error``. The code is printed because the page
is this host's display; the manager itself never logs it. Standard input
takes ``close``, ``send <address> <text>``, ``big <address> <n>`` and
``quit``; its end is not ``quit``. Diagnostics go to standard error.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import sys
import threading
from typing import Any

from offline_protocol_sdk.offline_protocol import (
    MlsStorageProvider,
    OverflowPolicy,
    ProtocolConfig,
    ProtocolStateStorageProvider,
)
from offline_protocol_sdk.protocol_manager import ProtocolManager

#: The prefix of an echo, which is never echoed back.
ECHO = "echo:"
#: A message's text is printed only up to this many bytes; ``len`` always.
MAX_PRINTED_TEXT = 4096


class MemoryStore(MlsStorageProvider):
    """MLS material in a dict: a new device every run."""

    def __init__(self) -> None:
        self._data: dict[tuple[str, str], bytes] = {}
        self._lock = threading.Lock()

    def store(self, key_type: str, key_id: str, data: list[int]) -> None:
        with self._lock:
            self._data[(key_type, key_id)] = bytes(data)

    def load(self, key_type: str, key_id: str) -> list[int] | None:
        with self._lock:
            raw = self._data.get((key_type, key_id))
            return None if raw is None else list(raw)

    def delete(self, key_type: str, key_id: str) -> None:
        with self._lock:
            self._data.pop((key_type, key_id), None)

    def list_keys(self, key_type: str) -> list[str]:
        with self._lock:
            return [kid for (kt, kid) in self._data if kt == key_type]


class StateMemoryStore(ProtocolStateStorageProvider):
    """Protocol state in a dict. Its own interface: the values are ``bytes``,
    where the MLS store's are lists of ints."""

    def __init__(self) -> None:
        self._data: dict[tuple[str, str], bytes] = {}
        self._lock = threading.Lock()

    def store(self, key_type: str, key_id: str, data: bytes) -> None:
        with self._lock:
            self._data[(key_type, key_id)] = bytes(data)

    def load(self, key_type: str, key_id: str) -> bytes | None:
        with self._lock:
            return self._data.get((key_type, key_id))

    def delete(self, key_type: str, key_id: str) -> None:
        with self._lock:
            self._data.pop((key_type, key_id), None)

    def list_keys(self, key_type: str) -> list[str]:
        with self._lock:
            return [kid for (kt, kid) in self._data if kt == key_type]


def emit(kind: str, **fields: Any) -> None:
    print(json.dumps({"type": kind, **fields}), flush=True)


def config(app_id: str) -> ProtocolConfig:
    return ProtocolConfig(
        app_id=app_id,
        profile="listener",
        ble_enabled=False,
        wifi_direct_enabled=True,
        internet_enabled=False,
        reticulum_enabled=False,
        nostr_enabled=False,
        prefer_online=False,
        initial_ttl=3,
        encryption_enabled=True,
        auto_key_exchange=True,
        store_pending=True,
        require_encryption=False,
        max_pending_per_peer=100,
        max_pending_global=1000,
        pending_ttl_ms=60000,
        overflow_policy=OverflowPolicy.DROP_OLDEST,
    )


class Host:
    def __init__(self, manager: ProtocolManager, loop: asyncio.AbstractEventLoop) -> None:
        self.manager = manager
        self.loop = loop
        self.done = asyncio.Event()
        # A message can reach the handler twice (the event and the receive
        # drain); it is printed and echoed once.
        self.seen: set[str] = set()

    # -- the manager's diagnostics: the bridge's decisions --------------------

    def diagnostic(self, level: str, message: str, context: dict[str, Any]) -> None:
        if message == "Dialled stream proved":
            emit(
                "proved",
                stream=context["stream"],
                address=context["peer"],
                remote=context["remote"],
                superseded=context["superseded"],
            )
        elif message == "Dialled stream refused":
            emit("refused", stream=context["stream"], address=context["peer"], reason=context["reason"])
        elif message == "Peer lost" and "stream" in context:
            emit("lost", stream=context["stream"], address=context["peer"])
        elif message == "WebRTC Direct refused a body":
            emit("dropped", to=context["peer"], reason=context["error"])

    # -- engine events ----------------------------------------------------------

    def on_event(self, event: dict[str, Any]) -> None:
        # From whichever thread the core called back on; acted on from the loop.
        self.loop.call_soon_threadsafe(self.react, event)

    def react(self, event: dict[str, Any]) -> None:
        kind = event.get("type")
        if kind == "neighbor_discovered":
            emit("neighbor_discovered", peer=event.get("peer_id"))
        elif kind == "neighbor_lost":
            emit("neighbor_lost", peer=event.get("peer_id"))
        elif kind == "secure_session_established" and event.get("is_session"):
            emit("session_ready", peer=event.get("peer_id"))
        elif kind == "connection_request_received":
            sender = event.get("sender")
            try:
                protocol = self.manager.protocol
                bundle = protocol.mls_generate_key_package()
                protocol.accept_connection_request(
                    recipient=sender, accepter_name="listener", key_package=bundle.key_package_data
                )
                emit("accepted", peer=sender)
            except Exception as exc:
                emit("error", reason=f"accept {sender}: {exc}")
        elif kind == "message_received":
            message_id = event.get("message_id") or event.get("id")
            if message_id in self.seen:
                return
            self.seen.add(message_id)
            content = event.get("content") or ""
            sender = event.get("sender") or event.get("sender_id")
            fields: dict[str, Any] = {
                "id": message_id,
                "from": sender,
                "len": len(content.encode("utf-8")),
                "encrypted": event.get("encrypted"),
            }
            if len(content.encode("utf-8")) <= MAX_PRINTED_TEXT:
                fields["text"] = content
            emit("message", **fields)
            if not content.startswith(ECHO):
                self.send(sender, ECHO + content)
        elif kind == "message_delivered":
            emit("delivered", id=event.get("message_id"))
        elif kind == "message_failed":
            emit("failed", id=event.get("message_id"), reason=event.get("reason"))

    # -- commands -----------------------------------------------------------------

    def send(self, to: str, text: str) -> None:
        try:
            message_id = self.manager.send_message(to, text)
        except Exception as exc:
            emit("error", reason=f"send to {to}: {exc}")
            return
        emit("sent", id=message_id, to=to, len=len(text.encode("utf-8")))

    def command(self, line: str) -> None:
        words = line.split()
        if not words:
            return
        if words[0] == "quit":
            self.done.set()
        elif words[0] == "close":
            peer_stream = self.manager.peer_stream
            assert peer_stream is not None
            peer_stream.close_webrtc_direct()
            emit("closed")
        elif words[0] == "send" and len(words) >= 2:
            # The text is the rest of the line, spaces kept.
            parts = line.strip().split(None, 2)
            self.send(words[1], parts[2].rstrip() if len(parts) > 2 else "")
        elif words[0] == "big" and len(words) == 3 and words[2].isdigit():
            self.send(words[1], "x" * int(words[2]))
        elif words[0] == "big":
            emit("error", reason="usage: big <address> <bytes>")
        else:
            emit("error", reason=f"unknown command: {words[0]}")


def read_stdin(loop: asyncio.AbstractEventLoop, host: Host) -> None:
    """Standard input, a line at a time, onto the loop. Its end is not quit."""
    for line in sys.stdin:
        loop.call_soon_threadsafe(host.command, line)


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("host", help="a private or link-local IP address of this machine")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--app-id", default="webrtc-direct-example")
    args = parser.parse_args()
    logging.basicConfig(stream=sys.stderr, level=logging.INFO)

    loop = asyncio.get_running_loop()
    manager = ProtocolManager(config(args.app_id), storage=MemoryStore(), state_storage=StateMemoryStore())
    host = Host(manager, loop)
    manager.on_event(host.on_event)
    await manager.start()
    peer_stream = manager.peer_stream
    assert peer_stream is not None
    # Dials only: no TCP listener, no peers, no discovery.
    peer_stream.configure(listen_port=None)
    peer_stream.set_delegate(on_diagnostic=host.diagnostic)
    try:
        await peer_stream.start()
        fragment = await peer_stream.open_webrtc_direct(args.host, args.port)
        code = peer_stream.webrtc_direct_code
        emit("listening", code=fragment, address=code.address, host=code.host, port=code.port)
        threading.Thread(target=read_stdin, args=(loop, host), daemon=True).start()
        await host.done.wait()
    finally:
        await manager.stop()
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
