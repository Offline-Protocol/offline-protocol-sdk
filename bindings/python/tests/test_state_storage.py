"""Tests for install-scoped protocol-state storage."""

from __future__ import annotations

import os

import pytest

from offline_protocol_sdk import state_storage as state_storage_module
from offline_protocol_sdk.offline_protocol import MlsStorageError
from offline_protocol_sdk.state_storage import AppStateStorage

ALICE = "account-" + "a" * 64
BOB = "account-" + "b" * 64


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
    alice = AppStateStorage(tmp_path / "state", namespace=ALICE)
    bob = AppStateStorage(tmp_path / "state", namespace=BOB)

    alice.store("outbox", "message-1", [1, 2, 3])

    assert alice.load("outbox", "message-1") == [1, 2, 3]
    assert bob.load("outbox", "message-1") is None


def test_state_storage_rejects_a_malformed_namespace(tmp_path) -> None:
    # The namespace becomes a directory component, so anything that could escape
    # the account's own root must be refused at the door.
    with pytest.raises(ValueError, match="Invalid protocol storage account"):
        AppStateStorage(tmp_path / "state", namespace="../../other-account")


def test_state_storage_requires_an_install_owned_root(monkeypatch) -> None:
    monkeypatch.delenv("OFFLINE_PROTOCOL_STATE_ROOT", raising=False)

    with pytest.raises(ValueError, match="no safe process-wide default"):
        AppStateStorage()


def test_state_storage_uses_configured_install_root(monkeypatch, tmp_path) -> None:
    install_root = tmp_path / "install-a" / "protocol-state"
    monkeypatch.setenv("OFFLINE_PROTOCOL_STATE_ROOT", str(install_root))

    before_reinstall = AppStateStorage(namespace=ALICE)
    before_reinstall.store("outbox", "message-1", [1, 2, 3])

    monkeypatch.setenv(
        "OFFLINE_PROTOCOL_STATE_ROOT",
        str(tmp_path / "install-b" / "protocol-state"),
    )
    after_reinstall = AppStateStorage(namespace=ALICE)

    assert after_reinstall.load("outbox", "message-1") is None


# -- filesystem-key safety ---------------------------------------------------


def test_case_folding_ids_are_distinct_records(tmp_path) -> None:
    # "AAG" and "AAa" differ only in the case of one base64url character, so an
    # encoding-based filename gives them the same name on a case-insensitive
    # volume (macOS and Windows both default to one) and one record silently
    # overwrites the other. A digest name cannot collide this way.
    storage = AppStateStorage(tmp_path / "state")

    storage.store("outbox", "AAG", [1])
    storage.store("outbox", "AAa", [2])

    assert storage.load("outbox", "AAG") == [1]
    assert storage.load("outbox", "AAa") == [2]
    assert storage.list_keys("outbox") == ["AAG", "AAa"]


def test_maximum_length_ids_round_trip(tmp_path) -> None:
    # Core accepts user ids up to 256 bytes. Base64 of 190 bytes already
    # overruns the 255-byte NAME_MAX most filesystems enforce; a digest name is
    # a fixed 66 characters no matter how long the key is.
    storage = AppStateStorage(tmp_path / "state")
    long_id = "u" * 256

    storage.store("outbox", long_id, [9])

    assert storage.load("outbox", long_id) == [9]
    assert storage.list_keys("outbox") == [long_id]
    assert len(state_storage_module._entry_name("outbox", long_id)) == 66


def test_every_entry_name_is_fixed_length_and_lowercase() -> None:
    for key_id in ("", "AAG", "x" * 4096, "péer/ id"):
        name = state_storage_module._entry_name("outbox", key_id)
        assert len(name) == 66
        assert name == name.lower()


# -- framing -----------------------------------------------------------------


def test_framing_golden_vector() -> None:
    # The iOS and Android providers must produce these exact bytes and names for
    # the same input, or a record written by one platform is unreadable by
    # another sharing a container.
    framed = state_storage_module._frame("outbox", "m-1", b"\xaa\xbb")

    assert framed == (
        b"OPS1"
        b"\x00\x06"  # key_type length
        b"\x00\x03"  # key_id length
        b"outbox"
        b"m-1"
        b"\xaa\xbb"
    )
    assert state_storage_module._type_directory_name("outbox") == (
        "t_d5fac01c82279b8b061df80b3c312942e2ce27a41a48b1b7479ff07ad5a6198d"
    )
    assert state_storage_module._entry_name("outbox", "m-1") == (
        "k_db5fcc2398ef2863d4269a61be6ea2de1f80d2889f34670c9a57c79cbe8058a1"
    )
    assert state_storage_module._parse_header(framed) == ("outbox", "m-1", 17)


def test_empty_value_round_trips(tmp_path) -> None:
    storage = AppStateStorage(tmp_path / "state")

    storage.store("blocked_users", "peer-1", [])

    assert storage.load("blocked_users", "peer-1") == []
    assert storage.list_keys("blocked_users") == ["peer-1"]


# -- bounded reads -----------------------------------------------------------


def test_oversized_file_is_rejected_without_being_read(tmp_path) -> None:
    # A record over the ceiling cannot have been written through store(), so it
    # must be dropped by size alone — never read into memory first.
    storage = AppStateStorage(tmp_path / "state")
    storage.store("outbox", "message-1", [1, 2, 3])

    path = (
        tmp_path
        / "state"
        / state_storage_module._type_directory_name("outbox")
        / state_storage_module._entry_name("outbox", "message-1")
    )
    # Sparse file: the ceiling is enforced on the *reported* size, so this never
    # occupies real disk in CI.
    with open(path, "r+b") as handle:
        handle.truncate(state_storage_module.MAX_FILE_BYTES + 1)

    with pytest.raises(MlsStorageError.CorruptedData):
        storage.load("outbox", "message-1")
    assert not path.exists()


def test_store_refuses_values_over_the_ceiling() -> None:
    with pytest.raises(Exception):
        state_storage_module._frame(
            "outbox", "m-1", b"\x00" * (state_storage_module.MAX_VALUE_BYTES + 1)
        )


def test_malformed_record_is_dropped_rather_than_returned(tmp_path) -> None:
    # A file whose framing does not name the key that was asked for is not that
    # record — drop it rather than hand back someone else's bytes, and report
    # the drop so the SDK can settle the message id the app holds.
    #
    # Destruction is not absence: a silent None is indistinguishable from a
    # record that was never written, which would leave that id unresolved
    # forever.
    storage = AppStateStorage(tmp_path / "state")
    storage.store("outbox", "message-1", [1, 2, 3])

    path = (
        tmp_path
        / "state"
        / state_storage_module._type_directory_name("outbox")
        / state_storage_module._entry_name("outbox", "message-1")
    )
    path.write_bytes(bytes(range(9)))

    with pytest.raises(MlsStorageError.CorruptedData):
        storage.load("outbox", "message-1")
    assert storage.list_keys("outbox") == []


def test_unframed_stray_files_are_ignored_by_listing(tmp_path) -> None:
    storage = AppStateStorage(tmp_path / "state")
    storage.store("outbox", "message-1", [1])

    directory = tmp_path / "state" / state_storage_module._type_directory_name("outbox")
    (directory / "k_not-a-record").write_bytes(b"\x01\x02\x03")
    (directory / "unrelated.tmp").write_bytes(b"\x01\x02\x03")

    assert storage.list_keys("outbox") == ["message-1"]


def test_enumeration_bound_counts_entries_examined_not_keys_returned(tmp_path) -> None:
    # The bound exists for a tampered container, and there the entries are
    # exactly the ones that yield no key. Counting keys collected would leave
    # every one of these opened on every launch while the counter sat at zero.
    storage = AppStateStorage(tmp_path / "state")
    storage.store("outbox", "message-1", [1])

    directory = tmp_path / "state" / state_storage_module._type_directory_name("outbox")
    for index in range(10):
        (directory / f"k_unparseable-{index}").write_bytes(b"\x01\x02\x03")

    keys, examined = storage.enumerate_keys("outbox", 4)

    assert examined == 4, "enumeration must stop at the bound it was given"
    assert len(keys) <= 1


def test_listing_dedupes_a_record_reachable_under_two_names(tmp_path) -> None:
    # A digest names exactly one record, so two names for one key id can only
    # come from a copy planted in the container. Restore must not walk the id
    # twice because of it.
    storage = AppStateStorage(tmp_path / "state")
    storage.store("outbox", "message-1", [1, 2, 3])

    directory = tmp_path / "state" / state_storage_module._type_directory_name("outbox")
    original = directory / state_storage_module._entry_name("outbox", "message-1")
    (directory / "k_copy-of-message-1").write_bytes(original.read_bytes())

    assert storage.list_keys("outbox") == ["message-1"]
