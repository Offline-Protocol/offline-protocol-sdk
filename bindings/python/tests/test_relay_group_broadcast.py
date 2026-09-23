"""A relay group broadcast reaches a Python member.

A phone that registered its group with the relay sends one
``SendGroupMessage``; the relay fans it out as ``GroupMessageReceived`` and
counts the socket write as delivered, so the sender never re-sends a
per-member copy. The bridge must re-inject that frame in the shape the core
decrypts (``__GROUP_MSG__``, addressed to our own address) or the message is
lost silently.

Two real ``ProtocolManager``s stand in for the phone and the Python member.
Frames are routed by recipient, as the relay does, except the one group
frame under test, which is turned into the relay's broadcast notification and
fed through the bridge's WebSocket handler.
"""

from __future__ import annotations

import asyncio
import contextlib
import json

import pytest

from offline_protocol_sdk.offline_protocol import (
    MessagePriority,
    OverflowPolicy,
    ProtocolConfig,
)
from offline_protocol_sdk.protocol_manager import ProtocolManager

GROUP_MLS_MSG = "__GRP_MLS_MSG__"


def _config(profile: str) -> ProtocolConfig:
    return ProtocolConfig(
        app_id="test-app",
        profile=profile,
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
        require_encryption=True,
        max_pending_per_peer=100,
        max_pending_global=1000,
        pending_ttl_ms=60_000,
        overflow_policy=OverflowPolicy.DROP_OLDEST,
    )


class Peer:
    """The app's side of pairing, run outside the event callback."""

    def __init__(self, profile: str) -> None:
        self.profile = profile
        self.pending: list[dict] = []
        self.seen: list[dict] = []
        self.pm = ProtocolManager(_config(profile), event_handler=self.pending.append)
        self.address = ""

    async def start(self) -> None:
        await self.pm.start()
        self.address = self.pm.local_address or ""
        assert self.address
        self.pm.protocol.internet_status_changed(is_connected=True)

    def key_package(self) -> list[int]:
        return list(self.pm.protocol.mls_get_or_create_key_package().key_package_data)

    def handle_events(self) -> None:
        p = self.pm.protocol
        while self.pending:
            e = self.pending.pop(0)
            self.seen.append(e)
            t = e.get("type")
            if t == "connection_request_received":
                if e.get("key_package"):
                    p.mls_import_key_package(e["sender"], e["key_package"])
                p.accept_connection_request(e["sender"], self.profile, self.key_package())
                self._establish(e["sender"])
            elif t == "connection_accepted":
                if e.get("key_package"):
                    p.mls_import_key_package(e["accepted_by"], e["key_package"])
                self._establish(e["accepted_by"])

    def _establish(self, addr: str) -> None:
        # Best-effort, like the app: with auto_key_exchange the SDK finishes
        # the session once the peer's key package arrives over the wire.
        with contextlib.suppress(Exception):
            self.pm.protocol.establish_secure_session(addr)

    def events(self, kind: str) -> list[dict]:
        return [e for e in self.seen if e.get("type") == kind]


def route(peers: list[Peer], grab=None) -> None:
    """Relay by recipient; ``grab(src, dst, frame)`` returning True swallows the frame."""
    by_addr = {p.address: p for p in peers}
    for src in peers:
        while (m := src.pm.protocol.internet_get_next_message()) is not None:
            dst = by_addr.get(m.recipient_id)
            if dst and not (grab and grab(src, dst, m)):
                dst.pm.protocol.internet_message_received(
                    sender_id=src.address, data=list(m.data)
                )


async def settle(peers: list[Peer], cond, what: str, grab=None, secs: int = 20) -> None:
    for _ in range(secs * 20):
        for p in peers:
            p.handle_events()
        route(peers, grab)
        if cond():
            return
        await asyncio.sleep(0.05)
    raise AssertionError(f"timed out: {what}")


@pytest.mark.asyncio
async def test_relay_group_broadcast_is_decrypted_and_emitted() -> None:
    phone, member = Peer("relay-phone"), Peer("python-member")
    peers = [phone, member]
    for p in peers:
        await p.start()
    try:
        phone.pm.protocol.send_connection_request(
            member.address, phone.profile, phone.key_package(), None
        )
        await settle(
            peers,
            lambda: phone.events("connection_accepted")
            and phone.pm.protocol.has_pending_key_package(member.address),
            "1:1 session",
        )

        gid = phone.pm.protocol.create_group("testers").group_id
        phone.pm.protocol.invite_to_group(gid, member.address)
        await settle(
            peers,
            lambda: any(e.get("group_id") == gid for e in member.events("group_member_added")),
            "join",
        )

        # The phone broadcasts through the relay: the member gets the
        # ciphertext as a relay GroupMessageReceived, not a per-member frame.
        def as_relay(src: Peer, dst: Peer, m) -> bool:
            content = json.loads(bytes(m.data))["content"]
            if dst is not member or GROUP_MLS_MSG not in content:
                return False
            inner = json.loads(content.split(GROUP_MLS_MSG, 1)[1])
            relay_frame = {
                "type": "GroupMessageReceived",
                "group_id": gid,
                "sender": src.address,
                "content": inner["ciphertext"],
                "timestamp": "2026-01-01T00:00:00Z",
                "message_id": inner.get("message_id") or m.message_id,
            }
            dst.pm.internet._process_received(json.dumps(relay_frame).encode())
            return True

        phone.pm.protocol.send_group_message(gid, "via relay", MessagePriority.HIGH, None)

        def received() -> list[dict]:
            return [
                e
                for e in member.events("group_message_received")
                if e.get("group_id") == gid and e.get("content") == "via relay"
            ]

        await settle(peers, received, "relay broadcast", as_relay)
        assert len(received()) == 1
        assert received()[0]["sender"] == phone.address
    finally:
        for p in peers:
            await p.pm.stop()
