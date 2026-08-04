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

#: Value written for a tombstone. Only its *presence* is the signal — nothing
#: reads the bytes back — so it stays one byte rather than restating the key.
_TOMBSTONE_VALUE = b"\x01"

#: Serialises legacy-store adoption across *every* provider in the process.
#:
#: Deliberately module-level rather than the per-instance ``self._lock``: two
#: accounts adopting concurrently are two ``SecureStorage`` objects, so an
#: instance lock cannot order them by construction. See
#: :meth:`SecureStorage._resolve_legacy_adoption`.
_ADOPTION_LOCK = threading.Lock()


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
                "sessions. Its pre-split delivery state is unreachable too — "
                "the SDK's protocol-state adoption sweep reads it through this "
                "provider — so the install also comes up with an empty outbox "
                "and an empty block list. Pass "
                "namespace=account_storage_namespace(app_id, user_id), or let "
                "ProtocolManager build the provider."
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
        """Load one entry, falling through to the adopted legacy store on a miss.

        Each primitive below takes ``self._lock``, but this *compound*
        read-then-promote is not atomic against :meth:`delete`, and deliberately
        so. Interleaved, they would resurrect key material: this method could
        observe a miss in the namespaced store, read the legacy value, and then
        promote it after a concurrent delete had already removed both copies —
        defeating the very guarantee :meth:`delete` documents.

        That is unreachable because the SDK is the only caller and serialises
        every storage operation behind its own mutex: ``OfflineProtocol``'s
        methods take ``&mut self`` and the UniFFI wrapper holds them under one
        lock, so no two provider calls overlap. Widening the lock to cover the
        whole compound operation would mean holding it across a keyring read on
        every miss, which is the common path during an upgrade. If a second
        caller is ever given this provider, that trade has to be revisited.

        A tombstoned key reads as absent without consulting the legacy store at
        all: its copy there outlived a delete, and promoting it would resurrect
        key material the caller was told was gone.
        """

        if legacy_store_adoption.is_reserved_entry(key_type):
            return None

        current = self._read(self._service, key_type, key_id)
        if current is not None:
            return list(current)

        legacy = self._read_through_service(key_type)
        if legacy is None:
            return None
        tombstone = self._tombstone_state(key_type, key_id)
        if tombstone.suppresses_read_through:
            if tombstone.allows_removal_retry:
                # Opportunistic heal: the removal that failed may succeed now,
                # which is the only thing that retires a tombstone. Gated on a
                # *confirmed* tombstone — a read that merely failed must not
                # delete a copy that may still be inheritable.
                self._retry_tombstoned_removal(legacy, key_type, key_id)
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
        """Delete one entry from the namespaced store and the legacy one.

        A delete that left the legacy copy in place would let read-through
        resurrect key material the caller believes is gone. When that removal
        fails, the key is tombstoned rather than reported: see
        ``legacy_store_adoption.TOMBSTONE_KEY_TYPE`` for why this cannot be
        signalled by raising. The delete has still done what it promised —
        nothing will hand that key back — so it returns successfully.

        Only a *double* fault raises: a legacy copy that will not delete and a
        namespaced store that will not record the tombstone leaves no way to
        keep the promise, and a store failing both is failing everything else
        too.
        """

        with self._lock:
            self._remove(self._service, key_type, key_id)
            legacy = self._read_through_service(key_type)
            if legacy is None:
                return
            try:
                self._remove(legacy, key_type, key_id)
            except Exception as exc:
                logger.warning(
                    "failed to delete inherited entry for %s; tombstoning it so "
                    "read-through cannot resurrect it",
                    key_type,
                    exc_info=True,
                )
                self._tombstone(key_type, key_id, exc)
                return
            self._clear_tombstone(key_type, key_id)

    def list_keys(self, key_type: str) -> list[str]:
        """Return all key IDs stored under *key_type*.

        ``keyring`` does not natively support listing, so we maintain a
        JSON-encoded index entry per key_type — the same strategy used by
        the Android ``MlsSecureStorage`` (index prefix approach). The adopted
        legacy store's index is unioned in, so a not-yet-promoted entry is
        still discoverable — except where a tombstone says that entry is a
        corpse, which must not be advertised as a key that can be loaded.
        """
        if legacy_store_adoption.is_reserved_entry(key_type):
            return []
        with self._lock:
            try:
                keys = list(self._index_get(self._service, key_type))
                legacy = self._read_through_service(key_type)
                if legacy is not None:
                    for key in self._index_get(legacy, key_type):
                        if key in keys:
                            continue
                        if self._tombstone_state(
                            key_type, key
                        ).suppresses_read_through:
                            continue
                        keys.append(key)
                return keys
            except Exception as exc:
                raise MlsStorageError.LoadFailed(
                    f"keyring list_keys failed: {exc}"
                ) from exc

    # -- tombstones ----------------------------------------------------------

    def _tombstone(self, key_type: str, key_id: str, cause: Exception) -> None:
        """Record that a legacy copy survived its deletion.

        *cause* is the removal failure this stands in for, folded into the
        raised message so a double fault names both halves.
        """

        try:
            self._store(
                self._service,
                legacy_store_adoption.TOMBSTONE_KEY_TYPE,
                legacy_store_adoption.tombstone_key_id(key_type, key_id),
                _TOMBSTONE_VALUE,
            )
        except Exception as exc:
            raise MlsStorageError.DeleteFailed(
                f"keyring delete left an inherited copy of {key_type} in place "
                f"({cause}) and could not tombstone it: {exc}"
            ) from exc

    def _tombstone_state(
        self, key_type: str, key_id: str
    ) -> legacy_store_adoption.TombstoneState:
        """What the namespaced store says about this key's legacy copy.

        A read that raises is ``UNREADABLE``, which fails closed as far as
        *reading* goes: read-through cannot be proven safe, and suppressing a
        legitimate inherited entry costs an identity rotation while resurrecting
        a consumed key costs forward secrecy. It deliberately stops short of
        authorising the removal retry — see
        :class:`legacy_store_adoption.TombstoneState`. Near-unreachable in
        practice: the namespaced read in :meth:`load` runs first against the
        same store and would have raised.
        """

        try:
            recorded = (
                self._read(
                    self._service,
                    legacy_store_adoption.TOMBSTONE_KEY_TYPE,
                    legacy_store_adoption.tombstone_key_id(key_type, key_id),
                )
                is not None
            )
        except Exception:
            logger.warning(
                "could not read the tombstone for %s; suppressing read-through "
                "without retiring the legacy copy",
                key_type,
                exc_info=True,
            )
            return legacy_store_adoption.TombstoneState.UNREADABLE
        return (
            legacy_store_adoption.TombstoneState.RECORDED
            if recorded
            else legacy_store_adoption.TombstoneState.ABSENT
        )

    def _retry_tombstoned_removal(
        self, legacy: str, key_type: str, key_id: str
    ) -> None:
        """Best-effort retry of the legacy removal a tombstone stands in for."""

        with self._lock:
            try:
                self._remove(legacy, key_type, key_id)
            except Exception:
                return
            self._clear_tombstone(key_type, key_id)

    def _clear_tombstone(self, key_type: str, key_id: str) -> None:
        """Retire a tombstone once the legacy copy is genuinely gone.

        Best effort: a tombstone that outlives its corpse only costs the
        inherited entry it suppresses, and there is nothing left to resurrect.
        """

        try:
            self._remove(
                self._service,
                legacy_store_adoption.TOMBSTONE_KEY_TYPE,
                legacy_store_adoption.tombstone_key_id(key_type, key_id),
            )
        except Exception:
            logger.debug(
                "failed to clear the tombstone for %s", key_type, exc_info=True
            )

    # -- legacy adoption -----------------------------------------------------

    def _resolve_legacy_adoption(self, namespace: str):
        """Resolve — and, when the legacy store is unclaimed, record — this
        account's right to inherit it.

        The whole probe → claim → read-back sequence runs under
        ``_ADOPTION_LOCK``, because reading it back is not on its own enough to
        make inheritance exclusive. The read back closes a write that silently
        failed, and a second account claiming between our probe and our write.
        It does not close two accounts interleaving like this::

            A._read_claim() -> None     B._read_claim() -> None
            A._store(nsA)
            A._read_claim() -> nsA  => adopt
                                        B._store(nsB)
                                        B._read_claim() -> nsB  => adopt

        Both adopt, both promote the same MLS signing identity, and each ends
        up holding the other's sessions and group state — the outcome the claim
        exists to prevent, arriving silently. The invariant is "at most one
        account holds a verified claim", and an unsynchronised
        read-modify-write does not provide it. The lock is module-level for the
        same reason: two accounts on one device are two providers, so the
        per-instance ``self._lock`` cannot order them.
        """

        assert self._legacy_service is not None
        with _ADOPTION_LOCK:
            return self._resolve_legacy_adoption_locked(namespace)

    def _resolve_legacy_adoption_locked(self, namespace: str):
        decision = legacy_store_adoption.decide(self._read_claim(), namespace)

        if decision.kind != "adopt":
            if decision.kind == "conflict":
                logger.error(
                    "legacy secure store already belongs to another account; this "
                    "account starts from a fresh MLS identity and cannot decrypt "
                    "its old sessions."
                )
            return decision

        # Read the claim back rather than assuming the write landed: a write
        # that failed silently leaves the store looking unclaimed to the next
        # account, which would then adopt the same MLS identity. See
        # ``legacy_store_adoption.confirm_claim``.
        try:
            self._store(
                self._legacy_service,
                legacy_store_adoption.CLAIM_KEY_TYPE,
                legacy_store_adoption.CLAIM_KEY_ID,
                namespace.encode("utf-8"),
            )
        except Exception:
            logger.warning("failed to claim the legacy secure store", exc_info=True)
            confirmed = legacy_store_adoption.CLAIM_UNVERIFIED
        else:
            confirmed = legacy_store_adoption.confirm_claim(self._read_claim(), namespace)

        if confirmed.kind == "adopt":
            logger.info("adopting the pre-namespace secure store for this account")
        elif confirmed.kind == "claim_unverified":
            logger.error(
                "could not record this account's claim on the legacy secure "
                "store, so it was not adopted: another account could otherwise "
                "inherit the same MLS identity. This account starts from a "
                "fresh identity."
            )
        elif confirmed.kind == "conflict":
            logger.error(
                "legacy secure store was claimed by another account "
                "concurrently; this account starts from a fresh MLS identity."
            )
        return confirmed

    def _read_claim(self) -> str | None:
        """The claim recorded in the legacy store, or None when absent or
        unreadable.

        A failed read is deliberately not distinguished from an absent claim on
        the way *in* (both mean "looks unclaimed") but is on the way back *out*,
        where it means the claim is unproven.
        """

        assert self._legacy_service is not None
        try:
            raw = self._read(
                self._legacy_service,
                legacy_store_adoption.CLAIM_KEY_TYPE,
                legacy_store_adoption.CLAIM_KEY_ID,
            )
        except Exception:
            logger.warning(
                "legacy secure store claim could not be read", exc_info=True
            )
            return None
        return raw.decode("utf-8", errors="replace") if raw is not None else None

    def _read_through_service(self, key_type: str) -> str | None:
        """The legacy service to consult for *key_type*, or None when
        read-through is off (no legacy store, another account claimed it, or
        this account could not prove its own claim)."""

        if self._legacy_service is None:
            return None
        if not self.legacy_adoption.allows_read_through:
            return None
        if legacy_store_adoption.is_reserved_entry(key_type):
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
