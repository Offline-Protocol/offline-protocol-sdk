"""SecureStorage warns when keyring resolved to a backend that cannot hold secrets.

On a host without a secret service (a headless Linux server, a container), the
keyring library selects ``keyring.backends.fail.Keyring``, whose every call
raises. Both it and ``keyring.backends.null.Keyring`` are classes named just
``Keyring``, so a check on the class name alone never recognised the case the
warning exists for.
"""

from __future__ import annotations

import logging

import keyring
import keyring.backends.fail
import keyring.backends.null
import pytest

from offline_protocol_sdk.secure_storage import SecureStorage
from offline_protocol_sdk.storage_namespace import account_storage_namespace

NAMESPACE = account_storage_namespace("backend-warning-test", "p")


def _plaintext_backend():
    # keyrings.alt is not a dependency; its file backend is a class of this name.
    return type("PlaintextKeyring", (keyring.backends.null.Keyring,), {"__module__": "keyrings.alt.file"})()


def _platform_backend():
    # A real secret service backend: class name and module name carry no marker.
    return type("Keyring", (keyring.backends.null.Keyring,), {"__module__": "keyring.backends.macOS"})()


@pytest.fixture
def backend(monkeypatch: pytest.MonkeyPatch):
    def use(instance):
        monkeypatch.setattr(keyring, "get_keyring", lambda: instance)
        return instance

    return use


def _warnings(caplog) -> list[str]:
    return [r.getMessage() for r in caplog.records
            if r.levelno >= logging.WARNING and "will NOT be stored securely" in r.getMessage()]


@pytest.mark.parametrize("make", [keyring.backends.fail.Keyring, keyring.backends.null.Keyring,
                                  _plaintext_backend],
                         ids=["fail", "null", "plaintext"])
def test_insecure_backend_is_warned_about(backend, caplog, make):
    instance = backend(make())
    with caplog.at_level(logging.WARNING, logger="offline_protocol_sdk.secure_storage"):
        SecureStorage(namespace=NAMESPACE, adopt_legacy_store=False)
    messages = _warnings(caplog)
    assert len(messages) == 1, caplog.text
    assert type(instance).__name__ in messages[0]


def test_platform_backend_is_not_warned_about(backend, caplog):
    backend(_platform_backend())
    with caplog.at_level(logging.WARNING, logger="offline_protocol_sdk.secure_storage"):
        SecureStorage(namespace=NAMESPACE, adopt_legacy_store=False)
    assert _warnings(caplog) == []
