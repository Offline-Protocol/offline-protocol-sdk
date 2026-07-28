"""Tests for install-scoped protocol-state storage."""

from __future__ import annotations

import pytest

from offline_protocol_sdk.state_storage import AppStateStorage


def test_state_storage_round_trip_and_listing(tmp_path) -> None:
    storage = AppStateStorage(tmp_path / "state")

    storage.store("pending_messages", "peer/with punctuation", [0, 1, 255])

    assert storage.load("pending_messages", "peer/with punctuation") == [0, 1, 255]
    assert storage.list_keys("pending_messages") == ["peer/with punctuation"]


def test_state_storage_overwrite_is_atomic_and_delete_is_idempotent(tmp_path) -> None:
    storage = AppStateStorage(tmp_path / "state")

    storage.store("outbox", "message-1", [1, 2, 3])
    storage.store("outbox", "message-1", [4, 5])

    assert storage.load("outbox", "message-1") == [4, 5]
    assert not list((tmp_path / "state").rglob(".write-*"))

    storage.delete("outbox", "message-1")
    storage.delete("outbox", "message-1")
    assert storage.load("outbox", "message-1") is None
    assert storage.list_keys("outbox") == []


def test_state_storage_namespaces_accounts(tmp_path) -> None:
    alice = AppStateStorage(tmp_path / "state", namespace="account-alice")
    bob = AppStateStorage(tmp_path / "state", namespace="account-bob")

    alice.store("outbox", "message-1", [1, 2, 3])

    assert alice.load("outbox", "message-1") == [1, 2, 3]
    assert bob.load("outbox", "message-1") is None


def test_state_storage_requires_an_install_owned_root(monkeypatch) -> None:
    monkeypatch.delenv("OFFLINE_PROTOCOL_STATE_ROOT", raising=False)

    with pytest.raises(ValueError, match="no safe process-wide default"):
        AppStateStorage()


def test_state_storage_uses_configured_install_root(monkeypatch, tmp_path) -> None:
    install_root = tmp_path / "install-a" / "protocol-state"
    monkeypatch.setenv("OFFLINE_PROTOCOL_STATE_ROOT", str(install_root))

    before_reinstall = AppStateStorage(namespace="account-alice")
    before_reinstall.store("outbox", "message-1", [1, 2, 3])

    monkeypatch.setenv(
        "OFFLINE_PROTOCOL_STATE_ROOT",
        str(tmp_path / "install-b" / "protocol-state"),
    )
    after_reinstall = AppStateStorage(namespace="account-alice")

    assert after_reinstall.load("outbox", "message-1") is None
