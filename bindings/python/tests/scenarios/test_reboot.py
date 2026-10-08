"""A message queued for an absent recipient survives the sender restarting:
the queue is on disk before ``send_message`` returns, and a new engine over
the same stores sends it once the recipient is back.

The sender's receipt is awaited by a client of the restarted server, which
was never handed the id: it is matched by the id it carries, not by the
server's correlation, which lives for one process only.
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


async def _queue_while_b_is_off(network):
    a = network.device("alpha", 0x11)
    b = network.device("bravo", 0x22)
    b.peers = [a]
    await a.switch_on()
    await b.switch_on()
    await _paired(a, b)
    await b.switch_off()
    await until(lambda: not a.linked_to(b), SESSION_TIMEOUT, "A noticing B is gone")

    before = await network.client(a)
    message_id = await before.call(
        "send_message", {"recipient": b.address, "content": "queued before the restart", "priority": "Medium"}
    )
    await Recorder(before).wait("message_deferred", DELIVERY_TIMEOUT, message_id=message_id)
    return a, b, message_id


async def test_a_queued_message_survives_the_sender_restarting(network):
    a, b, message_id = await _queue_while_b_is_off(network)

    await a.switch_off()
    await a.switch_on()
    # The server that will deliver it never handed this id to anyone.
    assert not a.server.router.knows(message_id)
    after = Recorder(await network.client(a))
    await b.switch_on()

    delivered = await after.wait("message_delivered", DELIVERY_TIMEOUT, message_id=message_id)
    assert delivered["transport"] == "wifiDirect"
    received = Recorder(await network.client(b))
    message = await received.wait("message_received", DELIVERY_TIMEOUT, message_id=message_id)
    assert message["content"] == "queued before the restart"
    assert message["sender"] == a.address


async def test_without_the_saved_state_the_message_is_gone(network):
    """The control for the test above: the same restart over an empty
    protocol-state directory (the identity and the sessions are in the MLS
    store and survive) delivers nothing, so it is the saved queue that
    carries the message across the restart, not anything in memory."""
    a, b, message_id = await _queue_while_b_is_off(network)

    await a.switch_off()
    await a.switch_on(state_root=a.root / "state-empty")
    after = Recorder(await network.client(a))
    await b.switch_on()
    await until(lambda: a.linked_to(b), SESSION_TIMEOUT, "A and B linked again")

    with pytest.raises(AssertionError, match="no message_delivered"):
        await after.wait("message_delivered", 5.0, message_id=message_id)
