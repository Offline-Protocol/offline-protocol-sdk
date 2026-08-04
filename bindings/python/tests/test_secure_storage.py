"""Tests for the SecureStorage (keyring-backed MlsStorageProvider)."""

from __future__ import annotations

import base64
import threading
from unittest.mock import MagicMock, patch

import keyring.errors
import pytest

from offline_protocol_sdk import legacy_store_adoption
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
        #: Service whose writes raise, for the "claim could not be recorded"
        #: path. A credential store that fails one write and serves reads is
        #: exactly the asymmetry the claim read-back exists to catch.
        self.refuse_writes_to: str | None = None
        #: Service whose deletes raise. A store that serves reads and refuses
        #: removals is what leaves a legacy copy alive after ``delete``.
        #:
        #: Deliberately not ``PasswordDeleteError``, which the provider treats
        #: as "already gone" — this is a store that still holds the value.
        self.refuse_deletes_to: str | None = None
        #: Hook run after a successful write, for simulating a concurrent
        #: writer landing between our probe and our read-back.
        self.on_write = None

    def set_password(self, service: str, key: str, value: str) -> None:
        if service == self.refuse_writes_to:
            raise RuntimeError(f"refusing writes to {service}")
        self.entries[(service, key)] = value
        if self.on_write is not None:
            self.on_write(service, key)

    def get_password(self, service: str, key: str) -> str | None:
        return self.entries.get((service, key))

    def delete_password(self, service: str, key: str) -> None:
        if service == self.refuse_deletes_to:
            raise RuntimeError(f"refusing deletes to {service}")
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

    def test_an_unrecordable_claim_does_not_adopt(
        self, fake_keyring: FakeKeyring
    ) -> None:
        # A claim write that fails must not leave read-through on. It would look
        # like a successful adoption to this account while leaving the store
        # unclaimed for the next one, so both would inherit the same MLS
        # identity — and with it each other's sessions and group state.
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "identity", "key_pair", b"old-key"
        )
        fake_keyring.refuse_writes_to = secure_storage_module._LEGACY_SERVICE

        alice = SecureStorage(namespace=ALICE)

        assert alice.legacy_adoption.kind == "claim_unverified"
        assert not alice.legacy_adoption.allows_read_through
        assert alice.load("identity", "key_pair") is None

    def test_only_one_account_inherits_even_if_the_first_claim_failed(
        self, fake_keyring: FakeKeyring
    ) -> None:
        # The invariant is not "the first account to launch wins", it is "at
        # most one account holds a verified claim". Alice's claim never lands,
        # so she does not inherit; Bob's does, so he does — and exactly one of
        # them ends up with the legacy identity.
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "identity", "key_pair", b"old-key"
        )
        fake_keyring.refuse_writes_to = secure_storage_module._LEGACY_SERVICE
        alice = SecureStorage(namespace=ALICE)
        fake_keyring.refuse_writes_to = None

        bob = SecureStorage(namespace=BOB)

        assert alice.legacy_adoption.kind == "claim_unverified"
        assert bob.legacy_adoption.kind == "adopt"
        assert alice.load("identity", "key_pair") is None
        assert bob.load("identity", "key_pair") == list(b"old-key")

    def test_a_claim_taken_between_our_read_and_write_is_a_conflict(
        self, fake_keyring: FakeKeyring
    ) -> None:
        # The read-back also catches a racing claim, which the pre-write probe
        # cannot see.
        from offline_protocol_sdk import legacy_store_adoption

        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "identity", "key_pair", b"old-key"
        )
        claim_key = (
            f"{legacy_store_adoption.CLAIM_KEY_TYPE}:"
            f"{legacy_store_adoption.CLAIM_KEY_ID}"
        )
        fake_keyring.on_write = lambda service, key: (
            fake_keyring.entries.__setitem__(
                (service, key), base64.b64encode(BOB.encode()).decode("ascii")
            )
            if key == claim_key
            else None
        )

        alice = SecureStorage(namespace=ALICE)

        assert alice.legacy_adoption.kind == "conflict"
        assert alice.legacy_adoption.claimed_by == BOB
        assert alice.load("identity", "key_pair") is None

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

    def test_a_legacy_copy_that_will_not_delete_is_tombstoned(
        self, fake_keyring: FakeKeyring
    ) -> None:
        # The removal of the legacy copy can fail on its own. Reporting that by
        # raising is not available — core treats a storage delete as fatal
        # almost everywhere and has no retry — so the key is tombstoned and the
        # delete keeps its promise: nothing hands that material back.
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "key_package", "peer-1", b"pkg"
        )
        store = SecureStorage(namespace=ALICE)
        assert store.load("key_package", "peer-1") == list(b"pkg")
        fake_keyring.refuse_deletes_to = secure_storage_module._LEGACY_SERVICE

        store.delete("key_package", "peer-1")

        assert store.load("key_package", "peer-1") is None
        # The corpse is still there — suppression, not deletion, is what makes
        # this safe, so the test would pass for the wrong reason otherwise.
        legacy_entry = (
            secure_storage_module._LEGACY_SERVICE,
            "key_package:peer-1",
        )
        assert legacy_entry in fake_keyring.entries

    def test_a_tombstoned_key_is_not_listed(self, fake_keyring: FakeKeyring) -> None:
        # listKeys unions the legacy index, so a suppressed entry must be
        # filtered out of it too: advertising a key that cannot be loaded would
        # send core looking for material this store has promised to withhold.
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "key_package", "peer-1", b"pkg"
        )
        store = SecureStorage(namespace=ALICE)
        store.store("key_package", "peer-2", [2])
        fake_keyring.refuse_deletes_to = secure_storage_module._LEGACY_SERVICE

        store.delete("key_package", "peer-1")

        assert store.list_keys("key_package") == ["peer-2"]

    def test_a_tombstone_does_not_shadow_a_fresh_write(
        self, fake_keyring: FakeKeyring
    ) -> None:
        # A tombstone suppresses read-through, not the key. Re-storing under the
        # same id must be readable again — otherwise a key package that failed
        # to clean up would poison its own id for the life of the install.
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "key_package", "peer-1", b"pkg"
        )
        store = SecureStorage(namespace=ALICE)
        fake_keyring.refuse_deletes_to = secure_storage_module._LEGACY_SERVICE
        store.delete("key_package", "peer-1")

        store.store("key_package", "peer-1", [9, 9])

        assert store.load("key_package", "peer-1") == [9, 9]
        assert store.list_keys("key_package") == ["peer-1"]

    def test_a_tombstone_is_retired_once_the_legacy_copy_goes(
        self, fake_keyring: FakeKeyring
    ) -> None:
        # The failure that stranded the copy may be transient. A later read
        # retries the removal, and once it lands the tombstone has nothing to
        # suppress and goes with it.
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "key_package", "peer-1", b"pkg"
        )
        store = SecureStorage(namespace=ALICE)
        fake_keyring.refuse_deletes_to = secure_storage_module._LEGACY_SERVICE
        store.delete("key_package", "peer-1")

        fake_keyring.refuse_deletes_to = None
        assert store.load("key_package", "peer-1") is None

        legacy_entry = (
            secure_storage_module._LEGACY_SERVICE,
            "key_package:peer-1",
        )
        assert legacy_entry not in fake_keyring.entries
        tombstone = (
            f"{secure_storage_module._DEFAULT_SERVICE}:{ALICE}",
            f"{legacy_store_adoption.TOMBSTONE_KEY_TYPE}:key_package:peer-1",
        )
        assert tombstone not in fake_keyring.entries

    def test_a_delete_that_can_neither_remove_nor_tombstone_is_reported(
        self, fake_keyring: FakeKeyring
    ) -> None:
        # The one case that must raise. With the legacy copy alive and the
        # tombstone unwritable there is no way to keep the delete's promise,
        # and a store failing both is failing everything else too.
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "key_package", "peer-1", b"pkg"
        )
        store = SecureStorage(namespace=ALICE)
        fake_keyring.refuse_deletes_to = secure_storage_module._LEGACY_SERVICE
        fake_keyring.refuse_writes_to = f"{secure_storage_module._DEFAULT_SERVICE}:{ALICE}"

        with pytest.raises(Exception, match="could not tombstone"):
            store.delete("key_package", "peer-1")

    def test_tombstones_are_never_exposed_as_key_material(
        self, fake_keyring: FakeKeyring
    ) -> None:
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "key_package", "peer-1", b"pkg"
        )
        store = SecureStorage(namespace=ALICE)
        fake_keyring.refuse_deletes_to = secure_storage_module._LEGACY_SERVICE
        store.delete("key_package", "peer-1")

        assert (
            store.load(
                legacy_store_adoption.TOMBSTONE_KEY_TYPE,
                legacy_store_adoption.tombstone_key_id("key_package", "peer-1"),
            )
            is None
        )
        assert store.list_keys(legacy_store_adoption.TOMBSTONE_KEY_TYPE) == []

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

    def test_two_accounts_adopting_concurrently_cannot_both_win(
        self, fake_keyring: FakeKeyring
    ) -> None:
        # Reading the claim back is not on its own enough to make inheritance
        # exclusive. It closes a write that silently failed, and a second
        # account claiming between our probe and our write. It does not close
        # two accounts interleaving like this:
        #
        #     A.probe -> None          B.probe -> None
        #     A.write(ALICE)
        #     A.read back -> ALICE  => adopt
        #                              B.write(BOB)
        #                              B.read back -> BOB  => adopt
        #
        # Both adopt, both promote the same MLS signing identity, and each ends
        # up holding the other's sessions and group state — silently. The
        # invariant is "at most one account holds a verified claim", which an
        # unsynchronised read-modify-write does not provide.
        #
        # The barrier below is what forces that interleaving: each provider
        # blocks *after* its probe has read, and before it can write, until the
        # other has also probed. It has to sit after the read rather than before
        # it — the whole probe/write/read-back sequence is in-memory dict work
        # that CPython runs inside one GIL slice, so releasing both threads
        # before the probe just lets the first finish before the second starts,
        # and the race never reproduces.
        #
        # Serialised, the second provider never reaches its probe while the
        # first holds the lock, so the barrier times out and the sequences stay
        # whole — which is the assertion. Unserialised, both probe an unclaimed
        # store and both adopt.
        fake_keyring.seed(
            secure_storage_module._LEGACY_SERVICE, "identity", "key_pair", b"old-key"
        )
        claim_key = (
            f"{legacy_store_adoption.CLAIM_KEY_TYPE}:"
            f"{legacy_store_adoption.CLAIM_KEY_ID}"
        )

        barrier = threading.Barrier(2, timeout=1.0)
        arrived: set[str] = set()
        journal: list[tuple[str, str]] = []
        journal_lock = threading.Lock()

        real_get = fake_keyring.get_password
        real_set = fake_keyring.set_password

        def get_password(service: str, key: str) -> str | None:
            if key != claim_key:
                return real_get(service, key)
            with journal_lock:
                journal.append((threading.current_thread().name, "read"))
                first = threading.current_thread().name not in arrived
                arrived.add(threading.current_thread().name)
            result = real_get(service, key)
            if first:
                # Hold the probe open until the other account has also probed,
                # so an unserialised implementation is guaranteed to see two
                # "unclaimed" answers rather than winning a scheduling race.
                try:
                    barrier.wait()
                except threading.BrokenBarrierError:
                    pass
            return result

        def set_password(service: str, key: str, value: str) -> None:
            if key == claim_key:
                with journal_lock:
                    journal.append((threading.current_thread().name, "write"))
            real_set(service, key, value)

        fake_keyring.get_password = get_password
        fake_keyring.set_password = set_password

        stores: dict[str, SecureStorage] = {}

        def build(namespace: str) -> None:
            stores[namespace] = SecureStorage(namespace=namespace)

        threads = [
            threading.Thread(target=build, args=(namespace,), name=name)
            for name, namespace in (("alice", ALICE), ("bob", BOB))
        ]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(timeout=10)
            assert not thread.is_alive(), "adoption deadlocked"

        adopted = [
            namespace
            for namespace, store in stores.items()
            if store.legacy_adoption.allows_read_through
        ]
        assert len(adopted) == 1, (
            f"exactly one account may inherit the legacy store, got {adopted}; "
            f"claim journal: {journal}"
        )

        # The loser must be told, not silently rotated into a fresh identity.
        loser = next(
            store
            for namespace, store in stores.items()
            if namespace not in adopted
        )
        assert loser.legacy_adoption.kind == "conflict"

        # And only the winner reads through to the legacy key material.
        winner = stores[adopted[0]]
        assert winner.load("identity", "key_pair") == list(b"old-key")
        assert loser.load("identity", "key_pair") is None

        # No interleaving: one account's whole probe/write/read-back sequence
        # completes before the other's begins.
        owners = [name for name, _ in journal]
        assert owners == sorted(owners, key=owners.index), (
            f"claim sequences interleaved, so the lock is not held across "
            f"probe -> write -> read back: {journal}"
        )
