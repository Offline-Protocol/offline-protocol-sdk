"""Two servers on one host over the peer-stream transport: a client of one
sends, a client of the other receives, and the routing rules hold across
the mesh."""

from __future__ import annotations

import asyncio

import pytest

from offline_protocol_sdk.protocol_manager import ProtocolManager

from .conftest import RpcFailure, make_config


async def until(predicate, timeout: float = 15.0) -> None:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while not predicate():
        if loop.time() > deadline:
            raise AssertionError("condition not met in time")
        await asyncio.sleep(0.05)


async def two_servers(harness):
    """Two engines over loopback streams: B keeps a stream to A."""
    config_a = make_config(profile="alice", app_id="server-app", wifi_direct_enabled=True, internet_enabled=False)
    manager_a = ProtocolManager(config_a)
    manager_a.peer_stream.configure(listen_host="127.0.0.1", listen_port=0)
    server_a = await harness.server(manager=manager_a)
    port_a = manager_a.peer_stream.listen_port
    config_b = make_config(profile="bob", app_id="server-app", wifi_direct_enabled=True, internet_enabled=False)
    manager_b = ProtocolManager(config_b)
    manager_b.peer_stream.configure(listen_host="127.0.0.1", listen_port=0, peers=[f"127.0.0.1:{port_a}"])
    server_b = await harness.server(manager=manager_b)
    await until(lambda: manager_a.local_address in manager_b.peer_stream.connected_peers())
    await until(lambda: manager_b.local_address in manager_a.peer_stream.connected_peers())
    return server_a, server_b


async def test_a_message_reaches_only_the_application_it_was_sent_from(harness):
    server_a, server_b = await two_servers(harness)
    notes_b = await harness.client(server_b)
    other_b = await harness.client(server_b)
    notes_a = await harness.client(server_a)
    other_a = await harness.client(server_a)
    await notes_b.hello("notes")
    await other_b.hello("other")
    await notes_a.hello("notes")
    await other_a.hello("other")

    message_id = await notes_a.call(
        "send_message",
        {"recipient": server_b.manager.local_address, "content": "hello over the stream", "priority": "Medium"},
    )
    received = await notes_b.wait_event("message_received", message_id=message_id)
    # Stamped with the sending client's application id, not the server's.
    assert received["app_id"] == "notes"
    assert received["content"] == "hello over the stream"
    assert received["sender"] == server_a.manager.local_address
    assert received["message_id"] == message_id
    assert "transport" in received
    # Exactly one `message_received` per message: the drain's copy of the
    # same message is never relayed.
    await asyncio.sleep(0.5)
    assert len(notes_b.events_of("message_received")) == 1
    assert other_b.events_of("message_received") == []

    # The sender's own events correlate to the sender's client only.
    sent = await notes_a.wait_event("message_sent", message_id=message_id)
    assert sent["message_id"] == message_id
    assert other_a.events_of("message_sent") == []
    delivered = await notes_a.wait_event("message_delivered", message_id=message_id)
    assert delivered["message_id"] == message_id
    assert other_a.events_of("message_delivered") == []


async def test_a_message_for_an_application_with_no_client_is_held_and_replayed(harness):
    server_a, server_b = await two_servers(harness)
    other_b = await harness.client(server_b)
    await other_b.hello("other")
    notes_a = await harness.client(server_a)
    await notes_a.hello("notes")
    first = await notes_a.call(
        "send_message", {"recipient": server_b.manager.local_address, "content": "one", "priority": "High"}
    )
    second = await notes_a.call(
        "send_message", {"recipient": server_b.manager.local_address, "content": "two", "priority": "High"}
    )
    await until(lambda: server_b.router.held_count("notes") == 2)
    assert other_b.events_of("message_received") == []
    late = await harness.client(server_b)
    hello = await late.hello("notes")
    assert hello["state"] == "Running"
    replayed = [await late.wait_event("message_received", message_id=m) for m in (first, second)]
    assert [e["content"] for e in replayed] == ["one", "two"]
    # Held events come after the hello result and before anything newer;
    # the client's log shows them first.
    assert [e["message_id"] for e in late.events_of("message_received")] == [first, second]
    assert server_b.router.held_count("notes") == 0


async def test_send_message_rich_is_stamped_too(harness):
    server_a, server_b = await two_servers(harness)
    receiver = await harness.client(server_b)
    await receiver.hello("mail")
    sender = await harness.client(server_a)
    await sender.hello("mail")
    message_id = await sender.call(
        "send_message_rich",
        {"recipient": server_b.manager.local_address, "content": "rich", "options": {"priority": "Low"}},
    )
    received = await receiver.wait_event("message_received", message_id=message_id)
    assert received["app_id"] == "mail"
    with pytest.raises(RpcFailure) as err:
        await sender.call(
            "send_message_rich",
            {"recipient": server_b.manager.local_address, "content": "x", "options": {"app_id": "mail"}},
        )
    assert err.value.variant == "InvalidArgument"
