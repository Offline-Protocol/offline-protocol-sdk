"""The device-facing contract, one MUST at a time."""

from __future__ import annotations

import asyncio
import base64

import pytest

from conftest import Client, address, fake_sign, fake_verify
from offline_gateway import Gateway, GatewayConfig


async def test_attach_runs_in_the_contracts_order(gateway):
    alice = await Client.connect(gateway.port)
    frames = await alice.attach(address(1))
    assert [f["type"] for f in frames] == ["AddressDeclared", "Capabilities", "StatusUpdate"]
    assert frames[0]["address"] == address(1)
    assert frames[1]["tokens"] == ["gateway_v1"]
    assert frames[2]["status"] == "connected"
    assert address(1) in gateway.bound


async def test_the_challenge_is_fresh_per_connection_and_32_bytes(gateway):
    seen = set()
    for _ in range(3):
        c = await Client.connect(gateway.port)
        await c.send({"type": "Identify", "device_id": "x", "protocol_version": 1})
        frame = await c.expect("Challenge")
        assert frame["protocol_version"] == 1
        nonce = base64.b64decode(frame["challenge"])
        assert len(nonce) == 32
        seen.add(nonce)
        c.close()
    assert len(seen) == 3


async def test_a_declaration_without_a_challenge_is_refused(gateway):
    c = await Client.connect(gateway.port)
    key, sig = fake_sign(address(1), b"\x00" * 32)
    await c.send(
        {
            "type": "DeclareAddress",
            "address": address(1),
            "public_key": base64.b64encode(key).decode(),
            "signature": base64.b64encode(sig).decode(),
        }
    )
    await c.expect("AddressError")
    assert not gateway.bound


async def test_a_proof_for_another_address_is_refused(gateway):
    """The attack the derivation check exists for: a valid signature under the
    sender's own key, naming somebody else's address."""
    c = await Client.connect(gateway.port)
    await c.send({"type": "Identify", "device_id": "x", "protocol_version": 1})
    challenge = base64.b64decode((await c.expect("Challenge"))["challenge"])
    key, sig = fake_sign(address(1), challenge)
    await c.send(
        {
            "type": "DeclareAddress",
            "address": address(2),
            "public_key": base64.b64encode(key).decode(),
            "signature": base64.b64encode(sig).decode(),
        }
    )
    await c.expect("AddressError")
    assert not gateway.bound
    # Refused means unbound: it may not even submit.
    await c.submit(address(1), b"x")
    verdict = await c.expect("DeliveryError")
    assert verdict["reason"] == "attach_required"


async def test_a_challenge_is_spent_by_the_declaration_it_binds(gateway):
    c = await Client.connect(gateway.port)
    await c.attach(address(1))
    # A second Identify on the same connection gets no fresh challenge, and a
    # replayed declaration is refused.
    await c.send({"type": "Identify", "device_id": "x", "protocol_version": 1})
    await c.nothing_for(0.2)
    key, sig = fake_sign(address(1), b"\x00" * 32)
    await c.send(
        {
            "type": "DeclareAddress",
            "address": address(1),
            "public_key": base64.b64encode(key).decode(),
            "signature": base64.b64encode(sig).decode(),
        }
    )
    await c.expect("AddressError")


async def test_repeated_bad_declarations_close_the_session(gateway):
    c = await Client.connect(gateway.port)
    await c.send({"type": "Identify", "device_id": "x", "protocol_version": 1})
    await c.expect("Challenge")
    for _ in range(3):
        await c.send({"type": "DeclareAddress", "address": address(1), "public_key": "AA==", "signature": "AA=="})
        await c.expect("AddressError")
    assert await c.closed()


async def test_unknown_types_and_garbage_lines_are_ignored_not_fatal(gateway):
    c = await Client.connect(gateway.port)
    c.writer.write(b"not json\n[1,2]\n{\"no\":\"type\"}\n")
    await c.send({"type": "SomethingFromTheFuture", "x": 1})
    frames = await c.attach(address(1))
    assert frames[0]["type"] == "AddressDeclared"


async def test_a_line_over_the_bound_closes_the_connection(gateway):
    c = await Client.connect(gateway.port)
    c.writer.write(b"x" * ((1 << 20) + 10))
    await c.writer.drain()
    assert await c.closed(5.0)


async def test_a_local_frame_is_delivered_and_answered_message_sent(gateway):
    alice, bob = await Client.connect(gateway.port), await Client.connect(gateway.port)
    await alice.attach(address(1))
    await bob.attach(address(2))
    await alice.send(
        {
            "type": "SendMessage",
            "recipient": address(2),
            "content": base64.b64encode(b"ciphertext").decode(),
            "encoding": "base64",
            "message_id": "abc-1",
            "reply_to_msg": "abc-0",
        }
    )
    verdict = await alice.expect("MessageSent")
    assert verdict == {"type": "MessageSent", "message_id": "abc-1", "recipient": address(2)}
    delivered = await bob.expect("MessageReceived")
    assert delivered["sender"] == address(1)
    assert base64.b64decode(delivered["content"]) == b"ciphertext"
    assert delivered["encoding"] == "base64"
    assert delivered["message_id"] == "abc-1"
    assert delivered["reply_to_msg"] == "abc-0"


async def test_an_absent_recipient_is_recipient_unreachable(gateway):
    alice = await Client.connect(gateway.port)
    await alice.attach(address(1))
    await alice.submit(address(9), b"x", message_id="m-9")
    verdict = await alice.expect("DeliveryError")
    assert verdict["message_id"] == "m-9"
    assert verdict["recipient"] == address(9)
    assert verdict["reason"].startswith("recipient_unreachable")
    assert "stored" not in verdict


async def test_ids_are_echoed_when_valid_and_minted_otherwise(gateway):
    alice = await Client.connect(gateway.port)
    await alice.attach(address(1))
    await alice.submit(address(9), b"x", message_id=None)
    minted = (await alice.expect("DeliveryError"))["message_id"]
    assert minted
    await alice.submit(address(9), b"x", message_id="not a valid id!")
    assert (await alice.expect("DeliveryError"))["message_id"] not in ("not a valid id!", minted)
    await alice.submit(address(9), b"x", message_id="u" * 64)
    assert (await alice.expect("DeliveryError"))["message_id"] == "u" * 64


async def test_every_submission_gets_exactly_one_verdict(gateway):
    alice, bob = await Client.connect(gateway.port), await Client.connect(gateway.port)
    await alice.attach(address(1))
    await bob.attach(address(2))
    for i in range(10):
        await alice.submit(address(2) if i % 2 else address(9), b"x", message_id=f"id{i}")
    verdicts = [await alice.recv() for _ in range(10)]
    assert sorted(v["message_id"] for v in verdicts) == sorted(f"id{i}" for i in range(10))
    await alice.nothing_for(0.2)


async def test_malformed_submissions_are_plain_failures(gateway):
    alice = await Client.connect(gateway.port)
    await alice.attach(address(1))
    await alice.send({"type": "SendMessage", "recipient": address(2), "message_id": "a"})
    assert (await alice.expect("DeliveryError"))["reason"].startswith("malformed_request")
    await alice.send({"type": "SendMessage", "recipient": "nobody", "content": "AA==", "encoding": "base64", "message_id": "b"})
    frame = await alice.expect("DeliveryError")
    assert frame["reason"].startswith("malformed_request")
    assert not frame["reason"].startswith("recipient_unreachable")


async def test_an_oversized_frame_is_frame_too_large():
    gw = Gateway(GatewayConfig(listen_port=0, attach_grace=0, max_frame_bytes=100), verifier=fake_verify)
    port = await gw.start()
    try:
        alice = await Client.connect(port)
        await alice.attach(address(1))
        await alice.submit(address(1), b"x" * 101)
        assert (await alice.expect("DeliveryError"))["reason"] == "frame_too_large"
    finally:
        await gw.stop()


async def test_the_device_budget_refuses_with_budget_exceeded():
    gw = Gateway(
        GatewayConfig(listen_port=0, attach_grace=0, device_per_minute=0.0001, device_burst=3),
        verifier=fake_verify,
    )
    port = await gw.start()
    try:
        alice = await Client.connect(port)
        await alice.attach(address(1))
        for i in range(4):
            await alice.submit(address(9), b"x", message_id=f"b{i}")
        reasons = [(await alice.recv())["reason"] for _ in range(4)]
        assert reasons[:3] == ["recipient_unreachable: not attached to this gateway"] * 3
        assert reasons[3] == "budget_exceeded"
    finally:
        await gw.stop()


async def test_presence_answers_and_watches(gateway):
    alice = await Client.connect(gateway.port)
    await alice.attach(address(1))
    await alice.send({"type": "CheckPresence", "peers": [address(2)]})
    offline = await alice.expect("PresenceStatus")
    assert offline["peer"] == address(2) and offline["online"] is False
    assert "last_seen_ms" not in offline

    bob = await Client.connect(gateway.port)
    await bob.attach(address(2))
    pushed = await alice.expect("PresenceStatus")
    assert pushed["peer"] == address(2) and pushed["online"] is True

    bob.close()
    left = await alice.expect("PresenceStatus")
    assert left["online"] is False and isinstance(left["last_seen_ms"], int)


async def test_an_unbound_session_learns_nothing_from_presence(gateway):
    c = await Client.connect(gateway.port)
    await c.send({"type": "CheckPresence", "peers": [address(1)]})
    await c.nothing_for(0.2)


async def test_presence_answers_at_most_64_peers(gateway):
    alice = await Client.connect(gateway.port)
    await alice.attach(address(1))
    await alice.send({"type": "CheckPresence", "peers": [address(100 + i) for i in range(70)]})
    for _ in range(64):
        await alice.expect("PresenceStatus")
    await alice.nothing_for(0.2)


async def test_a_reattach_supersedes_the_old_session(gateway):
    first = await Client.connect(gateway.port)
    await first.attach(address(1))
    second = await Client.connect(gateway.port)
    await second.attach(address(1))
    assert await first.closed()
    sender = await Client.connect(gateway.port)
    await sender.attach(address(2))
    await sender.submit(address(1), b"hi")
    await sender.expect("MessageSent")
    assert base64.b64decode((await second.expect("MessageReceived"))["content"]) == b"hi"
    assert address(1) in gateway.bound


async def test_an_undeclared_session_is_announced_verdict_only_after_the_grace():
    gw = Gateway(GatewayConfig(listen_port=0, attach_grace=10), verifier=fake_verify)
    gw.config.attach_grace = 0.2  # past validation, to keep the test fast
    port = await gw.start()
    try:
        c = await Client.connect(port)
        await c.expect("Capabilities", timeout=2)
        await c.expect("StatusUpdate", timeout=2)
        await c.submit(address(2), b"x")
        assert (await c.expect("DeliveryError"))["reason"] == "attach_required"
    finally:
        await gw.stop()


def test_a_grace_shorter_than_the_clients_attach_timeout_is_refused():
    from offline_gateway import ConfigError

    with pytest.raises(ConfigError):
        GatewayConfig(attach_grace=5).validate()


def test_a_backbone_verdict_timeout_past_the_clients_is_refused():
    from offline_gateway import ConfigError

    with pytest.raises(ConfigError):
        GatewayConfig(backbone_verdict_timeout=58).validate()


async def test_a_slow_reader_is_dropped_rather_than_buffered(gateway):
    gateway.config.max_queued_frames = 4
    reader = await Client.connect(gateway.port)
    await reader.attach(address(2))
    session = gateway.bound[address(2)]
    for _ in range(100):
        if not session.send({"type": "Filler", "x": "y" * 1000}):
            break
    assert session.closed or session._queue.qsize() <= 512


async def test_idle_sockets_cannot_lock_a_device_out():
    """Anyone on the network can open sockets that never attach. Past the
    unbound pool, the oldest of them is evicted, and a device still attaches."""
    gw = Gateway(
        GatewayConfig(listen_port=0, attach_grace=0, max_sessions=6, max_unbound_sessions=3, max_sessions_per_host=2),
        verifier=fake_verify,
    )
    # Every test client is 127.0.0.1, so the per-host cap is lifted past
    # validation to exercise the unbound pool from one host.
    gw.config.max_sessions_per_host = 64
    port = await gw.start()
    try:
        idle = [await Client.connect(port) for _ in range(10)]
        await asyncio.sleep(0.1)
        assert sum(1 for s in gw.sessions if not s.closed) <= 3
        assert await idle[0].closed()
        device = await Client.connect(port)
        frames = await device.attach(address(1))
        assert frames[0]["type"] == "AddressDeclared"
    finally:
        await gw.stop()


async def test_bound_devices_are_never_evicted_for_newcomers():
    gw = Gateway(
        GatewayConfig(listen_port=0, attach_grace=0, max_sessions=2, max_unbound_sessions=2, max_sessions_per_host=1),
        verifier=fake_verify,
    )
    gw.config.max_sessions_per_host = 64
    gw.config.max_unbound_sessions = 1
    port = await gw.start()
    try:
        a = await Client.connect(port)
        await a.attach(address(1))
        b = await Client.connect(port)
        await b.attach(address(2))
        late = await Client.connect(port)
        assert await late.closed()
        await a.submit(address(2), b"still here")
        await a.expect("MessageSent")
    finally:
        await gw.stop()


async def test_one_host_cannot_hold_more_than_its_share():
    gw = Gateway(
        GatewayConfig(listen_port=0, attach_grace=0, max_sessions_per_host=2),
        verifier=fake_verify,
    )
    port = await gw.start()
    try:
        first, second = await Client.connect(port), await Client.connect(port)
        await first.attach(address(1))
        await second.attach(address(2))
        third = await Client.connect(port)
        assert await third.closed()
    finally:
        await gw.stop()
