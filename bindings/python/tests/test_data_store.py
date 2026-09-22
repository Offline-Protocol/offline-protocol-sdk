"""Data layer over the Python bindings.

Python is the thinnest binding, which makes it the cheapest place to catch an
ABI or surface change in the data layer before the mobile bindings do.
"""

from __future__ import annotations

import json
import threading

import pytest
from conftest import InMemoryStorage

from offline_protocol_sdk.offline_protocol import (
    DataStore,
    OfflineProtocol,
    ProtocolError,
    ProtocolStateStorageProvider,
    run_storage_conformance,
)


class DictStateStorage(ProtocolStateStorageProvider):
    """The smallest backend that satisfies the adapter contract."""

    def __init__(self) -> None:
        self._data: dict[tuple[str, str], bytes] = {}
        self._lock = threading.Lock()

    def store(self, key_type: str, key_id: str, data: bytes) -> None:
        with self._lock:
            self._data[(key_type, key_id)] = bytes(data)

    def load(self, key_type: str, key_id: str) -> bytes | None:
        with self._lock:
            return self._data.get((key_type, key_id))

    def delete(self, key_type: str, key_id: str) -> None:
        with self._lock:
            self._data.pop((key_type, key_id), None)

    def list_keys(self, key_type: str) -> list[str]:
        with self._lock:
            return [key_id for (kt, key_id) in self._data if kt == key_type]

    def snapshot(self) -> dict[tuple[str, str], bytes]:
        with self._lock:
            return dict(self._data)


def value(kind: str, **fields: object) -> str:
    """Encode a DataValue the way the FFI expects it."""
    return json.dumps({"kind": kind, **fields})


@pytest.fixture
def data_config(default_config):
    default_config.data_enabled = True
    return default_config


def test_conformance_suite_passes_against_a_dict_backend() -> None:
    report = json.loads(run_storage_conformance(DictStateStorage()))
    assert report["failures"] == [], report["failures"]
    assert report["passed"], "the suite reported no checks at all"


def test_conformance_suite_catches_a_backend_that_drops_overwrites() -> None:
    class WriteOnce(DictStateStorage):
        def store(self, key_type: str, key_id: str, data: bytes) -> None:
            with self._lock:
                self._data.setdefault((key_type, key_id), bytes(data))

    # The negative control: a suite that passes everything proves nothing.
    report = json.loads(run_storage_conformance(WriteOnce()))
    assert report["failures"], "a write-once backend passed the suite"
    assert any(f["check"] == "store_overwrites" for f in report["failures"])


def test_data_store_requires_the_layer_to_be_enabled(default_config) -> None:
    # The layer defaults to on, so switching it off is what this test is
    # about: an application that does not want documents can say so, and the
    # refusal names the reason rather than failing somewhere further in.
    default_config.data_enabled = False
    protocol = OfflineProtocol(default_config)
    store = DataStore(protocol)
    with pytest.raises(ProtocolError.DataDisabled):
        store.create_doc("space-1", "doc-1")


def test_the_data_layer_is_on_by_default(default_config) -> None:
    # Pinned because the default moved once, when replication landed, and the
    # bridges restate it: a parser that fills in its own literal would hold
    # every app that omits the section at the old value forever.
    assert default_config.data_enabled is True


def test_documents_round_trip_through_a_custom_backend(data_config) -> None:
    protocol = OfflineProtocol(data_config)
    secure = _in_memory_mls_storage()
    documents = DictStateStorage()
    protocol.initialize_mls(secure, DictStateStorage())

    store = DataStore.with_storage(protocol, documents)
    store.map_set("space-1", "profile", "fields", "name", value("text", value="Ada"))
    store.text_insert("space-1", "profile", "body", 0, "hello")
    store.counter_increment("space-1", "profile", "views", 2.0)
    store.list_push("space-1", "profile", "log", value("int", value=7))
    store.flush("space-1", "profile")

    assert json.loads(store.map_get_json("space-1", "profile", "fields", "name")) == {
        "kind": "text",
        "value": "Ada",
    }
    assert store.text_value("space-1", "profile", "body") == "hello"
    assert store.counter_value("space-1", "profile", "views") == 2.0
    assert store.list_len("space-1", "profile", "log") == 1
    assert json.loads(store.doc_json("space-1", "profile"))["fields"]["name"] == "Ada"
    assert store.list_docs("space-1") == ["profile"]

    # Documents went to the backend the caller chose, and only documents.
    written = documents.snapshot()
    assert written, "the custom backend received nothing"
    assert all(key_type.startswith("data_") for (key_type, _) in written)

    # Sealing sits above the adapter: a backend never sees document content.
    assert not any(b"Ada" in blob for blob in written.values())

    store.wipe_all()
    assert documents.snapshot() == {}


def test_names_that_would_break_a_record_key_are_refused(data_config) -> None:
    protocol = OfflineProtocol(data_config)
    protocol.initialize_mls(_in_memory_mls_storage(), DictStateStorage())
    store = DataStore(protocol)

    with pytest.raises(ProtocolError.InvalidArgument):
        store.create_doc("space/one", "doc-1")


def test_a_malformed_value_is_refused_with_a_useful_error(data_config) -> None:
    protocol = OfflineProtocol(data_config)
    protocol.initialize_mls(_in_memory_mls_storage(), DictStateStorage())
    store = DataStore(protocol)

    with pytest.raises(ProtocolError.InvalidArgument):
        store.map_set("space-1", "doc-1", "m", "k", '{"kind":"nonsense"}')


def _in_memory_mls_storage() -> InMemoryStorage:
    """The MLS-side stand-in: keeps the login keychain out of the tests."""
    return InMemoryStorage()


def test_removing_a_document_and_evicting_one_are_different_calls(data_config) -> None:
    # Both take a document out of this store. Only one of them records that
    # it was removed, and that record is the whole difference: a peer cannot
    # otherwise tell a document this device deleted from one it has never
    # seen, so it offers it straight back.
    protocol = OfflineProtocol(data_config)
    protocol.initialize_mls(_in_memory_mls_storage(), DictStateStorage())
    documents = DictStateStorage()
    store = DataStore.with_storage(protocol, documents)

    store.map_set("space-1", "kept", "fields", "k", value("text", value="v"))
    store.map_set("space-1", "evicted", "fields", "k", value("text", value="v"))
    store.map_set("space-1", "removed", "fields", "k", value("text", value="v"))
    store.flush_all()

    store.delete_doc("space-1", "evicted")
    store.remove_doc("space-1", "removed")

    assert store.list_docs("space-1") == ["kept"]
    removals = [
        key_id
        for (key_type, key_id) in documents.snapshot()
        if key_type == "data_doc_meta"
    ]
    assert removals == ["space-1/removed"], (
        "eviction must record nothing about the name, and removal must record it"
    )

    # The name is refused rather than answered empty, so an application
    # cannot mistake a removed document for a cleared one.
    with pytest.raises(ProtocolError.InvalidState):
        store.map_get_json("space-1", "removed", "fields", "k")

    # And it can be used again, without its old contents.
    store.create_doc("space-1", "removed")
    assert store.map_get_json("space-1", "removed", "fields", "k") is None


def test_removing_a_space_removes_every_document_in_it(data_config) -> None:
    protocol = OfflineProtocol(data_config)
    protocol.initialize_mls(_in_memory_mls_storage(), DictStateStorage())
    store = DataStore.with_storage(protocol, DictStateStorage())

    for doc in ("one", "two", "three"):
        store.map_set("space-1", doc, "fields", "k", value("text", value="v"))
    store.flush_all()

    store.remove_space("space-1")
    assert store.list_docs("space-1") == []


def test_interest_scopes_what_this_device_stores(data_config) -> None:
    # The half that does not depend on any peer: a document outside the
    # declared interest is refused here, whoever offers it.
    protocol = OfflineProtocol(data_config)
    protocol.initialize_mls(_in_memory_mls_storage(), DictStateStorage())
    store = DataStore.with_storage(protocol, DictStateStorage())

    store.set_interest("space-1", ["wanted*"])

    # An application's own documents are not subject to it; the refusal is
    # about what arrives from a peer.
    store.map_set("space-1", "wanted.notes", "fields", "k", value("text", value="v"))
    store.flush_all()
    assert store.list_docs("space-1") == ["wanted.notes"]

    # Widening and narrowing are both accepted, and the default is
    # expressible.
    store.set_interest("space-1", ["*"])
    store.set_interest("space-1", [])


def test_an_interest_pattern_that_could_never_match_is_refused(data_config) -> None:
    protocol = OfflineProtocol(data_config)
    protocol.initialize_mls(_in_memory_mls_storage(), DictStateStorage())
    store = DataStore.with_storage(protocol, DictStateStorage())

    for bad in ("", "has space", "notes/sub", "a*b"):
        with pytest.raises(ProtocolError.InvalidArgument):
            store.set_interest("space-1", [bad])
    with pytest.raises(ProtocolError.InvalidArgument):
        store.set_interest("space-1", [f"d{n}*" for n in range(33)])


def test_a_group_fetch_names_the_member_and_a_one_to_one_fetch_does_not(data_config) -> None:
    # The two fetch surfaces refuse each other's spaces, and each refusal
    # names the call that works. A reference replicates to everybody and says
    # nothing about who holds the bytes, which is why the group one takes a
    # member.
    protocol = OfflineProtocol(data_config)
    protocol.initialize_mls(_in_memory_mls_storage(), DictStateStorage())
    store = DataStore.with_storage(protocol, DictStateStorage())
    blob_hash = store.attachment_hash(b"some bytes")

    # "space-1" is neither a peer address nor a group id, so it is local-only
    # and both calls refuse it, each for its own reason.
    with pytest.raises(ProtocolError.InvalidArgument) as one_to_one:
        store.fetch_attachment("space-1", blob_hash)
    assert "advertise" in str(one_to_one.value)

    with pytest.raises(ProtocolError.InvalidArgument) as group:
        store.fetch_attachment_from("space-1", "peer-1", blob_hash)
    assert "not a group" in str(group.value)
