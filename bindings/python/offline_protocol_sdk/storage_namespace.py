"""Stable account namespaces for built-in protocol storage."""

from __future__ import annotations

import hashlib
import re

_NAMESPACE_DOMAIN = b"offline-protocol-storage-v1"

_ACCOUNT_PATTERN = re.compile(r"\Aaccount-[0-9a-f]{64}\Z")


def account_storage_namespace(app_id: str, profile: str) -> str:
    """Return an opaque, filesystem-safe namespace for one protocol account."""

    digest = hashlib.sha256(
        _NAMESPACE_DOMAIN + b"\0" + app_id.encode("utf-8") + b"\0" + profile.encode("utf-8")
    ).hexdigest()
    return f"account-{digest}"


def require_account_storage_namespace(value: str) -> str:
    """Return *value* if it is a well-formed account namespace, else raise.

    Built-in stores turn a namespace into a path or credential-store component,
    so an arbitrary caller-supplied string could escape its own account (``..``)
    or collide with another's. Mirrors ``StorageNamespace.requireAccount`` on
    Android and iOS.
    """

    if not _ACCOUNT_PATTERN.match(value):
        raise ValueError("Invalid protocol storage account namespace")
    return value
