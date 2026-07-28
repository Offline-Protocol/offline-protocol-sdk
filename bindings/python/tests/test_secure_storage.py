"""Tests for the SecureStorage (keyring-backed MlsStorageProvider)."""

from __future__ import annotations

from unittest.mock import MagicMock, patch

import pytest

from offline_protocol_sdk.secure_storage import SecureStorage


@pytest.fixture
def storage() -> SecureStorage:
    return SecureStorage(service="test-offline-mls")


class TestSecureStorage:
    """Unit tests using a mocked keyring backend."""

    @patch("offline_protocol_sdk.secure_storage.keyring")
    def test_store_and_load(self, mock_kr: MagicMock) -> None:
        import base64

        store = SecureStorage(service="test-svc")
        data = [72, 101, 108, 108, 111]  # "Hello"

        # index_get reads the index entry first; return None (empty index)
        mock_kr.get_password.return_value = None

        # Store
        store.store("identity", "key-1", data)
        assert mock_kr.set_password.call_count >= 1  # data + index

        # Load — simulate keyring returning base64
        encoded = base64.b64encode(bytes(data)).decode("ascii")
        mock_kr.get_password.return_value = encoded

        result = store.load("identity", "key-1")
        assert result == data

    @patch("offline_protocol_sdk.secure_storage.keyring")
    def test_load_missing_key_returns_none(self, mock_kr: MagicMock) -> None:
        store = SecureStorage(service="test-svc")
        mock_kr.get_password.return_value = None
        assert store.load("identity", "nonexistent") is None

    @patch("offline_protocol_sdk.secure_storage.keyring")
    def test_delete(self, mock_kr: MagicMock) -> None:
        store = SecureStorage(service="test-svc")

        # Simulate empty index
        mock_kr.get_password.return_value = None

        store.delete("identity", "key-1")
        # Expect 2 calls: one for the data key, one for the empty index cleanup
        assert mock_kr.delete_password.call_count == 2

    @patch("offline_protocol_sdk.secure_storage.keyring")
    def test_list_keys_empty(self, mock_kr: MagicMock) -> None:
        store = SecureStorage(service="test-svc")
        mock_kr.get_password.return_value = None
        assert store.list_keys("identity") == []

    @patch("offline_protocol_sdk.secure_storage.keyring")
    def test_list_keys_with_entries(self, mock_kr: MagicMock) -> None:
        import json

        store = SecureStorage(service="test-svc")
        mock_kr.get_password.return_value = json.dumps(["key-1", "key-2"])

        result = store.list_keys("identity")
        assert result == ["key-1", "key-2"]

    @patch("offline_protocol_sdk.secure_storage.keyring")
    def test_namespace_is_part_of_keyring_service(self, mock_kr: MagicMock) -> None:
        store = SecureStorage(service="test-svc", namespace="account-alice")
        mock_kr.get_password.return_value = None

        store.store("identity", "key-1", [1])

        assert mock_kr.set_password.call_args_list[0].args[0] == (
            "test-svc:account-alice"
        )
