"""A SQLite-backed protocol-state adapter, as a worked reference.

This is the whole "bring your own backend" path in one file. It exists to
prove the claim rather than to be copied verbatim: the SDK ships a working
file-backed provider, and an application only reaches for something like
this when it already has a store it wants its data inside.

What the SDK guarantees to an adapter:

* Values are opaque bytes. Records whose category requires sealing are
  encrypted before ``store`` is called, so this file never sees document
  content or message plaintext, and cannot weaken the at-rest posture. It
  also means values must round-trip as **bytes**, never through ``str``.
* ``key_type`` is a namespace and ``key_id`` an identifier within it. Both
  are opaque; the data layer composes ids like ``space/doc/0000000000000001``,
  so do not treat them as paths.

What the adapter owes: the contract checked by ``run_storage_conformance``.
Run it (see ``test_conformance`` at the bottom) and check that ``failures``
is empty. Green is the definition of "this backend is supported".

Logout: an application that points documents here **must** call
``DataStore.wipe_all()`` on logout. ``wipePersistedState`` clears the
default provider's directory, which this database is not inside. Stop the
protocol first: with the engine running and sessions live, the peer's next
version offer recreates and refills every document it wiped.
"""

from __future__ import annotations

import sqlite3
import threading
from pathlib import Path

from offline_protocol_sdk.offline_protocol import (
    MlsStorageError,
    ProtocolStateStorageProvider,
)


class SqliteStateStorage(ProtocolStateStorageProvider):
    """Protocol-state records in a single SQLite database."""

    def __init__(self, path: str | Path) -> None:
        self._path = str(path)
        # One connection guarded by a lock. The SDK calls this from whichever
        # thread is doing protocol work, and sqlite3 connections are not
        # thread-safe by default; `check_same_thread=False` plus a lock is
        # simpler to reason about than a connection pool.
        self._lock = threading.Lock()
        self._db = sqlite3.connect(self._path, check_same_thread=False)
        with self._lock:
            # WAL keeps a reader from blocking the writer, which matters
            # because the SDK reads records during restore while its own
            # background work may still be writing.
            self._db.execute("PRAGMA journal_mode=WAL")
            # FULL, not NORMAL: these records are the crash-recovery state.
            # A store() that returns before the bytes are durable turns a
            # power loss into silent data loss.
            self._db.execute("PRAGMA synchronous=FULL")
            self._db.execute(
                """
                CREATE TABLE IF NOT EXISTS protocol_state (
                    key_type TEXT NOT NULL,
                    key_id   TEXT NOT NULL,
                    value    BLOB NOT NULL,
                    PRIMARY KEY (key_type, key_id)
                )
                """
            )
            self._db.commit()

    def store(self, key_type: str, key_id: str, data: bytes) -> None:
        try:
            with self._lock:
                # UPSERT, not INSERT: a second store under the same key must
                # replace. A backend that quietly keeps the first value is the
                # defect `store_overwrites` in the conformance suite exists to
                # catch, and it is invisible until data is stale.
                self._db.execute(
                    """
                    INSERT INTO protocol_state (key_type, key_id, value)
                    VALUES (?, ?, ?)
                    ON CONFLICT (key_type, key_id) DO UPDATE SET value = excluded.value
                    """,
                    (key_type, key_id, sqlite3.Binary(data)),
                )
                self._db.commit()
        except sqlite3.Error as exc:
            raise MlsStorageError.StoreFailed(f"sqlite store failed: {exc}") from exc

    def load(self, key_type: str, key_id: str) -> bytes | None:
        try:
            with self._lock:
                row = self._db.execute(
                    "SELECT value FROM protocol_state WHERE key_type = ? AND key_id = ?",
                    (key_type, key_id),
                ).fetchone()
        except sqlite3.Error as exc:
            # LoadFailed, not CorruptedData: the record may be perfectly fine
            # and this read may simply have failed. CorruptedData is a
            # permanent verdict and the SDK settles messages on it.
            raise MlsStorageError.LoadFailed(f"sqlite load failed: {exc}") from exc
        # A missing row is None, not an error. The SDK asks for records that
        # legitimately do not exist yet on every launch.
        return bytes(row[0]) if row is not None else None

    def delete(self, key_type: str, key_id: str) -> None:
        try:
            with self._lock:
                self._db.execute(
                    "DELETE FROM protocol_state WHERE key_type = ? AND key_id = ?",
                    (key_type, key_id),
                )
                self._db.commit()
        except sqlite3.Error as exc:
            raise MlsStorageError.DeleteFailed(f"sqlite delete failed: {exc}") from exc
        # Deleting an absent key is deliberately not an error: the data layer
        # removes folded delta records that a crash may already have taken.

    def list_keys(self, key_type: str) -> list[str]:
        try:
            with self._lock:
                rows = self._db.execute(
                    "SELECT key_id FROM protocol_state WHERE key_type = ?",
                    (key_type,),
                ).fetchall()
        except sqlite3.Error as exc:
            raise MlsStorageError.LoadFailed(f"sqlite list failed: {exc}") from exc
        return [row[0] for row in rows]

    def close(self) -> None:
        with self._lock:
            self._db.close()


def test_conformance(tmp_path) -> None:
    """The gate every adapter must pass. Run with pytest."""
    from offline_protocol_sdk.offline_protocol import run_storage_conformance
    import json

    storage = SqliteStateStorage(tmp_path / "state.db")
    try:
        report = json.loads(run_storage_conformance(storage))
        assert report["failures"] == [], report["failures"]
        assert report["passed"], "the suite reported no checks at all"
    finally:
        storage.close()


if __name__ == "__main__":
    import json
    import tempfile

    from offline_protocol_sdk.offline_protocol import run_storage_conformance

    with tempfile.TemporaryDirectory() as directory:
        adapter = SqliteStateStorage(Path(directory) / "state.db")
        result = json.loads(run_storage_conformance(adapter))
        adapter.close()
        if result["failures"]:
            for failure in result["failures"]:
                print(f"FAIL {failure['check']}: {failure['detail']}")
            raise SystemExit(1)
        print(f"conformance: {len(result['passed'])} checks passed")
