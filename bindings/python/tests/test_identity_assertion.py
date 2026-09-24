"""Cross-platform contract for the identity assertion.

Like ``test_derive_address``, this runs against the real compiled library
through UniFFI, which neither the Swift nor the Kotlin suite can do. Two
things are pinned here and nowhere else in a language that executes in CI:

* ``verify_identity_assertion`` is the one verifier of the
  ``public_key(32) || signature(64) || signed_data`` layout. The assertion
  below is RFC 8032 section 7.1 test vector 1, pasted from the RFC rather
  than produced by any code in this repository, so a verifier that accepts it
  and returns the pinned address has its whole pipeline checked against a
  published source. Anything under 96 bytes and any tampered signature is
  refused before a peer can be surfaced.
* ``identity_assertion`` on a live instance builds what that verifier
  accepts, naming the instance's own address. This is the pair the Python
  Bluetooth LE roles call: the peripheral serves the first, the central hands
  what it read to the second.
"""

from __future__ import annotations

import pytest

from offline_protocol_sdk.offline_protocol import (
    OfflineProtocol,
    OverflowPolicy,
    ProtocolConfig,
    ProtocolError,
    verify_identity_assertion,
)

from conftest import InMemoryStorage

# RFC 8032 section 7.1, TEST 1: the public key, the (empty) message and the
# signature, verbatim. The address is what the key derives to under
# docs/spec/identity.md, the same literal test_derive_address pins.
RFC8032_TV1_PUBLIC_KEY = bytes.fromhex(
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
)
RFC8032_TV1_SIGNATURE = bytes.fromhex(
    "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590"
    "a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
)
RFC8032_TV1_ADDRESS = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"

ASSERTION = RFC8032_TV1_PUBLIC_KEY + RFC8032_TV1_SIGNATURE


def test_the_published_assertion_verifies_to_the_pinned_address() -> None:
    assert len(ASSERTION) == 96
    assert verify_identity_assertion(list(ASSERTION)) == RFC8032_TV1_ADDRESS


def test_the_floor_is_ninety_six_bytes() -> None:
    """Ninety-five bytes is a key and most of a signature, and is refused
    before any cryptography runs. Ninety-six with an empty signed_data is
    whole, which the case above already shows."""
    for length in (0, 32, 95):
        with pytest.raises(ProtocolError):
            verify_identity_assertion(list(ASSERTION[:length]))


def test_a_tampered_signature_is_refused() -> None:
    tampered = bytearray(ASSERTION)
    tampered[40] ^= 0x01
    with pytest.raises(ProtocolError):
        verify_identity_assertion(list(tampered))


def test_a_signature_over_other_data_is_refused() -> None:
    """The signature covers what follows it. Appending a byte the signature
    does not cover is a different message under the same signature."""
    with pytest.raises(ProtocolError):
        verify_identity_assertion(list(ASSERTION + b"x"))


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


def test_a_device_builds_an_assertion_the_verifier_accepts() -> None:
    protocol = OfflineProtocol(_config("assertion-device"))
    with pytest.raises(ProtocolError):
        protocol.identity_assertion([])  # no identity before MLS

    protocol.initialize_mls(InMemoryStorage(), InMemoryStorage())
    address = protocol.local_address()
    assert address is not None

    for signed_data in (b"", b"mesh advertisement"):
        assertion = bytes(protocol.identity_assertion(list(signed_data)))
        assert len(assertion) == 96 + len(signed_data)
        assert assertion[96:] == signed_data
        assert verify_identity_assertion(list(assertion)) == address
