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
