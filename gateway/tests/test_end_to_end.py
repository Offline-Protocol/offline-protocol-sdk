"""Real SDK engines, through the real client, through this daemon.

Two ``ProtocolManager``s with only the gateway slot enabled attach with the
SDK's own ``GatewayManager``, pair over MLS and exchange a message. The daemon
checks each attach proof with the SDK's ``verify_gateway_address_proof``. The
second case puts the two devices on two gateways joined by the in-process
backbone, which is the shape of a LoRa deployment with the radio left out.

Skipped when the SDK build in this environment lacks the verifier.
"""

from __future__ import annotations

import asyncio
import contextlib

import pytest

sdk = pytest.importorskip("offline_protocol_sdk.offline_protocol")
if not hasattr(sdk, "verify_gateway_address_proof"):  # pragma: no cover
    pytest.skip("this SDK build has no verify_gateway_address_proof", allow_module_level=True)

from offline_protocol_sdk.offline_protocol import (  # noqa: E402
    ContentType,
    MessagePriority,
    OverflowPolicy,
    ProtocolConfig,
)
from offline_protocol_sdk.protocol_manager import ProtocolManager  # noqa: E402

from offline_gateway import Gateway, GatewayConfig, InMemoryBackbone, InMemoryNetwork  # noqa: E402


def _config(profile: str) -> ProtocolConfig:
    return ProtocolConfig(
        app_label="gateway-e2e",
        profile=profile,
        ble_enabled=False,
        wifi_direct_enabled=False,
        internet_enabled=False,
        reticulum_enabled=True,
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


class Device:
    """The app's side of pairing, run outside the event callback."""

    def __init__(self, profile: str, port: int) -> None:
        self.profile = profile
        self.port = port
        self.pending: list[dict] = []
        self.seen: list[dict] = []
        self.pm = ProtocolManager(_config(profile), event_handler=self.pending.append)
        self.address = ""

    async def start(self) -> None:
        await self.pm.start()
        self.address = self.pm.local_address or ""
        assert self.address
        assert self.pm.gateway is not None
        self.pm.gateway.configure(daemon_address=f"127.0.0.1:{self.port}")
        await self.pm.gateway.start()

    async def stop(self) -> None:
        await self.pm.stop()

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
                with contextlib.suppress(Exception):
                    p.establish_secure_session(e["sender"])
            elif t == "connection_accepted":
                if e.get("key_package"):
                    p.mls_import_key_package(e["accepted_by"], e["key_package"])
                with contextlib.suppress(Exception):
                    p.establish_secure_session(e["accepted_by"])

    def events(self, kind: str) -> list[dict]:
        return [e for e in self.seen if e.get("type") == kind]


async def settle(devices: list[Device], cond, what: str, secs: int = 30) -> None:
    for _ in range(secs * 20):
        for d in devices:
            d.handle_events()
        if cond():
            return
        await asyncio.sleep(0.05)
    trail = {d.profile: [e.get("type") for e in d.seen] for d in devices}
    raise AssertionError(f"timed out: {what}; events seen: {trail}")


async def _attached(gateway: Gateway, devices: list[Device]) -> None:
    await settle(devices, lambda: all(d.address in gateway.bound for d in devices), "attach", secs=10)


async def _pair_and_talk(alice: Device, bob: Device, secs: int = 30) -> None:
    devices = [alice, bob]
    alice.pm.protocol.send_connection_request(bob.address, alice.profile, alice.key_package(), None)
    await settle(
        devices,
        lambda: alice.events("connection_accepted")
        and alice.pm.protocol.has_pending_key_package(bob.address),
        "pairing",
        secs,
    )
    alice.pm.protocol.send_message(bob.address, "over the gateway", MessagePriority.HIGH, None)

    def received() -> list[dict]:
        return [e for e in bob.events("message_received") if e.get("content") == "over the gateway"]

    await settle(devices, received, "delivery", secs)
    assert received()[0]["sender"] == alice.address


async def test_two_engines_pair_and_talk_through_one_gateway() -> None:
    gateway = Gateway(GatewayConfig(listen_port=0))
    port = await gateway.start()
    alice, bob = Device("e2e-alice", port), Device("e2e-bob", port)
    try:
        for d in (alice, bob):
            await d.start()
        await _attached(gateway, [alice, bob])
        await _pair_and_talk(alice, bob)
    finally:
        for d in (alice, bob):
            await d.stop()
        await gateway.stop()


@pytest.mark.parametrize("rate_class", ["broad", "scarce"])
async def test_two_engines_talk_across_two_gateways(rate_class: str) -> None:
    """Under the scarce class the default per-device backbone budget applies,
    and a first contact has to fit inside it."""
    network = InMemoryNetwork()
    a = Gateway(GatewayConfig(listen_port=0), backbone=InMemoryBackbone(network, "a", rate_class))
    b = Gateway(GatewayConfig(listen_port=0), backbone=InMemoryBackbone(network, "b", rate_class))
    port_a, port_b = await a.start(), await b.start()
    alice, bob = Device("e2e-far-alice", port_a), Device("e2e-far-bob", port_b)
    try:
        for d in (alice, bob):
            await d.start()
        await _attached(a, [alice])
        await _attached(b, [bob])
        await _pair_and_talk(alice, bob)
    finally:
        for d in (alice, bob):
            await d.stop()
        await a.stop()
        await b.stop()


class _Recording(InMemoryBackbone):
    """Records the size of every frame handed to the backbone."""

    def __init__(self, *args, **kwargs) -> None:
        super().__init__(*args, **kwargs)
        self.carried: list[int] = []

    async def forward(self, frame, timeout):
        self.carried.append(len(frame.content))
        return await super().forward(frame, timeout)


@pytest.mark.parametrize("rate_class", ["broad", "scarce"])
async def test_media_crosses_a_broad_backbone_and_never_a_scarce_one(rate_class: str, caplog) -> None:
    """The contract: media MUST be excluded from a scarce backbone. The
    gateway sees only ciphertext, so the exclusion is by size: the device
    chunks the gateway slot at 64 KiB, and every full chunk is past the
    scarce frame bound and refused `frame_too_large`. A broad backbone
    carries the same transfer to the end."""
    import logging

    caplog.set_level(logging.DEBUG, logger="offline_gateway.gateway")
    network = InMemoryNetwork()
    backbone_a = _Recording(network, "a", rate_class)
    a = Gateway(GatewayConfig(listen_port=0), backbone=backbone_a)
    b = Gateway(GatewayConfig(listen_port=0), backbone=InMemoryBackbone(network, "b", rate_class))
    port_a, port_b = await a.start(), await b.start()
    alice, bob = Device(f"e2e-media-alice-{rate_class}", port_a), Device(f"e2e-media-bob-{rate_class}", port_b)
    try:
        for d in (alice, bob):
            await d.start()
        await _attached(a, [alice])
        await _attached(b, [bob])
        await _pair_and_talk(alice, bob)
        photo = bytes(range(256)) * 80  # 20 KB, one chunk at the slot's size
        alice.pm.protocol.send_media(bob.address, list(photo), "photo.jpg", ContentType.IMAGE, None)

        def received() -> list[dict]:
            return bob.events("file_received")

        if rate_class == "broad":
            await settle([alice, bob], received, "media over a broad backbone")
        else:
            await settle(
                [alice, bob],
                lambda: any("frame_too_large" in r.getMessage() for r in caplog.records),
                "a scarce gateway refusing the chunk",
            )
            await asyncio.sleep(1)
            for d in (alice, bob):
                d.handle_events()
            assert not received()
            bound = GatewayConfig().max_backbone_frame_bytes
            assert max(backbone_a.carried) <= bound
    finally:
        for d in (alice, bob):
            await d.stop()
        await a.stop()
        await b.stop()
