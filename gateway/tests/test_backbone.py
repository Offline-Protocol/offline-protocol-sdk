"""Two gateways joined by the in-process backbone: the routing and presence
rules every backbone shares, without a radio."""

from __future__ import annotations

import asyncio
import base64

import pytest

from conftest import Client, address, fake_verify
from offline_gateway import Gateway, GatewayConfig, InMemoryBackbone, InMemoryNetwork
from offline_gateway.backbone import PresenceTable


@pytest.fixture
async def pair():
    network = InMemoryNetwork()
    gateways = []
    for name in ("a", "b"):
        gw = Gateway(
            GatewayConfig(listen_port=0, attach_grace=0),
            verifier=fake_verify,
            backbone=InMemoryBackbone(network, name),
        )
        gw.port = await gw.start()  # type: ignore[attr-defined]
        gateways.append(gw)
    try:
        yield network, gateways[0], gateways[1]
    finally:
        for gw in gateways:
            await gw.stop()


async def test_the_backbone_is_advertised_as_a_token(pair):
    _, a, _ = pair
    c = await Client.connect(a.port)
    frames = await c.attach(address(1))
    assert frames[1]["tokens"] == ["gateway_v1", "backbone_memory_v1"]


async def test_a_frame_crosses_to_the_peer_gateway(pair):
    _, a, b = pair
    alice, bob = await Client.connect(a.port), await Client.connect(b.port)
    await alice.attach(address(1))
    await bob.attach(address(2))
    await alice.submit(address(2), b"sealed", message_id="x-1")
    verdict = await alice.expect("MessageSent")
    assert verdict["message_id"] == "x-1" and verdict["recipient"] == address(2)
    got = await bob.expect("MessageReceived")
    assert got["sender"] == address(1)
    assert base64.b64decode(got["content"]) == b"sealed"
    assert got["message_id"] == "x-1"


async def test_presence_follows_a_device_at_the_peer_gateway(pair):
    _, a, b = pair
    alice = await Client.connect(a.port)
    await alice.attach(address(1))
    await alice.send({"type": "CheckPresence", "peers": [address(2)]})
    assert (await alice.expect("PresenceStatus"))["online"] is False

    bob = await Client.connect(b.port)
    await bob.attach(address(2))
    assert (await alice.expect("PresenceStatus"))["online"] is True

    bob.close()
    left = await alice.expect("PresenceStatus")
    assert left["online"] is False and isinstance(left["last_seen_ms"], int)


async def test_a_lost_link_withdraws_the_peers_claims(pair):
    network, a, b = pair
    alice, bob = await Client.connect(a.port), await Client.connect(b.port)
    await alice.attach(address(1))
    await bob.attach(address(2))
    await alice.send({"type": "CheckPresence", "peers": [address(2)]})
    assert (await alice.expect("PresenceStatus"))["online"] is True

    network.cut("a", "b")
    assert (await alice.expect("PresenceStatus"))["online"] is False
    await alice.submit(address(2), b"x")
    assert (await alice.expect("DeliveryError"))["reason"].startswith("recipient_unreachable")

    # Back up: the snapshot on link-up restores the claim, and watchers hear it.
    network.join("a", "b")
    assert (await alice.expect("PresenceStatus"))["online"] is True
    await alice.submit(address(2), b"y", message_id="again")
    assert (await alice.expect("MessageSent"))["message_id"] == "again"


async def test_a_slow_peer_is_backbone_timeout_a_plain_failure(pair):
    _, a, b = pair
    a.config.backbone_verdict_timeout = 0.2
    b.backbone.delay = 1.0
    alice, bob = await Client.connect(a.port), await Client.connect(b.port)
    await alice.attach(address(1))
    await bob.attach(address(2))
    await alice.submit(address(2), b"x")
    reason = (await alice.expect("DeliveryError"))["reason"]
    assert reason.startswith("backbone_timeout")
    assert not reason.startswith("recipient_unreachable")


async def test_the_scarce_frame_bound_applies_to_backbone_frames_only():
    """The bound is per backbone frame on a scarce backbone; a frame to a
    device on the same gateway never meets it. Real media against the real
    default is test_end_to_end's."""
    network = InMemoryNetwork()
    gws = []
    try:
        for name in ("a", "b"):
            gw = Gateway(
                GatewayConfig(
                    listen_port=0, attach_grace=0, max_backbone_frame_bytes=64, peer_egress_burst_bytes=64,
                    host_backbone_burst_bytes=64,
                ),
                verifier=fake_verify,
                backbone=InMemoryBackbone(network, name, rate_class="scarce"),
            )
            gw.port = await gw.start()  # type: ignore[attr-defined]
            gws.append(gw)
        alice, bob = await Client.connect(gws[0].port), await Client.connect(gws[1].port)
        local = await Client.connect(gws[0].port)
        await alice.attach(address(1))
        await bob.attach(address(2))
        await local.attach(address(3))
        await alice.submit(address(2), b"m" * 65)
        assert (await alice.expect("DeliveryError"))["reason"] == "frame_too_large"
        # The same frame to a device on this gateway is not a backbone frame.
        await alice.submit(address(3), b"m" * 65, message_id="local")
        assert (await alice.expect("MessageSent"))["message_id"] == "local"
    finally:
        for gw in gws:
            await gw.stop()


async def test_the_per_host_backbone_budget_is_budget_exceeded():
    gws = await _scarce_pair(
        max_backbone_frame_bytes=100, host_backbone_bytes_per_minute=0.0001, host_backbone_burst_bytes=200
    )
    try:
        alice, bob = await Client.connect(gws[0].port), await Client.connect(gws[1].port)
        await alice.attach(address(1))
        await bob.attach(address(2))
        for i in range(3):
            await alice.submit(address(2), b"x" * 100, message_id=f"q{i}")
        frames = [await alice.recv() for _ in range(3)]
        by_id = {f["message_id"]: f for f in frames}
        assert by_id["q0"]["type"] == by_id["q1"]["type"] == "MessageSent"
        assert by_id["q2"]["reason"] == "budget_exceeded"
    finally:
        for gw in gws:
            await gw.stop()


async def test_a_host_that_re_keys_does_not_get_a_fresh_budget():
    """Addresses cost nothing to mint. The backbone budget follows the host,
    so a second identity from the same host shares what the first spent."""
    gws = await _scarce_pair(
        max_backbone_frame_bytes=100, host_backbone_bytes_per_minute=0.0001, host_backbone_burst_bytes=100
    )
    try:
        first, again = await Client.connect(gws[0].port), await Client.connect(gws[0].port)
        bob = await Client.connect(gws[1].port)
        await first.attach(address(1))
        await again.attach(address(3))
        await bob.attach(address(2))
        await first.submit(address(2), b"x" * 100, message_id="spent")
        await first.expect("MessageSent")
        await again.submit(address(2), b"x" * 10, message_id="fresh-key")
        assert (await again.expect("DeliveryError"))["reason"] == "budget_exceeded"
    finally:
        for gw in gws:
            await gw.stop()


async def test_a_peer_gateway_over_its_budget_is_refused_as_a_plain_failure(pair):
    _, a, b = pair
    b._peer_budget = type(b._peer_budget)(0.0001, 1)
    alice, bob = await Client.connect(a.port), await Client.connect(b.port)
    await alice.attach(address(1))
    await bob.attach(address(2))
    await alice.submit(address(2), b"x", message_id="p0")
    await alice.submit(address(2), b"x", message_id="p1")
    frames = {f["message_id"]: f for f in [await alice.recv(), await alice.recv()]}
    assert frames["p0"]["type"] == "MessageSent"
    assert frames["p1"]["reason"] == "budget_exceeded"


def test_an_online_claim_beats_an_offline_one_and_a_dropped_peer_forgets():
    clock = [0.0]
    table = PresenceTable(clock=lambda: clock[0])
    assert table.update("p1", address(5), True, None, 1, "e1") == [address(5)]
    clock[0] = 1
    table.update("p2", address(5), False, 123, 1, "e2")
    assert table.claim(address(5)).online and table.claim(address(5)).peer == "p1"
    assert table.drop_peer("p1") == [address(5)]
    claim = table.claim(address(5))
    assert claim.online is False and claim.last_seen_ms == 123
    assert table.snapshot("p2", {address(5): 2}, 2, "e2") == [address(5)]
    assert table.claim(address(5)).online
    assert table.snapshot("p2", {}, 3, "e2") == [address(5)]
    assert not table.claim(address(5)).online


def test_a_snapshot_that_lands_late_does_not_undo_a_newer_attach():
    """The race the numbering exists for: a snapshot taken before X attached
    (number 4) arrives after the one-packet update that announced X (number
    5). Applied in arrival order it would mark X offline and close the path."""
    table = PresenceTable()
    table.update("p", address(1), True, None, 3, "e")
    assert table.update("p", address(9), True, None, 5, "e") == [address(9)]
    assert table.snapshot("p", {address(1): 3}, 4, "e") == []
    assert table.claim(address(9)).online


def test_a_late_update_does_not_undo_a_newer_one():
    table = PresenceTable()
    table.update("p", address(2), False, 100, 7, "e")
    assert table.update("p", address(2), True, None, 6, "e") == []
    assert not table.claim(address(2)).online
    # A snapshot taken before the leave does not bring it back either.
    assert table.snapshot("p", {address(2): 6}, 6, "e") == []
    assert not table.claim(address(2)).online


def test_a_restarted_peer_is_believed_although_its_numbers_start_again():
    table = PresenceTable()
    table.update("p", address(3), True, None, 40, "old")
    assert table.snapshot("p", {address(4): 1}, 1, "new") == [address(3), address(4)]
    assert table.claim(address(3)) is None
    assert table.claim(address(4)).online


async def _scarce_pair(**budgets):
    network = InMemoryNetwork()
    gws = []
    for name in ("a", "b"):
        gw = Gateway(
            GatewayConfig(listen_port=0, attach_grace=0, **budgets),
            verifier=fake_verify,
            backbone=InMemoryBackbone(network, name, rate_class="scarce"),
        )
        gw.port = await gw.start()  # type: ignore[attr-defined]
        gws.append(gw)
    return gws


async def test_the_egress_budget_toward_a_peer_is_shared_by_every_device():
    """The scarce thing is the airtime toward one peer. Two devices each well
    inside their own budget still share that one."""
    gws = await _scarce_pair(
        max_backbone_frame_bytes=100,
        peer_egress_bytes_per_minute=0.0001,
        peer_egress_burst_bytes=150,
        host_backbone_bytes_per_minute=0.0001,
        host_backbone_burst_bytes=150,
    )
    # Every test client is 127.0.0.1, so lift the per-host budget out of the
    # way to see the shared one on its own.
    from offline_gateway.budget import BucketMap

    gws[0]._backbone_budget = BucketMap(1000.0, 100000.0)
    try:
        alice, carol = await Client.connect(gws[0].port), await Client.connect(gws[0].port)
        bob = await Client.connect(gws[1].port)
        await alice.attach(address(1))
        await carol.attach(address(3))
        await bob.attach(address(2))
        await alice.submit(address(2), b"x" * 100, message_id="a1")
        assert (await alice.expect("MessageSent"))["message_id"] == "a1"
        await carol.submit(address(2), b"x" * 100, message_id="c1")
        assert (await carol.expect("DeliveryError"))["reason"] == "budget_exceeded"
        # Smaller frames still fit what is left.
        await carol.submit(address(2), b"x" * 40, message_id="c2")
        assert (await carol.expect("MessageSent"))["message_id"] == "c2"
    finally:
        for gw in gws:
            await gw.stop()


async def test_a_frame_the_egress_budget_refuses_costs_the_device_nothing():
    gws = await _scarce_pair(
        max_backbone_frame_bytes=100,
        peer_egress_bytes_per_minute=0.0001,
        peer_egress_burst_bytes=100,
        host_backbone_bytes_per_minute=0.0001,
        host_backbone_burst_bytes=100,
    )
    from offline_gateway.budget import BucketMap

    # A host budget with room for both frames and no refill, so what is left
    # afterwards says exactly what the refused frame was charged.
    gws[0]._backbone_budget = BucketMap(0.0, 1000.0)
    try:
        alice, bob = await Client.connect(gws[0].port), await Client.connect(gws[1].port)
        await alice.attach(address(1))
        await bob.attach(address(2))
        await alice.submit(address(2), b"x" * 90, message_id="ok")
        await alice.expect("MessageSent")
        await alice.submit(address(2), b"x" * 90, message_id="refused")
        assert (await alice.expect("DeliveryError"))["reason"] == "budget_exceeded"
        # Only the first frame was charged: 1000 - 90 left, not 1000 - 180.
        assert gws[0]._backbone_budget.has("127.0.0.1", 910)
    finally:
        for gw in gws:
            await gw.stop()


def test_negative_rates_are_refused_at_load():
    from offline_gateway import ConfigError

    for name in ("device_per_minute", "host_backbone_bytes_per_minute", "peer_per_minute", "peer_egress_bytes_per_minute"):
        with pytest.raises(ConfigError):
            GatewayConfig(**{name: -1}).validate()


def test_one_host_may_not_be_allowed_more_than_the_link_toward_a_peer():
    from offline_gateway import ConfigError

    with pytest.raises(ConfigError):
        GatewayConfig(host_backbone_bytes_per_minute=2000, peer_egress_bytes_per_minute=1500).validate()
    with pytest.raises(ConfigError):
        GatewayConfig(host_backbone_burst_bytes=30000, peer_egress_burst_bytes=24576).validate()


async def test_a_forward_outlives_the_device_that_sent_it(pair):
    """A device that disconnects mid-forward does not cancel a frame already
    on its way: finishing it is what reaches the recipient."""
    _, a, b = pair
    b.backbone.delay = 0.3
    alice, bob = await Client.connect(a.port), await Client.connect(b.port)
    await alice.attach(address(1))
    await bob.attach(address(2))
    await alice.submit(address(2), b"in flight", message_id="orphan")
    await asyncio.sleep(0.05)
    alice.close()
    got = await bob.expect("MessageReceived", timeout=2)
    assert base64.b64decode(got["content"]) == b"in flight"


def test_a_forgotten_address_is_not_revived_by_a_late_resend():
    """Our link to the peer dropped and its claims were forgotten. The new
    snapshot no longer lists X, which left meanwhile. A resend of X's old
    attach, still in flight on the peer's own link, must not bring it back."""
    table = PresenceTable()
    table.update("p", address(7), True, None, 2, "e")
    table.drop_peer("p")
    table.snapshot("p", {}, 9, "e")
    assert table.update("p", address(7), True, None, 2, "e") == []
    assert table.claim(address(7)) is None
    # A change newer than the snapshot is still news.
    assert table.update("p", address(7), True, None, 10, "e") == [address(7)]


def test_a_snapshot_still_applies_its_own_entries():
    table = PresenceTable()
    table.snapshot("p", {address(1): 3}, 5, "e")
    table.snapshot("p", {address(1): 3, address(2): 6}, 8, "e")
    assert table.claim(address(1)).online and table.claim(address(2)).online


def test_a_snapshot_restores_a_long_attached_device_after_a_dropped_link():
    """X attached at 3 and is still attached. The link dropped (claims
    forgotten) after a snapshot at 5; the next snapshot lists X at 3 again
    and must bring it back, floor or no floor."""
    table = PresenceTable()
    table.snapshot("p", {address(3): 3}, 5, "e")
    table.drop_peer("p")
    assert table.snapshot("p", {address(3): 3}, 7, "e") == [address(3)]
    assert table.claim(address(3)).online


def test_a_change_older_than_a_snapshot_is_refused_even_over_a_held_claim():
    table = PresenceTable()
    table.update("p", address(8), False, 100, 2, "e")
    table.snapshot("p", {}, 4, "e")
    assert table.update("p", address(8), True, None, 3, "e") == []
    assert not table.claim(address(8)).online


def test_a_stale_snapshot_does_not_undo_a_newer_one():
    table = PresenceTable()
    table.snapshot("p", {address(9): 1}, 5, "e")
    table.snapshot("p", {}, 9, "e")
    table.drop_peer("p")
    assert table.snapshot("p", {address(9): 1}, 5, "e") == []
    assert table.claim(address(9)) is None
