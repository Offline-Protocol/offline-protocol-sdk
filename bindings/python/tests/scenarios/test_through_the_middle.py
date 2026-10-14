"""A reaches C through B when A and C cannot hear each other.

In the first two tests A and C meet once first, directly, so their engines
form the MLS session over a direct link. Then C restarts with B as its only
peer, A and C have no link, and a message from A to C is carried by B: B
reports ``message_relayed`` and C receives it one hop away.

The second test follows C's acknowledgement back: B carries it to A, and A
settles the message with ``message_delivered``, though no carrier of A ever
took the send (the engine settles any direct message still in the outbox on
its recipient's acknowledgement, not only one the relay parked).

The third has A and C never meet: the message waits in A's pending queue
while A's key package and C's Welcome cross B, and then it and its receipt
follow the same way.
"""

from __future__ import annotations

import pytest

from .network import DELIVERY_TIMEOUT, SESSION_TIMEOUT, Network, Recorder, until
from .test_store_and_forward import _paired


@pytest.fixture
async def network():
    net = Network()
    try:
        yield net
    finally:
        await net.close()


async def _line_of_three(network):
    """A - B - C, with A and C paired before their direct link goes away."""
    a = network.device("alpha", 0x11)
    b = network.device("bravo", 0x22)
    c = network.device("charlie", 0x33)
    await b.switch_on()
    a.peers = [b]
    await a.switch_on()

    # Met once: C dials A as well as B, and the two form their session.
    c.peers = [b, a]
    await c.switch_on()
    await _paired(a, c)

    # From now on C hears only B.
    await c.switch_off()
    c.peers = [b]
    await c.switch_on()
    await until(lambda: b.linked_to(c) and b.linked_to(a), SESSION_TIMEOUT, "B linked to A and C")
    assert not a.linked_to(c) and not c.linked_to(a)
    return a, b, c


async def test_a_message_crosses_the_middle_device(network):
    a, b, c = await _line_of_three(network)
    middle = Recorder(await network.client(b))
    receiver = Recorder(await network.client(c))
    sender = Recorder(await network.client(a))
    message_id = await sender.client.call(
        "send_message", {"recipient": c.address, "content": "via bravo", "priority": "Medium"}
    )

    received = await receiver.wait("message_received", DELIVERY_TIMEOUT, message_id=message_id)
    assert received["content"] == "via bravo"
    assert received["sender"] == a.address
    assert received["encrypted"] is True
    assert received["hop_count"] == 1

    relayed = await middle.wait("message_relayed", DELIVERY_TIMEOUT, message_id=message_id)
    assert relayed["sender"] == a.address and relayed["recipient"] == c.address
    stats = await middle.client.call("get_mesh_relay_stats")
    assert stats["forwarded"] >= 1
    assert not a.linked_to(c)


async def test_the_receipt_comes_back_through_the_middle_device(network):
    a, b, c = await _line_of_three(network)
    middle = Recorder(await network.client(b))
    sender = Recorder(await network.client(a))
    message_id = await sender.client.call(
        "send_message", {"recipient": c.address, "content": "via bravo", "priority": "Medium"}
    )
    # The receipt is carried: B forwards a frame from C to A.
    await middle.wait("message_relayed", DELIVERY_TIMEOUT, sender=c.address, recipient=a.address)
    delivered = await sender.wait("message_delivered", 20.0, message_id=message_id)
    assert delivered["hop_count"] == 1


async def test_devices_that_never_met_start_a_session_through_the_middle(network):
    a = network.device("alpha", 0x11)
    b = network.device("bravo", 0x22)
    c = network.device("charlie", 0x33)
    await b.switch_on()
    a.peers = [b]
    c.peers = [b]
    await a.switch_on()
    await c.switch_on()
    await until(lambda: b.linked_to(c) and b.linked_to(a), SESSION_TIMEOUT, "B linked to A and C")
    receiver = Recorder(await network.client(c))
    sender = Recorder(await network.client(a))
    message_id = await sender.client.call(
        "send_message", {"recipient": c.address, "content": "never met", "priority": "Medium"}
    )

    received = await receiver.wait("message_received", DELIVERY_TIMEOUT, message_id=message_id)
    assert received["encrypted"] is True
    assert received["hop_count"] == 1
    delivered = await sender.wait("message_delivered", DELIVERY_TIMEOUT, message_id=message_id)
    assert delivered["hop_count"] == 1
    assert not a.linked_to(c) and not c.linked_to(a)
