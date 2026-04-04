"""Cross-platform secure storage for MLS key material.

Uses the `keyring` library which delegates to platform-native credential stores:
  - macOS: Keychain
  - Linux: Secret Service (GNOME Keyring / KWallet)
  - Windows: Windows Credential Locker
"""

from __future__ import annotations

import base64
import json
import logging
import threading

import keyring

from .offline_protocol import MlsStorageError, MlsStorageProvider

logger = logging.getLogger(__name__)

# Service name used for all keyring entries
_DEFAULT_SERVICE = "offline-protocol-mls"

# Prefix for index entries that track key IDs per key_type
_INDEX_PREFIX = "index:"


class SecureStorage(MlsStorageProvider):
    """MlsStorageProvider backed by platform-native credential storage.

    Thread-safe: all mutating operations are serialised with a lock to keep
    the per-key_type index consistent (same approach as Android's
    ``MlsSecureStorage.kt``).

    Important: keep a reference to this object alive for the entire lifetime
    of the ``OfflineProtocol`` instance — if it is garbage-collected the
    callback pointers on the Rust side become dangling.
    """

    def __init__(self, service: str = _DEFAULT_SERVICE) -> None:
        self._service = service
        self._lock = threading.Lock()

    # -- MlsStorageProvider interface ----------------------------------------

    def store(self, key_type: str, key_id: str, data: list[int]) -> None:
        key = _make_key(key_type, key_id)
        encoded = base64.b64encode(bytes(data)).decode("ascii")

        with self._lock:
            try:
                keyring.set_password(self._service, key, encoded)
                self._index_add(key_type, key_id)
            except Exception as exc:
                raise MlsStorageError.StoreFailed(
                    f"keyring store failed: {exc}"
                ) from exc

    def load(self, key_type: str, key_id: str) -> list[int] | None:
        key = _make_key(key_type, key_id)
        try:
            encoded = keyring.get_password(self._service, key)
            if encoded is None:
                return None
            return list(base64.b64decode(encoded))
        except Exception as exc:
            raise MlsStorageError.LoadFailed(
                f"keyring load failed: {exc}"
            ) from exc

    def delete(self, key_type: str, key_id: str) -> None:
        key = _make_key(key_type, key_id)

        with self._lock:
            try:
                keyring.delete_password(self._service, key)
            except keyring.errors.PasswordDeleteError:
                pass  # already gone — not an error (matches iOS Keychain behaviour)
            except Exception as exc:
                raise MlsStorageError.DeleteFailed(
                    f"keyring delete failed: {exc}"
                ) from exc

            self._index_remove(key_type, key_id)

    def list_keys(self, key_type: str) -> list[str]:
        """Return all key IDs stored under *key_type*.

        ``keyring`` does not natively support listing, so we maintain a
        JSON-encoded index entry per key_type — the same strategy used by
        the Android ``MlsSecureStorage`` (index prefix approach).
        """
        with self._lock:
            try:
                return self._index_get(key_type)
            except Exception as exc:
                raise MlsStorageError.LoadFailed(
                    f"keyring list_keys failed: {exc}"
                ) from exc

    # -- index helpers -------------------------------------------------------

    def _index_key(self, key_type: str) -> str:
        return f"{_INDEX_PREFIX}{key_type}"

    def _index_get(self, key_type: str) -> list[str]:
        raw = keyring.get_password(self._service, self._index_key(key_type))
        if raw is None:
            return []
        return json.loads(raw)

    def _index_add(self, key_type: str, key_id: str) -> None:
        ids = set(self._index_get(key_type))
        ids.add(key_id)
        keyring.set_password(
            self._service,
            self._index_key(key_type),
            json.dumps(sorted(ids)),
        )

    def _index_remove(self, key_type: str, key_id: str) -> None:
        ids = set(self._index_get(key_type))
        ids.discard(key_id)
        if ids:
            keyring.set_password(
                self._service,
                self._index_key(key_type),
                json.dumps(sorted(ids)),
            )
        else:
            try:
                keyring.delete_password(
                    self._service, self._index_key(key_type)
                )
            except keyring.errors.PasswordDeleteError:
                pass


def _make_key(key_type: str, key_id: str) -> str:
    return f"{key_type}:{key_id}"
