"""A message to a device that is switched off is held by the sender and
delivered when the device comes back, and the sender hears the recipient's
own acknowledgement.

Two devices over loopback peer streams, as two hosts on a LAN: B dials A.
"""

from __future__ import annotations

import asyncio
import io
import json

import pytest

from offline_protocol_sdk.verify import commands

from .network import DELIVERY_TIMEOUT, SESSION_TIMEOUT, Network, Recorder, until


@pytest.fixture
async def network():
    net = Network()
    try:
        yield net
    finally:
        await net.close()


def _target(device) -> commands.Target:
    return commands.Target(socket_path=str(device.socket_path))


async def _paired(a, b) -> None:
    out = commands.Output(out=io.StringIO(), err=io.StringIO())
    assert await commands.pair(_target(a), b.address, SESSION_TIMEOUT, out) == commands.PASS
    assert await commands.pair(_target(b), a.address, SESSION_TIMEOUT, out) == commands.PASS


async def test_a_message_to_a_device_that_is_off_arrives_when_it_comes_back(network):
    a = network.device("alpha", 0x11)
    b = network.device("bravo", 0x22)
    b.peers = [a]
    await a.switch_on()
    await b.switch_on()
    await _paired(a, b)

    await b.switch_off()
    await until(lambda: not a.linked_to(b), SESSION_TIMEOUT, "A noticing B is gone")

    sender = Recorder(await network.client(a))
    message_id = await sender.client.call(
        "send_message", {"recipient": b.address, "content": "while you were out", "priority": "Medium"}
    )
    # Held, not lost and not failed: the peer stream refused a recipient it
    # has no stream to, and the message waits in the outbox.
    deferred = await sender.wait("message_deferred", DELIVERY_TIMEOUT, message_id=message_id)
    assert deferred["recipient"] == b.address
    assert "message_delivered" not in sender.tags(message_id)

    await b.switch_on()
    receiver = Recorder(await network.client(b))
    received = await receiver.wait("message_received", DELIVERY_TIMEOUT, message_id=message_id)
    assert received["content"] == "while you were out"
    assert received["sender"] == a.address
    assert received["encrypted"] is True

    delivered = await sender.wait("message_delivered", DELIVERY_TIMEOUT, message_id=message_id)
    assert delivered["transport"] == "wifiDirect"
    assert delivered["hop_count"] == 0
    assert "message_failed" not in sender.tags(message_id)


async def test_the_verifier_waits_out_the_absence_and_passes_on_the_receipt(network):
    """The same, through the commands an operator runs: ``await`` started on
    the sender while the recipient is still off, and passing once it is on."""
    a = network.device("alpha", 0x11)
    b = network.device("bravo", 0x22)
    b.peers = [a]
    await a.switch_on()
    await b.switch_on()
    await _paired(a, b)
    await b.switch_off()
    await until(lambda: not a.linked_to(b), SESSION_TIMEOUT, "A noticing B is gone")

    out = commands.Output(out=io.StringIO(), err=io.StringIO())
    assert await commands.send(_target(a), b.address, "later", out) == commands.PASS
    message_id = json.loads(out.out.getvalue().splitlines()[-1])["sent"]["message_id"]

    waiting = asyncio.ensure_future(
        commands.await_message(_target(a), message_id, "delivered", DELIVERY_TIMEOUT, out)
    )
    await asyncio.sleep(0.5)
    assert not waiting.done()
    await b.switch_on()
    assert await waiting == commands.PASS
    assert '"result":"pass"' in out.out.getvalue()
    assert await commands.await_message(_target(b), message_id, "received", DELIVERY_TIMEOUT, out) == commands.PASS
