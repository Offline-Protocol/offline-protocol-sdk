"""Tests for stable built-in storage namespaces."""

from offline_protocol_sdk.storage_namespace import account_storage_namespace


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
