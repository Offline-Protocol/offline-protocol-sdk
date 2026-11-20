"""A scripted device client and a stand-in verifier, so the daemon's own rules
are tested without the native library. ``test_end_to_end.py`` runs real SDK
engines against it instead."""

from __future__ import annotations

import asyncio
import base64
import hashlib
import json
import sys
import threading
from pathlib import Path
from typing import Any

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from offline_gateway import Gateway, GatewayConfig  # noqa: E402

#: Stand-in keys: an address's "public key" is fixed, and a "signature" is a
#: hash over key, address and challenge. Enough to exercise every branch of
#: the daemon's attach handling; the real proof is pinned in the SDK.
KEYS: dict[str, bytes] = {}


def address(n: int) -> str:
    name = f"off1q{n:0>10}testaddressxx"
    KEYS.setdefault(name, hashlib.sha256(name.encode()).digest())
    return name


def fake_sign(addr: str, challenge: bytes) -> tuple[bytes, bytes]:
    key = KEYS[addr]
    return key, hashlib.sha256(key + addr.encode() + challenge).digest() * 2


def fake_verify(addr: str, public_key: bytes, signature: bytes, challenge: bytes) -> str:
    if KEYS.get(addr) != public_key:
        raise ValueError("key does not derive to the address")
    if signature != hashlib.sha256(public_key + addr.encode() + challenge).digest() * 2:
        raise ValueError("signature does not verify")
    return addr


class Client:
    def __init__(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        self.reader = reader
        self.writer = writer

    @classmethod
    async def connect(cls, port: int) -> "Client":
        reader, writer = await asyncio.open_connection("127.0.0.1", port, limit=1 << 21)
        return cls(reader, writer)

    async def send(self, frame: dict[str, Any]) -> None:
        self.writer.write(json.dumps(frame).encode() + b"\n")
        await self.writer.drain()

    async def recv(self, timeout: float = 2.0) -> dict[str, Any]:
        line = await asyncio.wait_for(self.reader.readline(), timeout)
        if not line:
            raise EOFError("connection closed")
        return json.loads(line)

    async def expect(self, frame_type: str, timeout: float = 2.0) -> dict[str, Any]:
        frame = await self.recv(timeout)
        assert frame["type"] == frame_type, frame
        return frame

    async def nothing_for(self, seconds: float) -> bool:
        try:
            line = await asyncio.wait_for(self.reader.readline(), seconds)
        except asyncio.TimeoutError:
            return True
        raise AssertionError(f"expected silence, got {line!r}")

    async def closed(self, timeout: float = 2.0) -> bool:
        while True:
            line = await asyncio.wait_for(self.reader.readline(), timeout)
            if not line:
                return True

    async def attach(self, addr: str) -> list[dict[str, Any]]:
        await self.send({"type": "Identify", "device_id": addr, "protocol_version": 1})
        challenge = base64.b64decode((await self.expect("Challenge"))["challenge"])
        key, sig = fake_sign(addr, challenge)
        await self.send(
            {
                "type": "DeclareAddress",
                "address": addr,
                "public_key": base64.b64encode(key).decode(),
                "signature": base64.b64encode(sig).decode(),
            }
        )
        return [await self.recv() for _ in range(3)]

    async def submit(self, recipient: str, content: bytes, message_id: str | None = "m1") -> None:
        frame: dict[str, Any] = {
            "type": "SendMessage",
            "recipient": recipient,
            "content": base64.b64encode(content).decode(),
            "encoding": "base64",
        }
        if message_id is not None:
            frame["message_id"] = message_id
        await self.send(frame)

    def close(self) -> None:
        self.writer.close()


class _MemorySecrets:
    """The SDK's ``SecureStorage`` stand-in, as its own suite defines one:
    keys in memory, never in the platform keychain (which on macOS prompts on
    every access). Locked, because the engine calls it from its own threads."""

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        self._data: dict[tuple[str, str], bytes] = {}
        self._lock = threading.Lock()

    def store(self, key_type: str, key_id: str, data: list[int]) -> None:
        with self._lock:
            self._data[(key_type, key_id)] = bytes(data)

    def load(self, key_type: str, key_id: str) -> list[int] | None:
        with self._lock:
            raw = self._data.get((key_type, key_id))
            return None if raw is None else list(raw)

    def delete(self, key_type: str, key_id: str) -> None:
        with self._lock:
            self._data.pop((key_type, key_id), None)

    def list_keys(self, key_type: str) -> list[str]:
        with self._lock:
            return [kid for (kt, kid) in self._data if kt == key_type]


@pytest.fixture(autouse=True)
def _isolate_sdk_storage(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """Keeps an SDK engine started by a test off the real keychain and out of
    any shared state directory, as the SDK's own suite does."""
    try:
        from offline_protocol_sdk import protocol_manager as pm_module
    except ImportError:
        return
    monkeypatch.setattr(pm_module, "SecureStorage", _MemorySecrets)
    monkeypatch.setenv("OFFLINE_PROTOCOL_STATE_ROOT", str(tmp_path / "protocol-state"))


@pytest.fixture
def config() -> GatewayConfig:
    return GatewayConfig(listen_port=0, attach_grace=0)


@pytest.fixture
async def gateway(config):
    gw = Gateway(config, verifier=fake_verify)
    port = await gw.start()
    gw.port = port  # type: ignore[attr-defined]
    try:
        yield gw
    finally:
        await gw.stop()
