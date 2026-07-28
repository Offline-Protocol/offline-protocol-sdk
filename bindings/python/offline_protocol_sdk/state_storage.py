"""File-backed storage for restartable protocol and message-plane state.

Values are opaque bytes and are written verbatim: the SDK seals the categories
that can carry message plaintext or media key material before handing them over,
so they arrive as ciphertext. Do not inspect, re-encode, or truncate them.
"""

from __future__ import annotations

import base64
import os
import tempfile
import threading
from pathlib import Path

from .offline_protocol import MlsStorageError, ProtocolStateStorageProvider

_STATE_ROOT_ENV = "OFFLINE_PROTOCOL_STATE_ROOT"


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


def _encode_component(value: str) -> str:
    encoded = base64.urlsafe_b64encode(value.encode("utf-8")).decode("ascii")
    return f"k_{encoded.rstrip('=')}"


def _decode_component(value: str) -> str | None:
    if not value.startswith("k_"):
        return None
    encoded = value[2:]
    encoded += "=" * ((4 - len(encoded) % 4) % 4)
    try:
        return base64.urlsafe_b64decode(encoded).decode("utf-8")
    except (ValueError, UnicodeDecodeError):
        return None


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
            base_root / _encode_component(namespace)
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
        with self._lock:
            directory = self._type_directory(key_type)
            try:
                directory.mkdir(parents=True, exist_ok=True)
                descriptor, temporary = tempfile.mkstemp(prefix=".write-", dir=directory)
                try:
                    with os.fdopen(descriptor, "wb") as handle:
                        handle.write(bytes(data))
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
                return list(path.read_bytes())
            except FileNotFoundError:
                return None
            except OSError as exc:
                raise MlsStorageError.LoadFailed(
                    f"failed to load protocol state: {exc}"
                ) from exc

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
            try:
                decoded = (
                    _decode_component(path.name)
                    for path in directory.iterdir()
                    if path.is_file() and not path.name.startswith(".write-")
                )
                return sorted(value for value in decoded if value is not None)
            except OSError as exc:
                raise MlsStorageError.LoadFailed(
                    f"failed to list protocol-state keys: {exc}"
                ) from exc

    def _type_directory(self, key_type: str) -> Path:
        return self._root / _encode_component(key_type)

    def _entry_path(self, key_type: str, key_id: str) -> Path:
        return self._type_directory(key_type) / _encode_component(key_id)
