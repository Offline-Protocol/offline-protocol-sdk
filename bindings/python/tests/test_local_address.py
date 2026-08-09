"""Cross-platform contract for this device's own derived address.

Like ``test_derive_address``, this runs against the real compiled library
through UniFFI — the only harness that can, since neither the Swift nor the
Kotlin suite loads the native library. What it pins is the bootstrap
inversion: the app supplies a ``profile`` (a local namespace selector) and the
protocol answers with an address it derived from the identity key in that
namespace, rather than echoing back something the app chose.
"""

from __future__ import annotations

import pytest

from offline_protocol_sdk.offline_protocol import (
    OfflineProtocol,
    OverflowPolicy,
    ProtocolConfig,
    derive_address,
)

from conftest import InMemoryStorage


def _config(profile: str) -> ProtocolConfig:
    return ProtocolConfig(
        app_id="test-app",
        profile=profile,
        ble_enabled=False,
        wifi_direct_enabled=False,
        internet_enabled=True,
        reticulum_enabled=False,
        nostr_enabled=False,
        prefer_online=True,
        initial_ttl=3,
        encryption_enabled=True,
        auto_key_exchange=True,
        store_pending=True,
        require_encryption=False,
        max_pending_per_peer=100,
        max_pending_global=1000,
        pending_ttl_ms=60000,
        overflow_policy=OverflowPolicy.DROP_OLDEST,
    )


def _started(profile: str, storage: InMemoryStorage) -> OfflineProtocol:
    protocol = OfflineProtocol(_config(profile))
    protocol.initialize_mls(storage, InMemoryStorage())
    return protocol


def test_local_address_is_absent_until_mls_is_initialized() -> None:
    protocol = OfflineProtocol(_config("default"))
    assert protocol.local_address() is None

    protocol.initialize_mls(InMemoryStorage(), InMemoryStorage())
    assert protocol.local_address() is not None


def test_local_address_is_the_derivation_of_the_identity_key() -> None:
    protocol = _started("default", InMemoryStorage())

    address = protocol.local_address()
    assert address == derive_address(protocol.get_identity_public_key())


def test_local_address_is_canonical() -> None:
    address = _started("default", InMemoryStorage()).local_address()

    assert address is not None
    assert address.startswith("off1")
    assert len(address) == 44
    assert address == address.lower()


def test_local_address_is_stable_for_the_same_profile_storage() -> None:
    storage = InMemoryStorage()

    first = _started("default", storage).local_address()
    second = _started("default", storage).local_address()

    assert first == second


def test_separate_profile_storage_yields_separate_addresses() -> None:
    first = _started("account-a", InMemoryStorage()).local_address()
    second = _started("account-b", InMemoryStorage()).local_address()

    assert first != second


def test_profile_is_not_the_address() -> None:
    """The profile is a local selector, never the identity on the wire."""
    protocol = _started("default", InMemoryStorage())

    assert protocol.local_address() != "default"


@pytest.mark.parametrize("profile", ["", "bad/profile", "bad:profile"])
def test_invalid_profile_is_rejected(profile: str) -> None:
    with pytest.raises(Exception):
        OfflineProtocol(_config(profile))
