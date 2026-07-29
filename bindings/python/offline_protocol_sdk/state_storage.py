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
#:
#: The bound counts entries *examined*, not keys returned. Counting the latter
#: would not bound the tampered case at all — the case this exists for: an entry
#: whose header does not parse yields no key, so a directory full of unparseable
#: ``k_`` files would be opened in its entirety on every launch while the counter
#: sat at zero.
MAX_LISTED_KEYS = 65_536

#: Prefix of the temporary file ``store`` writes before renaming it into place.
#: Distinct from the ``k_`` entry prefix so enumeration never mistakes a
#: half-written record for one.
TEMP_PREFIX = ".write-"

#: Ceiling on entries one stale-temporary sweep examines, so a tampered
#: directory cannot turn the first store of a session into an unbounded scan.
MAX_SWEEP_ENTRIES = 4_096


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
        #: Type directories whose stale temporaries have already been swept this
        #: process. One sweep per category per process, off the restore path.
        self._swept: set[Path] = set()
        try:
            self._root.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            raise MlsStorageError.StoreFailed(
                f"failed to create protocol-state directory: {exc}"
            ) from exc

    def store(self, key_type: str, key_id: str, data: bytes) -> None:
        framed = _frame(key_type, key_id, data)
        with self._lock:
            directory = self._type_directory(key_type)
            try:
                directory.mkdir(parents=True, exist_ok=True)
                self._sweep_temporaries(directory)
                descriptor, temporary = tempfile.mkstemp(prefix=TEMP_PREFIX, dir=directory)
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

    def load(self, key_type: str, key_id: str) -> bytes | None:
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
                raise self._discard(path, "record is over the ceiling")

            header = _parse_header(raw)
            if header is None or header[0] != key_type or header[1] != key_id:
                # Malformed framing, or a name that resolves to some other
                # record: either way this is not the entry that was asked for.
                raise self._discard(
                    path, f"record framing does not name {key_type}/{key_id}"
                )
            return raw[header[2] :]

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
        return self.enumerate_keys(key_type, MAX_LISTED_KEYS)[0]

    def enumerate_keys(self, key_type: str, limit: int) -> tuple[list[str], int]:
        """Enumerate one category, opening at most ``limit`` entries.

        ``limit`` bounds the entries *examined* rather than the keys collected,
        because opening a file is the cost and an entry that fails to parse
        yields no key: a directory of unparseable records must not be walked in
        full on every launch. Exposed with an explicit ``limit`` so the bound is
        testable without materializing tens of thousands of files.

        Key ids are deduped: a name that resolves to a record already seen (a
        copy planted in the container) must not make the same id appear twice.

        Returns the sorted key ids and the number of entries examined.
        """
        with self._lock:
            directory = self._type_directory(key_type)
            if not directory.exists():
                return [], 0
            keys: set[str] = set()
            examined = 0
            try:
                # Stream the directory rather than materializing it, and read
                # only each record's header: enumeration must stay bounded even
                # when the container has been tampered with.
                with os.scandir(directory) as entries:
                    for entry in entries:
                        if examined >= limit:
                            break
                        if not entry.name.startswith("k_"):
                            continue
                        examined += 1
                        if not entry.is_file():
                            continue
                        header = self._read_header(Path(entry.path))
                        if header is not None and header[0] == key_type:
                            keys.add(header[1])
            except OSError as exc:
                raise MlsStorageError.LoadFailed(
                    f"failed to list protocol-state keys: {exc}"
                ) from exc
            return sorted(keys), examined

    # -- internals -----------------------------------------------------------

    def _sweep_temporaries(self, directory: Path) -> None:
        """Remove temporaries a previous process died before renaming.

        ``store`` writes to a ``mkstemp`` file and renames it into place, so a
        crash in between orphans that file permanently — and enumeration
        filters on the ``k_`` prefix, so nothing ever sees it again. Left alone
        they accumulate for the life of the install, in a directory the
        application cannot reasonably be asked to clean itself.

        Safe to unlink unconditionally: ``store`` holds the instance lock for
        the whole write, so no temporary this instance is using can be visible
        here. Runs once per type directory per process — the caller is the
        first store into that category, which keeps it off the restore path
        — and is best effort, since losing the race with another process's
        rename costs nothing.
        """

        if directory in self._swept:
            return
        self._swept.add(directory)
        try:
            with os.scandir(directory) as entries:
                for examined, entry in enumerate(entries):
                    if examined >= MAX_SWEEP_ENTRIES:
                        break
                    if not entry.name.startswith(TEMP_PREFIX):
                        continue
                    try:
                        os.unlink(entry.path)
                    except OSError:
                        continue
                    logger.debug("removed stale protocol-state temporary %s", entry.name)
        except OSError:
            # A directory we cannot scan is not a reason to fail the store that
            # is about to create it fresh.
            return

    @staticmethod
    def _read_header(path: Path) -> tuple[str, str, int] | None:
        try:
            with open(path, "rb") as handle:
                return _parse_header(handle.read(MAX_HEADER_BYTES))
        except OSError:
            return None

    @staticmethod
    def _discard(path: Path, reason: str) -> Exception:
        """Remove a record that can never be read and return the error saying so.

        ``CorruptedData``, not a silent ``None``: absence and destruction are
        different answers upstream. The SDK settles a destroyed record — the
        application is holding the message id ``send_message`` returned for it —
        while absence is simply nothing to restore, reported to no one.
        """

        logger.warning("dropping protocol-state record %s: %s", path.name, reason)
        try:
            path.unlink()
        except OSError:
            pass
        else:
            _sync_directory(path.parent)
        return MlsStorageError.CorruptedData(
            f"dropped unreadable protocol-state record: {reason}"
        )

    def _type_directory(self, key_type: str) -> Path:
        return self._root / _type_directory_name(key_type)

    def _entry_path(self, key_type: str, key_id: str) -> Path:
        return self._type_directory(key_type) / _entry_name(key_type, key_id)
