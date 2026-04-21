#!/usr/bin/env python3
"""Basic end-to-end example: create protocol, connect to relay, send/receive messages.

Prerequisites:
  1. Build the native library and Python bindings:
       cd bindings/python && bash scripts/build-desktop.sh
  2. Install the package:
       pip install -e .
  3. Have a relay server running (e.g. ws://localhost:8080).

Usage:
  python examples/basic_messaging.py --user alice --server ws://localhost:8080
  python examples/basic_messaging.py --user bob   --server ws://localhost:8080
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import signal
import sys
from typing import Any

from offline_protocol_sdk import (
    InternetManager,
    ProtocolManager,
)
from offline_protocol_sdk.offline_protocol import (
    OverflowPolicy,
    ProtocolConfig,
)

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
)
logger = logging.getLogger("example")


def on_event(event: dict[str, Any]) -> None:
    """Handle protocol events."""
    event_type = event.get("type", "unknown")

    if event_type == "message_received":
        sender = event.get("sender", "?")
        content = event.get("content", "")
        msg_id = event.get("message_id", "")
        print(f"\n[Message from {sender}] {content}  (id={msg_id})")

    elif event_type == "transport_changed":
        transport = event.get("transport", "?")
        logger.info("Transport switched to: %s", transport)

    elif event_type == "connection_request_received":
        sender = event.get("sender", "?")
        logger.info("Connection request from %s", sender)

    else:
        logger.debug("Event: %s", json.dumps(event, indent=2))


async def main(user_id: str, server_url: str) -> None:
    config = ProtocolConfig(
        app_id="offline-example",
        user_id=user_id,
        ble_enabled=False,
        wifi_direct_enabled=False,
        internet_enabled=True,
        reticulum_enabled=False,
        nostr_enabled=False,
        prefer_online=True,
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

    async with ProtocolManager(config, event_handler=on_event) as pm:
        # Configure and start Internet transport
        assert pm.internet is not None
        pm.internet.configure(server_url=server_url)
        await pm.internet.start()

        logger.info("Protocol running as '%s', connected to %s", user_id, server_url)
        logger.info("Type a message as  <recipient> <text>  (e.g. 'bob hello!')")
        logger.info("Type 'quit' to exit.\n")

        # Read stdin in a non-blocking way
        loop = asyncio.get_running_loop()
        reader = asyncio.StreamReader()
        protocol = await loop.connect_read_pipe(
            lambda: asyncio.StreamReaderProtocol(reader), sys.stdin
        )

        while True:
            try:
                line_bytes = await reader.readline()
            except asyncio.CancelledError:
                break
            if not line_bytes:
                break

            line = line_bytes.decode().strip()
            if not line:
                continue
            if line.lower() == "quit":
                break

            parts = line.split(None, 1)
            if len(parts) < 2:
                print("Usage: <recipient> <message>")
                continue

            recipient, text = parts
            try:
                msg_id = pm.send_message(recipient, text)
                print(f"  -> sent to {recipient} (id={msg_id})")
            except Exception as exc:
                print(f"  !! send failed: {exc}")

    logger.info("Goodbye.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Offline Protocol SDK example")
    parser.add_argument("--user", required=True, help="Your user ID")
    parser.add_argument("--server", required=True, help="Relay WebSocket URL")
    args = parser.parse_args()

    try:
        asyncio.run(main(args.user, args.server))
    except KeyboardInterrupt:
        pass
