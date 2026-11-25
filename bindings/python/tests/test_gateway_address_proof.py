"""Both halves of the gateway attach proof, against the real library.

A device signs with ``gateway_address_declaration(challenge)`` on its engine;
a gateway daemon, which runs no engine, checks the result with the
instance-less ``verify_gateway_address_proof``. Neither half builds the
payload in Python: it has one home, in the sealed crate, and this test is
what shows the two halves agree across the FFI.
"""

from __future__ import annotations

import os

import pytest

from offline_protocol_sdk.offline_protocol import (
    OfflineProtocol,
    OverflowPolicy,
    ProtocolConfig,
    ProtocolError,
    verify_gateway_address_proof,
)

from conftest import InMemoryStorage


def _config(profile: str) -> ProtocolConfig:
    return ProtocolConfig(
        app_label="test-app",
        profile=profile,
        ble_enabled=False,
        wifi_direct_enabled=False,
        internet_enabled=False,
        reticulum_enabled=True,
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


def _device(profile: str) -> OfflineProtocol:
    protocol = OfflineProtocol(_config(profile))
    protocol.initialize_mls(InMemoryStorage(), InMemoryStorage())
    assert protocol.local_address() is not None
    return protocol


def test_a_device_declaration_verifies_to_its_own_address() -> None:
    device = _device("gateway-proof-device")
    challenge = os.urandom(32)
    declaration = device.gateway_address_declaration(list(challenge))
    bound = verify_gateway_address_proof(
        declaration.address,
        declaration.public_key,
        declaration.signature,
        list(challenge),
    )
    assert bound == device.local_address() == declaration.address


def test_a_declaration_does_not_verify_on_another_connection() -> None:
    device = _device("gateway-proof-replay")
    declaration = device.gateway_address_declaration(list(os.urandom(32)))
    with pytest.raises(ProtocolError):
        verify_gateway_address_proof(
            declaration.address,
            declaration.public_key,
            declaration.signature,
            list(os.urandom(32)),
        )


def test_a_signature_cannot_claim_another_devices_address() -> None:
    """The check that makes the binding mean anything: a valid signature
    under the sender's own key, naming somebody else's address."""
    attacker = _device("gateway-proof-attacker")
    victim = _device("gateway-proof-victim")
    challenge = list(os.urandom(32))
    declaration = attacker.gateway_address_declaration(challenge)
    with pytest.raises(ProtocolError):
        verify_gateway_address_proof(
            victim.local_address(),
            declaration.public_key,
            declaration.signature,
            challenge,
        )


def test_a_tampered_signature_is_refused() -> None:
    device = _device("gateway-proof-tamper")
    challenge = list(os.urandom(32))
    declaration = device.gateway_address_declaration(challenge)
    signature = list(declaration.signature)
    signature[10] ^= 0x01
    with pytest.raises(ProtocolError):
        verify_gateway_address_proof(
            declaration.address, declaration.public_key, signature, challenge
        )
