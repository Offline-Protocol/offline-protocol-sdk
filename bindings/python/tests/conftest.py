"""Shared fixtures for offline-protocol-sdk tests."""

from __future__ import annotations

import pytest

from offline_protocol_sdk.offline_protocol import (
    MlsStorageError,
    MlsStorageProvider,
    OverflowPolicy,
    ProtocolConfig,
)


class InMemoryStorage(MlsStorageProvider):
    """Dict-backed MlsStorageProvider for testing (no keyring dependency)."""

    def __init__(self) -> None:
        self._data: dict[tuple[str, str], bytes] = {}

    def store(self, key_type: str, key_id: str, data: list[int]) -> None:
        self._data[(key_type, key_id)] = bytes(data)

    def load(self, key_type: str, key_id: str) -> list[int] | None:
        raw = self._data.get((key_type, key_id))
        if raw is None:
            return None
        return list(raw)

    def delete(self, key_type: str, key_id: str) -> None:
        self._data.pop((key_type, key_id), None)

    def list_keys(self, key_type: str) -> list[str]:
        return [kid for (kt, kid) in self._data if kt == key_type]


@pytest.fixture
def in_memory_storage() -> InMemoryStorage:
    return InMemoryStorage()


@pytest.fixture
def default_config() -> ProtocolConfig:
    """A minimal ProtocolConfig with only Internet enabled."""
    return ProtocolConfig(
        app_id="test-app",
        user_id="test-user-1",
        ble_enabled=False,
        wifi_direct_enabled=False,
        internet_enabled=True,
        reticulum_enabled=False,
        prefer_online=True,
        initial_ttl=3,
        encryption_enabled=True,
        auto_key_exchange=True,
        store_pending=True,
        require_encryption=False,
        max_pending_per_peer=100,
        max_pending_global=1000,
        pending_ttl_ms=60000,
        overflow_policy=OverflowPolicy.DROP_OLDEST,
    )
