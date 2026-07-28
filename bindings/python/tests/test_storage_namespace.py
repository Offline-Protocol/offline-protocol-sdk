"""Tests for stable built-in storage namespaces."""

import pytest

from offline_protocol_sdk.storage_namespace import (
    account_storage_namespace,
    require_account_storage_namespace,
)


def test_account_storage_namespace_is_stable_and_opaque() -> None:
    assert account_storage_namespace("test-app", "test-user-1") == (
        "account-814873e0cbdb2a1f25f14b31625e7f904cf9923e55b415b91ca4b29b210c12a1"
    )


def test_account_storage_namespace_separates_accounts() -> None:
    assert account_storage_namespace("chat", "alice") != account_storage_namespace(
        "chat", "bob"
    )
    assert account_storage_namespace("chat", "alice") != account_storage_namespace(
        "other-chat", "alice"
    )


def test_generated_namespaces_pass_validation() -> None:
    namespace = account_storage_namespace("chat", "alice")
    assert require_account_storage_namespace(namespace) == namespace


@pytest.mark.parametrize(
    "value",
    [
        "",
        "account-",
        "../../other-account",
        "account-" + "a" * 63,
        "account-" + "a" * 65,
        "account-" + "A" * 64,
        "account-" + "g" * 64,
    ],
)
def test_malformed_namespaces_are_refused(value: str) -> None:
    # A namespace becomes a directory component and a credential-store service
    # suffix, so anything that could escape or collide must be refused at the
    # door.
    with pytest.raises(ValueError, match="Invalid protocol storage account"):
        require_account_storage_namespace(value)
