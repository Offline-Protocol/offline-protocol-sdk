"""Stable account namespaces for built-in protocol storage."""

from __future__ import annotations

import hashlib

_NAMESPACE_DOMAIN = b"offline-protocol-storage-v1"


def account_storage_namespace(app_id: str, user_id: str) -> str:
    """Return an opaque, filesystem-safe namespace for one protocol account."""

    digest = hashlib.sha256(
        _NAMESPACE_DOMAIN + b"\0" + app_id.encode("utf-8") + b"\0" + user_id.encode("utf-8")
    ).hexdigest()
    return f"account-{digest}"
