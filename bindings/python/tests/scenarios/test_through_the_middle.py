"""A reaches C through B when A and C cannot hear each other.

A and C meet once first, directly, so their engines form the MLS session
(the automatic key exchange and the Welcome travel only over a direct link,
never through the mesh). Then C restarts with B as its only peer, A and C
have no link, and a message from A to C is carried by B: B reports
``message_relayed`` and C receives it one hop away.

The sender's receipt does not come back, and the second test records that as
an expected failure. C's acknowledgement is carried back by B and reaches A,
but A drops it: every send of a message whose only route is the mesh is
refused by A's own carriers (``handle_send_failure``, ``send.rs``), so no
pending acknowledgement is ever registered for it, and an acknowledgement
with no pending record settles only a DM the relay parked
(``settle_parked_dm_from_ack``, ``send.rs:5185``). A keeps re-offering the
message, C keeps re-acknowledging the duplicates, and the outbox reports the
delivered message failed when its lifetime ends. The Rust neighbourhood
simulator does not catch it: ``the_answer_finds_its_way_back`` asserts that
B transmits toward A, never that A emits ``message_delivered``. PR #537
fixes it in the engine; the expected failure is strict, so the test turns
red once that lands, and the marker comes off then.
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


@pytest.mark.xfail(
    strict=True,
    reason="a mesh-only DM registers no pending acknowledgement, so the carried receipt is dropped; fixed by #537 (module docstring)",
)
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
