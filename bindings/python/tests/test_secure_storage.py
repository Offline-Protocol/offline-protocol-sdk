"""Tests for the SecureStorage (keyring-backed MlsStorageProvider)."""

from __future__ import annotations

import base64
from unittest.mock import MagicMock, patch

import keyring.errors
import pytest

from offline_protocol_sdk import secure_storage as secure_storage_module
from offline_protocol_sdk.secure_storage import SecureStorage

ALICE = "account-" + "a" * 64
BOB = "account-" + "b" * 64


class FakeKeyring:
    """In-memory stand-in for the ``keyring`` module.

    Keyed by ``(service, key)`` exactly like the real backends, so a test can
    seed a pre-upgrade store under the legacy service name and watch the
    namespaced store inherit from it.
    """

    def __init__(self) -> None:
        self.entries: dict[tuple[str, str], str] = {}
        self.errors = keyring.errors

    def set_password(self, service: str, key: str, value: str) -> None:
        self.entries[(service, key)] = value

    def get_password(self, service: str, key: str) -> str | None:
        return self.entries.get((service, key))

    def delete_password(self, service: str, key: str) -> None:
        if (service, key) not in self.entries:
            raise keyring.errors.PasswordDeleteError(key)
        del self.entries[(service, key)]

    def get_keyring(self) -> object:
        return self

    # -- helpers -------------------------------------------------------------

    def seed(self, service: str, key_type: str, key_id: str, data: bytes) -> None:
        self.entries[(service, f"{key_type}:{key_id}")] = base64.b64encode(data).decode(
            "ascii"
        )
        self.entries[(service, f"index:{key_type}")] = f'["{key_id}"]'


@pytest.fixture
def fake_keyring(monkeypatch: pytest.MonkeyPatch) -> FakeKeyring:
    fake = FakeKeyring()
    monkeypatch.setattr(secure_storage_module, "keyring", fake)
    return fake


@pytest.fixture
def storage() -> SecureStorage:
    return SecureStorage(service="test-offline-mls")


class TestSecureStorage:
    """Unit tests using a mocked keyring backend."""

    @patch("offline_protocol_sdk.secure_storage.keyring")
    def test_store_and_load(self, mock_kr: MagicMock) -> None:
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
        store = SecureStorage(service="test-svc", namespace=ALICE)
        mock_kr.get_password.return_value = None

        store.store("identity", "key-1", [1])

        assert mock_kr.set_password.call_args_list[0].args[0] == f"test-svc:{ALICE}"

    @patch("offline_protocol_sdk.secure_storage.keyring")
    def test_malformed_namespace_is_refused(self, mock_kr: MagicMock) -> None:
        # The namespace becomes part of the credential-store service name, so a
        # caller-supplied string must not be able to collide with another
        # account's store.
        with pytest.raises(ValueError, match="Invalid protocol storage account"):
            SecureStorage(service="test-svc", namespace="../other-account")


class TestLegacyStoreAdoption:
    """Upgrading an install must not silently rotate its MLS identity."""

    def test_upgrade_inherits_the_pre_namespace_identity(
        self, fake_keyring: FakeKeyring
    ) -> None:
        # An install that predates namespacing has its identity under the old,
        # un-suffixed service. Without adoption the new store looks empty,
        # ensure_identity() mints a fresh signing key, and every existing
        # session, group, and TOFU pin is abandoned.
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "identity", "key_pair", b"old-key"
        )

        store = SecureStorage(namespace=ALICE)

        assert store.load("identity", "key_pair") == list(b"old-key")
        assert store.legacy_adoption.kind == "adopt"

    def test_inherited_entries_are_promoted_into_the_namespaced_store(
        self, fake_keyring: FakeKeyring
    ) -> None:
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "identity", "key_pair", b"old-key"
        )
        store = SecureStorage(namespace=ALICE)

        store.load("identity", "key_pair")

        namespaced = f"{secure_storage_module._DEFAULT_SERVICE}:{ALICE}"
        assert (namespaced, "identity:key_pair") in fake_keyring.entries

    def test_adoption_is_resumable_across_launches(
        self, fake_keyring: FakeKeyring
    ) -> None:
        # A launch that claimed the legacy store but died before promoting every
        # entry must keep reading through on the next one.
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "identity", "key_pair", b"old-key"
        )
        first = SecureStorage(namespace=ALICE)
        assert first.legacy_adoption.kind == "adopt"

        second = SecureStorage(namespace=ALICE)

        assert second.legacy_adoption.kind == "resume"
        assert second.load("identity", "key_pair") == list(b"old-key")

    def test_a_second_account_cannot_inherit_the_same_identity(
        self, fake_keyring: FakeKeyring
    ) -> None:
        # The legacy store was shared by every account on the install, so only
        # one can inherit it. The second is genuinely new — but it must say so.
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "identity", "key_pair", b"old-key"
        )
        SecureStorage(namespace=ALICE).load("identity", "key_pair")

        bob = SecureStorage(namespace=BOB)

        assert bob.legacy_adoption.kind == "conflict"
        assert bob.legacy_adoption.claimed_by == ALICE
        assert bob.load("identity", "key_pair") is None

    def test_delete_does_not_resurrect_from_the_legacy_store(
        self, fake_keyring: FakeKeyring
    ) -> None:
        # A delete that left the legacy copy in place would let read-through
        # hand back key material the caller believes is gone.
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "key_package", "peer-1", b"pkg"
        )
        store = SecureStorage(namespace=ALICE)
        assert store.load("key_package", "peer-1") == list(b"pkg")

        store.delete("key_package", "peer-1")

        assert store.load("key_package", "peer-1") is None

    def test_list_keys_unions_the_legacy_index(
        self, fake_keyring: FakeKeyring
    ) -> None:
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "key_package", "peer-1", b"pkg"
        )
        store = SecureStorage(namespace=ALICE)
        store.store("key_package", "peer-2", [2])

        assert sorted(store.list_keys("key_package")) == ["peer-1", "peer-2"]

    def test_claim_entry_is_never_exposed_as_key_material(
        self, fake_keyring: FakeKeyring
    ) -> None:
        from offline_protocol_sdk import legacy_store_adoption

        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "identity", "key_pair", b"old-key"
        )
        store = SecureStorage(namespace=ALICE)

        assert (
            store.load(
                legacy_store_adoption.CLAIM_KEY_TYPE,
                legacy_store_adoption.CLAIM_KEY_ID,
            )
            is None
        )
        assert store.list_keys(legacy_store_adoption.CLAIM_KEY_TYPE) == []

    def test_a_fresh_install_has_nothing_to_adopt(
        self, fake_keyring: FakeKeyring
    ) -> None:
        store = SecureStorage(namespace=ALICE)

        assert store.legacy_adoption.kind == "adopt"
        assert store.load("identity", "key_pair") is None

    def test_adoption_can_be_disabled(self, fake_keyring: FakeKeyring) -> None:
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "identity", "key_pair", b"old-key"
        )

        store = SecureStorage(namespace=ALICE, adopt_legacy_store=False)

        assert store.legacy_adoption.kind == "none"
        assert store.load("identity", "key_pair") is None

    def test_a_namespaceless_store_says_it_cannot_adopt(
        self, fake_keyring: FakeKeyring, caplog: pytest.LogCaptureFixture
    ) -> None:
        # Adoption records its claim under the account namespace, so a store
        # built without one cannot participate — and on the default service
        # that silently mints a fresh MLS identity on upgrade. ProtocolManager
        # always supplies the namespace; a caller constructing its own provider
        # must not have that happen quietly.
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "identity", "key_pair", b"old-key"
        )

        with caplog.at_level("WARNING"):
            store = SecureStorage()

        assert store.legacy_adoption.kind == "none"
        assert store.load("identity", "key_pair") is None
        assert any(
            "account namespace" in record.message for record in caplog.records
        ), caplog.text

    def test_an_explicitly_disabled_store_stays_quiet(
        self, fake_keyring: FakeKeyring, caplog: pytest.LogCaptureFixture
    ) -> None:
        # Opting out is a decision, not an accident — no warning for it.
        with caplog.at_level("WARNING"):
            SecureStorage(adopt_legacy_store=False)

        assert not any(
            "account namespace" in record.message for record in caplog.records
        ), caplog.text
