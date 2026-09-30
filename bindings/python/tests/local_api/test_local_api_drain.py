"""The drain's return value is never relayed (bridge rule L2).

``receive_message()`` returns the core ``Message`` as JSON, and the Python
manager synthesises a second ``message_received`` from it in the drain and
hands it to the same handler as the engine's event. On four of the five
carriers the FFI drains inside the inbound entry point, so the manager's
drain sees ``None`` and no copy is made; Nostr, and a message the engine
releases on a later ``process()`` tick, do reach the drain. The server must
drop the copy on every path, so this test feeds the seam directly rather
than depending on which carrier a test happens to run over.
"""

from __future__ import annotations

import asyncio
import json
from unittest.mock import MagicMock

#: What the FFI's `receive_message()` returns for one message: the core
#: `Message` keyed `id`, a capitalised priority, no `transport`.
DRAIN_JSON = json.dumps(
    {
        "id": "8d1f0a2c-5a5e-4b8e-9c6a-0f1e2d3c4b5a",
        "sender": "off1qsender",
        "recipient": "off1qrecipient",
        "app_id": "notes",
        "content": "from the drain",
        "timestamp": 1700000000000,
        "lamport_clock": 7,
        "ttl": 3,
        "hop_count": 0,
        "priority": "Medium",
        "content_type": "text",
    }
)

#: The engine's own event for the same message, as emitted inside the drain.
ENGINE_EVENT = {
    "type": "message_received",
    "message_id": "8d1f0a2c-5a5e-4b8e-9c6a-0f1e2d3c4b5a",
    "sender": "off1qsender",
    "recipient": "off1qrecipient",
    "content": "from the drain",
    "hop_count": 0,
    "transport": "nostr",
    "timestamp": 1700000000000,
    "lamport_clock": 7,
    "reply_to_msg": None,
    "reply_context": None,
    "content_type": "text",
    "media_metadata": None,
    "forward_info": None,
    "encrypted": False,
    "app_id": "notes",
}


async def test_the_drains_copy_is_dropped_and_the_engines_event_relayed_once(harness):
    server = await harness.server()
    client = await harness.client(server)
    await client.hello("notes")
    manager = server.manager

    # The engine's event, then the drain's copy of the same message, in the
    # order the manager produces them on a tick that finds a message.
    manager._protocol.receive_message = MagicMock(side_effect=[DRAIN_JSON, None])
    server._on_engine_event(dict(ENGINE_EVENT))
    manager._drain_incoming_messages()
    await asyncio.sleep(0.2)

    received = client.events_of("message_received")
    assert len(received) == 1, received
    assert received[0]["message_id"] == ENGINE_EVENT["message_id"]
    assert received[0]["transport"] == "nostr"
    assert "id" not in received[0]


async def test_the_drains_copy_is_not_held_either(harness):
    server = await harness.server()
    manager = server.manager
    manager._protocol.receive_message = MagicMock(side_effect=[DRAIN_JSON, None])
    manager._drain_incoming_messages()
    assert server.router.held_count("notes") == 0
    server._on_engine_event(dict(ENGINE_EVENT))
    assert server.router.held_count("notes") == 1
