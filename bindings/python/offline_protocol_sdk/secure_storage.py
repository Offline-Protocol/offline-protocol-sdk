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

from . import legacy_store_adoption
from .offline_protocol import MlsStorageError, MlsStorageProvider
from .storage_namespace import require_account_storage_namespace

logger = logging.getLogger(__name__)

# Base service name used for keyring entries
_DEFAULT_SERVICE = "offline-protocol-mls-v2"

# Service the pre-namespace store used, adopted on upgrade so an existing
# install keeps its MLS identity (see ``legacy_store_adoption``).
_LEGACY_SERVICE = "offline-protocol-mls"

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

    def __init__(
        self,
        service: str = _DEFAULT_SERVICE,
        *,
        namespace: str | None = None,
        adopt_legacy_store: bool = True,
    ) -> None:
        if namespace is not None:
            require_account_storage_namespace(namespace)
        self._service = f"{service}:{namespace}" if namespace is not None else service
        self._lock = threading.RLock()

        # The legacy store predates namespacing and predates the "-v2" suffix,
        # so it is only ever the default service under its old name.
        self._legacy_service = (
            _LEGACY_SERVICE
            if adopt_legacy_store and service == _DEFAULT_SERVICE and namespace is not None
            else None
        )

        # Adoption records its claim under the account namespace, so a store
        # built without one cannot participate — and on the default service
        # that means an upgraded install silently mints a fresh MLS identity,
        # abandoning every session, group, and TOFU pin it had. Say so: every
        # other platform surfaces the analogous case as an error diagnostic.
        # ``ProtocolManager`` supplies the namespace; this fires for callers
        # that construct their own provider.
        if adopt_legacy_store and service == _DEFAULT_SERVICE and namespace is None:
            logger.warning(
                "SecureStorage built without an account namespace: the "
                "pre-namespace store cannot be adopted, so an upgraded install "
                "starts from a fresh MLS identity and cannot decrypt its old "
                "sessions. Pass namespace=account_storage_namespace(app_id, "
                "user_id), or let ProtocolManager build the provider."
            )

        # Warn if keyring resolved to a plaintext or null backend — MLS key
        # material would be stored unprotected.
        try:
            backend = keyring.get_keyring()
            backend_name = type(backend).__name__
            if "Fail" in backend_name or "Null" in backend_name or "PlaintextKeyring" in backend_name:
                logger.warning(
                    "keyring backend is '%s' — MLS keys will NOT be stored "
                    "securely. Install a platform secret service (e.g. "
                    "gnome-keyring, kwallet) for production use.",
                    backend_name,
                )
        except Exception:
            logger.debug("Could not inspect keyring backend", exc_info=True)

        #: Outcome of the one-time legacy-store adoption, for the caller to
        #: surface. A ``"conflict"`` in particular must not pass silently: this
        #: account is starting from a fresh identity.
        self.legacy_adoption = (
            self._resolve_legacy_adoption(namespace)
            if self._legacy_service is not None and namespace is not None
            else legacy_store_adoption.NONE
        )

    # -- MlsStorageProvider interface ----------------------------------------

    def store(self, key_type: str, key_id: str, data: list[int]) -> None:
        with self._lock:
            self._store(self._service, key_type, key_id, bytes(data))

    def load(self, key_type: str, key_id: str) -> list[int] | None:
        current = self._read(self._service, key_type, key_id)
        if current is not None:
            return list(current)

        legacy = self._read_through_service(key_type)
        if legacy is None:
            return None
        inherited = self._read(legacy, key_type, key_id)
        if inherited is None:
            return None

        # Best-effort promotion: a failed copy still returns the value, it just
        # costs another read-through next launch.
        with self._lock:
            try:
                self._store(self._service, key_type, key_id, inherited)
            except Exception:
                logger.warning(
                    "failed to promote inherited entry for %s", key_type, exc_info=True
                )
        return list(inherited)

    def delete(self, key_type: str, key_id: str) -> None:
        with self._lock:
            self._remove(self._service, key_type, key_id)
            # A delete that left the legacy copy in place would let
            # read-through resurrect key material the caller believes is gone.
            legacy = self._read_through_service(key_type)
            if legacy is not None:
                try:
                    self._remove(legacy, key_type, key_id)
                except Exception:
                    logger.warning(
                        "failed to delete inherited entry for %s",
                        key_type,
                        exc_info=True,
                    )

    def list_keys(self, key_type: str) -> list[str]:
        """Return all key IDs stored under *key_type*.

        ``keyring`` does not natively support listing, so we maintain a
        JSON-encoded index entry per key_type — the same strategy used by
        the Android ``MlsSecureStorage`` (index prefix approach). The adopted
        legacy store's index is unioned in, so a not-yet-promoted entry is
        still discoverable.
        """
        with self._lock:
            try:
                keys = list(self._index_get(self._service, key_type))
                legacy = self._read_through_service(key_type)
                if legacy is not None:
                    for key in self._index_get(legacy, key_type):
                        if key not in keys:
                            keys.append(key)
                return keys
            except Exception as exc:
                raise MlsStorageError.LoadFailed(
                    f"keyring list_keys failed: {exc}"
                ) from exc

    # -- legacy adoption -----------------------------------------------------

    def _resolve_legacy_adoption(self, namespace: str):
        """Resolve — and, when the legacy store is unclaimed, record — this
        account's right to inherit it."""

        assert self._legacy_service is not None
        try:
            raw = self._read(
                self._legacy_service,
                legacy_store_adoption.CLAIM_KEY_TYPE,
                legacy_store_adoption.CLAIM_KEY_ID,
            )
        except Exception:
            logger.warning(
                "legacy secure store could not be inspected; not adopting it",
                exc_info=True,
            )
            self._legacy_service = None
            return legacy_store_adoption.NONE

        existing = raw.decode("utf-8", errors="replace") if raw is not None else None
        decision = legacy_store_adoption.decide(existing, namespace)

        if decision.kind == "adopt":
            try:
                self._store(
                    self._legacy_service,
                    legacy_store_adoption.CLAIM_KEY_TYPE,
                    legacy_store_adoption.CLAIM_KEY_ID,
                    namespace.encode("utf-8"),
                )
            except Exception:
                logger.warning("failed to claim the legacy secure store", exc_info=True)
            logger.info("adopting the pre-namespace secure store for this account")
        elif decision.kind == "conflict":
            logger.error(
                "legacy secure store already belongs to another account; this "
                "account starts from a fresh MLS identity and cannot decrypt "
                "its old sessions."
            )
        return decision

    def _read_through_service(self, key_type: str) -> str | None:
        """The legacy service to consult for *key_type*, or None when
        read-through is off (no legacy store, or another account claimed it)."""

        if self._legacy_service is None:
            return None
        if not self.legacy_adoption.allows_read_through:
            return None
        if legacy_store_adoption.is_claim_entry(key_type):
            return None
        return self._legacy_service

    # -- keyring primitives --------------------------------------------------

    def _store(self, service: str, key_type: str, key_id: str, data: bytes) -> None:
        try:
            keyring.set_password(
                service, _make_key(key_type, key_id), base64.b64encode(data).decode("ascii")
            )
            self._index_add(service, key_type, key_id)
        except Exception as exc:
            raise MlsStorageError.StoreFailed(f"keyring store failed: {exc}") from exc

    def _read(self, service: str, key_type: str, key_id: str) -> bytes | None:
        try:
            encoded = keyring.get_password(service, _make_key(key_type, key_id))
            if encoded is None:
                return None
            return base64.b64decode(encoded)
        except Exception as exc:
            raise MlsStorageError.LoadFailed(f"keyring load failed: {exc}") from exc

    def _remove(self, service: str, key_type: str, key_id: str) -> None:
        try:
            keyring.delete_password(service, _make_key(key_type, key_id))
        except keyring.errors.PasswordDeleteError:
            pass  # already gone — not an error (matches iOS Keychain behaviour)
        except Exception as exc:
            raise MlsStorageError.DeleteFailed(f"keyring delete failed: {exc}") from exc

        self._index_remove(service, key_type, key_id)

    # -- index helpers -------------------------------------------------------

    def _index_key(self, key_type: str) -> str:
        return f"{_INDEX_PREFIX}{key_type}"

    def _index_get(self, service: str, key_type: str) -> list[str]:
        raw = keyring.get_password(service, self._index_key(key_type))
        if raw is None:
            return []
        return json.loads(raw)

    def _index_add(self, service: str, key_type: str, key_id: str) -> None:
        ids = set(self._index_get(service, key_type))
        ids.add(key_id)
        keyring.set_password(
            service,
            self._index_key(key_type),
            json.dumps(sorted(ids)),
        )

    def _index_remove(self, service: str, key_type: str, key_id: str) -> None:
        ids = set(self._index_get(service, key_type))
        ids.discard(key_id)
        if ids:
            keyring.set_password(
                service,
                self._index_key(key_type),
                json.dumps(sorted(ids)),
            )
        else:
            try:
                keyring.delete_password(service, self._index_key(key_type))
            except keyring.errors.PasswordDeleteError:
                pass


def _make_key(key_type: str, key_id: str) -> str:
    return f"{key_type}:{key_id}"
