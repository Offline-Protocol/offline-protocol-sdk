"""Shared fixtures for offline-protocol-sdk tests."""

from __future__ import annotations

import threading

import pytest

from offline_protocol_sdk.offline_protocol import (
    MlsStorageError,
    MlsStorageProvider,
    OverflowPolicy,
    ProtocolConfig,
)


class InMemoryStorage(MlsStorageProvider):
    """Thread-safe dict-backed MlsStorageProvider for testing.

    Mirrors the locking behaviour of :class:`SecureStorage` so that tests
    exercise the same concurrency contract the real storage provides.
    """

    def __init__(self, *args: object, **kwargs: object) -> None:
        self._data: dict[tuple[str, str], bytes] = {}
        self._lock = threading.Lock()

    def store(self, key_type: str, key_id: str, data: list[int]) -> None:
        with self._lock:
            self._data[(key_type, key_id)] = bytes(data)

    def load(self, key_type: str, key_id: str) -> list[int] | None:
        with self._lock:
            raw = self._data.get((key_type, key_id))
            if raw is None:
                return None
            return list(raw)

    def delete(self, key_type: str, key_id: str) -> None:
        with self._lock:
            self._data.pop((key_type, key_id), None)

    def list_keys(self, key_type: str) -> list[str]:
        with self._lock:
            return [kid for (kt, kid) in self._data if kt == key_type]


@pytest.fixture
def in_memory_storage() -> InMemoryStorage:
    return InMemoryStorage()


@pytest.fixture(autouse=True)
def _stub_default_storage(monkeypatch: pytest.MonkeyPatch) -> None:
    """Prevent tests from hitting real platform storage.

    ``ProtocolManager`` instantiates ``SecureStorage()`` by default, which
    on macOS prompts for the login-keychain password on every access.
    Redirect the symbol imported by ``protocol_manager`` to an in-memory
    stand-in so tests run unattended.
    """
    from offline_protocol_sdk import protocol_manager as pm_module

    monkeypatch.setattr(pm_module, "SecureStorage", InMemoryStorage)
    monkeypatch.setattr(pm_module, "AppStateStorage", InMemoryStorage)


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
        nostr_enabled=False,
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
