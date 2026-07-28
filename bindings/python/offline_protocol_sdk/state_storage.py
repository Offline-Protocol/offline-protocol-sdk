"""File-backed storage for restartable protocol and message-plane state.

Values are opaque bytes and are written verbatim: the SDK seals the categories
that can carry message plaintext or media key material before handing them over,
so they arrive as ciphertext. Do not inspect, re-encode, or truncate them.
"""

from __future__ import annotations

import hashlib
import logging
import os
import tempfile
import threading
from pathlib import Path

from .offline_protocol import MlsStorageError, ProtocolStateStorageProvider
from .storage_namespace import require_account_storage_namespace

logger = logging.getLogger(__name__)

_STATE_ROOT_ENV = "OFFLINE_PROTOCOL_STATE_ROOT"

# -- on-disk record format ---------------------------------------------------
#
# Filenames are a fixed-length lowercase hex digest rather than an encoding of
# the key itself. An encoding cannot be a correct filesystem key: it is
# case-sensitive (so ``AAG`` and ``AAa`` name the same file on a
# case-insensitive volume, and one record silently overwrites the other) and its
# length grows with the key (so a long-but-valid protocol id overruns
# ``NAME_MAX``). A digest is fixed-length, lowercase, and collision-free in
# practice.
#
# Because a digest is one-way, the exact key cannot be recovered from the name,
# so each record carries it in a header instead. That also makes every file
# independently attributable and lets a read verify it opened the record it
# asked for.
#
#     bytes 0..4    magic "OPS1"
#     bytes 4..6    key_type length, big-endian u16
#     bytes 6..8    key_id length, big-endian u16
#     then          key_type UTF-8, key_id UTF-8, value bytes
#
# Keep this format, its limits, and the golden vectors in
# ``tests/test_state_storage.py`` in sync with the iOS and Android providers.

RECORD_MAGIC = b"OPS1"

#: Longest accepted ``key_type`` / ``key_id``, in UTF-8 bytes.
MAX_COMPONENT_BYTES = 4096

#: Provider ceiling on a record's value. Deliberately a generous superset of
#: core's ``MAX_PROTOCOL_STATE_RECORD_BYTES`` (4 MiB) plus its seal envelope, so
#: this never rejects a record the SDK legitimately wrote — it exists to bound
#: allocation for a file the SDK never wrote.
MAX_VALUE_BYTES = 8 * 1024 * 1024

#: Largest file the reader will pull into memory.
MAX_FILE_BYTES = 8 + 2 * MAX_COMPONENT_BYTES + MAX_VALUE_BYTES

#: Longest possible header, bounding the partial read ``list_keys`` does.
MAX_HEADER_BYTES = 8 + 2 * MAX_COMPONENT_BYTES

#: Ceiling on entries one ``list_keys`` will open. Core caps every category far
#: below this; a directory holding more has been tampered with.
MAX_LISTED_KEYS = 65_536


def _digest(*components: str) -> str:
    sha = hashlib.sha256()
    for component in components:
        sha.update(component.encode("utf-8"))
        sha.update(b"\x00")
    return sha.hexdigest()


def _type_directory_name(key_type: str) -> str:
    return f"t_{_digest(key_type)}"


def _entry_name(key_type: str, key_id: str) -> str:
    return f"k_{_digest(key_type, key_id)}"


def _frame(key_type: str, key_id: str, value: bytes) -> bytes:
    type_bytes = key_type.encode("utf-8")
    id_bytes = key_id.encode("utf-8")
    if len(type_bytes) > MAX_COMPONENT_BYTES or len(id_bytes) > MAX_COMPONENT_BYTES:
        raise MlsStorageError.StoreFailed(
            f"protocol-state key exceeds {MAX_COMPONENT_BYTES} bytes"
        )
    if len(value) > MAX_VALUE_BYTES:
        raise MlsStorageError.StoreFailed(
            f"protocol-state record is {len(value)} bytes, over the "
            f"{MAX_VALUE_BYTES} byte limit"
        )
    return b"".join(
        (
            RECORD_MAGIC,
            len(type_bytes).to_bytes(2, "big"),
            len(id_bytes).to_bytes(2, "big"),
            type_bytes,
            id_bytes,
            value,
        )
    )


def _parse_header(raw: bytes) -> tuple[str, str, int] | None:
    """Return ``(key_type, key_id, value_offset)``, or ``None`` when *raw* is
    not a record this SDK wrote."""
    if len(raw) < 8 or raw[:4] != RECORD_MAGIC:
        return None
    type_len = int.from_bytes(raw[4:6], "big")
    id_len = int.from_bytes(raw[6:8], "big")
    if type_len > MAX_COMPONENT_BYTES or id_len > MAX_COMPONENT_BYTES:
        return None
    if len(raw) < 8 + type_len + id_len:
        return None
    try:
        key_type = raw[8 : 8 + type_len].decode("utf-8")
        key_id = raw[8 + type_len : 8 + type_len + id_len].decode("utf-8")
    except UnicodeDecodeError:
        return None
    return key_type, key_id, 8 + type_len + id_len


def _default_root() -> Path:
    configured = os.environ.get(_STATE_ROOT_ENV)
    if configured:
        return Path(configured)
    raise ValueError(
        "protocol state has no safe process-wide default; pass root=... or set "
        f"{_STATE_ROOT_ENV} to an application-owned directory that is removed "
        "when the application is uninstalled"
    )


def _sync_directory(directory: Path) -> None:
    """Flush a directory entry so a rename or unlink in it survives a crash.

    ``fsync`` on the temporary file only makes its *contents* durable; the link
    that ``os.replace`` (or ``unlink``) creates or removes lives in the parent
    directory and needs its own flush. Without this a power loss can lose a
    store the SDK was told succeeded — most sharply for the sealed record key's
    counterpart records — or resurrect a deleted outbox entry and resend work
    that was already settled.

    Best effort: directories cannot be opened for ``fsync`` on every platform
    (notably Windows), and there the file-level ``fsync`` above remains the
    strongest guarantee available.
    """
    try:
        descriptor = os.open(directory, os.O_RDONLY)
    except OSError:
        return
    try:
        os.fsync(descriptor)
    except OSError:
        pass
    finally:
        os.close(descriptor)


class AppStateStorage(ProtocolStateStorageProvider):
    """Atomic application-data storage kept outside the OS credential store."""

    def __init__(
        self,
        root: str | Path | None = None,
        *,
        namespace: str | None = None,
    ) -> None:
        base_root = Path(root) if root is not None else _default_root()
        self._root = (
            base_root / require_account_storage_namespace(namespace)
            if namespace is not None
            else base_root
        )
        self._lock = threading.RLock()
        try:
            self._root.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            raise MlsStorageError.StoreFailed(
                f"failed to create protocol-state directory: {exc}"
            ) from exc

    def store(self, key_type: str, key_id: str, data: list[int]) -> None:
        framed = _frame(key_type, key_id, bytes(data))
        with self._lock:
            directory = self._type_directory(key_type)
            try:
                directory.mkdir(parents=True, exist_ok=True)
                descriptor, temporary = tempfile.mkstemp(prefix=".write-", dir=directory)
                try:
                    with os.fdopen(descriptor, "wb") as handle:
                        handle.write(framed)
                        handle.flush()
                        os.fsync(handle.fileno())
                    os.replace(temporary, self._entry_path(key_type, key_id))
                    _sync_directory(directory)
                except BaseException:
                    try:
                        os.unlink(temporary)
                    except FileNotFoundError:
                        pass
                    raise
            except OSError as exc:
                raise MlsStorageError.StoreFailed(
                    f"failed to persist protocol state: {exc}"
                ) from exc

    def load(self, key_type: str, key_id: str) -> list[int] | None:
        with self._lock:
            path = self._entry_path(key_type, key_id)
            try:
                # Stat before reading. A record over the ceiling cannot have
                # been written through ``store``, so it is corrupt or tampered —
                # removing it keeps a poison file from being re-examined on
                # every boot, and keeps the read itself from becoming an
                # unbounded allocation.
                with open(path, "rb") as handle:
                    oversized = os.fstat(handle.fileno()).st_size > MAX_FILE_BYTES
                    # Read one byte past the ceiling so a stat that raced a
                    # concurrent writer (or a filesystem that misreports size)
                    # is still caught below.
                    raw = b"" if oversized else handle.read(MAX_FILE_BYTES + 1)
            except FileNotFoundError:
                return None
            except OSError as exc:
                raise MlsStorageError.LoadFailed(
                    f"failed to load protocol state: {exc}"
                ) from exc

            # Unlink outside the `with`: Windows refuses to remove an open file.
            if oversized or len(raw) > MAX_FILE_BYTES:
                self._discard(path, "oversized")
                return None

            header = _parse_header(raw)
            if header is None or header[0] != key_type or header[1] != key_id:
                # Malformed framing, or a name that resolves to some other
                # record: either way this is not the entry that was asked for.
                self._discard(path, "malformed")
                return None
            return list(raw[header[2] :])

    def delete(self, key_type: str, key_id: str) -> None:
        with self._lock:
            path = self._entry_path(key_type, key_id)
            try:
                path.unlink()
            except FileNotFoundError:
                return
            except OSError as exc:
                raise MlsStorageError.DeleteFailed(
                    f"failed to delete protocol state: {exc}"
                ) from exc
            _sync_directory(path.parent)

    def list_keys(self, key_type: str) -> list[str]:
        with self._lock:
            directory = self._type_directory(key_type)
            if not directory.exists():
                return []
            keys: list[str] = []
            try:
                # Stream the directory rather than materializing it, and read
                # only each record's header: enumeration must stay bounded even
                # when the container has been tampered with.
                with os.scandir(directory) as entries:
                    for entry in entries:
                        if len(keys) >= MAX_LISTED_KEYS:
                            break
                        if not entry.name.startswith("k_") or not entry.is_file():
                            continue
                        header = self._read_header(Path(entry.path))
                        if header is not None and header[0] == key_type:
                            keys.append(header[1])
            except OSError as exc:
                raise MlsStorageError.LoadFailed(
                    f"failed to list protocol-state keys: {exc}"
                ) from exc
            return sorted(keys)

    # -- internals -----------------------------------------------------------

    @staticmethod
    def _read_header(path: Path) -> tuple[str, str, int] | None:
        try:
            with open(path, "rb") as handle:
                return _parse_header(handle.read(MAX_HEADER_BYTES))
        except OSError:
            return None

    @staticmethod
    def _discard(path: Path, reason: str) -> None:
        logger.warning("dropping %s protocol-state record %s", reason, path.name)
        try:
            path.unlink()
        except OSError:
            pass

    def _type_directory(self, key_type: str) -> Path:
        return self._root / _type_directory_name(key_type)

    def _entry_path(self, key_type: str, key_id: str) -> Path:
        return self._type_directory(key_type) / _entry_name(key_type, key_id)
