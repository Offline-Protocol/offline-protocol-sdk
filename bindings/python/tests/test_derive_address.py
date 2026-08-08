"""Cross-platform contract for self-certifying address derivation.

This suite is the cross-platform pin. It runs against the real compiled
library through UniFFI, so it proves what the pure-Rust unit tests cannot:
that the address a binding hands an app is the address the protocol means.
The Swift and Kotlin harnesses cannot do this — neither can load the native
library — which is exactly why the derivation must have only one
implementation rather than a hand-rolled copy per platform.

The literals below come from the BIP-350 reference implementation
(github.com/sipa/bech32), independent of the `bech32` crate the SDK uses.
They are the addressing format itself: if one changes, every peer's identity
changed. Re-derive them from the reference implementation, never edit them to
match new code output.
"""

from __future__ import annotations

import pytest

from offline_protocol_sdk.offline_protocol import ProtocolError, derive_address

# RFC 8032 section 7.1 TEST 1 Ed25519 public key.
RFC8032_TV1_PUBLIC_KEY = list(
    bytes.fromhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
)
RFC8032_TV1_ADDRESS = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"

ZERO_KEY = [0] * 32
ZERO_KEY_ADDRESS = "off1q9nxs74dlp3t6amv3lqchr5l3csq39c5s5grq3yh"


def test_derive_address_matches_the_pinned_vector() -> None:
    assert derive_address(RFC8032_TV1_PUBLIC_KEY) == RFC8032_TV1_ADDRESS


def test_derive_address_matches_the_pinned_zero_key_vector() -> None:
    assert derive_address(ZERO_KEY) == ZERO_KEY_ADDRESS


def test_derive_address_is_deterministic() -> None:
    assert derive_address(RFC8032_TV1_PUBLIC_KEY) == derive_address(
        RFC8032_TV1_PUBLIC_KEY
    )


def test_derive_address_separates_keys() -> None:
    other = list(RFC8032_TV1_PUBLIC_KEY)
    other[0] ^= 0x01
    assert derive_address(other) != derive_address(RFC8032_TV1_PUBLIC_KEY)


def test_derived_address_has_the_canonical_shape() -> None:
    address = derive_address(RFC8032_TV1_PUBLIC_KEY)
    assert address.startswith("off1")
    assert len(address) == 44
    # Exactly one accepted form: lowercase, and within the bech32 charset
    # (which excludes 'b', 'i', 'o' and '1' outside the separator).
    assert address == address.lower()
    assert set(address[4:]) <= set("qpzry9x8gf2tvdw0s3jn54khce6mua7l")


@pytest.mark.parametrize("length", [0, 31, 33, 64])
def test_derive_address_rejects_wrong_key_lengths(length: int) -> None:
    # Length is part of the format contract: hashing a differently-sized input
    # would yield a different address for the same identity.
    with pytest.raises(ProtocolError):
        derive_address([0] * length)
