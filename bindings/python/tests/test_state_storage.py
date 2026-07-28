"""Tests for install-scoped protocol-state storage."""

from __future__ import annotations

import os

import pytest

from offline_protocol_sdk import state_storage as state_storage_module
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


def test_state_storage_flushes_the_directory_after_rename_and_unlink(
    tmp_path, monkeypatch
) -> None:
    # fsyncing the temporary file only makes its contents durable; the link
    # os.replace creates (and unlink removes) lives in the parent directory and
    # needs its own flush, or a crash can lose an acknowledged store or
    # resurrect a deleted outbox entry.
    synced: list[str] = []
    real_sync = state_storage_module._sync_directory

    def recording_sync(directory) -> None:
        synced.append(str(directory))
        real_sync(directory)

    monkeypatch.setattr(state_storage_module, "_sync_directory", recording_sync)

    storage = AppStateStorage(tmp_path / "state")
    storage.store("outbox", "message-1", [1, 2, 3])
    assert len(synced) == 1

    storage.delete("outbox", "message-1")
    assert len(synced) == 2

    # A delete of an absent entry changed no directory entry, so it has nothing
    # to flush.
    storage.delete("outbox", "message-1")
    assert len(synced) == 2


def test_sync_directory_is_best_effort_on_platforms_that_refuse_it(
    tmp_path, monkeypatch
) -> None:
    # Windows cannot open a directory for fsync. That must degrade to the
    # file-level fsync, never raise out of a store the caller was told
    # succeeded.
    def refusing_open(*args, **kwargs):
        raise OSError("directories cannot be opened here")

    monkeypatch.setattr(os, "open", refusing_open)

    state_storage_module._sync_directory(tmp_path)


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
