"""Devices for the scenario tests: each one a local API server over its own
engine, with its own file stores, a peer-stream listener on loopback, and a
static peer list. A device can be switched off (its server and engine stop,
its stores are released) and on again over the same stores and port, which
is what a power cycle leaves: the identity, the sessions and every queued
message, and nothing that lived in memory.

Encryption is on and required, as in the container image: a message crosses
only inside an MLS session the engines formed by themselves.
"""

from __future__ import annotations

import asyncio
import os
import shutil
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import pytest

from offline_protocol_sdk.local_api.server import LocalApiServer
from offline_protocol_sdk.offline_protocol import OverflowPolicy, ProtocolConfig
from offline_protocol_sdk.protocol_manager import ProtocolManager
from offline_protocol_sdk.verify.client import VerifyClient

#: Generous for loopback, where a session forms in well under a second; a
#: loaded CI runner is the reason for the margin.
SESSION_TIMEOUT = 30.0
DELIVERY_TIMEOUT = 45.0


def scenario_config(profile: str) -> ProtocolConfig:
    return ProtocolConfig(
        app_id="scenario",
        profile=profile,
        ble_enabled=False,
        wifi_direct_enabled=True,
        internet_enabled=False,
        reticulum_enabled=False,
        nostr_enabled=False,
        prefer_online=False,
        initial_ttl=5,
        encryption_enabled=True,
        auto_key_exchange=True,
        store_pending=True,
        require_encryption=True,
        max_pending_per_peer=100,
        max_pending_global=1000,
        pending_ttl_ms=604_800_000,
        overflow_policy=OverflowPolicy.DROP_OLDEST,
    )


@dataclass
class Device:
    name: str
    root: Path
    key: bytes
    peers: list["Device"] = field(default_factory=list)
    port: int = 0
    address: str | None = None
    server: LocalApiServer | None = None

    @property
    def socket_path(self) -> Path:
        return self.root / "run" / "api.sock"

    @property
    def on(self) -> bool:
        return self.server is not None

    async def switch_on(self, *, state_root: Path | None = None) -> None:
        """Starts the engine over this device's stores, listening on the
        port it had before (any free one the first time), dialling its
        peers. ``state_root`` replaces the protocol-state directory, for a
        test that must show what is lost without it."""
        manager = ProtocolManager(
            scenario_config(self.name),
            store_key=self.key,
            mls_root=self.root / "mls",
            state_root=state_root or self.root / "state",
        )
        manager.peer_stream.configure(
            listen_host="127.0.0.1",
            listen_port=self.port,
            peers=[f"127.0.0.1:{peer.port}" for peer in self.peers],
        )
        server = LocalApiServer(manager, socket_path=self.socket_path, health=False)
        await server.start()
        self.server = server
        self.port = manager.peer_stream.listen_port or 0
        address = manager.local_address
        assert address is not None and (self.address is None or address == self.address), (
            f"{self.name} came back as {address}, not {self.address}"
        )
        self.address = address

    async def switch_off(self) -> None:
        server, self.server = self.server, None
        if server is not None:
            await server.stop()

    def linked_to(self, other: "Device") -> bool:
        return self.server is not None and other.address in self.server.manager.peer_stream.connected_peers()

    async def client(self) -> VerifyClient:
        return await VerifyClient.open(socket_path=self.socket_path)


class Network:
    """Creates devices under one short temporary directory (a Unix socket
    path is limited to about a hundred bytes) and switches them all off at
    the end."""

    def __init__(self) -> None:
        # The release also runs this suite on Windows, where asyncio has no
        # Unix sockets; the verifier's TCP carrier is covered in tests/verify.
        if os.name == "nt":
            pytest.skip("the scenario devices serve the local API on a Unix socket")
        self._tmp =Path(tempfile.mkdtemp(prefix="opnet-", dir="/tmp"))
        self.devices: list[Device] = []
        self._clients: list[VerifyClient] = []

    def device(self, name: str, key_byte: int) -> Device:
        root = self._tmp / name
        (root / "run").mkdir(parents=True, mode=0o700)
        device = Device(name=name, root=root, key=bytes([key_byte] * 32))
        self.devices.append(device)
        return device

    async def client(self, device: Device) -> VerifyClient:
        client = await device.client()
        self._clients.append(client)
        return client

    async def close(self) -> None:
        for client in self._clients:
            await client.close()
        for device in self.devices:
            await device.switch_off()
        shutil.rmtree(self._tmp, ignore_errors=True)


async def until(predicate: Any, timeout: float, what: str) -> None:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while not predicate():
        if loop.time() > deadline:
            raise AssertionError(f"{what}: not within {timeout:g} s")
        await asyncio.sleep(0.05)


class Recorder:
    """Every event one client sees, kept in order, so a test can assert on
    what happened before the outcome as well as on the outcome."""

    def __init__(self, client: VerifyClient) -> None:
        self.client = client
        self.events: list[dict[str, Any]] = []

    async def wait(self, tag: str, timeout: float, **fields: Any) -> dict[str, Any]:
        def matches(event: dict[str, Any]) -> bool:
            return event.get("type") == tag and all(event.get(k) == v for k, v in fields.items())

        for event in self.events:
            if matches(event):
                return event
        try:
            event = await self.client.wait_for(matches, timeout, on_other=self.events.append)
        except TimeoutError:
            raise AssertionError(f"no {tag} {fields} within {timeout:g} s; saw {self.tags()}") from None
        self.events.append(event)
        return event

    def tags(self, message_id: str | None = None) -> list[str]:
        return [
            e["type"] for e in self.events if message_id is None or e.get("message_id") == message_id
        ]
